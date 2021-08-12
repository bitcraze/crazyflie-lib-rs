use half::prelude::*;

use crate::{Error, Result, WaitForPacket};
use crate::{Value, ValueType};
use crazyflie_link::Packet;
use flume as channel;
use futures::lock::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
    convert::{TryFrom, TryInto},
    sync::Arc,
};

#[derive(Debug)]
struct ParamItemInfo {
    item_type: ValueType,
    writable: bool,
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
        })
    }
}

/// # Access to the Crazyflie Param Subsystem
///
/// The Crazyflie exposes a param subsystem that allows to easily declare parameter
/// variables in the Crazyflie and to discover, read and write them from the ground.
///
/// Variables are defined in a table of content that is downloaded uppon connection.
/// Each param variable have a unique name composed from a group and a variable name.
/// Functions that accesses variables, take a `name` parameter that accepts a string
/// in the format "group.variable"
///
/// During connection, the full param table of content is downloaded form the
/// Crazyflie as well as the values of all the variable. If a variable value
/// is modified by the Crazyflie during runtime, it sends a packet with the new
/// value which updates the local value cache.
#[derive(Debug)]
pub struct Param {
    uplink: channel::Sender<Packet>,
    read_downlink: channel::Receiver<Packet>,
    write_downlink: Mutex<channel::Receiver<Packet>>,
    toc: Arc<BTreeMap<String, (u16, ParamItemInfo)>>,
    values: Arc<Mutex<HashMap<String, Value>>>,
}

fn not_found(name: &str) -> Error {
    Error::ParamError(format!("Parameter {} not found", name))
}

const PARAM_PORT: u8 = 2;
const READ_CHANNEL: u8 = 1;
const _WRITE_CHANNEL: u8 = 2;
const _MISC_CHANNEL: u8 = 3;

impl Param {
    pub(crate) async fn new(
        downlink: channel::Receiver<Packet>,
        uplink: channel::Sender<Packet>,
    ) -> Result<Self> {
        let (toc_downlink, read_downlink, write_downlink, misc_downlink) =
            crate::crtp_channel_dispatcher(downlink);

        let toc = crate::fetch_toc(PARAM_PORT, uplink.clone(), toc_downlink).await?;

        let mut param = Self {
            uplink,
            read_downlink,
            write_downlink: Mutex::new(write_downlink),
            toc: Arc::new(toc),
            values: Arc::new(Mutex::new(HashMap::new())),
        };

        param.update_values().await?;

        param.spawn_misc_loop(misc_downlink).await;

        Ok(param)
    }

    async fn update_values(&mut self) -> Result<()> {
        for (name, (param_id, info)) in self.toc.as_ref() {
            let mut values = self.values.lock().await;
            values.insert(
                name.into(),
                self.read_value(*param_id, info.item_type).await?,
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

    async fn spawn_misc_loop(&self, misc_downlink: channel::Receiver<Packet>) {
        let values = self.values.clone();
        let toc = self.toc.clone();

        crate::spawn(async move {
            while let Ok(pk) = misc_downlink.recv_async().await {
                if pk.get_data()[0] != 1 {
                    continue;
                }

                // The range sets the buffer to 2 bytes long so this unwrap cannot fail
                let param_id = u16::from_le_bytes(pk.get_data()[1..3].try_into().unwrap());
                if let Some((param, (_, item_info))) = toc.iter().find(|v| v.1 .0 == param_id) {
                    if let Ok(value) =
                        Value::from_le_bytes(&pk.get_data()[3..], item_info.item_type)
                    {
                        // The param is tested as being in the toc so this unwrap cannot fail.
                        *values.lock().await.get_mut(param).unwrap() = value;
                    } else {
                        println!("Warning: Mallformed param update");
                    }
                } else {
                    println!("Warning: malformed param update");
                }
            }
        });
    }

    /// Get the names of all the parameters
    ///
    /// The names contain group and name of the parameter variable formated as
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
    /// Crazyflie. If the set is successfull `Ok(())` is returned, otherwise the
    /// error code reported by the Crazyflie is returned in the error.
    ///
    /// This function accepts any pritimive type as well as the [Value](crate::Value) type. The
    /// type of the param variable is checked at runtime and must match the type
    /// given to the function, either the direct primitive type or the type
    /// contained in the `Value` enum. For example, to write a u16 value, one
    /// would write:
    ///
    /// ```no_run
    /// # use crazyflie_lib::{Crazyflie, Value, Error};
    /// # use async_executors::AsyncStd;
    /// # use crazyflie_link::LinkContext;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Error> {
    /// # let context = LinkContext::new(Arc::new(AsyncStd));
    /// # let cf = Crazyflie::connect_from_uri(&context, "radio://0/60/2M/E7E7E7E7E7").await;
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

        if answer.get_data()[2] == 0 {
            // The param is tested as being in the TOC so this unwrap cannot fail
            *self.values.lock().await.get_mut(param).unwrap() = value;
            Ok(())
        } else {
            Err(Error::ParamError(format!(
                "Error setting the parameter: code {}",
                answer.get_data()[2]
            )))
        }
    }

    /// Get param value
    ///
    /// Get value of a parameter. This frunction takes the value from a local
    /// cache and so is quick.
    ///
    /// Similarly to the `set` function above, the type of the param must match
    /// the return parameter. For example to get a u16 param:
    /// ```no_run
    /// # use crazyflie_lib::{Crazyflie, Value, Error};
    /// # use async_executors::AsyncStd;
    /// # use crazyflie_link::LinkContext;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Error> {
    /// # let context = LinkContext::new(Arc::new(AsyncStd));
    /// # let cf = Crazyflie::connect_from_uri(&context, "radio://0/60/2M/E7E7E7E7E7").await;
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
        let value = *self
            .values
            .lock()
            .await
            .get(name)
            .ok_or_else(|| not_found(name))?;

        Ok(value
            .try_into()
            .map_err(|e| Error::ParamError(format!("Type error reading param: {:?}", e)))?)
    }

    /// Set a parameter from a f64 potentially loosing data
    ///
    /// This function is a forgiving version of the `set` function. It allows
    /// to set any parameter of any type from a `f64` value. This allows to set
    /// parameters without caring about the type and risking a type conversion
    /// runtime error. Since there is no type or value check, loss of information
    /// can happen when using this function.
    ///
    /// Loss of information can happen in the following cases:
    ///  - When setting an integer, the value is truncated to the number of bit of the parameter
    ///    - Example: Setting `257` to a `u8` variable will set it to the value `1`
    ///  - Similarly floating point precision will be truncated to the parameter precision. Rounding is undefined.
    ///  - Setting a floating point outside the range of the parameter is undefined.
    ///  - It is not possible to represent accuratly a `u64` parameter in a `f64`.
    ///
    /// Returns an error if the param does not exists.
    pub async fn set_lossy(&self, name: &str, value: f64) -> Result<()> {
        let param_type = self
            .toc
            .get(name)
            .ok_or_else(|| not_found(name))?
            .1
            .item_type;

        let value = match param_type {
            ValueType::U8 => Value::U8((value as u64) as u8),
            ValueType::U16 => Value::U16((value as u64) as u16),
            ValueType::U32 => Value::U32((value as u64) as u32),
            ValueType::U64 => Value::U64((value as u64) as u64),
            ValueType::I8 => Value::I8((value as i64) as i8),
            ValueType::I16 => Value::I16((value as i64) as i16),
            ValueType::I32 => Value::I32((value as i64) as i32),
            ValueType::I64 => Value::I64((value as i64) as i64),
            ValueType::F16 => Value::F16(f16::from_f64(value)),
            ValueType::F32 => Value::F32(value as f32),
            ValueType::F64 => Value::F64(value),
        };

        self.set(name, value).await
    }

    /// Get a parameter as a `f64` independently of the parameter type
    ///
    /// This function is a forgiving version of the `get` function. It allows
    /// to get any parameter of any type as a `f64` value. This allows to get
    /// parameters without caring about the type and risking a type conversion
    /// runtime error. Since there is no type or value check, loss of information
    /// can happen when using this function.
    ///
    /// Loss of information can happen in the following cases:
    ///  - It is not possible to represent accuratly a `u64` parameter in a `f64`.
    ///
    /// Returns an error if the param does not exists.
    pub async fn get_lossy(&self, name: &str) -> Result<f64> {
        let value: Value = self.get(name).await?;

        Ok(match value {
            Value::U8(v) => v as f64,
            Value::U16(v) => v as f64,
            Value::U32(v) => v as f64,
            Value::U64(v) => v as f64,
            Value::I8(v) => v as f64,
            Value::I16(v) => v as f64,
            Value::I32(v) => v as f64,
            Value::I64(v) => v as f64,
            Value::F16(v) => v.to_f64(),
            Value::F32(v) => v as f64,
            Value::F64(v) => v as f64,
        })
    }
}
