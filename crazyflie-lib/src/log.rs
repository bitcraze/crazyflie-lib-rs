use crate::WaitForPacket;
use crate::{Error, Result, Value, ValueType};
use crazyflie_link::Packet;
use flume as channel;
use futures::lock::Mutex;
use std::collections::HashMap;
use std::convert::TryInto;
use std::sync::Weak;
use std::{collections::BTreeMap, convert::TryFrom, sync::Arc, time::Duration};

#[derive(Debug)]
pub struct Log {
    uplink: channel::Sender<Packet>,
    control_downlink: Arc<Mutex<channel::Receiver<Packet>>>,
    toc: Arc<BTreeMap<String, (u16, LogItemInfo)>>,
    next_block_id: Mutex<u8>,
    data_channels: Arc<Mutex<BTreeMap<u8, flume::Sender<Packet>>>>,
    active_blocks: Mutex<BTreeMap<u8, Weak<()>>>,
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

/// Crazyflie Log subsystem
///
/// The Crazyflie log subsystem allows to asynchronously log the value of exposed Crazyflie variables from the ground.
///
/// At connection time, a Table Of Content (TOC) of the log variable is fetched from the Crazyflie which allows to
/// log variables using their names. To log variable a [LogBlock] needs to be created. The variable to be logged are
/// added to the LogBlock and then the LogBlock can be started returning a LogStream that will yield the log datas.
///
/// ```no_run
/// # use crazyflie_lib::{Crazyflie, Value, Error};
/// # use async_executors::AsyncStd;
/// # use crazyflie_link::LinkContext;
/// # use std::sync::Arc;
/// # async fn example() -> Result<(), Error> {
/// # let context = LinkContext::new(Arc::new(AsyncStd));
/// # let cf = Crazyflie::connect_from_uri(&context, "radio://0/60/2M/E7E7E7E7E7").await;
/// // Create the log block
/// let block = cf.log.create_block();
///
/// // Append Variables
/// block.add_variable("stateEstimate.roll").await?;
/// block.add_variable("stateEstimate.pitch").await?;
/// block.add_variable("stateEstimate.yaw").await?;
///
/// // Start the block
/// let stream = block.start().await;
///
/// // Get Data!
/// while let Ok(data) = stream.next().await {
///     println("Yaw is {:?}", data.data["stateEstimate.yaw"]);
/// }
/// # Ok(())
/// # };
/// ```
impl Log {
    pub(crate) async fn new(
        downlink: channel::Receiver<Packet>,
        uplink: channel::Sender<Packet>,
    ) -> Result<Self> {
        let (toc_downlink, control_downlink, data_downlink, _) =
            crate::crtp_channel_dispatcher(downlink);

        let toc = crate::fetch_toc(LOG_PORT, uplink.clone(), toc_downlink).await?;
        let toc = Arc::new(toc);

        let control_downlink = Arc::new(Mutex::new(control_downlink));

        let next_block_id = Mutex::new(0);

        let data_channels = Arc::new(Mutex::new(BTreeMap::new()));

        let active_blocks = Mutex::new(BTreeMap::new());

        let log = Self {
            uplink,
            control_downlink,
            toc,
            next_block_id,
            data_channels,
            active_blocks,
        };
        log.reset().await?;
        log.spawn_data_dispatcher(data_downlink).await;

        Ok(log)
    }

    async fn reset(&self) -> Result<()> {
        let downlink = self.control_downlink.lock().await;

        let pk = Packet::new(LOG_PORT, CONTROL_CHANNEL, vec![RESET]);
        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let pk = downlink
            .wait_packet(LOG_PORT, CONTROL_CHANNEL, &[RESET])
            .await?;
        assert_eq!(pk.get_data()[2], 0);

        Ok(())
    }

    async fn spawn_data_dispatcher(&self, data_downlink: flume::Receiver<Packet>) {
        let data_channels = self.data_channels.clone();
        crate::spawn(async move {
            while let Ok(packet) = data_downlink.recv_async().await {
                if packet.get_data().len() > 1 {
                    let block_id = packet.get_data()[0];
                    let data_channels = data_channels.lock().await;
                    if data_channels.contains_key(&block_id)
                        && data_channels
                            .get(&block_id)
                            .unwrap()
                            .send_async(packet)
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
            }
        });
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

    /// Cleanup dropped LogBlocks
    async fn cleanup_blocks(&self) -> Result<()> {
        let mut active_blocks = self.active_blocks.lock().await;

        for (block_id, canary) in active_blocks.clone().into_iter() {
            if canary.upgrade() == None {
                // Delete the block!
                let control_downlink = self.control_downlink.lock().await;

                let pk = Packet::new(LOG_PORT, CONTROL_CHANNEL, vec![DELETE_BLOCK, block_id]);
                self.uplink
                    .send_async(pk)
                    .await
                    .map_err(|_| Error::Disconnected)?;

                let pk = control_downlink
                    .wait_packet(LOG_PORT, CONTROL_CHANNEL, &[DELETE_BLOCK, block_id])
                    .await?;
                let error = pk.get_data()[2];

                if error != 0 {
                    return Err(Error::LogError(format!(
                        "Protocol error when deleting block: {}",
                        error
                    )));
                }

                active_blocks.remove_entry(&block_id);
            }
        }

        Ok(())
    }

    pub async fn create_block(&self) -> Result<LogBlock> {
        self.cleanup_blocks().await?;

        let block_id = self.generate_next_block_id().await?;
        let control_downlink = self.control_downlink.lock().await;

        let pk = Packet::new(LOG_PORT, CONTROL_CHANNEL, vec![CREATE_BLOCK_V2, block_id]);
        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let pk = control_downlink
            .wait_packet(LOG_PORT, CONTROL_CHANNEL, &[CREATE_BLOCK_V2, block_id])
            .await?;
        let error = pk.get_data()[2];

        if error != 0 {
            return Err(Error::LogError(format!(
                "Protocol error when creating block: {}",
                error
            )));
        }

        // Todo: Create data channel for the block
        let (tx, rx) = flume::unbounded();
        self.data_channels.lock().await.insert(block_id, tx);

        let canary = Arc::new(());
        self.active_blocks
            .lock()
            .await
            .insert(block_id, Arc::downgrade(&canary));

        Ok(LogBlock {
            _canary: canary,
            toc: Arc::downgrade(&self.toc),
            uplink: self.uplink.clone(),
            control_downlink: Arc::downgrade(&self.control_downlink),
            block_id,
            variables: Vec::new(),
            data_channel: rx,
        })
    }
}

#[derive(Debug, Clone, Copy)]
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
            _ => {
                return Err(Error::ProtocolError(format!(
                    "Invalid log item type: {}",
                    log_type
                )))
            }
        };

        Ok(LogItemInfo { item_type })
    }
}

impl TryInto<u8> for LogItemInfo {
    type Error = Error;

    fn try_into(self) -> Result<u8> {
        let value = match self.item_type {
            ValueType::U8 => 1,
            ValueType::U16 => 2,
            ValueType::U32 => 3,
            ValueType::I8 => 4,
            ValueType::I16 => 5,
            ValueType::I32 => 6,
            ValueType::F32 => 7,
            ValueType::F16 => 8,
            _ => {
                return Err(Error::LogError(format!(
                    "Value type {:?} not handled by log",
                    self.item_type
                )))
            }
        };
        Ok(value)
    }
}

pub struct LogBlock {
    _canary: Arc<()>,
    toc: Weak<BTreeMap<String, (u16, LogItemInfo)>>,
    uplink: channel::Sender<Packet>,
    control_downlink: Weak<Mutex<channel::Receiver<Packet>>>,
    block_id: u8,
    variables: Vec<(String, ValueType)>,
    data_channel: flume::Receiver<Packet>,
}

impl LogBlock {
    /// Start log block and return a stream to read  the value
    ///
    /// Since a log-block cannot be modified after being started, this function
    /// consumes the logblock object and return a `LogStream`. The function
    /// [stop()](struct.LogStream.html#method.stop) can be called on the LogStream to get back the logblock object.
    ///
    /// This function is failable. It can fail if there is a protocol error or an error
    /// reported by the Crazyflie. In such case, the LogBlock object will be dropped and the block will be deleted in
    /// the Crazyflie
    pub async fn start(self, period: LogPeriod) -> Result<LogStream> {
        let control_uplink = self.control_downlink.upgrade().ok_or(Error::Disconnected)?;
        let control_uplink = control_uplink.lock().await;

        let pk = Packet::new(
            LOG_PORT,
            CONTROL_CHANNEL,
            vec![START_BLOCK, self.block_id, period.0],
        );
        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let answer = control_uplink
            .wait_packet(LOG_PORT, CONTROL_CHANNEL, &[START_BLOCK, self.block_id])
            .await?;
        if answer.get_data().len() != 3 {
            return Err(Error::ProtocolError(
                "Malformed Log control packet".to_owned(),
            ));
        }
        let error_code = answer.get_data()[2];
        if error_code != 0 {
            return Err(Error::LogError(format!(
                "Error starting lock: {}",
                error_code
            )));
        }

        Ok(LogStream { log_block: self })
    }

    /// Add variable to the log block
    ///
    /// A packet will be sent to the Crazyflie to add the variable. The variable is logged in the same format as
    /// it is stored in the Crazyflie (ie. there is no conversion done)
    ///
    /// This function can fail if the variable is not found in the toc or of the Crazyflie returns an error
    /// The most common error reported by the Crazyflie would be if the log block is already too full.
    pub async fn add_variable(&mut self, name: &str) -> Result<()> {
        let toc = self.toc.upgrade().ok_or(Error::Disconnected)?;
        let (variable_id, info) = toc.get(name).ok_or(Error::VariableNotFound)?;

        // Add variable to Crazyflie
        let control_uplink = self.control_downlink.upgrade().ok_or(Error::Disconnected)?;
        let control_uplink = control_uplink.lock().await;

        let mut payload = vec![APPEND_BLOCK_V2, self.block_id, (*info).try_into()?];
        payload.extend_from_slice(&variable_id.to_le_bytes());
        let pk = Packet::new(LOG_PORT, CONTROL_CHANNEL, payload);
        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let answer = control_uplink
            .wait_packet(LOG_PORT, CONTROL_CHANNEL, &[APPEND_BLOCK_V2, self.block_id])
            .await?;
        if answer.get_data().len() != 3 {
            return Err(Error::ProtocolError(
                "Mallformed Log control packet".to_owned(),
            ));
        }
        let error_code = answer.get_data()[2];
        if error_code != 0 {
            return Err(Error::LogError(format!(
                "Error appending variable to block: {}",
                error_code
            )));
        }

        // Add variable to local list
        self.variables.push((name.to_owned(), info.item_type));

        Ok(())
    }
}

pub struct LogStream {
    log_block: LogBlock,
}

impl LogStream {
    /// Stops the log block from streaming
    ///
    /// This method consumes the stream and returns back the log block object so that it can be started again later
    /// with a different period.
    ///
    /// This function can only fail on unexpected protocol error. If it does, the log block is dropped and will be
    /// cleaned-up next time a log block is created.
    pub async fn stop(self) -> Result<LogBlock> {
        let control_uplink = self
            .log_block
            .control_downlink
            .upgrade()
            .ok_or(Error::Disconnected)?;
        let control_uplink = control_uplink.lock().await;

        let pk = Packet::new(
            LOG_PORT,
            CONTROL_CHANNEL,
            vec![STOP_BLOCK, self.log_block.block_id],
        );
        self.log_block
            .uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let answer = control_uplink
            .wait_packet(
                LOG_PORT,
                CONTROL_CHANNEL,
                &[STOP_BLOCK, self.log_block.block_id],
            )
            .await?;
        if answer.get_data().len() != 3 {
            return Err(Error::ProtocolError(
                "Malformed Log control packet".to_owned(),
            ));
        }
        let error_code = answer.get_data()[2];
        if error_code != 0 {
            return Err(Error::LogError(format!(
                "Error starting lock: {}",
                error_code
            )));
        }

        Ok(self.log_block)
    }

    /// Get the next log data from the log block stream
    ///
    /// This function will return an error if the Crazyflie gets disconnected.
    pub async fn next(&self) -> Result<LogData> {
        let packet = self
            .log_block
            .data_channel
            .recv_async()
            .await
            .map_err(|_| Error::Disconnected)?;

        self.decode_packet(&packet.get_data()[1..])
    }

    fn decode_packet(&self, data: &[u8]) -> Result<LogData> {
        let mut timestamp = data[0..=2].to_vec();
        timestamp.insert(0, 0);
        // The timestamp is 2 bytes long by design so this unwrap cannot fail
        let timestamp = u32::from_le_bytes(timestamp.try_into().unwrap());

        let mut index = 3;
        let mut log_data = HashMap::new();
        for (name, value_type) in &self.log_block.variables {
            let byte_length = value_type.byte_length();
            log_data.insert(
                name.clone(),
                Value::from_le_bytes(&data[index..(index + byte_length)], *value_type)?,
            );
            index += byte_length;
        }

        Ok(LogData {
            timestamp,
            data: log_data,
        })
    }
}

#[derive(Debug)]
pub struct LogData {
    pub timestamp: u32,
    pub data: HashMap<String, Value>,
}

pub struct LogPeriod(u8);

impl LogPeriod {
    pub fn from_millis(millis: u64) -> Result<Self> {
        Duration::from_millis(millis).try_into()
    }
}

impl TryFrom<Duration> for LogPeriod {
    type Error = Error;

    fn try_from(value: Duration) -> Result<Self> {
        let period_ms = value.as_millis();
        let period_arg = period_ms / 10;
        if period_arg == 0 || period_arg > 255 {
            return Err(Error::LogError(
                "Invalid log period, should be between 10ms and 2550ms".to_owned(),
            ));
        }
        Ok(LogPeriod(period_arg as u8))
    }
}
