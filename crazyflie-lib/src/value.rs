use half::f16;
use std::convert::{TryFrom, TryInto};

/// # Typed data value
/// 
/// This enum supports all the data types that can be exchanged with the Crazyflie
/// using the [log](crate::subsystems::log) and [param](crate::subsystems::param) subsystems.
/// 
/// A function allows to convert a `[u8]` to a [Value] and the [Into<Vec<U8>>]
/// trait is implemented to convert a [Value] into a vector of bytes. 
/// 
/// The [TryFrom] trait is implemented for all matching rust primitive type. There
/// is only direct conversion implemented. For example the following is OK:
/// ```
/// # use std::convert::TryInto;
/// # use crazyflie_lib::Value;
/// let v:u32 = Value::U32(42).try_into().unwrap();
/// ```
/// 
/// However the following **will panic**:
/// ``` should_panic
/// # use std::convert::TryInto;
/// # use crazyflie_lib::Value;
/// let v:u32 = Value::U8(42).try_into().unwrap();
/// ```
#[derive(Debug, Clone, Copy)]
pub enum Value {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    F16(f16),
    F32(f32),
    F64(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F16,
    F32,
    F64,
}

/// # Value type
/// 
/// This enum contains all the possible type of a [Value]
impl ValueType {
    pub fn byte_length(&self) -> usize {
        match self {
            ValueType::U8 => 1,
            ValueType::U16 => 2,
            ValueType::U32 => 4,
            ValueType::U64 => 8,
            ValueType::I8 => 1,
            ValueType::I16 => 2,
            ValueType::I32 => 4,
            ValueType::I64 => 8,
            ValueType::F16 => 2,
            ValueType::F32 => 4,
            ValueType::F64 => 8,
        }
    }
}

impl From<Value> for ValueType {
    fn from(value: Value) -> Self {
        match value {
            Value::U8(_) => ValueType::U8,
            Value::U16(_) => ValueType::U16,
            Value::U32(_) => ValueType::U32,
            Value::U64(_) => ValueType::U64,
            Value::I8(_) => ValueType::I8,
            Value::I16(_) => ValueType::I16,
            Value::I32(_) => ValueType::I32,
            Value::I64(_) => ValueType::I64,
            Value::F16(_) => ValueType::F16,
            Value::F32(_) => ValueType::F32,
            Value::F64(_) => ValueType::F64,
        }
    }
}

// Conversion from and to matching primitive types

macro_rules! primitive_impl {
    ($ty:ident, $name:ident) => {
        impl From<$ty> for Value {
            fn from(v: $ty) -> Self {
                Value::$name(v)
            }
        }

        impl TryFrom<Value> for $ty {
            type Error = crate::Error;

            fn try_from(value: Value) -> Result<$ty, Self::Error> {
                match value {
                    Value::$name(v) => Ok(v),
                    _ => Err(Self::Error::ConversionError(format!(
                        "Cannot convert {:?} to {}",
                        value,
                        stringify!($ty)
                    ))),
                }
            }
        }
    };
}

primitive_impl!(u8, U8);
primitive_impl!(u16, U16);
primitive_impl!(u32, U32);
primitive_impl!(u64, U64);
primitive_impl!(i8, I8);
primitive_impl!(i16, I16);
primitive_impl!(i32, I32);
primitive_impl!(i64, I64);
primitive_impl!(f16, F16);
primitive_impl!(f32, F32);
primitive_impl!(f64, F64);

// Conversion from and to u8 slice (little endian serde)
impl Value {
    /// Convert a `&[u8]` slice to a [Value]. The length of the slice must match
    /// the length of the value type, otherwise an error will be returned.
    pub fn from_le_bytes(bytes: &[u8], value_type: ValueType) -> Result<Value, crate::Error> {
        match value_type {
            ValueType::U8 => Ok(Value::U8(u8::from_le_bytes(bytes.try_into()?))),
            ValueType::U16 => Ok(Value::U16(u16::from_le_bytes(bytes.try_into()?))),
            ValueType::U32 => Ok(Value::U32(u32::from_le_bytes(bytes.try_into()?))),
            ValueType::U64 => Ok(Value::U64(u64::from_le_bytes(bytes.try_into()?))),
            ValueType::I8 => Ok(Value::I8(i8::from_le_bytes(bytes.try_into()?))),
            ValueType::I16 => Ok(Value::I16(i16::from_le_bytes(bytes.try_into()?))),
            ValueType::I32 => Ok(Value::I32(i32::from_le_bytes(bytes.try_into()?))),
            ValueType::I64 => Ok(Value::I64(i64::from_le_bytes(bytes.try_into()?))),
            ValueType::F16 => Ok(Value::F16(f16::from_le_bytes(bytes.try_into()?))),
            ValueType::F32 => Ok(Value::F32(f32::from_le_bytes(bytes.try_into()?))),
            ValueType::F64 => Ok(Value::F64(f64::from_le_bytes(bytes.try_into()?))),
        }
    }

    /// Convert a [Value] to a [f64].
    /// 
    /// This conversion is lossless in most case but can be lossy if the value
    /// is a u64: a f64 cannot accurately store large values of a u64.
    pub fn to_f64_lossy(&self) -> f64 {
        match *self {
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
        }
    }

    /// Make a [Value] from a [f64] and a [ValueType]
    /// 
    /// This function allows to construct any type of value from a f64.
    /// 
    /// The conversion has possibility to be lossy in a couple of cases:
    ///  - When making an integer, the value is truncated to the number of bit of the parameter
    ///    - Example: Setting `257` to a `u8` variable will set it to the value `1`
    ///  - Similarly floating point precision will be truncated to the parameter precision. Rounding is undefined.
    ///  - Making a floating point outside the range of the parameter is undefined.
    ///  - It is not possible to represent accurately a `u64` parameter in a `f64`.
    pub fn from_f64_lossy(value_type: ValueType, value: f64) -> Value {
        match value_type {
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
        }
    }
}

impl From<Value> for Vec<u8> {
    fn from(value: Value) -> Self {
        match value {
            Value::U8(v) => v.to_le_bytes().into(),
            Value::U16(v) => v.to_le_bytes().into(),
            Value::U32(v) => v.to_le_bytes().into(),
            Value::U64(v) => v.to_le_bytes().into(),
            Value::I8(v) => v.to_le_bytes().into(),
            Value::I16(v) => v.to_le_bytes().into(),
            Value::I32(v) => v.to_le_bytes().into(),
            Value::I64(v) => v.to_le_bytes().into(),
            Value::F16(v) => v.to_le_bytes().into(),
            Value::F32(v) => v.to_le_bytes().into(),
            Value::F64(v) => v.to_le_bytes().into(),
        }
    }
}
