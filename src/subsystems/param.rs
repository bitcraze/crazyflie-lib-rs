//! # Parameter subsystem
//!
//! The Crazyflie exposes a param subsystem that allows to easily declare parameter
//! variables in the Crazyflie and to discover, read and write them from the ground.
//!
//! Variables are defined in a table of content that is downloaded upon connection.
//! Each param variable have a unique name composed from a group and a variable name.
//! Functions that accesses variables, take a `name` parameter that accepts a string
//! in the format "group.variable"
//!
//! During connection, the full param table of content is downloaded from the
//! Crazyflie. Parameter values are loaded on-demand when first accessed via `get()`.
//! Parameters can also be set without reading them first. If a variable value
//! is modified by the Crazyflie during runtime, it sends a packet with the new
//! value which updates the local value cache.

use crate::crtp_utils::TocCache;
use crate::{crtp_utils::WaitForPacket, Error, Result};
use crate::{Value, ValueType};
use crazyflie_link::Packet;
use flume as channel;
use futures::lock::Mutex;
use serde::{Serialize, Deserialize};
use std::{
    collections::{BTreeMap, HashMap},
    convert::{TryFrom, TryInto},
    sync::Arc,
};

use crate::crazyflie::PARAM_PORT;

/// State of a persistent parameter
#[derive(Debug, Clone)]
pub struct PersistentParamState {
    /// True if a value is currently stored in EEPROM
    pub is_stored: bool,
    /// The firmware's default value for this parameter
    pub default_value: Value,
    /// The value stored in EEPROM (if is_stored is true)
    pub stored_value: Option<Value>,
}

/// Cached state for a parameter's default value.
#[derive(Debug, Clone, Copy)]
enum DefaultValueCache {
    /// Parameter has this default value
    Value(Value),
    /// Parameter doesn't support default value fetching (ENOENT)
    Unsupported,
}

#[derive(Debug, Serialize, Deserialize)]
struct ParamItemInfo {
    item_type: ValueType,
    writable: bool,
    has_extended_type: bool, // Bit 4: indicates extended type info exists
}

impl TryFrom<u8> for ParamItemInfo {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        Ok(Self {
            item_type: match value & 0x0f {
                0x08 => ValueType::U8,
                0x09 => ValueType::U16,
                0x0A => ValueType::U32,
                0x0B => ValueType::U64,
                0x00 => ValueType::I8,
                0x01 => ValueType::I16,
                0x02 => ValueType::I32,
                0x03 => ValueType::I64,
                0x05 => ValueType::F16,
                0x06 => ValueType::F32,
                0x07 => ValueType::F64,
                _ => {
                    return Err(Error::ParamError(format!(
                        "Type error in TOC: type {} is unknown",
                        value & 0x0f
                    )))
                }
            },
            writable: (value & (1 << 6)) == 0,
            has_extended_type: (value & (1 << 4)) != 0,
        })
    }
}

type ParamChangeWatchers =
    Arc<Mutex<Vec<futures::channel::mpsc::UnboundedSender<(String, Value)>>>>;

async fn notify_watchers(watchers: &ParamChangeWatchers, name: String, value: Value) {
    let mut to_remove = Vec::new();
    let mut watchers = watchers.lock().await;

    for (i, watcher) in watchers.iter().enumerate() {
        if watcher.unbounded_send((name.clone(), value)).is_err() {
            to_remove.push(i);
        }
    }

    // Remove watchers that have dropped
    for i in to_remove.into_iter().rev() {
        watchers.remove(i);
    }
}

/// # Access to the Crazyflie Param Subsystem
///
/// This struct provide methods to interact with the parameter subsystem. See the
/// [param module documentation](crate::subsystems::param) for more context and information.
#[derive(Debug)]
pub struct Param {
    uplink: channel::Sender<Packet>,
    read_downlink: channel::Receiver<Packet>,
    write_downlink: Mutex<channel::Receiver<Packet>>,
    misc_downlink: Mutex<channel::Receiver<Packet>>,
    toc: Arc<BTreeMap<String, (u16, ParamItemInfo)>>,
    values: Arc<Mutex<HashMap<String, Option<Value>>>>,
    default_values: Arc<Mutex<HashMap<String, DefaultValueCache>>>,
    watchers: ParamChangeWatchers,
}

fn not_found(name: &str) -> Error {
    Error::ParamError(format!("Parameter {} not found", name))
}

const READ_CHANNEL: u8 = 1;
const _WRITE_CHANNEL: u8 = 2;
const MISC_CHANNEL: u8 = 3;

// MISC channel and commands for persistent parameters
const _MISC_GET_EXTENDED_TYPE: u8 = 2; // V1 - deprecated, use V2
const MISC_PERSISTENT_STORE: u8 = 3;
const MISC_PERSISTENT_GET_STATE: u8 = 4;
const MISC_PERSISTENT_CLEAR: u8 = 5;
const _MISC_GET_DEFAULT_VALUE: u8 = 6; // V1 - deprecated, use V2
const MISC_GET_EXTENDED_TYPE_V2: u8 = 7;
const MISC_GET_DEFAULT_VALUE_V2: u8 = 8;

impl Param {
    pub(crate) async fn new<T>(
        downlink: channel::Receiver<Packet>,
        uplink: channel::Sender<Packet>,
        toc_cache: T,
    ) -> Result<Self>
    where
        T: TocCache,
    {
        let (toc_downlink, read_downlink, write_downlink, misc_downlink) =
            crate::crtp_utils::crtp_channel_dispatcher(downlink);

        let toc = crate::crtp_utils::fetch_toc(PARAM_PORT, uplink.clone(), toc_downlink, toc_cache).await?;

        // Create a channel for MISC commands (not param updates)
        let (misc_cmd_tx, misc_cmd_rx) = channel::unbounded();

        let mut param = Self {
            uplink,
            read_downlink,
            write_downlink: Mutex::new(write_downlink),
            misc_downlink: Mutex::new(misc_cmd_rx),
            toc: Arc::new(toc),
            values: Arc::new(Mutex::new(HashMap::new())),
            default_values: Arc::new(Mutex::new(HashMap::new())),
            watchers: Arc::default(),
        };

        param.initialize_values().await?;

        param.spawn_misc_loop(misc_downlink, misc_cmd_tx).await;

        Ok(param)
    }

    async fn initialize_values(&mut self) -> Result<()> {
        for (name, (_param_id, _info)) in self.toc.as_ref() {
            let mut values = self.values.lock().await;
            values.insert(
                name.into(),
                None,
            );
        }

        Ok(())
    }

    async fn read_value(&self, param_id: u16, param_type: ValueType) -> Result<Value> {
        let request = Packet::new(PARAM_PORT, READ_CHANNEL, param_id.to_le_bytes().into());
        self.uplink
            .send_async(request.clone())
            .await
            .map_err(|_| Error::Disconnected)?;

        let response = self
            .read_downlink
            .wait_packet(
                request.get_port(),
                request.get_channel(),
                request.get_data(),
            )
            .await?;

        Value::from_le_bytes(&response.get_data()[3..], param_type)
    }

    async fn spawn_misc_loop(&self, misc_downlink: channel::Receiver<Packet>, misc_cmd_tx: channel::Sender<Packet>) {
        let values = self.values.clone();
        let toc = self.toc.clone();
        let watchers = self.watchers.clone();

        tokio::spawn(async move {
            while let Ok(pk) = misc_downlink.recv_async().await {
                // Command byte 1 = parameter update notification
                if pk.get_data().first() == Some(&1) {
                    // The range sets the buffer to 2 bytes long so this unwrap cannot fail
                    let param_id = u16::from_le_bytes(pk.get_data()[1..3].try_into().unwrap());
                    if let Some((param, (_, item_info))) = toc.iter().find(|v| v.1 .0 == param_id) {
                        if let Ok(value) =
                            Value::from_le_bytes(&pk.get_data()[3..], item_info.item_type)
                        {
                            // The param is tested as being in the toc so this unwrap cannot fail.
                            *values.lock().await.get_mut(param).unwrap() = Some(value);

                            notify_watchers(&watchers, param.clone(), value).await;
                        } else {
                            println!("Warning: Malformed param update");
                        }
                    } else {
                        println!("Warning: malformed param update");
                    }
                } else {
                    // Other MISC commands - forward to misc_cmd_tx
                    let _ = misc_cmd_tx.send_async(pk).await;
                }
            }
        });
    }

    /// Get the names of all the parameters
    ///
    /// The names contain group and name of the parameter variable formatted as
    /// "group.name".
    pub fn names(&self) -> Vec<String> {
        self.toc.keys().cloned().collect()
    }

    /// Return the type of a parameter variable or an Error if the parameter does not exist.
    pub fn get_type(&self, name: &str) -> Result<ValueType> {
        Ok(self
            .toc
            .get(name)
            .ok_or_else(|| not_found(name))?
            .1
            .item_type)
    }

    /// Return true if he parameter variable is writable. False otherwise.
    ///
    /// Return an error if the parameter does not exist.
    pub fn is_writable(&self, name: &str) -> Result<bool> {
        Ok(self
            .toc
            .get(name)
            .ok_or_else(|| not_found(name))?
            .1
            .writable)
    }

    /// Set a parameter value.
    ///
    /// This function will set the variable value and wait for confirmation from the
    /// Crazyflie. If the set is successful `Ok(())` is returned, otherwise the
    /// error code reported by the Crazyflie is returned in the error.
    ///
    /// This function accepts any primitive type as well as the [Value](crate::Value) type. The
    /// type of the param variable is checked at runtime and must match the type
    /// given to the function, either the direct primitive type or the type
    /// contained in the `Value` enum. For example, to write a u16 value, both lines are valid:
    ///
    /// ```no_run
    /// # use crazyflie_lib::{Crazyflie, Value, Error};
    /// # use crazyflie_link::LinkContext;
    /// # async fn example() -> Result<(), Error> {
    /// # let context = LinkContext::new();
    /// # let cf = Crazyflie::connect_from_uri(
    /// #   &context,
    /// #   "radio://0/60/2M/E7E7E7E7E7",
    /// #   crazyflie_lib::NoTocCache
    /// # ).await?;
    /// cf.param.set("example.param", 42u16).await?;  // From primitive
    /// cf.param.set("example.param", Value::U16(42)).await?;  // From Value
    /// # Ok(())
    /// # };
    /// ```
    ///
    /// Return an error in case of type mismatch or if the variable does not exist.
    pub async fn set<T: Into<Value>>(&self, param: &str, value: T) -> Result<()> {
        let value: Value = value.into();
        let (param_id, param_info) = self.toc.get(param).ok_or_else(|| not_found(param))?;

        if param_info.item_type != value.into() {
            return Err(Error::ParamError(format!(
                "Parameter {} is type {:?}, cannot set with value {:?}",
                param, param_info.item_type, value
            )));
        }

        let downlink = self.write_downlink.lock().await;

        let mut request_data = Vec::from(param_id.to_le_bytes());
        request_data.append(&mut value.into());
        let request = Packet::new(PARAM_PORT, _WRITE_CHANNEL, request_data);
        self.uplink
            .send_async(request)
            .await
            .map_err(|_| Error::Disconnected)?;

        let answer = downlink
            .wait_packet(PARAM_PORT, _WRITE_CHANNEL, &param_id.to_le_bytes())
            .await?;

        // Success response: firmware echoes back the written value
        let expected_bytes: Vec<u8> = value.into();
        let data = answer.get_data();
        if data.len() < 2 {
            return Err(Error::ProtocolError(
                format!("Parameter write response too short: expected at least 2 bytes, got {}", data.len())
            ));
        }
        let echoed_bytes = &data[2..];

        if echoed_bytes == expected_bytes.as_slice() {
            // The param is tested as being in the TOC so this unwrap cannot fail
            *self.values.lock().await.get_mut(param).unwrap() = Some(value);
            notify_watchers(&self.watchers, param.to_owned(), value).await;
            Ok(())
        } else {
            // If echoed value doesn't match, it's likely a parameter error code
            if echoed_bytes.is_empty() {
                return Err(Error::ProtocolError(
                    "Parameter write response invalid: no error code or echoed value".to_string()
                ));
            }
            let error_code = echoed_bytes[0]; // For u8 params, single byte error code
            Err(Error::ParamError(format!(
                "Error setting parameter: parameter error code {}",
                error_code
            )))
        }
    }

    /// Get param value
    ///
    /// Get value of a parameter. The first access will fetch the value from the
    /// Crazyflie. Subsequent accesses are served from a local cache and are quick.
    ///
    /// Similarly to the `set` function above, the type of the param must match
    /// the return parameter. For example to get a u16 param:
    /// ```no_run
    /// # use crazyflie_lib::{Crazyflie, Value, Error};
    /// # use crazyflie_link::LinkContext;
    /// # async fn example() -> Result<(), Error> {
    /// # let context = LinkContext::new();
    /// # let cf = Crazyflie::connect_from_uri(
    /// #   &context,
    /// #   "radio://0/60/2M/E7E7E7E7E7",
    /// #   crazyflie_lib::NoTocCache
    /// # ).await?;
    /// let example: u16 = cf.param.get("example.param").await?;  // To primitive
    /// dbg!(example);  // 42
    /// let example: Value = cf.param.get("example.param").await?;  // To Value
    /// dbg!(example);  // Value::U16(42)
    /// # Ok(())
    /// # };
    /// ```
    ///
    /// Return an error in case of type mismatch or if the variable does not exist.
    pub async fn get<T: TryFrom<Value>>(&self, name: &str) -> Result<T>
    where
        <T as TryFrom<Value>>::Error: std::fmt::Debug,
    {
        let mut values = self.values.lock().await;

        let value = *values.get(name)
            .ok_or_else(|| not_found(name))?;

        // If the value is None it means it has never been read, read it now and update the value
        let value = match value {
            Some(v) => v,
            None => {
                let (param_id, param_info) = self
                    .toc
                    .get(name)
                    .ok_or_else(|| not_found(name))?;
                let v = self.read_value(*param_id, param_info.item_type).await?;
                // Update the cache
                *values.get_mut(name).unwrap() = Some(v.clone());
                v
            }
        };

        Ok(value
            .try_into()
            .map_err(|e| Error::ParamError(format!("Type error reading param: {:?}", e)))?)
    }

    /// Set a parameter from a f64 potentially loosing data
    ///
    /// This function is a forgiving version of the `set` function. It allows
    /// to set any parameter of any type from a `f64` value. This allows to set
    /// parameters without caring about the type and risking a type mismatch
    /// runtime error. Since there is no type or value check, loss of information
    /// can happen when using this function.
    ///
    /// Loss of information can happen in the following cases:
    ///  - When setting an integer, the value is truncated to the number of bit of the parameter
    ///    - Example: Setting `257` to a `u8` variable will set it to the value `1`
    ///  - Similarly floating point precision will be truncated to the parameter precision. Rounding is undefined.
    ///  - Setting a floating point outside the range of the parameter is undefined.
    ///  - It is not possible to represent accurately a `u64` parameter in a `f64`.
    ///
    /// Returns an error if the param does not exists.
    pub async fn set_lossy(&self, name: &str, value: f64) -> Result<()> {
        let param_type = self
            .toc
            .get(name)
            .ok_or_else(|| not_found(name))?
            .1
            .item_type;

        let value = Value::from_f64_lossy(param_type, value);

        self.set(name, value).await
    }

    /// Get a parameter as a `f64` independently of the parameter type
    ///
    /// This function is a forgiving version of the `get` function. It allows
    /// to get any parameter of any type as a `f64` value. This allows to get
    /// parameters without caring about the type and risking a type mismatch
    /// runtime error. Since there is no type or value check, loss of information
    /// can happen when using this function.
    ///
    /// Loss of information can happen in the following cases:
    ///  - It is not possible to represent accurately a `u64` parameter in a `f64`.
    ///
    /// Returns an error if the param does not exists.
    pub async fn get_lossy(&self, name: &str) -> Result<f64> {
        let value: Value = self.get(name).await?;

        Ok(value.to_f64_lossy())
    }

    /// Get notified for all parameter value change
    ///
    /// This function returns an async stream that will generate a tuple containing
    /// the name of the variable that has changed (in the form of group.name)
    /// and its new value.
    ///
    /// There can be two reasons for a parameter to change:
    ///  - Either the parameter was changed by a call to [Param::set()]. The
    ///    notification will be generated when the Crazyflie confirms the parameter
    ///    has been set.
    ///  - Or it can be a parameter change in the Crazyflie itself. The Crazyflie
    ///    will send notification packet for every internal parameter change.
    pub async fn watch_change(&self) -> impl futures::Stream<Item = (String, Value)> + use<> {
        let (tx, rx) = futures::channel::mpsc::unbounded();

        let mut watchers = self.watchers.lock().await;
        watchers.push(tx);

        rx
    }

    /// Check if a parameter supports persistent storage
    ///
    /// Returns `true` if the parameter can be stored in EEPROM, `false` otherwise.
    ///
    /// This queries the firmware for the parameter's extended type flags to determine
    /// if the PERSISTENT flag is set.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The parameter does not exist
    /// - Communication with the Crazyflie fails
    pub async fn is_persistent(&self, name: &str) -> Result<bool> {
        // Check if parameter has extended type flag (bit 4)
        let (_, param_info) = self.toc.get(name).ok_or_else(|| not_found(name))?;

        // If no extended type, it's not persistent
        if !param_info.has_extended_type {
            return Ok(false);
        }

        // Query the actual extended type flags
        let extended_type = self.get_extended_type(name).await?;

        // Check if PERSISTENT flag (bit 0) is set
        Ok((extended_type & 0x01) != 0)
    }

    /// Get the extended type flags of a parameter from the firmware
    ///
    /// Returns a bitfield of extended type flags. Currently defined flags:
    /// - `0x01`: PERSISTENT - parameter can be stored in EEPROM
    ///
    /// This queries the firmware directly. For most use cases, [`is_persistent()`](Self::is_persistent)
    /// is more convenient as it provides a boolean result and first checks the TOC's
    /// `has_extended_type` flag before querying the firmware.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The parameter does not exist
    /// - The parameter does not have extended type information
    /// - Communication with the Crazyflie fails
    pub async fn get_extended_type(&self, name: &str) -> Result<u8> {
        let (param_id, _) = self.toc.get(name).ok_or_else(|| not_found(name))?;

        // Send request: [CMD(1), ID(2)]
        let request_data = vec![
            MISC_GET_EXTENDED_TYPE_V2,
            (param_id & 0xff) as u8,
            (param_id >> 8) as u8,
        ];
        let request = Packet::new(PARAM_PORT, MISC_CHANNEL, request_data.clone());

        // Lock before sending to prevent race conditions with concurrent requests
        let misc_downlink = self.misc_downlink.lock().await;

        self.uplink
            .send_async(request)
            .await
            .map_err(|_| Error::Disconnected)?;

        // Wait for response
        // V2 success: [CMD(1), ID(2), STATUS(1), EXTENDED_TYPE(1)]
        // Error: [CMD(1), ID(2), ERROR(1)]
        let response = misc_downlink
            .wait_packet(PARAM_PORT, MISC_CHANNEL, &request_data)
            .await?;

        let data = response.get_data();

        // Verify minimum response length
        if data.len() < 4 {
            return Err(Error::ProtocolError(format!(
                "Response too short: expected at least 4 bytes, got {}",
                data.len()
            )));
        }

        // Check if this is an error response (exactly 4 bytes)
        if data.len() == 4 {
            let error_code = data[3];
            if error_code == 0x02 {
                // ENOENT: parameter ID invalid OR parameter doesn't have PARAM_EXTENDED flag
                return Err(Error::ParamError(format!(
                    "Parameter '{}' does not have extended type info (not marked as PARAM_EXTENDED in firmware)",
                    name
                )));
            } else {
                return Err(Error::ParamError(format!(
                    "Failed to get extended type for '{}': error code {}",
                    name, error_code
                )));
            }
        }

        // V2 success response: [CMD, ID_LOW, ID_HIGH, 0x00, EXTENDED_TYPE]
        if data.len() < 5 {
            return Err(Error::ProtocolError(format!(
                "Response too short for V2 success: expected 5 bytes, got {}",
                data.len()
            )));
        }

        let status = data[3];
        if status != 0x00 {
            return Err(Error::ProtocolError(format!(
                "Unexpected status byte in V2 response: expected 0x00, got 0x{:02x}",
                status
            )));
        }

        Ok(data[4])
    }

    /// Get the default value of a parameter as defined in the firmware
    ///
    /// This retrieves the default value that the parameter has in the firmware,
    /// regardless of whether a different value has been stored in EEPROM.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The parameter does not exist
    /// - The firmware does not support getting default values for this parameter
    /// - Communication with the Crazyflie fails
    pub async fn get_default_value(&self, name: &str) -> Result<Value> {
        // Check cache first
        {
            let cache = self.default_values.lock().await;
            if let Some(cached) = cache.get(name) {
                return match cached {
                    DefaultValueCache::Value(v) => Ok(*v),
                    DefaultValueCache::Unsupported => Err(Error::ParamError(format!(
                        "Parameter '{}' does not support get_default_value (read-only or invalid)",
                        name
                    ))),
                };
            }
        }

        let (param_id, param_info) = self.toc.get(name).ok_or_else(|| not_found(name))?;

        // Send request: [CMD(1), ID(2)]
        let request_data = vec![
            MISC_GET_DEFAULT_VALUE_V2,
            (param_id & 0xff) as u8,
            (param_id >> 8) as u8,
        ];
        let request = Packet::new(PARAM_PORT, MISC_CHANNEL, request_data.clone());

        // Lock before sending to prevent race conditions with concurrent requests
        let misc_downlink = self.misc_downlink.lock().await;

        self.uplink
            .send_async(request)
            .await
            .map_err(|_| Error::Disconnected)?;

        // Wait for response
        // V2 success: [CMD(1), ID(2), STATUS(1), VALUE(?)]
        // Error: [CMD(1), ID(2), ERROR(1)]
        let response = misc_downlink
            .wait_packet(PARAM_PORT, MISC_CHANNEL, &request_data)
            .await?;

        let data = response.get_data();

        // Verify minimum response length
        if data.len() < 4 {
            return Err(Error::ProtocolError(format!(
                "Response too short: expected at least 4 bytes, got {}",
                data.len()
            )));
        }

        // Check if this is an error response (exactly 4 bytes)
        if data.len() == 4 {
            let error_code = data[3];
            if error_code == 0x02 {
                // ENOENT: parameter ID invalid OR parameter is read-only
                // (read-only params have no default value concept in firmware)
                // Cache the unsupported state so we don't query again
                let mut cache = self.default_values.lock().await;
                cache.insert(name.to_owned(), DefaultValueCache::Unsupported);

                return Err(Error::ParamError(format!(
                    "Parameter '{}' does not support get_default_value (read-only or invalid)",
                    name
                )));
            } else {
                return Err(Error::ParamError(format!(
                    "Failed to get default value for '{}': error code {}",
                    name, error_code
                )));
            }
        }

        // V2 success response: [CMD, ID_LOW, ID_HIGH, 0x00, VALUE...]
        let status = data[3];
        if status != 0x00 {
            return Err(Error::ProtocolError(format!(
                "Unexpected status byte in V2 response: expected 0x00, got 0x{:02x}",
                status
            )));
        }

        // Parse value from data[4..] and cache it
        let value = Value::from_le_bytes(&data[4..], param_info.item_type)?;

        {
            let mut cache = self.default_values.lock().await;
            cache.insert(name.to_owned(), DefaultValueCache::Value(value));
        }

        Ok(value)
    }

    /// Get the complete state of a persistent parameter
    ///
    /// This retrieves comprehensive information about a persistent parameter:
    /// - Whether a value is currently stored in EEPROM
    /// - The firmware's default value
    /// - The stored value (if one exists)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The parameter does not exist
    /// - The parameter is not persistent
    /// - Communication with the Crazyflie fails
    /// - The firmware does not support this operation for the parameter
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use crazyflie_lib::{Crazyflie, NoTocCache};
    /// # use crazyflie_link::LinkContext;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let context = LinkContext::new();
    /// # let cf = Crazyflie::connect_from_uri(
    /// #   &context,
    /// #   "radio://0/60/2M/E7E7E7E7E7",
    /// #   crazyflie_lib::NoTocCache
    /// # ).await?;
    /// let state = cf.param.persistent_get_state("ring.effect").await?;
    /// 
    /// println!("Default value: {:?}", state.default_value);
    /// if state.is_stored {
    ///     println!("Stored value: {:?}", state.stored_value.unwrap());
    /// } else {
    ///     println!("Using default (not stored)");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn persistent_get_state(&self, name: &str) -> Result<PersistentParamState> {
        let (param_id, param_info) = self.toc.get(name).ok_or_else(|| not_found(name))?;

        if !self.is_persistent(name).await? {
            return Err(Error::ParamError(format!(
                "Parameter '{}' is not persistent",
                name
            )));
        }

        // Send request: [CMD(1), ID(2)]
        let request_data = vec![
            MISC_PERSISTENT_GET_STATE,
            (param_id & 0xff) as u8,
            (param_id >> 8) as u8,
        ];
        let request = Packet::new(PARAM_PORT, MISC_CHANNEL, request_data.clone());

        // Lock before sending to prevent race conditions with concurrent requests
        let misc_downlink = self.misc_downlink.lock().await;

        self.uplink
            .send_async(request)
            .await
            .map_err(|_| Error::Disconnected)?;

        // Wait for response: [CMD(1), ID(2), STATUS(1), VALUE_DATA(?)]
        let response = misc_downlink
            .wait_packet(PARAM_PORT, MISC_CHANNEL, &request_data)
            .await?;

        let data = response.get_data();

        // Response format: [CMD(1), ID(2), STATUS(1), VALUE_DATA(?)]
        // Verify minimum response length
        if data.len() < 4 {
            return Err(Error::ProtocolError(format!(
                "Response too short: expected at least 4 bytes, got {}",
                data.len()
            )));
        }

        let status = data[3];

        // Validate status code:
        // 0x00 = PARAM_PERSISTENT_NOT_STORED (no value in persistent storage)
        // 0x01 = PARAM_PERSISTENT_STORED (value exists in persistent storage)
        // 0x02 = ENOENT (parameter ID doesn't exist in firmware)
        let is_stored = match status {
            0x00 => false,
            0x01 => true,
            0x02 => {
                return Err(Error::ParamError(format!(
                    "Parameter ID for '{}' is invalid or doesn't exist in firmware (ENOENT)",
                    name
                )));
            }
            _ => {
                return Err(Error::ProtocolError(format!(
                    "Unexpected status code {} in persistent_get_state response for '{}'",
                    status, name
                )));
            }
        };
        let value_size = param_info.item_type.byte_length();

        // Parse values from data[4..]
        if is_stored {
            // Both default and stored values present
            if data.len() < 4 + 2 * value_size {
                return Err(Error::ProtocolError(format!(
                    "Response too short for stored state: expected {} bytes, got {}",
                    4 + 2 * value_size,
                    data.len()
                )));
            }

            let default_value = Value::from_le_bytes(&data[4..4 + value_size], param_info.item_type)?;
            let stored_value = Value::from_le_bytes(&data[4 + value_size..4 + 2 * value_size], param_info.item_type)?;

            Ok(PersistentParamState {
                is_stored: true,
                default_value,
                stored_value: Some(stored_value),
            })
        } else {
            // Only default value present
            if data.len() < 4 + value_size {
                return Err(Error::ProtocolError(format!(
                    "Response too short for default value: expected {} bytes, got {}",
                    4 + value_size,
                    data.len()
                )));
            }

            let default_value = Value::from_le_bytes(&data[4..4 + value_size], param_info.item_type)?;

            Ok(PersistentParamState {
                is_stored: false,
                default_value,
                stored_value: None,
            })
        }
    }

    /// Store the current value of a persistent parameter to EEPROM.
    ///
    /// This writes the parameter's current value to non-volatile memory,
    /// so it will be used as the default on subsequent boots.
    ///
    /// # Example
    /// ```no_run
    /// # async fn example(cf: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
    /// // First set the value you want to persist
    /// cf.param.set("ring.effect", 10u8).await?;
    /// 
    /// // Then store it to EEPROM
    /// cf.param.persistent_store("ring.effect").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn persistent_store(&self, name: &str) -> Result<()> {
        let (param_id, _) = self.toc.get(name).ok_or_else(|| not_found(name))?;

        if !self.is_persistent(name).await? {
            return Err(Error::ParamError(format!(
                "Parameter '{}' is not persistent",
                name
            )));
        }

        // Send request: [CMD(1), ID(2)]
        let request_data = vec![
            MISC_PERSISTENT_STORE,
            (param_id & 0xff) as u8,
            (param_id >> 8) as u8,
        ];
        let request = Packet::new(PARAM_PORT, MISC_CHANNEL, request_data.clone());

        // Lock before sending to prevent race conditions with concurrent requests
        let misc_downlink = self.misc_downlink.lock().await;

        self.uplink
            .send_async(request)
            .await
            .map_err(|_| Error::Disconnected)?;

        // Wait for response: [CMD(1), ID(2), STATUS(1)]
        let response = misc_downlink
            .wait_packet(PARAM_PORT, MISC_CHANNEL, &request_data)
            .await?;

        let data = response.get_data();

        // Verify response length
        if data.len() < 4 {
            return Err(Error::ProtocolError(format!(
                "Response too short: expected 4 bytes, got {}",
                data.len()
            )));
        }

        let status = data[3];

        match status {
            0x00 => Ok(()),
            0x02 => {
                // ENOENT: storage operation failed (couldn't write to EEPROM)
                // or parameter ID invalid (shouldn't happen since we verified the ID)
                Err(Error::ParamError(format!(
                    "Failed to store parameter '{}' to EEPROM (storage write failed)",
                    name
                )))
            }
            _ => Err(Error::ProtocolError(format!(
                "Unexpected status code {} in persistent_store response for '{}'",
                status, name
            ))),
        }
    }

    /// Clear the stored value of a persistent parameter from EEPROM.
    ///
    /// This removes the parameter's stored value from non-volatile memory,
    /// causing it to revert to the firmware default on subsequent boots.
    ///
    /// # Example
    /// ```no_run
    /// # async fn example(cf: &crazyflie_lib::Crazyflie) -> crazyflie_lib::Result<()> {
    /// // Clear the stored value, reverting to default
    /// cf.param.persistent_clear("ring.effect").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn persistent_clear(&self, name: &str) -> Result<()> {
        let (param_id, _) = self.toc.get(name).ok_or_else(|| not_found(name))?;

        if !self.is_persistent(name).await? {
            return Err(Error::ParamError(format!(
                "Parameter '{}' is not persistent",
                name
            )));
        }

        // Send request: [CMD(1), ID(2)]
        let request_data = vec![
            MISC_PERSISTENT_CLEAR,
            (param_id & 0xff) as u8,
            (param_id >> 8) as u8,
        ];
        let request = Packet::new(PARAM_PORT, MISC_CHANNEL, request_data.clone());

        // Lock before sending to prevent race conditions with concurrent requests
        let misc_downlink = self.misc_downlink.lock().await;

        self.uplink
            .send_async(request)
            .await
            .map_err(|_| Error::Disconnected)?;

        // Wait for response: [CMD(1), ID(2), STATUS(1)]
        let response = misc_downlink
            .wait_packet(PARAM_PORT, MISC_CHANNEL, &request_data)
            .await?;

        let data = response.get_data();

        // Verify response length
        if data.len() < 4 {
            return Err(Error::ProtocolError(format!(
                "Response too short: expected 4 bytes, got {}",
                data.len()
            )));
        }

        let status = data[3];

        match status {
            0x00 => Ok(()),
            0x02 => {
                // ENOENT: storage delete failed (couldn't delete from EEPROM)
                // or parameter ID invalid (shouldn't happen since we verified the ID)
                Err(Error::ParamError(format!(
                    "Failed to clear parameter '{}' from EEPROM (storage delete failed)",
                    name
                )))
            }
            _ => Err(Error::ProtocolError(format!(
                "Unexpected status code {} in persistent_clear response for '{}'",
                status, name
            ))),
        }
    }
}
