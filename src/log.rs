use crazyflie_link::Packet;
use flume as channel;
use futures::lock::Mutex;
use std::collections::HashMap;
use std::{collections::BTreeMap, convert::TryFrom, sync::Arc, time::Duration};
use crate::{Error, Result, Value, ValueType};
use crate::WaitForPacket;

#[derive(Debug)]
pub struct Log {
    uplink: channel::Sender<Packet>,
    control_downlink: Arc<Mutex<channel::Receiver<Packet>>>,
    toc: BTreeMap<String, (u16, LogItemInfo)>,
    next_block_id: Mutex<u8>,
}

fn not_found(name: &str) -> Error {
    Error::ParamError(format!("Log variable {} not found", name))
}

const LOG_PORT: u8 = 5;

const CONTROL_CHANNEL: u8 = 1;

const DELETE_BLOCK: u8 = 2;
const START_BLOCK: u8 = 3;
const STOP_BLOCK: u8 = 4;
const RESET: u8 = 5;
const CREATE_BLOCK_V2: u8 = 6;
const APPEND_BLOCK_V2: u8 = 7;

impl Log {
    pub(crate) async fn new(
        downlink: channel::Receiver<Packet>,
        uplink: channel::Sender<Packet>,
    ) -> Self {
        let (toc_downlink, control_downlink, _, _) = crate::crtp_channel_dispatcher(downlink);

        let toc = crate::fetch_toc(LOG_PORT, uplink.clone(), toc_downlink).await;

        let control_downlink = Arc::new(Mutex::new(control_downlink));

        let next_block_id = Mutex::new(0);

        let log = Self { uplink, control_downlink, toc, next_block_id };
        log.reset().await;

        log
    }

    async fn reset(&self) {
        let downlink = self.control_downlink.lock().await;

        let pk = Packet::new(LOG_PORT, CONTROL_CHANNEL, vec![RESET]);
        self.uplink.send_async(pk).await.unwrap();

        let pk = downlink.wait_packet(LOG_PORT, CONTROL_CHANNEL, &[RESET]).await;
        assert_eq!(pk.get_data()[2], 0);
    }

    /// Get the names of all the log variables
    ///
    /// The names contain group and name of the log variable formated as
    /// "group.name".
    pub fn names(&self) -> Vec<String> {
        self.toc.keys().cloned().collect()
    }

    /// Return the type of a log variable or an Error if the parameter does not exist.
    pub fn get_type(&self, name: &str) -> Result<ValueType> {
        Ok(self
            .toc
            .get(name)
            .ok_or_else(|| not_found(name))?
            .1
            .item_type)
    }

    async fn generate_next_block_id(&self) -> Result<u8> {
        let mut next_block_id = self.next_block_id.lock().await;
        if *next_block_id == u8::MAX {
            return Err(Error::LogError("No more block ID available!".into()));
        }
        let id = *next_block_id;
        *next_block_id += 1;
        Ok(id)
    }

    pub async fn create_block(&self) -> Result<LogBlock> {

        let block_id =self.generate_next_block_id().await?;
        let control_downlink = self.control_downlink.lock().await;

        let pk = Packet::new(LOG_PORT, CONTROL_CHANNEL, vec![CREATE_BLOCK_V2, block_id]);
        self.uplink.send_async(pk).await.unwrap();
        
        let pk = control_downlink.wait_packet(LOG_PORT, CONTROL_CHANNEL, &[CREATE_BLOCK_V2, block_id]).await;
        let error = pk.get_data()[2];

        if error != 0 {
            return Err(Error::LogError(format!("Protocol error when creating block: {}", error)));
        }

        // Todo: Create data channel for the block 

        Ok(LogBlock {
            uplink: self.uplink.clone(),
            block_id: block_id,
            variables: Vec::new(),
        })
    }
}

#[derive(Debug)]
struct LogItemInfo {
    item_type: ValueType,
}

impl TryFrom<u8> for LogItemInfo {
    type Error = Error;

    fn try_from(log_type: u8) -> Result<Self> {
        let item_type = match log_type {
            1 => ValueType::U8,
            2 => ValueType::U16,
            3 => ValueType::U32,
            4 => ValueType::I8,
            5 => ValueType::I16,
            6 => ValueType::I32,
            7 => ValueType::F32,
            8 => ValueType::F16,
            _ => return Err(Error::ProtocolError(format!("Invalid log item type: {}", log_type))),
        };

        Ok(LogItemInfo { item_type })
    }
}

pub struct LogBlock {
    uplink: channel::Sender<Packet>,
    block_id: u8,
    variables: Vec<(String, ValueType)>
}

impl LogBlock {
    /// Start log block and return a stream to read  the value
    ///
    /// Since a log-block cannot be modified after being started, this function
    /// consumes the logblock object and return a `LogStream`. The function
    /// [stop()](struct.LogStream.html#method.stop) can be called on the LogStream to get back the logblock object.
    pub fn start(self, period: Duration) -> LogStream {
        todo!()
    }
}

pub struct LogStream {
    log_block: LogBlock,
}

impl LogStream {
    pub async fn stop() -> LogBlock {
        todo!()
    }
}

impl futures::Stream for LogStream {
    type Item = LogData;

    fn poll_next(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
        todo!()
    }
}

pub struct LogData {
    timestamp: u32,
    data: HashMap<String, Value>,
}