//! # Console subsystem
//! 
//! The Crazyflie has a test console that is used to communicate various information
//! and debug message to the groung.

use std::sync::Arc;

use async_executors::{JoinHandle, LocalSpawnHandleExt};
use flume as channel;
use crazyflie_link::Packet;
use futures::lock::Mutex;
use crate::{Error, Result};


/// # Access to the console subsystem
/// 
/// See the [console module documentation](crate::subsystems::console) for more context and information.
pub struct Console {
    stream_channels: Arc<Mutex<Vec<channel::Sender<String>>>>,
    console_buffer: Arc<Mutex<String>>,
    _console_task: JoinHandle<()>,
}

impl Console {
    pub(crate) async fn new(executor: impl crate::Executor, downlink: channel::Receiver<Packet>) -> Result<Self> {
        let stream_channels: Arc<Mutex<Vec<channel::Sender<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let console_buffer = Arc::new(Mutex::new(String::new()));


        let streams = stream_channels.clone();
        let buffer = console_buffer.clone();

        let _console_task = executor.spawn_handle_local(async move {
            while let Ok(pk) = downlink.recv_async().await {                
                // Decode text from the console
                let text = String::from_utf8_lossy(pk.get_data());
                buffer.lock().await.push_str(&text);

                // Push the text to all active streams
                let mut invalid_stream_index = None;
                for (i, stream) in streams.lock().await.iter().enumerate() {
                    if stream.send_async(text.to_string()).await.is_err() {
                        invalid_stream_index = Some(i);
                    }
                }

                // Remove stream that have been dropped one by one: the last
                // stream to return an error will be removed at each run
                if let Some(i) = invalid_stream_index {
                    streams.lock().await.swap_remove(i);
                }
            }
        })?;

        Ok(Self {
            stream_channels,
            console_buffer,
            _console_task,
        })
    }
    
    pub async fn get_stream(&self) -> ConsoleStream {
        let (tx, rx) = channel::unbounded();

        // Lock the buffer to make sure no more log can be added to it while we are working
        let console_buffer = self.console_buffer.lock().await;
        let mut stream_channels = self.stream_channels.lock().await;

        // Send the current console state in the buffer
        let _ = tx.send_async(console_buffer.clone()).await;

        // Add the curent tx to the list of channels
        stream_channels.push(tx);

        ConsoleStream { incoming: rx, buffer: None }
    }
}

pub struct ConsoleStream {
    incoming: channel::Receiver<String>,
    buffer: Option<String>,
}

impl ConsoleStream {
    pub async fn next(&mut self) -> Result<String> {
        if let Some(data) = self.buffer.take() {
            return Ok(data);
        }

        self.incoming.recv_async().await.map_err(|_| Error::Disconnected)
    }


    pub async fn next_line(&mut self) -> String{
        todo!();
        // if let Some(buffer) = self.buffer.borrow() {
        //     // let buffbuffer.lines().
        // }
    }
}