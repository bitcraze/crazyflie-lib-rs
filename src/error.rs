use std::array::TryFromSliceError;

use crazyflie_link::Packet;
use futures::task::SpawnError;

/// [Result] alias for return types of the crate API
pub type Result<T> = std::result::Result<T, Error>;

/// Error enum type
#[derive(Debug)]
pub enum Error {
    /// Protocol version not supported, you need to update either the lib or the Crazyflie.
    ///
    /// see [the crate documentation](crate#compatibility) for more information.
    ProtocolVersionNotSupported,
    /// Unexpected protocol error. The String contains the reason.
    ProtocolError(String),
    /// Parameter subsystem error. The String contains the reason.
    ParamError(String),
    /// Log Subsystem error. The String contains the reason.
    LogError(String),
    /// [Value](crate::Value) conversion error. The String contains the reason.
    ConversionError(String),
    /// Crazyflie link configuration error. Returns the [error from the Link](crazyflie_link::Error).
    LinkError(crazyflie_link::Error),
    /// The Crazyflie object is currently disconnected.
    Disconnected,
    /// Variable not found in TOC.
    VariableNotFound,
    /// Error with the async executors.
    SystemError(String),
    /// App channel packets should be no larger than [APPCHANNEL_MTU](crate::subsystems::platform::APPCHANNEL_MTU)
    AppchannelPacketTooLarge,
    /// Invalid argument passed to a function.
    /// This error indicates that one or more arguments provided to a function are invalid.
    InvalidArgument(String),
    /// Operation timed out waiting for response.
    Timeout,
    /// Memory content malformed or not as expected. The String contains the reason.
    MemoryError(String),
    /// Invalid parameter provided to a function. The String contains the reason.
    InvalidParameter(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::ProtocolVersionNotSupported => write!(f, "Protocol version not supported"),
            Error::ProtocolError(msg) => write!(f, "Protocol error: {}", msg),
            Error::ParamError(msg) => write!(f, "Parameter error: {}", msg),
            Error::LogError(msg) => write!(f, "Log error: {}", msg),
            Error::ConversionError(msg) => write!(f, "Conversion error: {}", msg),
            Error::LinkError(e) => write!(f, "Link error: {}", e),
            Error::Disconnected => write!(f, "Disconnected"),
            Error::VariableNotFound => write!(f, "Variable not found"),
            Error::SystemError(msg) => write!(f, "System error: {}", msg),
            Error::AppchannelPacketTooLarge => write!(f, "Appchannel packet too large"),
            Error::InvalidArgument(msg) => write!(f, "Invalid argument: {}", msg),
            Error::Timeout => write!(f, "Operation timed out"),
            Error::MemoryError(msg) => write!(f, "Memory error: {}", msg),
            Error::InvalidParameter(msg) => write!(f, "Invalid parameter: {}", msg),
        }
    }
}

impl std::error::Error for Error {}

impl From<TryFromSliceError> for Error {
    fn from(e: TryFromSliceError) -> Self {
        Self::ConversionError(format!("{:?}", e))
    }
}

impl From<crazyflie_link::Error> for Error {
    fn from(error: crazyflie_link::Error) -> Self {
        Self::LinkError(error)
    }
}

impl From<SpawnError> for Error {
    fn from(error: SpawnError) -> Self {
        Self::SystemError(format!("{}", error))
    }
}

impl From<flume::RecvError> for Error {
    fn from(_: flume::RecvError) -> Self {
        self::Error::Disconnected
    }
}

impl From<flume::SendError<Packet>> for Error {
    fn from(_: flume::SendError<Packet>) -> Self {
        self::Error::Disconnected
    }
}
