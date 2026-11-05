//! # Console subsystem
//!
//! The Crazyflie exposes several text consoles used to send information and
//! debug messages to the ground. Each console has a path, such as `cf:stm32` for
//! the Crazyflie's STM32 or `deck:bcCam` for the Camera Deck.
//!
//! [Console::list()] returns the paths available on the connected Crazyflie and
//! [Console::open()] returns a [ConsoleHandle] that is ready to be read:
//!
//! ```no_run
//! use futures::StreamExt;
//!
//! # async fn show_consoles(cf: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
//! println!("Available consoles: {:?}", cf.console.list().await?);
//!
//! let mut console = cf.console.open("cf:stm32").await?;
//! let mut lines = console.lines();
//! while let Some(line) = lines.next().await {
//!     println!("{line}");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! A handle offers three views of the same output: complete
//! [lines](ConsoleHandle::lines()), incrementally decoded
//! [text](ConsoleHandle::text()) and raw [bytes](ConsoleHandle::bytes()). Text is
//! decoded as [UTF-8 lossy](String::from_utf8_lossy()), including characters split
//! across radio packets.
//!
//! ## History
//!
//! [Console::open()] first replays the console's output since connection, and then
//! continues with new output. This is needed to see startup messages.
//! [Console::open_live()] skips everything received before the console was opened.
//!
//! ## Closing a console
//!
//! Dropping the last [ConsoleHandle] of a console asks the Crazyflie to stop
//! sending its output. [ConsoleHandle::close()] does the same but waits for the
//! Crazyflie's answer and reports any error. Handles are independent: each has its
//! own reading position, and the output keeps flowing until the last one is closed.
//!
//! Closing does not discard anything. A later [Console::open()] replays the output
//! already received, and a source can keep buffering new output on its side, up to
//! its capacity, to deliver it when opened again.
//!
//! The `cf:stm32` console is the exception: the Crazyflie always sends it, on every
//! firmware version, and closing it only releases the handle.
//!
//! ## Output from before the console is opened
//!
//! When connecting, the library asks the Crazyflie to stop sending all consoles
//! other than `cf:stm32`. Output received before that takes effect can come from an
//! earlier connection. It is held and enters the console's history when the console
//! is first opened, though the firmware cannot replay packets lost over radio. The
//! library holds up to 64 KiB of such output and drops the oldest packets when full.
//!
//! The list of consoles is discovered once, on first use, and transparently retries
//! while the firmware reports that startup is not yet complete. Requests continue
//! internally if their calling future is dropped, keeping subsequent requests and
//! responses synchronized.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::crazyflie::CONSOLE_PORT;
use crate::{Error, Result};
use crazyflie_link::Packet;
use flume as channel;
use futures::{StreamExt, lock::Mutex, stream::BoxStream};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;

const SOURCED_CONSOLE_PROTOCOL_VERSION: u8 = 13;
const LOCAL_CHANNEL: u8 = 0;
const SOURCED_CHANNEL: u8 = 1;
const CONTROL_CHANNEL: u8 = 2;
const CONTROL_SET_ENABLED: u8 = 0;
const CATALOG_CHANNEL: u8 = 3;
const CATALOG_GET_ITEM: u8 = 0;
const CATALOG_GET_INFO: u8 = 1;
const ALL_SOURCES: u8 = u8::MAX;
const NOT_READY_RETRY_DELAY: Duration = Duration::from_millis(10);
const EARLY_SOURCE_BUFFER_LIMIT: usize = 64 * 1024;
/// Implicit path of channel 0, which the firmware does not list in its catalog.
const STM32_PATH: &str = "cf:stm32";

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

/// Where a new handle starts reading.
#[derive(Clone, Copy)]
enum StartAt {
    History,
    Live,
}

/// Sends the release command for a handle whose source the firmware can disable.
struct SourceRelease {
    id: u8,
    // Weak so that a handle outliving the Crazyflie does not keep the worker alive.
    commands: channel::WeakSender<ConsoleCommand>,
}

impl SourceRelease {
    fn send(self, reply: Option<oneshot::Sender<Result<()>>>) -> Result<()> {
        let commands = self.commands.upgrade().ok_or(Error::Disconnected)?;
        commands
            .send(ConsoleCommand::Release { id: self.id, reply })
            .map_err(|_| Error::Disconnected)
    }
}

/// An open console of the Crazyflie.
///
/// See the [console module documentation](crate::subsystems::console) for more
/// context. Output flows from the Crazyflie as long as a handle to this console
/// exists: use [ConsoleHandle::close()] or drop the handle to stop it.
///
/// The streams returned by this handle borrow it, so a stream can never outlive
/// the handle that keeps its output flowing:
///
/// ```compile_fail,E0505
/// # async fn outlive(cf: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
/// use futures::StreamExt;
///
/// let mut console = cf.console.open("deck:bcCam").await?;
/// let mut lines = console.lines();
/// drop(console);
/// lines.next().await;
/// # Ok(())
/// # }
/// ```
///
/// Each view keeps its position in the handle. A stream can therefore be dropped
/// and created again, or used in a `select!`, without losing or repeating output.
pub struct ConsoleHandle {
    path: Arc<str>,
    state: Arc<ConsoleSourceState>,
    release: Option<SourceRelease>,
    byte_cursor: usize,
    text_cursor: usize,
    line_cursor: usize,
}

impl std::fmt::Debug for ConsoleHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleHandle")
            .field("path", &self.path)
            .finish()
    }
}

impl ConsoleHandle {
    async fn new(
        path: Arc<str>,
        state: Arc<ConsoleSourceState>,
        release: Option<SourceRelease>,
        start: StartAt,
    ) -> Self {
        let (byte_cursor, text_cursor, line_cursor) = match start {
            StartAt::History => (0, 0, 0),
            StartAt::Live => {
                let history = state.history.lock().await;
                (history.bytes.len(), history.text.len(), history.lines.len())
            }
        };
        Self {
            path,
            state,
            release,
            byte_cursor,
            text_cursor,
            line_cursor,
        }
    }

    /// Returns the path of this console.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns a stream of complete lines. It ends when the Crazyflie disconnects.
    ///
    /// Newline delimiters are not included in returned strings. An incomplete
    /// final line remains buffered until a newline is received.
    pub fn lines(&mut self) -> BoxStream<'_, String> {
        history_stream(&self.state, &mut self.line_cursor, |history| &history.lines)
    }

    /// Returns a stream of incrementally decoded, loss-tolerant UTF-8 chunks. It
    /// ends when the Crazyflie disconnects.
    ///
    /// The chunk boundaries do not delimit lines or records.
    pub fn text(&mut self) -> BoxStream<'_, String> {
        history_stream(&self.state, &mut self.text_cursor, |history| &history.text)
    }

    /// Returns a stream of received byte chunks. It ends when the Crazyflie
    /// disconnects.
    ///
    /// Chunk boundaries are transport artifacts and do not delimit text or lines.
    pub fn bytes(&mut self) -> BoxStream<'_, Vec<u8>> {
        history_stream(&self.state, &mut self.byte_cursor, |history| &history.bytes)
    }

    /// Closes this handle and waits for the outcome.
    ///
    /// This ends in the same state as dropping the handle, but reports errors that
    /// a drop has to ignore. If this was the last handle of a console the firmware
    /// can disable, a successful return means that the firmware stopped sending
    /// it, and that all output it sent before stopping and that was not lost over
    /// radio has entered the console's history.
    ///
    /// Returns [Error::Disconnected] if the Crazyflie is no longer connected. The
    /// request still completes if the returned future is dropped.
    pub async fn close(mut self) -> Result<()> {
        let Some(release) = self.release.take() else {
            return Ok(());
        };
        let (reply, result) = oneshot::channel();
        release.send(Some(reply))?;
        result.await.map_err(|_| Error::Disconnected)?
    }
}

impl Drop for ConsoleHandle {
    /// Queues the same request as [ConsoleHandle::close()] without waiting for it.
    /// A failure cannot be reported: the output keeps flowing into the history.
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(None);
        }
    }
}

fn history_stream<'a, T>(
    state: &'a ConsoleSourceState,
    cursor: &'a mut usize,
    items: for<'h> fn(&'h ConsoleSourceHistory) -> &'h [T],
) -> BoxStream<'a, T>
where
    T: Clone + Send + 'a,
{
    async_stream::stream! {
        loop {
            let notified = state.notify.notified();
            // Check closure under the lock: closing can add a final text item.
            let (next, closed) = {
                let history = state.history.lock().await;
                (
                    items(&history).get(*cursor).cloned(),
                    state.closed.load(Ordering::Relaxed),
                )
            };
            if let Some(item) = next {
                *cursor += 1;
                yield item;
            } else if closed {
                break;
            } else {
                notified.await;
            }
        }
    }
    .boxed()
}

/// Error specific to Console operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConsoleError {
    /// The Crazyflie has no console with this path.
    UnknownSource(String),
    /// The firmware rejected an enable or disable command.
    CommandRejected {
        /// Path of the console, or `None` for the command sent to all consoles
        /// when connecting.
        path: Option<String>,
        /// Requested enabled state.
        enabled: bool,
        /// Raw errno-compatible result returned by the firmware.
        errno: u8,
    },
    /// The firmware rejected a request listing its consoles.
    CatalogCommandRejected {
        /// Operation sent to the firmware.
        operation: ConsoleCatalogOperation,
        /// Raw errno-compatible result returned by the firmware.
        errno: u8,
    },
}

/// Operation listing the firmware's consoles that a firmware error can answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleCatalogOperation {
    /// Read the number of consoles.
    GetInfo,
    /// Read the entry at this position.
    GetItem(u8),
}

impl std::fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSource(path) => write!(f, "unknown Console {path:?}"),
            Self::CommandRejected {
                path,
                enabled,
                errno,
            } => {
                let path = path.as_deref().unwrap_or("all consoles");
                write!(
                    f,
                    "firmware rejected {path:?} enabled={enabled} with errno {errno}"
                )
            }
            Self::CatalogCommandRejected { operation, errno } => write!(
                f,
                "firmware rejected Console catalog operation {operation:?} with errno {errno}"
            ),
        }
    }
}

impl std::error::Error for ConsoleError {}

enum ConsoleCommand {
    ListPaths {
        reply: oneshot::Sender<Result<Vec<String>>>,
    },
    // A handle travels in the reply so that its Drop releases the source if the
    // caller is gone, including when it goes away after the reply was sent.
    Open {
        path: String,
        start: StartAt,
        reply: oneshot::Sender<Result<ConsoleHandle>>,
    },
    Release {
        id: u8,
        reply: Option<oneshot::Sender<Result<()>>>,
    },
    Shutdown,
}

/// One entry of the firmware's catalog. Its position in the catalog is its ID.
struct CatalogEntry {
    path: Arc<str>,
    state: Arc<ConsoleSourceState>,
    handles: usize,
}

// Registration, release of early packets, and router closure must be serialized.
#[derive(Default)]
struct ConsoleSourceRegistry {
    states: BTreeMap<u8, Arc<ConsoleSourceState>>,
    early_packets: VecDeque<(u8, Vec<u8>)>,
    early_bytes: usize,
    held_sources: BTreeSet<u8>,
    catalog_known: bool,
    closed: bool,
}

impl ConsoleSourceRegistry {
    fn packet_size(bytes: &[u8]) -> usize {
        bytes.len() + std::mem::size_of::<(u8, Vec<u8>)>()
    }

    fn hold_early(&mut self, source_id: u8, bytes: &[u8]) {
        self.held_sources.insert(source_id);
        let size = Self::packet_size(bytes);
        self.early_packets.push_back((source_id, bytes.to_vec()));
        self.early_bytes += size;
        while self.early_bytes > EARLY_SOURCE_BUFFER_LIMIT {
            if let Some((_, removed)) = self.early_packets.pop_front() {
                self.early_bytes -= Self::packet_size(&removed);
            }
        }
    }

    async fn release(&mut self, id: u8) -> Result<()> {
        if self.closed {
            return Err(Error::Disconnected);
        }
        if !self.held_sources.remove(&id) {
            return Ok(());
        }
        let mut remaining = VecDeque::new();
        while let Some((packet_id, bytes)) = self.early_packets.pop_front() {
            if packet_id == id {
                self.early_bytes -= Self::packet_size(&bytes);
                if let Some(state) = self.states.get(&id) {
                    state.push_bytes(&bytes).await;
                }
            } else {
                remaining.push_back((packet_id, bytes));
            }
        }
        self.early_packets = remaining;
        Ok(())
    }
}

struct ConsoleTransactionWorker {
    uplink: channel::Sender<Packet>,
    control_downlink: channel::Receiver<Packet>,
    catalog_downlink: channel::Receiver<Packet>,
    protocol_version: u8,
    source_states: Arc<Mutex<ConsoleSourceRegistry>>,
    catalog: Option<Vec<CatalogEntry>>,
    commands: channel::WeakSender<ConsoleCommand>,
}

enum SetEnabledResponse {
    CommandError(u8),
    Success,
}

fn parse_set_enabled_response(response: &Packet, request: &[u8; 3]) -> Result<SetEnabledResponse> {
    let data = response.get_data();
    if data.len() == 2 && data[0] == CONTROL_SET_ENABLED && data[1] != 0 {
        return Ok(SetEnabledResponse::CommandError(data[1]));
    }
    if data.len() != 4
        || data[0] != CONTROL_SET_ENABLED
        || data[1] != 0
        || data[2..] != request[1..]
    {
        return Err(Error::ProtocolError(
            "Malformed sourced Console control response".to_owned(),
        ));
    }
    Ok(SetEnabledResponse::Success)
}

async fn set_enabled_raw(
    uplink: &channel::Sender<Packet>,
    control_downlink: &channel::Receiver<Packet>,
    wire_id: u8,
    enabled: bool,
) -> Result<SetEnabledResponse> {
    let request = [CONTROL_SET_ENABLED, wire_id, u8::from(enabled)];
    uplink
        .send_async(Packet::new(CONSOLE_PORT, CONTROL_CHANNEL, request.to_vec()))
        .await
        .map_err(|_| Error::Disconnected)?;

    let response = control_downlink
        .recv_async()
        .await
        .map_err(|_| Error::Disconnected)?;
    parse_set_enabled_response(&response, &request)
}

fn command_rejected(path: Option<&str>, enabled: bool, errno: u8) -> Error {
    Error::Console(ConsoleError::CommandRejected {
        path: path.map(str::to_owned),
        enabled,
        errno,
    })
}

async fn initialize_sourced_console(
    uplink: &channel::Sender<Packet>,
    control_downlink: &channel::Receiver<Packet>,
    protocol_version: u8,
) -> Result<()> {
    if protocol_version < SOURCED_CONSOLE_PROTOCOL_VERSION {
        return Ok(());
    }

    loop {
        match set_enabled_raw(uplink, control_downlink, ALL_SOURCES, false).await? {
            SetEnabledResponse::Success => return Ok(()),
            SetEnabledResponse::CommandError(errno) if errno == crate::firmware_errno::EAGAIN => {
                tokio::time::sleep(NOT_READY_RETRY_DELAY).await;
            }
            SetEnabledResponse::CommandError(errno) => {
                return Err(command_rejected(None, false, errno));
            }
        }
    }
}

impl ConsoleTransactionWorker {
    async fn run(mut self, commands: channel::Receiver<ConsoleCommand>) {
        while let Ok(command) = commands.recv_async().await {
            match command {
                ConsoleCommand::ListPaths { reply } => {
                    let result = self.catalog().await.map(|catalog| {
                        catalog.iter().map(|entry| entry.path.to_string()).collect()
                    });
                    let _ = reply.send(result);
                }
                ConsoleCommand::Open { path, start, reply } => {
                    let result = self.open(&path, start).await;
                    let _ = reply.send(result);
                }
                ConsoleCommand::Release { id, reply } => {
                    let result = self.release(id).await;
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
                ConsoleCommand::Shutdown => break,
            }
        }
    }

    async fn catalog(&mut self) -> Result<&mut Vec<CatalogEntry>> {
        if self.catalog.is_none() {
            self.catalog = Some(self.discover_catalog().await?);
        }
        Ok(self.catalog.as_mut().expect("catalog was just discovered"))
    }

    async fn open(&mut self, path: &str, start: StartAt) -> Result<ConsoleHandle> {
        let catalog = self.catalog().await?;
        let position = catalog
            .iter()
            .position(|entry| &*entry.path == path)
            .ok_or_else(|| Error::Console(ConsoleError::UnknownSource(path.to_owned())))?;
        let id = u8::try_from(position).expect("catalog has at most 255 entries");

        let already_enabled = catalog[position].handles > 0;
        let entry_path = catalog[position].path.clone();
        let state = catalog[position].state.clone();

        if !already_enabled {
            self.set_enabled(id, path, true).await?;
            self.source_states.lock().await.release(id).await?;
        }

        // Counted before the handle exists so that its Drop always has a match.
        self.catalog.as_mut().expect("catalog is known")[position].handles += 1;
        let release = SourceRelease {
            id,
            commands: self.commands.clone(),
        };
        Ok(ConsoleHandle::new(entry_path, state, Some(release), start).await)
    }

    async fn release(&mut self, id: u8) -> Result<()> {
        let Some(entry) = self
            .catalog
            .as_mut()
            .and_then(|catalog| catalog.get_mut(usize::from(id)))
        else {
            return Ok(());
        };
        entry.handles -= 1;
        if entry.handles > 0 {
            return Ok(());
        }
        let path = entry.path.clone();
        self.set_enabled(id, &path, false).await
    }

    async fn set_enabled(&self, id: u8, path: &str, enabled: bool) -> Result<()> {
        match set_enabled_raw(&self.uplink, &self.control_downlink, id, enabled).await? {
            SetEnabledResponse::Success => Ok(()),
            SetEnabledResponse::CommandError(errno) => {
                Err(command_rejected(Some(path), enabled, errno))
            }
        }
    }

    async fn discover_catalog(&self) -> Result<Vec<CatalogEntry>> {
        if self.protocol_version < SOURCED_CONSOLE_PROTOCOL_VERSION {
            return Ok(Vec::new());
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
            if data.len() == 2 && data[0] == CATALOG_GET_INFO && data[1] != 0 {
                if data[1] == crate::firmware_errno::EAGAIN {
                    tokio::time::sleep(NOT_READY_RETRY_DELAY).await;
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
        if data.len() != 7 || data[0] != CATALOG_GET_INFO || data[1] != 0 {
            return Err(Error::ProtocolError(
                "Malformed sourced Console catalog info response".to_owned(),
            ));
        }

        let source_count = data[2];
        let mut sources: Vec<CatalogEntry> = Vec::with_capacity(source_count.into());

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
                if data.len() == 2 && data[0] == CATALOG_GET_ITEM && data[1] != 0 {
                    if data[1] == crate::firmware_errno::EAGAIN {
                        tokio::time::sleep(NOT_READY_RETRY_DELAY).await;
                        continue;
                    }
                    return Err(Error::Console(ConsoleError::CatalogCommandRejected {
                        operation: ConsoleCatalogOperation::GetItem(id),
                        errno: data[1],
                    }));
                }
                break item;
            };
            let data = item.get_data();
            if data.len() < 4 || data[0] != CATALOG_GET_ITEM || data[1] != 0 || data[2] != id {
                return Err(Error::ProtocolError(
                    "Malformed sourced Console catalog item response".to_owned(),
                ));
            }

            let path = std::str::from_utf8(&data[3..]).map_err(|_| {
                Error::ProtocolError("Sourced Console catalog path is not valid UTF-8".to_owned())
            })?;
            if path.split(':').any(str::is_empty) {
                return Err(Error::ProtocolError(
                    "Sourced Console catalog path has invalid segments".to_owned(),
                ));
            }
            if path == STM32_PATH || sources.iter().any(|source| &*source.path == path) {
                return Err(Error::ProtocolError(
                    "Sourced Console catalog contains a duplicate path".to_owned(),
                ));
            }
            sources.push(CatalogEntry {
                path: Arc::from(path),
                state: Arc::default(),
                handles: 0,
            });
        }

        {
            let mut registry = self.source_states.lock().await;
            if registry.closed {
                return Err(Error::Disconnected);
            }
            for (id, source) in (0..source_count).zip(&sources) {
                registry.states.insert(id, source.state.clone());
            }
            registry.catalog_known = true;
            registry.early_packets.retain(|(id, _)| *id < source_count);
            registry.held_sources.retain(|id| *id < source_count);
            registry.early_bytes = registry
                .early_packets
                .iter()
                .map(|(_, bytes)| ConsoleSourceRegistry::packet_size(bytes))
                .sum();
        }

        Ok(sources)
    }
}

struct ConsoleConstructionGuard {
    console_task: Option<JoinHandle<()>>,
}

impl ConsoleConstructionGuard {
    fn new(console_task: JoinHandle<()>) -> Self {
        Self {
            console_task: Some(console_task),
        }
    }

    fn disarm(mut self) -> JoinHandle<()> {
        self.console_task
            .take()
            .expect("Console construction guard must own its task")
    }
}

impl Drop for ConsoleConstructionGuard {
    fn drop(&mut self) {
        if let Some(task) = self.console_task.take() {
            task.abort();
        }
    }
}

/// # Access to the console subsystem
///
/// See the [console module documentation](crate::subsystems::console) for more context and information.
pub struct Console {
    stm32_state: Arc<ConsoleSourceState>,
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
        let (control_sender, control_downlink) = channel::unbounded();
        let (catalog_sender, catalog_downlink) = channel::unbounded();

        let stm32_state: Arc<ConsoleSourceState> = Arc::default();
        let routed_stm32_state = stm32_state.clone();
        let source_states: Arc<Mutex<ConsoleSourceRegistry>> = Default::default();
        let routed_source_states = source_states.clone();

        // Keep every port-0 packet in its original receive order. In particular,
        // processing sourced data before forwarding a later control response
        // preserves the firmware's successful-disable ordering barrier.
        let console_task = tokio::spawn(async move {
            while let Ok(packet) = downlink.recv_async().await {
                match packet.get_channel() {
                    LOCAL_CHANNEL => routed_stm32_state.push_bytes(packet.get_data()).await,
                    SOURCED_CHANNEL => {
                        let data = packet.get_data();
                        let Some((&source_id, bytes)) = data.split_first() else {
                            continue;
                        };
                        let mut registry = routed_source_states.lock().await;
                        if !registry.catalog_known || registry.held_sources.contains(&source_id) {
                            registry.hold_early(source_id, bytes);
                        } else if let Some(state) = registry.states.get(&source_id) {
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

            routed_stm32_state.close().await;
            let states: Vec<_> = {
                let mut registry = routed_source_states.lock().await;
                registry.closed = true;
                registry.states.values().cloned().collect()
            };
            for state in states {
                state.close().await;
            }
        });

        let console_task_guard = ConsoleConstructionGuard::new(console_task);
        initialize_sourced_console(&uplink, &control_downlink, protocol_version).await?;

        let (command_sender, command_receiver) = channel::unbounded();
        let transaction_task = tokio::spawn(
            ConsoleTransactionWorker {
                uplink,
                control_downlink,
                catalog_downlink,
                protocol_version,
                source_states,
                catalog: None,
                commands: command_sender.downgrade(),
            }
            .run(command_receiver),
        );
        let console_task = console_task_guard.disarm();

        Ok(Self {
            stm32_state,
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

    /// Returns the path of every console of the connected Crazyflie.
    ///
    /// The list is discovered once and cached. `cf:stm32` is available on every
    /// firmware version; other consoles require CRTP protocol 13.
    pub async fn list(&self) -> Result<Vec<String>> {
        let (reply, result) = oneshot::channel();
        self.command_sender
            .send(ConsoleCommand::ListPaths { reply })
            .map_err(|_| Error::Disconnected)?;
        let sources = result.await.map_err(|_| Error::Disconnected)??;
        Ok(std::iter::once(STM32_PATH.to_owned())
            .chain(sources)
            .collect())
    }

    /// Opens a console, replaying its output since connection before new output.
    ///
    /// The console is flowing when this returns: there is nothing to start.
    /// Returns [ConsoleError::UnknownSource] if the Crazyflie has no such console.
    pub async fn open(&self, path: &str) -> Result<ConsoleHandle> {
        self.open_at(path, StartAt::History).await
    }

    /// Version of [Console::open()] that skips anything received before the
    /// console was opened.
    ///
    /// The first text item may finish a character, and the first line may finish
    /// a line, that began before that point.
    pub async fn open_live(&self, path: &str) -> Result<ConsoleHandle> {
        self.open_at(path, StartAt::Live).await
    }

    async fn open_at(&self, path: &str, start: StartAt) -> Result<ConsoleHandle> {
        if path == STM32_PATH {
            let state = self.stm32_state.clone();
            return Ok(ConsoleHandle::new(Arc::from(STM32_PATH), state, None, start).await);
        }

        let (reply, result) = oneshot::channel();
        self.command_sender
            .send(ConsoleCommand::Open {
                path: path.to_owned(),
                start,
                reply,
            })
            .map_err(|_| Error::Disconnected)?;
        result.await.map_err(|_| Error::Disconnected)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::join;

    const CAMERA: &str = "deck:bcCam";
    const NRF: &str = "cf:nRF51";
    const QUIET: Duration = Duration::from_millis(20);
    const PATIENCE: Duration = Duration::from_secs(1);

    /// The firmware end of the link.
    struct Firmware {
        uplink: channel::Receiver<Packet>,
        downlink: channel::Sender<Packet>,
    }

    impl Firmware {
        async fn send(&self, channel: u8, data: Vec<u8>) {
            self.downlink
                .send_async(Packet::new(CONSOLE_PORT, channel, data))
                .await
                .unwrap();
        }

        async fn expect(&self, channel: u8, data: &[u8]) {
            let request = tokio::time::timeout(PATIENCE, self.uplink.recv_async())
                .await
                .expect("the firmware did not receive the expected request")
                .unwrap();
            assert_eq!(request.get_channel(), channel);
            assert_eq!(request.get_data(), data);
        }

        async fn source_output(&self, id: u8, bytes: &[u8]) {
            self.send(SOURCED_CHANNEL, [&[id], bytes].concat()).await;
        }

        async fn answer_catalog(&self, paths: &[&str]) {
            self.expect(CATALOG_CHANNEL, &[CATALOG_GET_INFO]).await;
            self.send(
                CATALOG_CHANNEL,
                vec![CATALOG_GET_INFO, 0, paths.len() as u8, 0, 0, 0, 0],
            )
            .await;
            for (id, path) in paths.iter().enumerate() {
                let id = id as u8;
                self.expect(CATALOG_CHANNEL, &[CATALOG_GET_ITEM, id]).await;
                self.send(
                    CATALOG_CHANNEL,
                    [&[CATALOG_GET_ITEM, 0, id], path.as_bytes()].concat(),
                )
                .await;
            }
        }

        async fn answer_control(&self, id: u8, enabled: u8) {
            self.expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, id, enabled])
                .await;
            self.send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, id, enabled])
                .await;
        }

        async fn assert_silent(&self) {
            assert!(
                tokio::time::timeout(QUIET, self.uplink.recv_async())
                    .await
                    .is_err(),
                "the firmware received an unexpected request"
            );
        }
    }

    fn link() -> (channel::Receiver<Packet>, channel::Sender<Packet>, Firmware) {
        let (uplink, uplink_receiver) = channel::unbounded();
        let (downlink_sender, downlink) = channel::unbounded();
        (
            downlink,
            uplink,
            Firmware {
                uplink: uplink_receiver,
                downlink: downlink_sender,
            },
        )
    }

    async fn connect() -> (Console, Firmware) {
        let (downlink, uplink, firmware) = link();
        let answer = firmware.answer_control(ALL_SOURCES, 0);
        let (console, ()) = join!(Console::new(downlink, uplink, 13), answer);
        (console.unwrap(), firmware)
    }

    /// Discovers `catalog` on `console` and opens `path` from it.
    async fn open_first(
        console: &Console,
        firmware: &Firmware,
        catalog: &[&str],
        path: &str,
    ) -> ConsoleHandle {
        let id = catalog.iter().position(|entry| *entry == path).unwrap() as u8;
        let answer = async {
            firmware.answer_catalog(catalog).await;
            firmware.answer_control(id, 1).await;
        };
        let (handle, ()) = join!(console.open(path), answer);
        handle.unwrap()
    }

    async fn connect_and_open_camera() -> (Console, Firmware, ConsoleHandle) {
        let (console, firmware) = connect().await;
        let handle = open_first(&console, &firmware, &[CAMERA], CAMERA).await;
        (console, firmware, handle)
    }

    async fn assert_router_stopped(downlink_sender: &channel::Sender<Packet>) {
        tokio::time::timeout(Duration::from_millis(250), async {
            while !downlink_sender.is_disconnected() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Console router task was not stopped");
    }

    #[tokio::test]
    async fn list_includes_stm32_and_the_catalog_and_caches_discovery() {
        let (console, firmware) = connect().await;

        let (paths, ()) = join!(console.list(), firmware.answer_catalog(&[CAMERA, NRF]));
        assert_eq!(paths.unwrap(), ["cf:stm32", CAMERA, NRF]);

        assert_eq!(console.list().await.unwrap(), ["cf:stm32", CAMERA, NRF]);
        firmware.assert_silent().await;
    }

    #[tokio::test]
    async fn protocol_12_lists_only_stm32_and_sends_nothing() {
        let (downlink, uplink, firmware) = link();
        let console = Console::new(downlink, uplink, 12).await.unwrap();

        assert_eq!(console.list().await.unwrap(), ["cf:stm32"]);
        firmware.assert_silent().await;
    }

    #[tokio::test]
    async fn stm32_works_on_protocol_12_and_assembles_several_lines_per_packet() {
        let (downlink, uplink, firmware) = link();
        let console = Console::new(downlink, uplink, 12).await.unwrap();

        firmware
            .send(LOCAL_CHANNEL, b"first\nsecond\npar".to_vec())
            .await;
        let mut stm32 = console.open("cf:stm32").await.unwrap();
        assert_eq!(stm32.path(), "cf:stm32");
        firmware.send(LOCAL_CHANNEL, b"tial\n".to_vec()).await;

        let mut lines = stm32.lines();
        assert_eq!(lines.next().await.unwrap(), "first");
        assert_eq!(lines.next().await.unwrap(), "second");
        assert_eq!(lines.next().await.unwrap(), "partial");
    }

    #[tokio::test]
    async fn stm32_cannot_be_disabled_so_closing_sends_nothing() {
        let (console, firmware) = connect().await;

        console
            .open("cf:stm32")
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        drop(console.open("cf:stm32").await.unwrap());
        firmware.assert_silent().await;

        firmware.send(LOCAL_CHANNEL, b"kept\n".to_vec()).await;
        let mut reopened = console.open("cf:stm32").await.unwrap();
        assert_eq!(reopened.lines().next().await.unwrap(), "kept");
    }

    #[tokio::test]
    async fn opening_stm32_does_not_wait_for_catalog_discovery() {
        let (console, firmware) = connect().await;

        let discovery = console.list();
        tokio::pin!(discovery);
        tokio::select! {
            () = firmware.expect(CATALOG_CHANNEL, &[CATALOG_GET_INFO]) => {}
            result = discovery.as_mut() => panic!("list returned early: {result:?}"),
        }

        tokio::time::timeout(PATIENCE, console.open("cf:stm32"))
            .await
            .expect("cf:stm32 waited for the catalog")
            .unwrap();
    }

    #[tokio::test]
    async fn firmware_cannot_redefine_the_stm32_path() {
        let (console, firmware) = connect().await;

        let (paths, ()) = join!(console.list(), firmware.answer_catalog(&["cf:stm32"]));
        assert!(matches!(paths, Err(Error::ProtocolError(_))));
    }

    #[tokio::test]
    async fn opening_needs_no_start_and_reports_unknown_paths_without_traffic() {
        let (console, firmware) = connect().await;

        let (result, ()) = join!(
            console.open("deck:other"),
            firmware.answer_catalog(&[CAMERA])
        );
        assert!(matches!(
            result,
            Err(Error::Console(ConsoleError::UnknownSource(path))) if path == "deck:other"
        ));
        firmware.assert_silent().await;

        let (camera, ()) = join!(console.open(CAMERA), firmware.answer_control(0, 1));
        let mut camera = camera.unwrap();
        firmware.source_output(0, b"hello\n").await;
        assert_eq!(camera.lines().next().await.unwrap(), "hello");
    }

    #[tokio::test]
    async fn dropping_the_handle_disables_the_source() {
        let (_console, firmware, camera) = connect_and_open_camera().await;

        drop(camera);
        firmware.answer_control(0, 0).await;
    }

    #[tokio::test]
    async fn close_waits_for_the_firmware_and_reports_its_errors() {
        let (_console, firmware, camera) = connect_and_open_camera().await;

        let answer = async {
            firmware
                .expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 0])
                .await;
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 5])
                .await;
        };
        let (result, ()) = join!(camera.close(), answer);
        assert!(matches!(
            result,
            Err(Error::Console(ConsoleError::CommandRejected {
                path: Some(path),
                enabled: false,
                errno: 5,
            })) if path == CAMERA
        ));
    }

    #[tokio::test]
    async fn close_does_not_also_send_the_drop_disable() {
        let (_console, firmware, camera) = connect_and_open_camera().await;

        let (result, ()) = join!(camera.close(), firmware.answer_control(0, 0));
        result.unwrap();
        firmware.assert_silent().await;
    }

    #[tokio::test]
    async fn source_stays_enabled_until_its_last_handle_is_gone() {
        let (console, firmware, first) = connect_and_open_camera().await;

        let mut second = console.open(CAMERA).await.unwrap();
        let third = console.open(CAMERA).await.unwrap();
        firmware.assert_silent().await;

        first.close().await.unwrap();
        drop(third);
        firmware.assert_silent().await;

        firmware.source_output(0, b"still flowing\n").await;
        assert_eq!(second.lines().next().await.unwrap(), "still flowing");

        let (result, ()) = join!(second.close(), firmware.answer_control(0, 0));
        result.unwrap();
    }

    #[tokio::test]
    async fn concurrent_opens_of_one_source_enable_it_once() {
        let (console, firmware) = connect().await;
        let (paths, ()) = join!(console.list(), firmware.answer_catalog(&[CAMERA]));
        paths.unwrap();

        let (first, second, ()) = join!(
            console.open(CAMERA),
            console.open(CAMERA),
            firmware.answer_control(0, 1)
        );
        first.unwrap();
        second.unwrap();
    }

    #[tokio::test]
    async fn failed_enable_leaves_the_source_closed() {
        let (console, firmware) = connect().await;

        let answer = async {
            firmware.answer_catalog(&[CAMERA]).await;
            firmware
                .expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 1])
                .await;
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 22])
                .await;
        };
        let (result, ()) = join!(console.open(CAMERA), answer);
        assert!(matches!(
            result,
            Err(Error::Console(ConsoleError::CommandRejected {
                path: Some(path),
                enabled: true,
                errno: 22,
            })) if path == CAMERA
        ));

        // No disable for the failed open, and the next open enables again.
        let (retry, ()) = join!(console.open(CAMERA), firmware.answer_control(0, 1));
        retry.unwrap();
    }

    #[tokio::test]
    async fn drop_then_immediate_open_keeps_the_source_enabled_in_order() {
        let (console, firmware, camera) = connect_and_open_camera().await;

        drop(camera);
        let answer = async {
            firmware.answer_control(0, 0).await;
            firmware.answer_control(0, 1).await;
        };
        let (reopened, ()) = join!(console.open(CAMERA), answer);
        reopened.unwrap();
    }

    #[tokio::test]
    async fn reopening_replays_history_and_open_live_skips_it() {
        let (console, firmware, mut camera) = connect_and_open_camera().await;
        firmware.source_output(0, b"old\n").await;
        assert_eq!(camera.lines().next().await.unwrap(), "old");

        let (closed, ()) = join!(camera.close(), firmware.answer_control(0, 0));
        closed.unwrap();

        let (replay, ()) = join!(console.open(CAMERA), firmware.answer_control(0, 1));
        let mut replay = replay.unwrap();
        let mut live = console.open_live(CAMERA).await.unwrap();
        firmware.source_output(0, b"new\n").await;

        let mut replayed = replay.lines();
        assert_eq!(replayed.next().await.unwrap(), "old");
        assert_eq!(replayed.next().await.unwrap(), "new");
        assert_eq!(live.lines().next().await.unwrap(), "new");
    }

    #[tokio::test]
    async fn handles_read_independently() {
        let (console, firmware, mut first) = connect_and_open_camera().await;
        let mut second = console.open(CAMERA).await.unwrap();

        firmware.source_output(0, b"a\nb\n").await;
        assert_eq!(first.lines().next().await.unwrap(), "a");
        assert_eq!(second.lines().next().await.unwrap(), "a");
        assert_eq!(first.lines().next().await.unwrap(), "b");
        assert_eq!(second.lines().next().await.unwrap(), "b");
    }

    #[tokio::test]
    async fn recreating_a_stream_neither_loses_nor_repeats_output() {
        let (_console, firmware, mut camera) = connect_and_open_camera().await;

        firmware.source_output(0, b"one\ntwo\nthree\n").await;
        assert_eq!(camera.lines().next().await.unwrap(), "one");
        assert_eq!(camera.lines().next().await.unwrap(), "two");
        assert!(
            tokio::time::timeout(QUIET, async {
                let mut lines = camera.lines();
                assert_eq!(lines.next().await.unwrap(), "three");
                lines.next().await
            })
            .await
            .is_err(),
            "the stream produced output that was never sent"
        );

        firmware.source_output(0, b"four\n").await;
        assert_eq!(camera.lines().next().await.unwrap(), "four");
    }

    #[tokio::test]
    async fn views_keep_separate_positions() {
        let (_console, firmware, mut camera) = connect_and_open_camera().await;

        firmware.source_output(0, b"line\n").await;
        assert_eq!(camera.bytes().next().await.unwrap(), b"line\n");
        assert_eq!(camera.text().next().await.unwrap(), "line\n");
        assert_eq!(camera.lines().next().await.unwrap(), "line");
    }

    #[tokio::test]
    async fn cancelled_open_does_not_leave_the_source_enabled() {
        let (console, firmware) = connect().await;
        let (paths, ()) = join!(console.list(), firmware.answer_catalog(&[CAMERA]));
        paths.unwrap();

        {
            let open = console.open(CAMERA);
            tokio::pin!(open);
            tokio::select! {
                () = firmware.expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 1]) => {}
                result = open.as_mut() => panic!("open returned early: {result:?}"),
            }
        }
        firmware
            .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, 0, 1])
            .await;

        firmware.answer_control(0, 0).await;
    }

    #[tokio::test]
    async fn cancelled_close_still_disables_the_source() {
        let (console, firmware, camera) = connect_and_open_camera().await;

        {
            let close = camera.close();
            tokio::pin!(close);
            tokio::select! {
                () = firmware.expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 0]) => {}
                result = close.as_mut() => panic!("close returned early: {result:?}"),
            }
        }
        firmware
            .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, 0, 0])
            .await;

        let (reopened, ()) = join!(console.open(CAMERA), firmware.answer_control(0, 1));
        reopened.unwrap();
    }

    #[tokio::test]
    async fn control_commands_are_serialized() {
        let (console, firmware) = connect().await;
        let (paths, ()) = join!(console.list(), firmware.answer_catalog(&[CAMERA, NRF]));
        paths.unwrap();

        let answer = async {
            firmware
                .expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 1])
                .await;
            firmware.assert_silent().await;
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, 0, 1])
                .await;
            firmware.answer_control(1, 1).await;
        };
        let (first, second, ()) = join!(console.open(CAMERA), console.open(NRF), answer);
        first.unwrap();
        second.unwrap();
    }

    #[tokio::test]
    async fn close_waits_until_earlier_source_packets_are_processed() {
        let (_console, firmware, camera) = connect_and_open_camera().await;
        let state = camera.state.clone();

        let history_guard = state.history.lock().await;
        let answer = async {
            firmware
                .expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 0])
                .await;
            firmware.source_output(0, b"before disable\n").await;
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, 0, 0])
                .await;
        };
        let close = camera.close();
        tokio::pin!(close);
        tokio::pin!(answer);

        tokio::select! {
            () = answer.as_mut() => {}
            result = close.as_mut() => panic!("close returned too early: {result:?}"),
        }
        assert!(
            tokio::time::timeout(QUIET, close.as_mut()).await.is_err(),
            "close returned before the earlier source packet was processed"
        );

        drop(history_guard);
        tokio::time::timeout(PATIENCE, close)
            .await
            .expect("close did not finish after source processing resumed")
            .unwrap();
        assert_eq!(state.history.lock().await.lines, ["before disable"]);
    }

    #[tokio::test]
    async fn early_output_enters_history_when_its_source_is_first_opened() {
        let (downlink, uplink, firmware) = link();
        let construction = Console::new(downlink, uplink, 13);
        tokio::pin!(construction);
        tokio::select! {
            () = firmware.expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, ALL_SOURCES, 0]) => {}
            _ = construction.as_mut() => panic!("Console returned before initialization"),
        }
        assert!(
            firmware.uplink.try_recv().is_err(),
            "catalog discovery started before initialization completed"
        );

        firmware.source_output(0, b"stale one").await;
        firmware.source_output(0, b"stale two").await;
        firmware.source_output(42, b"unknown").await;
        firmware.source_output(1, b"other source").await;
        firmware
            .send(
                CONTROL_CHANNEL,
                vec![CONTROL_SET_ENABLED, 0, ALL_SOURCES, 0],
            )
            .await;
        let console = construction.await.unwrap();

        let (paths, ()) = join!(console.list(), firmware.answer_catalog(&[CAMERA, NRF]));
        paths.unwrap();
        firmware.source_output(0, b"after catalog").await;

        let answer = async {
            firmware
                .expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 1])
                .await;
            firmware.source_output(0, b"before reply").await;
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, 0, 1])
                .await;
        };
        let (camera, ()) = join!(console.open(CAMERA), answer);
        let mut camera = camera.unwrap();
        firmware.source_output(0, b"fresh").await;

        let mut bytes = camera.bytes();
        for expected in [
            b"stale one".as_slice(),
            b"stale two",
            b"after catalog",
            b"before reply",
            b"fresh",
        ] {
            assert_eq!(bytes.next().await.unwrap(), expected);
        }

        let (nrf, ()) = join!(console.open(NRF), firmware.answer_control(1, 1));
        assert_eq!(nrf.unwrap().bytes().next().await.unwrap(), b"other source");
    }

    #[tokio::test]
    async fn open_live_skips_early_output() {
        let (console, firmware) = connect().await;
        firmware.source_output(0, b"early").await;

        let answer = async {
            firmware.answer_catalog(&[CAMERA]).await;
            firmware.answer_control(0, 1).await;
        };
        let (camera, ()) = join!(console.open_live(CAMERA), answer);
        let mut camera = camera.unwrap();

        firmware.source_output(0, b"fresh").await;
        assert_eq!(camera.bytes().next().await.unwrap(), b"fresh");
    }

    #[tokio::test]
    async fn early_output_buffer_drops_the_oldest_packets_when_full() {
        let (console, firmware) = connect().await;

        let packet_count = EARLY_SOURCE_BUFFER_LIMIT / (28 + 32) + 10;
        for value in 0..packet_count {
            let mut data = (value as u32).to_le_bytes().to_vec();
            data.resize(28, 0);
            firmware.source_output(0, &data).await;
        }

        let mut camera = open_first(&console, &firmware, &[CAMERA], CAMERA).await;
        firmware.source_output(0, b"end").await;

        let mut received = Vec::new();
        let mut bytes = camera.bytes();
        loop {
            let chunk = bytes.next().await.unwrap();
            if chunk == b"end" {
                break;
            }
            received.push(u32::from_le_bytes(chunk[..4].try_into().unwrap()));
        }
        assert!(received.len() < packet_count);
        assert_eq!(*received.last().unwrap() as usize, packet_count - 1);
    }

    #[tokio::test]
    async fn text_decodes_utf8_across_packets() {
        let (_console, firmware, mut camera) = connect_and_open_camera().await;

        firmware.source_output(0, &[b'a', 0xe2]).await;
        firmware.source_output(0, &[0x82, 0xac, b'b']).await;

        let mut text = camera.text();
        assert_eq!(text.next().await.unwrap(), "a");
        assert_eq!(text.next().await.unwrap(), "€b");
    }

    #[tokio::test]
    async fn text_is_lossy() {
        let (_console, firmware, mut camera) = connect_and_open_camera().await;

        firmware.source_output(0, &[b'o', b'k', 0xff]).await;
        assert_eq!(camera.text().next().await.unwrap(), "ok�");
    }

    #[tokio::test]
    async fn stm32_text_decodes_utf8_across_packets() {
        let (console, firmware) = connect().await;
        let mut stm32 = console.open("cf:stm32").await.unwrap();

        firmware.send(LOCAL_CHANNEL, vec![b'a', 0xe2]).await;
        firmware.send(LOCAL_CHANNEL, vec![0x82, 0xac]).await;

        let mut text = stm32.text();
        assert_eq!(text.next().await.unwrap(), "a");
        assert_eq!(text.next().await.unwrap(), "€");
    }

    #[tokio::test]
    async fn lines_are_assembled_independently_per_source() {
        let (console, firmware) = connect().await;
        let mut camera = open_first(&console, &firmware, &[CAMERA, NRF], CAMERA).await;
        let (nrf, ()) = join!(console.open(NRF), firmware.answer_control(1, 1));
        let mut nrf = nrf.unwrap();

        firmware.source_output(0, b"cam ").await;
        firmware.source_output(1, b"nrf\n").await;
        firmware.source_output(0, b"line\nsecond\npartial").await;

        assert_eq!(nrf.lines().next().await.unwrap(), "nrf");
        let mut camera_lines = camera.lines();
        assert_eq!(camera_lines.next().await.unwrap(), "cam line");
        assert_eq!(camera_lines.next().await.unwrap(), "second");
    }

    #[tokio::test]
    async fn slow_reader_keeps_its_position() {
        let (_console, firmware, mut camera) = connect_and_open_camera().await;

        for value in 0..1100_u16 {
            firmware.source_output(0, &[(value % 251) as u8]).await;
        }
        let mut bytes = camera.bytes();
        for value in 0..1100_u16 {
            assert_eq!(bytes.next().await.unwrap(), vec![(value % 251) as u8]);
        }
    }

    #[tokio::test]
    async fn packets_from_unknown_source_ids_are_ignored() {
        let (_console, firmware, mut camera) = connect_and_open_camera().await;

        firmware.source_output(42, b"wrong").await;
        firmware.source_output(0, b"right").await;
        assert_eq!(camera.bytes().next().await.unwrap(), b"right");
    }

    #[tokio::test]
    async fn text_flushes_incomplete_utf8_and_ends_on_disconnect() {
        let (_console, firmware, mut camera) = connect_and_open_camera().await;

        firmware.source_output(0, &[0xe2]).await;
        drop(firmware.downlink);

        let mut text = camera.text();
        assert_eq!(text.next().await.unwrap(), "�");
        assert_eq!(text.next().await, None);
    }

    #[tokio::test]
    async fn shutdown_ends_every_stream_and_later_closes_report_disconnected() {
        let (console, firmware, mut camera) = connect_and_open_camera().await;
        let mut stm32 = console.open("cf:stm32").await.unwrap();

        drop(firmware.downlink);
        tokio::time::timeout(PATIENCE, console.shutdown())
            .await
            .expect("Console shutdown did not join its tasks");

        tokio::time::timeout(PATIENCE, async {
            assert_eq!(camera.bytes().next().await, None);
            assert_eq!(camera.text().next().await, None);
            assert_eq!(camera.lines().next().await, None);
            assert_eq!(stm32.lines().next().await, None);
        })
        .await
        .expect("streams did not end after shutdown");
        assert!(console.console_task.lock().await.is_none());
        assert!(console.transaction_task.lock().await.is_none());

        assert!(matches!(camera.close().await, Err(Error::Disconnected)));
        stm32.close().await.unwrap();
    }

    #[tokio::test]
    async fn handle_does_not_keep_the_worker_alive_after_the_console_is_gone() {
        let (console, firmware, camera) = connect_and_open_camera().await;

        drop(console);
        tokio::time::timeout(PATIENCE, async {
            while !firmware.uplink.is_disconnected() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the handle kept the worker alive");

        assert!(matches!(camera.close().await, Err(Error::Disconnected)));
    }

    #[tokio::test]
    async fn dropping_a_handle_after_the_console_is_gone_is_harmless() {
        let (console, _firmware, camera) = connect_and_open_camera().await;

        drop(console);
        drop(camera);
    }

    #[test]
    fn dropping_a_handle_needs_no_runtime() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (console, firmware, camera) = runtime.block_on(connect_and_open_camera());

        runtime.shutdown_background();
        drop(camera);
        drop((console, firmware));
    }

    #[tokio::test]
    async fn initialization_retries_eagain_error_response() {
        let (downlink, uplink, firmware) = link();

        let answer = async {
            firmware
                .expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, ALL_SOURCES, 0])
                .await;
            firmware
                .send(
                    CONTROL_CHANNEL,
                    vec![CONTROL_SET_ENABLED, crate::firmware_errno::EAGAIN],
                )
                .await;
            firmware.answer_control(ALL_SOURCES, 0).await;
        };
        let (console, ()) = join!(Console::new(downlink, uplink, 13), answer);
        let _console = console.unwrap();
        firmware.assert_silent().await;
    }

    #[tokio::test]
    async fn initialization_rejects_malformed_response_and_stops_router() {
        let (downlink, uplink, firmware) = link();

        let answer = async {
            let _ = firmware.uplink.recv_async().await.unwrap();
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, 0, 0])
                .await;
        };
        let (result, ()) = join!(Console::new(downlink, uplink, 13), answer);
        assert!(matches!(result, Err(Error::ProtocolError(_))));
        assert_router_stopped(&firmware.downlink).await;
    }

    #[tokio::test]
    async fn initialization_surfaces_firmware_rejection_and_stops_router() {
        let (downlink, uplink, firmware) = link();

        let answer = async {
            let _ = firmware.uplink.recv_async().await.unwrap();
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 5])
                .await;
        };
        let (result, ()) = join!(Console::new(downlink, uplink, 13), answer);
        assert!(matches!(
            result,
            Err(Error::Console(ConsoleError::CommandRejected {
                path: None,
                enabled: false,
                errno: 5,
            }))
        ));
        assert_router_stopped(&firmware.downlink).await;
    }

    #[tokio::test]
    async fn initialization_reports_uplink_closure_and_stops_router() {
        let (downlink, uplink, firmware) = link();
        drop(firmware.uplink);

        let result = Console::new(downlink, uplink, 13).await;
        assert!(matches!(result, Err(Error::Disconnected)));
        assert_router_stopped(&firmware.downlink).await;
    }

    #[tokio::test]
    async fn initialization_reports_control_downlink_closure() {
        let (downlink, uplink, firmware) = link();
        drop(firmware.downlink);

        let result = Console::new(downlink, uplink, 13).await;
        assert!(matches!(result, Err(Error::Disconnected)));
    }

    #[tokio::test]
    async fn cancelling_initialization_stops_router() {
        let (downlink, uplink, firmware) = link();

        {
            let construction = Console::new(downlink, uplink, 13);
            tokio::pin!(construction);
            tokio::select! {
                () = firmware.expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, ALL_SOURCES, 0]) => {}
                _ = construction.as_mut() => panic!("Console returned before initialization"),
            }
        }

        assert_router_stopped(&firmware.downlink).await;
    }

    #[tokio::test]
    async fn catalog_retries_info_while_firmware_is_not_ready() {
        let (console, firmware) = connect().await;

        let answer = async {
            firmware.expect(CATALOG_CHANNEL, &[CATALOG_GET_INFO]).await;
            firmware
                .send(
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_INFO, crate::firmware_errno::EAGAIN],
                )
                .await;
            firmware.answer_catalog(&[]).await;
        };
        let (paths, ()) = join!(console.list(), answer);
        assert_eq!(paths.unwrap(), ["cf:stm32"]);
    }

    #[tokio::test]
    async fn catalog_retries_items_while_firmware_is_not_ready() {
        let (console, firmware) = connect().await;

        let answer = async {
            firmware.expect(CATALOG_CHANNEL, &[CATALOG_GET_INFO]).await;
            firmware
                .send(CATALOG_CHANNEL, vec![CATALOG_GET_INFO, 0, 1, 0, 0, 0, 0])
                .await;
            firmware
                .expect(CATALOG_CHANNEL, &[CATALOG_GET_ITEM, 0])
                .await;
            firmware
                .send(
                    CATALOG_CHANNEL,
                    vec![CATALOG_GET_ITEM, crate::firmware_errno::EAGAIN],
                )
                .await;
            firmware
                .expect(CATALOG_CHANNEL, &[CATALOG_GET_ITEM, 0])
                .await;
            firmware
                .send(
                    CATALOG_CHANNEL,
                    [&[CATALOG_GET_ITEM, 0, 0], CAMERA.as_bytes()].concat(),
                )
                .await;
        };
        let (paths, ()) = join!(console.list(), answer);
        assert_eq!(paths.unwrap(), ["cf:stm32", CAMERA]);
    }

    #[tokio::test]
    async fn cancelling_discovery_does_not_desynchronize_later_discovery() {
        let (console, firmware) = connect().await;

        {
            let discovery = console.list();
            tokio::pin!(discovery);
            tokio::select! {
                () = firmware.expect(CATALOG_CHANNEL, &[CATALOG_GET_INFO]) => {}
                result = discovery.as_mut() => panic!("list returned early: {result:?}"),
            }
        }
        firmware
            .send(CATALOG_CHANNEL, vec![CATALOG_GET_INFO, 0, 1, 0, 0, 0, 0])
            .await;

        let answer = async {
            firmware
                .expect(CATALOG_CHANNEL, &[CATALOG_GET_ITEM, 0])
                .await;
            firmware
                .send(
                    CATALOG_CHANNEL,
                    [&[CATALOG_GET_ITEM, 0, 0], CAMERA.as_bytes()].concat(),
                )
                .await;
        };
        let (paths, ()) = join!(console.list(), answer);
        assert_eq!(paths.unwrap(), ["cf:stm32", CAMERA]);
        firmware.assert_silent().await;
    }

    #[tokio::test]
    async fn catalog_rejects_a_path_with_an_empty_segment() {
        let (console, firmware) = connect().await;

        let (paths, ()) = join!(console.list(), firmware.answer_catalog(&["deck:"]));
        assert!(matches!(paths, Err(Error::ProtocolError(_))));
    }

    #[tokio::test]
    async fn catalog_rejects_duplicate_paths() {
        let (console, firmware) = connect().await;

        let (paths, ()) = join!(
            console.list(),
            firmware.answer_catalog(&["deck:duplicate", "deck:duplicate"])
        );
        assert!(matches!(paths, Err(Error::ProtocolError(_))));
    }

    #[tokio::test]
    async fn catalog_reports_command_error_responses() {
        let (console, firmware) = connect().await;

        let answer = async {
            firmware.expect(CATALOG_CHANNEL, &[CATALOG_GET_INFO]).await;
            firmware
                .send(CATALOG_CHANNEL, vec![CATALOG_GET_INFO, 0, 1, 0, 0, 0, 0])
                .await;
            firmware
                .expect(CATALOG_CHANNEL, &[CATALOG_GET_ITEM, 0])
                .await;
            firmware
                .send(CATALOG_CHANNEL, vec![CATALOG_GET_ITEM, 2])
                .await;
        };
        let (paths, ()) = join!(console.list(), answer);
        assert!(matches!(
            paths,
            Err(Error::Console(ConsoleError::CatalogCommandRejected {
                operation: ConsoleCatalogOperation::GetItem(0),
                errno: 2,
            }))
        ));
    }

    #[tokio::test]
    async fn malformed_control_responses_are_protocol_errors() {
        let (console, firmware) = connect().await;

        let answer = async {
            firmware.answer_catalog(&[CAMERA]).await;
            firmware
                .expect(CONTROL_CHANNEL, &[CONTROL_SET_ENABLED, 0, 1])
                .await;
            firmware
                .send(CONTROL_CHANNEL, vec![CONTROL_SET_ENABLED, 0, 0, 0])
                .await;
        };
        let (result, ()) = join!(console.open(CAMERA), answer);
        assert!(matches!(result, Err(Error::ProtocolError(_))));
    }

    // The current-thread runtime routes the queued final reply and EOF before
    // waking the transaction worker to register the discovered sources.
    #[tokio::test]
    async fn disconnect_during_final_catalog_response_rejects_discovery() {
        let (console, firmware) = connect().await;

        let answer = async move {
            firmware.answer_catalog(&[CAMERA]).await;
            drop(firmware.downlink);
        };
        let (paths, ()) = join!(console.list(), answer);
        assert!(matches!(paths, Err(Error::Disconnected)));
        console.shutdown().await;
    }
}
