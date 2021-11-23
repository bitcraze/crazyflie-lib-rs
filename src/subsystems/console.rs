//! # Console subsystem
//! 
//! The Crazyflie has a test console that is used to communicate various information
//! and debug message to the ground.

use std::{sync::Arc};

use async_executors::{JoinHandle, LocalSpawnHandleExt};
use flume as channel;
use crazyflie_link::Packet;
use futures::{Stream, lock::Mutex, StreamExt};
use crate::{Result};
use async_broadcast::{Receiver, broadcast};


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
    pub(crate) async fn new(executor: impl crate::Executor, downlink: channel::Receiver<Packet>) -> Result<Self> {
        let (stream_broadcast, stream_broadcast_receiver) = broadcast(1000);
        let console_buffer: Arc<Mutex<String>> = Default::default();

        let (line_broadcast, line_broadcast_receiver) = broadcast(1000);
        let console_lines: Arc<Mutex<Vec<String>>> = Default::default();

        let buffer = console_buffer.clone();
        let lines = console_lines.clone();

        let _console_task = executor.spawn_handle_local(async move {
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
        })?;

        Ok(Self {
            stream_broadcast_receiver,
            console_buffer,
            line_broadcast_receiver,
            console_lines,
            _console_task,
        })
    }
    
    pub async fn get_stream(&self) -> impl Stream<Item = String> {
        let buffer = self.console_buffer.lock().await;
        let history_buffer = buffer.clone();
        let history_stream = futures::stream::once(async { history_buffer }).boxed();

        history_stream.chain(self.stream_broadcast_receiver.clone())
    }

    pub async fn get_stream_no_history(&self) -> impl Stream<Item = String> {
        self.stream_broadcast_receiver.clone()
    }

    pub async fn get_line_stream(&self) -> impl Stream<Item = String> {
        let lines = self.console_lines.lock().await;
        let history_lines = lines.clone();
        let history_stream = futures::stream::iter(history_lines.into_iter()).boxed();

        history_stream.chain(self.line_broadcast_receiver.clone())
    }

    pub async fn get_line_stream_no_history(&self) -> impl Stream<Item = String> {
        self.line_broadcast_receiver.clone()
    }
}
