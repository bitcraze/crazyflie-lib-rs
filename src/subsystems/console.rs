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

use std::sync::Arc;

use crate::Result;
use async_broadcast::{broadcast, Receiver};
use tokio::task::JoinHandle;
use crazyflie_link::Packet;
use flume as channel;
use futures::{lock::Mutex, Stream, StreamExt};

/// # Access to the console subsystem
///
/// See the [console module documentation](crate::subsystems::console) for more context and information.
pub struct Console {
    stream_broadcast_receiver: Receiver<String>,
    console_buffer: Arc<Mutex<String>>,
    line_broadcast_receiver: Receiver<String>,
    console_lines: Arc<Mutex<Vec<String>>>,
    _console_task: JoinHandle<()>,
}

impl Console {
    pub(crate) async fn new(
        downlink: channel::Receiver<Packet>,
    ) -> Result<Self> {
        let (mut stream_broadcast, stream_broadcast_receiver) = broadcast(1000);
        let console_buffer: Arc<Mutex<String>> = Default::default();

        let (mut line_broadcast, line_broadcast_receiver) = broadcast(1000);

        // Enable overflow mode so old messages are dropped instead of blocking
        stream_broadcast.set_overflow(true);
        line_broadcast.set_overflow(true);
        let console_lines: Arc<Mutex<Vec<String>>> = Default::default();

        let buffer = console_buffer.clone();
        let lines = console_lines.clone();

        let _console_task = tokio::spawn(async move {
            let mut line_buffer = String::new();
            while let Ok(pk) = downlink.recv_async().await {
                // Decode text from the console
                let text = String::from_utf8_lossy(pk.get_data());

                buffer.lock().await.push_str(&text);

                // Push the text to all active streams, we ignore any error there
                let _ = stream_broadcast.broadcast(text.clone().into_owned()).await;

                // Extract lines and push them to all active line streams
                line_buffer.push_str(&text);
                if let Some((line, rest)) = line_buffer.clone().split_once("\n") {
                    line_buffer = rest.to_owned();
                    lines.lock().await.push(line.to_owned().clone());
                    let _ = line_broadcast.broadcast(line.to_owned()).await;
                }
            }
        });

        Ok(Self {
            stream_broadcast_receiver,
            console_buffer,
            line_broadcast_receiver,
            console_lines,
            _console_task,
        })
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
    pub async fn stream(&self) -> impl Stream<Item = String> {
        let buffer = self.console_buffer.lock().await;
        let history_buffer = buffer.clone();
        let history_stream = futures::stream::once(async { history_buffer }).boxed();

        history_stream.chain(self.stream_broadcast_receiver.clone())
    }

    /// Version of [Console::stream()] but that does not produce the history
    /// first.
    pub async fn stream_no_history(&self) -> impl Stream<Item = String> {
        self.stream_broadcast_receiver.clone()
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
    pub async fn line_stream(&self) -> impl Stream<Item = String> {
        let lines = self.console_lines.lock().await;
        let history_lines = lines.clone();
        let history_stream = futures::stream::iter(history_lines.into_iter()).boxed();

        history_stream.chain(self.line_broadcast_receiver.clone())
    }

    /// Version of [Console::line_stream()] but that does not produce the history
    /// first.
    pub async fn line_stream_no_history(&self) -> impl Stream<Item = String> {
        self.line_broadcast_receiver.clone()
    }
}
