use half::prelude::*;

use crate::{Error, WaitForPacket};
use crazyflie_link::Packet;
use flume as channel;
use futures::lock::Mutex;
use num_enum::TryFromPrimitive;
use std::{
    collections::HashMap,
    convert::{TryFrom, TryInto},
};

#[derive(Debug, TryFromPrimitive, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ParamItemType {
    U8 = 0x08,
    U16 = 0x09,
    U32 = 0x0A,
    U64 = 0x0B,
    I8 = 0x00,
    I16 = 0x01,
    I32 = 0x02,
    I64 = 0x03,
    F16 = 0x05,
    F32 = 0x06,
    F64 = 0x07,
}

#[derive(Debug)]
struct ParamItemInfo {
    item_type: ParamItemType,
    writable: bool,
}

impl TryFrom<u8> for ParamItemInfo {
    type Error = num_enum::TryFromPrimitiveError<ParamItemType>;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(Self {
            item_type: (value & 0x0f).try_into()?,
            writable: (value & 0x10) == 0,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ParamValue {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F16(f32),
    F32(f32),
    F64(f64),
}

impl ParamValue {
    fn from_bytes(bytes: &[u8], param_type: ParamItemType) -> Result<Self, Error> {
        match param_type {
            ParamItemType::U8 => Ok(ParamValue::U8(u8::from_le_bytes(bytes.try_into()?))),
            ParamItemType::U16 => Ok(ParamValue::U16(u16::from_le_bytes(bytes.try_into()?))),
            ParamItemType::U32 => Ok(ParamValue::U32(u32::from_le_bytes(bytes.try_into()?))),
            ParamItemType::U64 => Ok(ParamValue::U64(u64::from_le_bytes(bytes.try_into()?))),
            ParamItemType::I8 => Ok(ParamValue::I8(i8::from_le_bytes(bytes.try_into()?))),
            ParamItemType::I16 => Ok(ParamValue::I16(i16::from_le_bytes(bytes.try_into()?))),
            ParamItemType::I32 => Ok(ParamValue::I32(i32::from_le_bytes(bytes.try_into()?))),
            ParamItemType::I64 => Ok(ParamValue::I64(i64::from_le_bytes(bytes.try_into()?))),
            ParamItemType::F16 => Ok(ParamValue::F16(
                f16::from_le_bytes(bytes.try_into()?).into(),
            )),
            ParamItemType::F32 => Ok(ParamValue::F32(f32::from_le_bytes(bytes.try_into()?))),
            ParamItemType::F64 => Ok(ParamValue::F64(f64::from_le_bytes(bytes.try_into()?))),
        }
    }
}

#[derive(Debug)]
pub struct Param {
    uplink: channel::Sender<Packet>,
    read_downlink: channel::Receiver<Packet>,
    write_downlink: Mutex<channel::Receiver<Packet>>,
    toc: HashMap<String, (u16, ParamItemInfo)>,
    values: Mutex<HashMap<String, ParamValue>>,
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
    ) -> Self {
        let (toc_downlink, read_downlink, write_downlink, _misc_downlink) =
            crate::crtp_channel_dispatcher(downlink);

        let toc = crate::fetch_toc(PARAM_PORT, uplink.clone(), toc_downlink).await;

        let mut param = Self {
            uplink,
            read_downlink,
            write_downlink: Mutex::new(write_downlink),
            toc,
            values: Mutex::new(HashMap::new()),
        };

        param.update_values().await.unwrap();

        param
    }

    async fn update_values(&mut self) -> Result<(), Error> {
        for (name, (param_id, info)) in &self.toc {
            let mut values = self.values.lock().await;
            values.insert(
                name.into(),
                self.read_value(*param_id, info.item_type).await?,
            );
        }

        Ok(())
    }

    async fn read_value(
        &self,
        param_id: u16,
        param_type: ParamItemType,
    ) -> Result<ParamValue, Error> {
        let request = Packet::new(PARAM_PORT, READ_CHANNEL, param_id.to_le_bytes().into());
        self.uplink.send_async(request.clone()).await.unwrap();

        let response = self
            .read_downlink
            .wait_packet(
                request.get_port(),
                request.get_channel(),
                request.get_data(),
            )
            .await;

        ParamValue::from_bytes(&response.get_data()[3..], param_type)
    }

    pub async fn set<T: Into<ParamValue>>(&self, param: &str, value: T) -> Result<(), Error> {
        let value: ParamValue = value.into();
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
        self.uplink.send_async(request).await.unwrap();

        let answer = downlink
            .wait_packet(PARAM_PORT, _WRITE_CHANNEL, &param_id.to_le_bytes())
            .await;

        if answer.get_data()[2] == 0 {
            Ok(())
        } else {
            Err(Error::ParamError(format!(
                "Error setting the parameter: code {}",
                answer.get_data()[2]
            )))
        }
    }

    pub fn get_type(&self, name: &str) -> Result<ParamItemType, Error> {
        Ok(self
            .toc
            .get(name)
            .ok_or_else(|| not_found(name))?
            .1
            .item_type)
    }
}

impl From<i64> for ParamValue {
    fn from(value: i64) -> Self {
        ParamValue::I64(value)
    }
}
impl From<i32> for ParamValue {
    fn from(value: i32) -> Self {
        ParamValue::I32(value)
    }
}
impl From<i16> for ParamValue {
    fn from(value: i16) -> Self {
        ParamValue::I16(value)
    }
}
impl From<i8> for ParamValue {
    fn from(value: i8) -> Self {
        ParamValue::I8(value)
    }
}

impl From<u64> for ParamValue {
    fn from(value: u64) -> Self {
        ParamValue::U64(value)
    }
}
impl From<u32> for ParamValue {
    fn from(value: u32) -> Self {
        ParamValue::U32(value)
    }
}
impl From<u16> for ParamValue {
    fn from(value: u16) -> Self {
        ParamValue::U16(value)
    }
}
impl From<u8> for ParamValue {
    fn from(value: u8) -> Self {
        ParamValue::U8(value)
    }
}

impl From<f64> for ParamValue {
    fn from(value: f64) -> Self {
        ParamValue::F64(value)
    }
}
impl From<f32> for ParamValue {
    fn from(value: f32) -> Self {
        ParamValue::F32(value)
    }
}
impl From<f16> for ParamValue {
    fn from(value: f16) -> Self {
        ParamValue::F16(value.into())
    }
}

impl From<ParamValue> for Vec<u8> {
    fn from(value: ParamValue) -> Self {
        match value {
            ParamValue::U8(v) => v.to_le_bytes().into(),
            ParamValue::U16(v) => v.to_le_bytes().into(),
            ParamValue::U32(v) => v.to_le_bytes().into(),
            ParamValue::U64(v) => v.to_le_bytes().into(),
            ParamValue::I8(v) => v.to_le_bytes().into(),
            ParamValue::I16(v) => v.to_le_bytes().into(),
            ParamValue::I32(v) => v.to_le_bytes().into(),
            ParamValue::I64(v) => v.to_le_bytes().into(),
            ParamValue::F16(v) => f16::from_f32(v).to_le_bytes().into(),
            ParamValue::F32(v) => v.to_le_bytes().into(),
            ParamValue::F64(v) => v.to_le_bytes().into(),
        }
    }
}

impl From<ParamValue> for ParamItemType {
    fn from(value: ParamValue) -> Self {
        match value {
            ParamValue::U8(_) => ParamItemType::U8,
            ParamValue::U16(_) => ParamItemType::U16,
            ParamValue::U32(_) => ParamItemType::U32,
            ParamValue::U64(_) => ParamItemType::U64,
            ParamValue::I8(_) => ParamItemType::I8,
            ParamValue::I16(_) => ParamItemType::I16,
            ParamValue::I32(_) => ParamItemType::I32,
            ParamValue::I64(_) => ParamItemType::I64,
            ParamValue::F16(_) => ParamItemType::F16,
            ParamValue::F32(_) => ParamItemType::F32,
            ParamValue::F64(_) => ParamItemType::F64,
        }
    }
}
