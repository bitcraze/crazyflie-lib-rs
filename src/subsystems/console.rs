//! # Console subsystem
//!
//! The Crazyflie has a test console that is used to communicate various information
//! and debug message to the ground.
//!
//! The log is available either as a data stream that produces the same data as
//! returned by the crazyflie (ie. can be incomplete lines):
//! ``` no_run
//! use futures::StreamExt;
//!
//! # async fn as_stream(crazyflie: &crazyflie_lib::Crazyflie) {
//! let mut console_stream = crazyflie.console.stream().await;
//!
//! while let Some(data) = console_stream.next().await {
//!     println!("{}", data);
//! }
//! // If the Crazyflie send "Hello .................................................... World!"
//! // The println would show:
//! // Hello ........................
//! // ............................ W
//! // orld!
//! # }
//! ```
//!
//! Or a line streams that assemble and returns full lines:
//! ``` no_run
//! use futures::StreamExt;
//!
//! # async fn as_stream(crazyflie: &crazyflie_lib::Crazyflie) {
//! let mut line_stream = crazyflie.console.line_stream().await;
//!
//! while let Some(data) = line_stream.next().await {
//!     println!("{}", data);
//! }
//! // If the Crazyflie send "Hello .................................................... World!"
//! // The println would show:
//! // Hello .................................................... World!
//! # }
//! ```
//!
//! The data received from the Crazyflie is decoded as
//! [UTF8 lossy](String::from_utf8_lossy()). before being sent as [String] to the
//! streams.
//!
//! ## History or no History
//!
//! By default, the [Console::stream()] and [Console::line_stream()] functions
//! will return a stream that will produce the full console history since connection
//! and then produce the console as it arrives from the Crazyflie. This is needed
//! if the startup message needs to be displayed but can be problematic for more
//! advanced use-case to observe the console some time after the connection only.
//!
//! There exist functions for both data stream and line stream to get the stream
//! without getting the history first.
//!
//! ## Sourced consoles
//!
//! Protocol version 13 adds an immutable catalog of additional Console sources.
//! Catalog discovery is lazy and cached, and transparently retries while the
//! firmware reports that startup is not yet complete. Older protocol versions
//! return an empty catalog, so callers can use the same discovery flow across
//! supported firmware:
//!
//! ```no_run
//! use crazyflie_lib::subsystems::console::ConsoleHistory;
//! use futures::StreamExt;
//!
//! # async fn show_first_source(cf: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
//! let catalog = cf.console.catalog().await?;
//! if let Some(source) = catalog.iter().next() {
//!     let selector = source.selector();
//!     let mut lines = source.line_stream(ConsoleHistory::Replay).await;
//!     cf.console.enable(selector).await?;
//!
//!     if let Some(line) = lines.next().await {
//!         println!("[{}] {line}", source.path());
//!     }
//!
//!     cf.console.disable(selector).await?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! A source exposes independent byte, incrementally decoded text, and complete-line
//! streams. Only enable and disable operations accept an all-sources selector; data
//! streams always belong to exactly one [`ConsoleSource`].
//! Catalog and control transactions continue internally if their calling future is
//! dropped, keeping subsequent request and response pairs synchronized.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::crazyflie::CONSOLE_PORT;
use crate::{Error, Result};
use async_broadcast::{Receiver, broadcast};
use crazyflie_link::Packet;
use flume as channel;
use futures::{Stream, StreamExt, lock::Mutex};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

const SOURCED_CONSOLE_PROTOCOL_VERSION: u8 = 13;
const CONTROL_CHANNEL: u8 = 2;
const CONTROL_SET_ENABLED: u8 = 0;
const CATALOG_CHANNEL: u8 = 3;
const CATALOG_GET_ITEM: u8 = 0;
const CATALOG_GET_INFO: u8 = 1;
const CATALOG_NOT_READY_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Identifier assigned to a sourced console for the current Crazyflie boot.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConsoleSourceId(u8);

impl ConsoleSourceId {
    /// Constructs a source identifier, rejecting the reserved all-sources value.
    pub const fn new(value: u8) -> Option<Self> {
        if value == u8::MAX {
            None
        } else {
            Some(Self(value))
        }
    }

    /// Returns the protocol source identifier.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Immutable metadata for one sourced console.
#[derive(Clone)]
pub struct ConsoleSource {
    id: ConsoleSourceId,
    path: Arc<str>,
    state: Arc<ConsoleSourceState>,
}

impl std::fmt::Debug for ConsoleSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleSource")
            .field("id", &self.id)
            .field("path", &self.path)
            .finish()
    }
}

impl PartialEq for ConsoleSource {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.path == other.path
    }
}

impl Eq for ConsoleSource {}

#[derive(Default)]
struct ConsoleSourceHistory {
    bytes: Vec<Vec<u8>>,
    text: Vec<String>,
    lines: Vec<String>,
    pending_utf8: Vec<u8>,
    pending_line: String,
}

#[derive(Default)]
struct ConsoleSourceState {
    history: Mutex<ConsoleSourceHistory>,
    notify: Notify,
    closed: AtomicBool,
}

impl ConsoleSourceState {
    fn push_decoded(history: &mut ConsoleSourceHistory, decoded: String) {
        if decoded.is_empty() {
            return;
        }
        history.pending_line.push_str(&decoded);
        while let Some(newline) = history.pending_line.find('\n') {
            let rest = history.pending_line.split_off(newline + 1);
            history.pending_line.truncate(newline);
            let line = std::mem::replace(&mut history.pending_line, rest);
            history.lines.push(line);
        }
        history.text.push(decoded);
    }

    async fn push_bytes(&self, bytes: &[u8]) {
        let mut history = self.history.lock().await;
        history.bytes.push(bytes.to_vec());
        history.pending_utf8.extend_from_slice(bytes);

        let mut decoded = String::new();
        loop {
            match std::str::from_utf8(&history.pending_utf8) {
                Ok(valid) => {
                    decoded.push_str(valid);
                    history.pending_utf8.clear();
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    let error_len = error.error_len();
                    if valid_up_to > 0 {
                        let valid = std::str::from_utf8(&history.pending_utf8[..valid_up_to])
                            .expect("valid_up_to must delimit valid UTF-8")
                            .to_owned();
                        decoded.push_str(&valid);
                        history.pending_utf8.drain(..valid_up_to);
                    }
                    if let Some(error_len) = error_len {
                        decoded.push(char::REPLACEMENT_CHARACTER);
                        history.pending_utf8.drain(..error_len);
                    } else {
                        break;
                    }
                }
            }
        }
        Self::push_decoded(&mut history, decoded);
        drop(history);
        self.notify.notify_waiters();
    }

    async fn close(&self) {
        let mut history = self.history.lock().await;
        if !history.pending_utf8.is_empty() {
            let pending = std::mem::take(&mut history.pending_utf8);
            Self::push_decoded(&mut history, String::from_utf8_lossy(&pending).into_owned());
        }
        self.closed.store(true, Ordering::Relaxed);
        drop(history);
        self.notify.notify_waiters();
    }
}

/// Determines whether a source stream replays connection-lifetime history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleHistory {
    /// Replay all data received by the library, then continue live.
    Replay,
    /// Start with the next item emitted after the stream's creation point.
    ///
    /// A text item may finish a code point, and a line may finish a line, that
    /// began before the stream was created.
    Live,
}

impl ConsoleSource {
    /// Returns the boot-lifetime source identifier.
    pub const fn id(&self) -> ConsoleSourceId {
        self.id
    }

    /// Returns the display and filtering path advertised by the source.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns a selector suitable for enabling or disabling this source.
    pub const fn selector(&self) -> ConsoleSourceSelector {
        ConsoleSourceSelector::Source(self.id)
    }

    /// Returns a stream of byte chunks received for this source.
    ///
    /// Chunk boundaries are transport artifacts and do not delimit text or lines.
    pub async fn byte_stream(
        &self,
        history: ConsoleHistory,
    ) -> futures::stream::BoxStream<'static, Vec<u8>> {
        let state = self.state.clone();
        let mut index = match history {
            ConsoleHistory::Replay => 0,
            ConsoleHistory::Live => state.history.lock().await.bytes.len(),
        };

        async_stream::stream! {
            loop {
                let notified = state.notify.notified();
                let next = state.history.lock().await.bytes.get(index).cloned();
                if let Some(bytes) = next {
                    index += 1;
                    yield bytes;
                } else if state.closed.load(Ordering::Relaxed) {
                    break;
                } else {
                    notified.await;
                }
            }
        }
        .boxed()
    }

    /// Returns incrementally decoded, loss-tolerant UTF-8 chunks for this source.
    ///
    /// The returned chunk boundaries do not delimit lines or records.
    pub async fn text_stream(
        &self,
        history: ConsoleHistory,
    ) -> futures::stream::BoxStream<'static, String> {
        let state = self.state.clone();
        let mut index = match history {
            ConsoleHistory::Replay => 0,
            ConsoleHistory::Live => state.history.lock().await.text.len(),
        };

        async_stream::stream! {
            loop {
                let notified = state.notify.notified();
                let next = state.history.lock().await.text.get(index).cloned();
                if let Some(text) = next {
                    index += 1;
                    yield text;
                } else if state.closed.load(Ordering::Relaxed) {
                    break;
                } else {
                    notified.await;
                }
            }
        }
        .boxed()
    }

    /// Returns complete, incrementally decoded lines for this source.
    ///
    /// Newline delimiters are not included in returned strings. An incomplete
    /// final line remains buffered until a newline is received.
    pub async fn line_stream(
        &self,
        history: ConsoleHistory,
    ) -> futures::stream::BoxStream<'static, String> {
        let state = self.state.clone();
        let mut index = match history {
            ConsoleHistory::Replay => 0,
            ConsoleHistory::Live => state.history.lock().await.lines.len(),
        };

        async_stream::stream! {
            loop {
                let notified = state.notify.notified();
                let next = state.history.lock().await.lines.get(index).cloned();
                if let Some(line) = next {
                    index += 1;
                    yield line;
                } else if state.closed.load(Ordering::Relaxed) {
                    break;
                } else {
                    notified.await;
                }
            }
        }
        .boxed()
    }
}

/// Selects one sourced console or every source in a catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleSourceSelector {
    /// Select one catalog source.
    Source(ConsoleSourceId),
    /// Select every catalog source.
    All,
}

impl ConsoleSourceSelector {
    const fn wire_id(self) -> u8 {
        match self {
            Self::Source(id) => id.get(),
            Self::All => u8::MAX,
        }
    }
}

/// Error specific to sourced Console operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsoleError {
    /// The selected source is not present in this connection's catalog.
    UnknownSource(ConsoleSourceId),
    /// The firmware rejected an enable or disable command.
    CommandRejected {
        /// Source or all-sources selector sent to the firmware.
        selector: ConsoleSourceSelector,
        /// Requested enabled state.
        enabled: bool,
        /// Raw errno-compatible result returned by the firmware.
        errno: u8,
    },
    /// The firmware rejected a source-catalog operation.
    CatalogCommandRejected {
        /// Catalog operation sent to the firmware.
        operation: ConsoleCatalogOperation,
        /// Raw errno-compatible result returned by the firmware.
        errno: u8,
    },
}

/// Source-catalog operation associated with a firmware error response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleCatalogOperation {
    /// Read the source count and catalog CRC.
    GetInfo,
    /// Read one source entry.
    GetItem(ConsoleSourceId),
}

impl std::fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSource(id) => write!(f, "unknown Console source {}", id.get()),
            Self::CommandRejected {
                selector,
                enabled,
                errno,
            } => write!(
                f,
                "firmware rejected Console {selector:?} enabled={enabled} with errno {errno}"
            ),
            Self::CatalogCommandRejected { operation, errno } => write!(
                f,
                "firmware rejected Console catalog operation {operation:?} with errno {errno}"
            ),
        }
    }
}

impl std::error::Error for ConsoleError {}

/// Immutable sourced-console catalog for one Crazyflie connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleCatalog {
    sources: Arc<[ConsoleSource]>,
    crc32: Option<u32>,
}

impl ConsoleCatalog {
    fn empty() -> Self {
        Self {
            sources: Arc::from([]),
            crc32: None,
        }
    }

    /// Returns the number of advertised sources.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// Returns whether no sourced consoles are advertised.
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Returns the firmware-provided catalog CRC, or `None` when unsupported.
    pub const fn crc32(&self) -> Option<u32> {
        self.crc32
    }

    /// Returns the source with the given boot-lifetime identifier.
    pub fn get(&self, id: ConsoleSourceId) -> Option<&ConsoleSource> {
        self.sources.iter().find(|source| source.id == id)
    }

    /// Returns the source with the given path.
    pub fn find(&self, path: &str) -> Option<&ConsoleSource> {
        self.sources.iter().find(|source| source.path() == path)
    }

    /// Iterates over sources in source-identifier order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ConsoleSource> {
        self.sources.iter()
    }

    /// Returns a selector covering every source in this catalog.
    pub const fn all(&self) -> ConsoleSourceSelector {
        ConsoleSourceSelector::All
    }
}

impl<'a> IntoIterator for &'a ConsoleCatalog {
    type Item = &'a ConsoleSource;
    type IntoIter = std::slice::Iter<'a, ConsoleSource>;

    fn into_iter(self) -> Self::IntoIter {
        self.sources.iter()
    }
}

enum ConsoleCommand {
    GetCatalog {
        reply: oneshot::Sender<Result<ConsoleCatalog>>,
    },
    SetEnabled {
        selector: ConsoleSourceSelector,
        enabled: bool,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown,
}

struct ConsoleTransactionWorker {
    uplink: channel::Sender<Packet>,
    control_downlink: channel::Receiver<Packet>,
    catalog_downlink: channel::Receiver<Packet>,
    protocol_version: u8,
    source_states: Arc<Mutex<BTreeMap<u8, Arc<ConsoleSourceState>>>>,
    catalog: Option<ConsoleCatalog>,
}

impl ConsoleTransactionWorker {
    async fn run(mut self, commands: channel::Receiver<ConsoleCommand>) {
        while let Ok(command) = commands.recv_async().await {
            match command {
                ConsoleCommand::GetCatalog { reply } => {
                    let result = self.get_catalog().await;
                    let _ = reply.send(result);
                }
                ConsoleCommand::SetEnabled {
                    selector,
                    enabled,
                    reply,
                } => {
                    let result = self.set_enabled(selector, enabled).await;
                    let _ = reply.send(result);
                }
                ConsoleCommand::Shutdown => break,
            }
        }
    }

    async fn get_catalog(&mut self) -> Result<ConsoleCatalog> {
        if let Some(catalog) = &self.catalog {
            return Ok(catalog.clone());
        }

        let catalog = self.discover_catalog().await?;
        self.catalog = Some(catalog.clone());
        Ok(catalog)
    }

    async fn discover_catalog(&self) -> Result<ConsoleCatalog> {
        if self.protocol_version < SOURCED_CONSOLE_PROTOCOL_VERSION {
            return Ok(ConsoleCatalog::empty());
        }

        let info = loop {
            self.uplink
                .send_async(Packet::new(
                    CONSOLE_PORT,
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_INFO],
                ))
                .await
                .map_err(|_| Error::Disconnected)?;

            let info = self
                .catalog_downlink
                .recv_async()
                .await
                .map_err(|_| Error::Disconnected)?;
            let data = info.get_data();
            if data.len() == 2 && data[0] == CATALOG_GET_INFO {
                if data[1] == libc::EAGAIN as u8 {
                    tokio::time::sleep(CATALOG_NOT_READY_RETRY_DELAY).await;
                    continue;
                }
                return Err(Error::Console(ConsoleError::CatalogCommandRejected {
                    operation: ConsoleCatalogOperation::GetInfo,
                    errno: data[1],
                }));
            }
            break info;
        };
        let data = info.get_data();
        if data.len() != 6 || data[0] != CATALOG_GET_INFO {
            return Err(Error::ProtocolError(
                "Malformed sourced Console catalog info response".to_owned(),
            ));
        }

        let source_count = data[1];
        let crc32 = u32::from_le_bytes(data[2..6].try_into()?);
        let mut sources = Vec::with_capacity(source_count.into());

        for id in 0..source_count {
            let item = loop {
                self.uplink
                    .send_async(Packet::new(
                        CONSOLE_PORT,
                        CATALOG_CHANNEL,
                        vec![CATALOG_GET_ITEM, id],
                    ))
                    .await
                    .map_err(|_| Error::Disconnected)?;

                let item = self
                    .catalog_downlink
                    .recv_async()
                    .await
                    .map_err(|_| Error::Disconnected)?;
                let data = item.get_data();
                if data.len() == 2 && data[0] == CATALOG_GET_ITEM {
                    if data[1] == libc::EAGAIN as u8 {
                        tokio::time::sleep(CATALOG_NOT_READY_RETRY_DELAY).await;
                        continue;
                    }
                    return Err(Error::Console(ConsoleError::CatalogCommandRejected {
                        operation: ConsoleCatalogOperation::GetItem(ConsoleSourceId(id)),
                        errno: data[1],
                    }));
                }
                break item;
            };
            let data = item.get_data();
            if data.len() < 3 || data[0] != CATALOG_GET_ITEM || data[1] != id {
                return Err(Error::ProtocolError(
                    "Malformed sourced Console catalog item response".to_owned(),
                ));
            }

            let path = std::str::from_utf8(&data[2..]).map_err(|_| {
                Error::ProtocolError("Sourced Console catalog path is not valid UTF-8".to_owned())
            })?;
            if path.split(':').any(str::is_empty) {
                return Err(Error::ProtocolError(
                    "Sourced Console catalog path has invalid segments".to_owned(),
                ));
            }
            if sources
                .iter()
                .any(|source: &ConsoleSource| source.path() == path)
            {
                return Err(Error::ProtocolError(
                    "Sourced Console catalog contains a duplicate path".to_owned(),
                ));
            }
            sources.push(ConsoleSource {
                id: ConsoleSourceId(id),
                path: Arc::from(path),
                state: Arc::default(),
            });
        }

        {
            let mut states = self.source_states.lock().await;
            for source in &sources {
                states.insert(source.id.get(), source.state.clone());
            }
        }

        Ok(ConsoleCatalog {
            sources: sources.into(),
            crc32: Some(crc32),
        })
    }

    async fn set_enabled(&self, selector: ConsoleSourceSelector, enabled: bool) -> Result<()> {
        let enabled_byte = u8::from(enabled);
        let request = [CONTROL_SET_ENABLED, selector.wire_id(), enabled_byte];
        self.uplink
            .send_async(Packet::new(CONSOLE_PORT, CONTROL_CHANNEL, request.to_vec()))
            .await
            .map_err(|_| Error::Disconnected)?;

        let response = self
            .control_downlink
            .recv_async()
            .await
            .map_err(|_| Error::Disconnected)?;
        let data = response.get_data();
        if data.len() == 2 && data[0] == CONTROL_SET_ENABLED {
            return Err(Error::Console(ConsoleError::CommandRejected {
                selector,
                enabled,
                errno: data[1],
            }));
        }
        if data.len() != 4 || data[..3] != request {
            return Err(Error::ProtocolError(
                "Malformed sourced Console control response".to_owned(),
            ));
        }
        if data[3] != 0 {
            return Err(Error::Console(ConsoleError::CommandRejected {
                selector,
                enabled,
                errno: data[3],
            }));
        }
        Ok(())
    }
}

/// # Access to the console subsystem
///
/// See the [console module documentation](crate::subsystems::console) for more context and information.
pub struct Console {
    stream_broadcast_receiver: Receiver<String>,
    console_buffer: Arc<Mutex<String>>,
    line_broadcast_receiver: Receiver<String>,
    console_lines: Arc<Mutex<Vec<String>>>,
    command_sender: channel::Sender<ConsoleCommand>,
    console_task: Mutex<Option<JoinHandle<()>>>,
    transaction_task: Mutex<Option<JoinHandle<()>>>,
}

impl Console {
    pub(crate) async fn new(
        downlink: channel::Receiver<Packet>,
        uplink: channel::Sender<Packet>,
        protocol_version: u8,
    ) -> Result<Self> {
        let (mut stream_broadcast, stream_broadcast_receiver) = broadcast(1000);
        let console_buffer: Arc<Mutex<String>> = Default::default();

        let (mut line_broadcast, line_broadcast_receiver) = broadcast(1000);
        let (control_sender, control_downlink) = channel::unbounded();
        let (catalog_sender, catalog_downlink) = channel::unbounded();

        // Enable overflow mode so old messages are dropped instead of blocking
        stream_broadcast.set_overflow(true);
        line_broadcast.set_overflow(true);
        let console_lines: Arc<Mutex<Vec<String>>> = Default::default();

        let buffer = console_buffer.clone();
        let lines = console_lines.clone();
        let source_states: Arc<Mutex<BTreeMap<u8, Arc<ConsoleSourceState>>>> = Default::default();
        let routed_source_states = source_states.clone();

        // Keep every port-0 packet in its original receive order. In particular,
        // processing sourced data before forwarding a later control response
        // preserves the firmware's successful-disable ordering barrier.
        let console_task = tokio::spawn(async move {
            let mut line_buffer = String::new();
            while let Ok(packet) = downlink.recv_async().await {
                match packet.get_channel() {
                    0 => {
                        // Decode text from the legacy console.
                        let text = String::from_utf8_lossy(packet.get_data());
                        buffer.lock().await.push_str(&text);

                        // Push the text to all active streams, we ignore any error there.
                        let _ = stream_broadcast.broadcast(text.clone().into_owned()).await;

                        // Extract lines and push them to all active line streams.
                        line_buffer.push_str(&text);
                        if let Some((line, rest)) = line_buffer.clone().split_once("\n") {
                            line_buffer = rest.to_owned();
                            lines.lock().await.push(line.to_owned().clone());
                            let _ = line_broadcast.broadcast(line.to_owned()).await;
                        }
                    }
                    1 => {
                        let data = packet.get_data();
                        let Some((&source_id, bytes)) = data.split_first() else {
                            continue;
                        };
                        let state = routed_source_states.lock().await.get(&source_id).cloned();
                        if let Some(state) = state {
                            state.push_bytes(bytes).await;
                        }
                    }
                    CONTROL_CHANNEL => {
                        let _ = control_sender.send_async(packet).await;
                    }
                    CATALOG_CHANNEL => {
                        let _ = catalog_sender.send_async(packet).await;
                    }
                    _ => {}
                }
            }

            let states: Vec<_> = routed_source_states
                .lock()
                .await
                .values()
                .cloned()
                .collect();
            for state in states {
                state.close().await;
            }
        });

        let (command_sender, command_receiver) = channel::unbounded();
        let transaction_task = tokio::spawn(
            ConsoleTransactionWorker {
                uplink,
                control_downlink,
                catalog_downlink,
                protocol_version,
                source_states,
                catalog: None,
            }
            .run(command_receiver),
        );

        Ok(Self {
            stream_broadcast_receiver,
            console_buffer,
            line_broadcast_receiver,
            console_lines,
            command_sender,
            console_task: Mutex::new(Some(console_task)),
            transaction_task: Mutex::new(Some(transaction_task)),
        })
    }

    pub(crate) async fn shutdown(&self) {
        let task = self.console_task.lock().await.take();
        if let Some(task) = task {
            task.await.expect("Console task failed");
        }
        let _ = self.command_sender.send(ConsoleCommand::Shutdown);
        let task = self.transaction_task.lock().await.take();
        if let Some(task) = task {
            task.await.expect("Console transaction task failed");
        }
    }

    /// Lazily discovers and caches the immutable sourced-console catalog.
    ///
    /// Discovery waits and retries both catalog operations when protocol-13
    /// firmware reports `EAGAIN` during startup. Crazyflies using an older
    /// protocol return an empty catalog.
    pub async fn catalog(&self) -> Result<ConsoleCatalog> {
        let (reply, result) = oneshot::channel();
        self.command_sender
            .send(ConsoleCommand::GetCatalog { reply })
            .map_err(|_| Error::Disconnected)?;
        result.await.map_err(|_| Error::Disconnected)?
    }

    /// Enables one sourced console or every source in the catalog.
    pub async fn enable(&self, selector: ConsoleSourceSelector) -> Result<()> {
        self.set_enabled(selector, true).await
    }

    /// Disables one sourced console or every source in the catalog.
    ///
    /// On success, every sourced packet ordered before the firmware response
    /// has been incorporated into its source history. Existing streams may
    /// still yield such buffered history after this method returns.
    pub async fn disable(&self, selector: ConsoleSourceSelector) -> Result<()> {
        self.set_enabled(selector, false).await
    }

    async fn set_enabled(&self, selector: ConsoleSourceSelector, enabled: bool) -> Result<()> {
        let catalog = self.catalog().await?;
        if let ConsoleSourceSelector::Source(id) = selector
            && catalog.get(id).is_none()
        {
            return Err(Error::Console(ConsoleError::UnknownSource(id)));
        }
        if catalog.is_empty() {
            return Ok(());
        }

        let (reply, result) = oneshot::channel();
        self.command_sender
            .send(ConsoleCommand::SetEnabled {
                selector,
                enabled,
                reply,
            })
            .map_err(|_| Error::Disconnected)?;
        result.await.map_err(|_| Error::Disconnected)?
    }

    /// Return a [Stream] that generates a [String] each time a console packet
    /// is received from the Crazyflie.
    ///
    /// With the current Crazyflie algorithms, packets are up to 30 character
    /// long and a new line triggers the send of a packet. Though this is not a
    /// guarantee and nothing should be expected from this Stream other that
    /// getting the console data when they are received.
    ///
    /// The lib keeps track of the console history since connection, the stream
    /// will first produce the full history since connection in one String and then
    /// will start returning Strings as they come from the Crazyflie.
    pub async fn stream(&self) -> impl Stream<Item = String> + use<> {
        let buffer = self.console_buffer.lock().await;
        let history_buffer = buffer.clone();
        let history_stream = futures::stream::once(async { history_buffer }).boxed();

        history_stream.chain(self.stream_broadcast_receiver.new_receiver())
    }

    /// Version of [Console::stream()] but that does not produce the history
    /// first.
    pub async fn stream_no_history(&self) -> impl Stream<Item = String> + use<> {
        self.stream_broadcast_receiver.new_receiver()
    }

    /// Return a [Stream] that generate a [String] each time a line is received
    /// from the Crazyflie.
    ///
    /// This is a useful function if you want to receive the console line by line.
    /// (for example to print it in a terminal or a file)
    ///
    /// Similar to [Console::stream()], this stream will generate first the
    /// console history since connection. The history is generated by the Stream
    /// line-by-line.
    pub async fn line_stream(&self) -> impl Stream<Item = String> + use<> {
        let lines = self.console_lines.lock().await;
        let history_lines = lines.clone();
        let history_stream = futures::stream::iter(history_lines).boxed();

        history_stream.chain(self.line_broadcast_receiver.new_receiver())
    }

    /// Version of [Console::line_stream()] but that does not produce the history
    /// first.
    pub async fn line_stream_no_history(&self) -> impl Stream<Item = String> + use<> {
        self.line_broadcast_receiver.new_receiver()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::join;

    #[tokio::test]
    async fn catalog_is_discovered_once_and_cached() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let firmware = async {
            let request = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(request.get_channel(), 3);
            assert_eq!(request.get_data(), &[1]);
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 2, 0x78, 0x56, 0x34, 0x12]))
                .await
                .unwrap();

            for (id, path) in [(0, "deck:bcCam"), (1, "nRF51")] {
                let request = uplink_receiver.recv_async().await.unwrap();
                assert_eq!(request.get_channel(), 3);
                assert_eq!(request.get_data(), &[0, id]);

                let mut response = vec![0, id];
                response.extend_from_slice(path.as_bytes());
                downlink_sender
                    .send_async(Packet::new(0, 3, response))
                    .await
                    .unwrap();
            }
        };

        let (catalog, ()) = join!(console.catalog(), firmware);
        let catalog = catalog.unwrap();
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog.crc32(), Some(0x1234_5678));
        assert_eq!(
            catalog
                .get(ConsoleSourceId::new(0).unwrap())
                .unwrap()
                .path(),
            "deck:bcCam"
        );
        assert_eq!(catalog.find("nRF51").unwrap().id().get(), 1);

        let cached = console.catalog().await.unwrap();
        assert_eq!(cached, catalog);
        assert!(uplink_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn catalog_waits_and_retries_while_firmware_is_not_ready() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let firmware = async {
            let first = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(first.get_data(), &[CATALOG_GET_INFO]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_INFO, libc::EAGAIN as u8],
                ))
                .await
                .unwrap();

            let retry = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(retry.get_data(), &[CATALOG_GET_INFO]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_INFO, 0, 0, 0, 0, 0],
                ))
                .await
                .unwrap();
        };

        let result = tokio::time::timeout(Duration::from_millis(250), async {
            let (catalog, ()) = join!(console.catalog(), firmware);
            catalog
        })
        .await
        .expect("catalog did not retry its readiness request")
        .unwrap();
        assert!(result.is_empty());
        assert_eq!(result.crc32(), Some(0));
    }

    #[tokio::test]
    async fn catalog_retries_items_while_firmware_is_not_ready() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let firmware = async {
            let info = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(info.get_data(), &[CATALOG_GET_INFO]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_INFO, 1, 0, 0, 0, 0],
                ))
                .await
                .unwrap();

            let first_item = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(first_item.get_data(), &[CATALOG_GET_ITEM, 0]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_ITEM, libc::EAGAIN as u8],
                ))
                .await
                .unwrap();

            let retry = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(retry.get_data(), &[CATALOG_GET_ITEM, 0]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    [vec![CATALOG_GET_ITEM, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };

        let catalog = tokio::time::timeout(Duration::from_millis(250), async {
            let (catalog, ()) = join!(console.catalog(), firmware);
            catalog
        })
        .await
        .expect("catalog did not retry the item readiness response")
        .unwrap();
        assert_eq!(catalog.find("deck:bcCam").unwrap().id().get(), 0);
    }

    #[tokio::test]
    async fn cancelling_catalog_discovery_does_not_desynchronize_later_discovery() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        {
            let discovery = console.catalog();
            tokio::pin!(discovery);
            tokio::select! {
                request = uplink_receiver.recv_async() => {
                    assert_eq!(request.unwrap().get_data(), &[CATALOG_GET_INFO]);
                }
                result = discovery.as_mut() => {
                    panic!("catalog returned before sending its request: {result:?}");
                }
            }
        }

        downlink_sender
            .send_async(Packet::new(
                0,
                CATALOG_CHANNEL,
                vec![CATALOG_GET_INFO, 1, 0, 0, 0, 0],
            ))
            .await
            .unwrap();

        let firmware = async {
            let item = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(item.get_data(), &[CATALOG_GET_ITEM, 0]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    [vec![CATALOG_GET_ITEM, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };

        let catalog = tokio::time::timeout(Duration::from_millis(250), async {
            let (catalog, ()) = join!(console.catalog(), firmware);
            catalog
        })
        .await
        .expect("cancelled discovery did not continue in the background")
        .unwrap();
        assert_eq!(catalog.find("deck:bcCam").unwrap().id().get(), 0);
        assert!(uplink_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn catalog_rejects_a_path_with_an_empty_segment() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let firmware = async {
            assert_eq!(
                uplink_receiver.recv_async().await.unwrap().get_data(),
                &[CATALOG_GET_INFO]
            );
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            assert_eq!(
                uplink_receiver.recv_async().await.unwrap().get_data(),
                &[CATALOG_GET_ITEM, 0]
            );
            downlink_sender
                .send_async(Packet::new(0, 3, vec![0, 0, b'd', b'e', b'c', b'k', b':']))
                .await
                .unwrap();
        };

        let (catalog, ()) = join!(console.catalog(), firmware);
        assert!(matches!(catalog, Err(Error::ProtocolError(_))));
    }

    #[tokio::test]
    async fn catalog_rejects_duplicate_paths() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let firmware = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 2, 0, 0, 0, 0]))
                .await
                .unwrap();
            for id in 0..2 {
                let _ = uplink_receiver.recv_async().await.unwrap();
                downlink_sender
                    .send_async(Packet::new(
                        0,
                        3,
                        [vec![0, id], b"deck:duplicate".to_vec()].concat(),
                    ))
                    .await
                    .unwrap();
            }
        };

        let (catalog, ()) = join!(console.catalog(), firmware);
        assert!(matches!(catalog, Err(Error::ProtocolError(_))));
    }

    #[tokio::test]
    async fn catalog_reports_command_error_responses() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let firmware = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let item = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(item.get_data(), &[CATALOG_GET_ITEM, 0]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_ITEM, libc::ENOENT as u8],
                ))
                .await
                .unwrap();
        };

        let (catalog, ()) = join!(console.catalog(), firmware);
        assert!(matches!(
            catalog,
            Err(Error::Console(ConsoleError::CatalogCommandRejected {
                operation: ConsoleCatalogOperation::GetItem(id),
                errno,
            })) if id.get() == 0 && errno == libc::ENOENT as u8
        ));
    }

    #[tokio::test]
    async fn enable_reports_the_firmware_errno() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            assert_eq!(uplink_receiver.recv_async().await.unwrap().get_data(), &[1]);
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            assert_eq!(
                uplink_receiver.recv_async().await.unwrap().get_data(),
                &[0, 0]
            );
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();

        let firmware = async {
            let request = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(request.get_channel(), 2);
            assert_eq!(request.get_data(), &[0, 0, 1]);
            downlink_sender
                .send_async(Packet::new(0, 2, vec![0, 0, 1, libc::EIO as u8]))
                .await
                .unwrap();
        };

        let (result, ()) = join!(console.enable(source.selector()), firmware);
        assert!(matches!(
            result,
            Err(Error::Console(ConsoleError::CommandRejected {
                selector: ConsoleSourceSelector::Source(id),
                enabled: true,
                errno,
            })) if id == source.id() && errno == libc::EIO as u8
        ));
    }

    #[tokio::test]
    async fn protocol_12_has_an_empty_catalog_and_all_control_is_a_noop() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (_downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 12).await.unwrap();

        let catalog = console.catalog().await.unwrap();
        assert!(catalog.is_empty());
        assert_eq!(catalog.crc32(), None);
        console.enable(catalog.all()).await.unwrap();
        console.disable(catalog.all()).await.unwrap();
        assert!(uplink_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn control_commands_are_serialized() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let catalog = catalog.unwrap();
        let source = catalog.find("deck:bcCam").unwrap().selector();

        let firmware = async {
            let first = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(first.get_data(), &[0, 0, 1]);
            assert!(uplink_receiver.try_recv().is_err());
            downlink_sender
                .send_async(Packet::new(0, 2, vec![0, 0, 1, 0]))
                .await
                .unwrap();

            let second = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(second.get_data(), &[0, u8::MAX, 0]);
            downlink_sender
                .send_async(Packet::new(0, 2, vec![0, u8::MAX, 0, 0]))
                .await
                .unwrap();
        };

        let (enabled, disabled, ()) = join!(
            console.enable(source),
            console.disable(catalog.all()),
            firmware
        );
        enabled.unwrap();
        disabled.unwrap();
    }

    #[tokio::test]
    async fn disable_waits_until_earlier_source_packets_are_processed() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let request = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(request.get_data(), &[CATALOG_GET_INFO]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_INFO, 1, 0, 0, 0, 0],
                ))
                .await
                .unwrap();
            let request = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(request.get_data(), &[CATALOG_GET_ITEM, 0]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CATALOG_CHANNEL,
                    [vec![CATALOG_GET_ITEM, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().get(ConsoleSourceId(0)).unwrap().clone();
        let selector = source.selector();

        let history_guard = source.state.history.lock().await;
        let firmware = async {
            let request = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(request.get_data(), &[CONTROL_SET_ENABLED, 0, 0]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    1,
                    [vec![0], b"before disable\n".to_vec()].concat(),
                ))
                .await
                .unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CONTROL_CHANNEL,
                    vec![CONTROL_SET_ENABLED, 0, 0, 0],
                ))
                .await
                .unwrap();
        };
        let disable = console.disable(selector);
        tokio::pin!(disable);
        tokio::pin!(firmware);

        tokio::select! {
            () = firmware.as_mut() => {}
            result = disable.as_mut() => {
                panic!("disable returned before the firmware response was sent: {result:?}");
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), disable.as_mut())
                .await
                .is_err(),
            "disable returned before the earlier source packet was processed"
        );

        drop(history_guard);
        tokio::time::timeout(Duration::from_secs(1), disable)
            .await
            .expect("disable did not finish after source processing resumed")
            .unwrap();

        let mut live = source.line_stream(ConsoleHistory::Live).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), live.next())
                .await
                .is_err(),
            "a pre-disable packet appeared in a stream created after disable returned"
        );
    }

    #[tokio::test]
    async fn cancelling_control_does_not_desynchronize_the_next_command() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let selector = catalog.unwrap().find("deck:bcCam").unwrap().selector();

        {
            let enable = console.enable(selector);
            tokio::pin!(enable);
            tokio::select! {
                request = uplink_receiver.recv_async() => {
                    assert_eq!(request.unwrap().get_data(), &[CONTROL_SET_ENABLED, 0, 1]);
                }
                result = enable.as_mut() => {
                    panic!("enable returned before the firmware response: {result:?}");
                }
            }
        }

        downlink_sender
            .send_async(Packet::new(
                0,
                CONTROL_CHANNEL,
                vec![CONTROL_SET_ENABLED, 0, 1, 0],
            ))
            .await
            .unwrap();

        let firmware = async {
            let disable = uplink_receiver.recv_async().await.unwrap();
            assert_eq!(disable.get_data(), &[CONTROL_SET_ENABLED, 0, 0]);
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CONTROL_CHANNEL,
                    vec![CONTROL_SET_ENABLED, 0, 0, 0],
                ))
                .await
                .unwrap();
        };

        let (disabled, ()) = tokio::time::timeout(Duration::from_millis(250), async {
            join!(console.disable(selector), firmware)
        })
        .await
        .expect("control worker did not finish the cancelled transaction");
        disabled.unwrap();
    }

    #[tokio::test]
    async fn control_rejects_unknown_sources_without_sending_a_packet() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 0, 0, 0, 0, 0]))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        assert!(catalog.unwrap().is_empty());

        let unknown = ConsoleSourceId::new(42).unwrap();
        assert!(matches!(
            console
                .enable(ConsoleSourceSelector::Source(unknown))
                .await,
            Err(Error::Console(ConsoleError::UnknownSource(id))) if id == unknown
        ));
        assert!(uplink_receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn malformed_control_responses_are_protocol_errors() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let selector = catalog.unwrap().find("deck:bcCam").unwrap().selector();

        let firmware = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 2, vec![0, 0, 0, 0]))
                .await
                .unwrap();
        };
        let (result, ()) = join!(console.enable(selector), firmware);
        assert!(matches!(result, Err(Error::ProtocolError(_))));
    }

    #[tokio::test]
    async fn short_control_error_responses_preserve_the_errno() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let selector = catalog.unwrap().find("deck:bcCam").unwrap().selector();

        let firmware = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    CONTROL_CHANNEL,
                    vec![CONTROL_SET_ENABLED, libc::EINVAL as u8],
                ))
                .await
                .unwrap();
        };
        let (result, ()) = join!(console.enable(selector), firmware);
        assert!(matches!(
            result,
            Err(Error::Console(ConsoleError::CommandRejected {
                selector: rejected,
                enabled: true,
                errno,
            })) if rejected == selector && errno == libc::EINVAL as u8
        ));
    }

    #[tokio::test]
    async fn byte_stream_replays_history_then_continues_live() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            assert_eq!(uplink_receiver.recv_async().await.unwrap().get_data(), &[1]);
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            assert_eq!(
                uplink_receiver.recv_async().await.unwrap().get_data(),
                &[0, 0]
            );
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();

        downlink_sender
            .send_async(Packet::new(0, 1, [vec![0], b"retained ".to_vec()].concat()))
            .await
            .unwrap();
        let mut bytes = source.byte_stream(ConsoleHistory::Replay).await;
        assert_eq!(bytes.next().await.unwrap(), b"retained ");

        downlink_sender
            .send_async(Packet::new(0, 1, [vec![0], b"live".to_vec()].concat()))
            .await
            .unwrap();
        assert_eq!(bytes.next().await.unwrap(), b"live");
    }

    #[tokio::test]
    async fn text_stream_decodes_utf8_across_packets() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();
        let mut text = source.text_stream(ConsoleHistory::Live).await;

        downlink_sender
            .send_async(Packet::new(0, 1, vec![0, b'a', 0xe2]))
            .await
            .unwrap();
        downlink_sender
            .send_async(Packet::new(0, 1, vec![0, 0x82, 0xac, b'b']))
            .await
            .unwrap();

        assert_eq!(text.next().await.unwrap(), "a");
        assert_eq!(text.next().await.unwrap(), "€b");
    }

    #[tokio::test]
    async fn line_streams_assemble_multiple_lines_independently_per_source() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 2, 0, 0, 0, 0]))
                .await
                .unwrap();
            for (id, path) in [(0, "deck:bcCam"), (1, "cf:nRF51")] {
                let _ = uplink_receiver.recv_async().await.unwrap();
                downlink_sender
                    .send_async(Packet::new(
                        0,
                        3,
                        [vec![0, id], path.as_bytes().to_vec()].concat(),
                    ))
                    .await
                    .unwrap();
            }
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let catalog = catalog.unwrap();
        let camera = catalog.find("deck:bcCam").unwrap();
        let nrf = catalog.find("cf:nRF51").unwrap();
        let mut camera_lines = camera.line_stream(ConsoleHistory::Live).await;
        let mut nrf_lines = nrf.line_stream(ConsoleHistory::Live).await;

        for data in [
            [vec![0], b"cam ".to_vec()].concat(),
            [vec![1], b"nrf\n".to_vec()].concat(),
            [vec![0], b"line\nsecond\npartial".to_vec()].concat(),
        ] {
            downlink_sender
                .send_async(Packet::new(0, 1, data))
                .await
                .unwrap();
        }

        assert_eq!(nrf_lines.next().await.unwrap(), "nrf");
        assert_eq!(camera_lines.next().await.unwrap(), "cam line");
        assert_eq!(camera_lines.next().await.unwrap(), "second");
    }

    #[tokio::test]
    async fn text_stream_flushes_incomplete_utf8_and_ends_on_disconnect() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();
        let mut text = source.text_stream(ConsoleHistory::Live).await;

        downlink_sender
            .send_async(Packet::new(0, 1, vec![0, 0xe2]))
            .await
            .unwrap();
        drop(downlink_sender);

        assert_eq!(text.next().await.unwrap(), "�");
        assert_eq!(text.next().await, None);
    }

    #[tokio::test]
    async fn shutdown_waits_for_source_streams_to_close() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();
        let mut bytes = source.byte_stream(ConsoleHistory::Live).await;

        drop(downlink_sender);
        tokio::time::timeout(Duration::from_secs(1), console.shutdown())
            .await
            .expect("Console shutdown did not join its router task");

        assert_eq!(bytes.next().await, None);
        assert!(console.console_task.lock().await.is_none());
        assert!(console.transaction_task.lock().await.is_none());
    }

    #[tokio::test]
    async fn text_stream_is_lossy_and_live_does_not_replay_history() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();

        downlink_sender
            .send_async(Packet::new(0, 1, vec![0, b'o', b'l', b'd', 0xff]))
            .await
            .unwrap();
        let mut replay = source.text_stream(ConsoleHistory::Replay).await;
        assert_eq!(replay.next().await.unwrap(), "old�");

        let mut live = source.text_stream(ConsoleHistory::Live).await;
        downlink_sender
            .send_async(Packet::new(0, 1, [vec![0], b"new".to_vec()].concat()))
            .await
            .unwrap();
        assert_eq!(live.next().await.unwrap(), "new");
    }

    #[tokio::test]
    async fn slow_byte_consumer_keeps_its_connection_history_cursor() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();
        let mut bytes = source.byte_stream(ConsoleHistory::Live).await;

        for value in 0..1100_u16 {
            downlink_sender
                .send_async(Packet::new(0, 1, vec![0, (value % 251) as u8]))
                .await
                .unwrap();
        }
        for value in 0..1100_u16 {
            assert_eq!(bytes.next().await.unwrap(), vec![(value % 251) as u8]);
        }
    }

    #[tokio::test]
    async fn packets_from_unknown_source_ids_are_ignored() {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 13).await.unwrap();

        let discover = async {
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(0, 3, vec![1, 1, 0, 0, 0, 0]))
                .await
                .unwrap();
            let _ = uplink_receiver.recv_async().await.unwrap();
            downlink_sender
                .send_async(Packet::new(
                    0,
                    3,
                    [vec![0, 0], b"deck:bcCam".to_vec()].concat(),
                ))
                .await
                .unwrap();
        };
        let (catalog, ()) = join!(console.catalog(), discover);
        let source = catalog.unwrap().find("deck:bcCam").unwrap().clone();
        let mut bytes = source.byte_stream(ConsoleHistory::Live).await;

        downlink_sender
            .send_async(Packet::new(0, 1, [vec![42], b"wrong".to_vec()].concat()))
            .await
            .unwrap();
        downlink_sender
            .send_async(Packet::new(0, 1, [vec![0], b"right".to_vec()].concat()))
            .await
            .unwrap();
        assert_eq!(bytes.next().await.unwrap(), b"right");
    }

    #[tokio::test]
    async fn legacy_console_stream_still_works_on_protocol_12() {
        let (uplink, _uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        let console = Console::new(downlink, uplink, 12).await.unwrap();
        let mut legacy = console.stream_no_history().await;

        downlink_sender
            .send_async(Packet::new(0, 0, b"legacy".to_vec()))
            .await
            .unwrap();
        assert_eq!(legacy.next().await.unwrap(), "legacy");
    }
}
