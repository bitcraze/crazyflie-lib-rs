use std::array::TryFromSliceError;

/// [Result] alias for return types of the crate API
pub type Result<T> = std::result::Result<T, Error>;

/// Error enum type
#[derive(Debug)]
pub enum Error {
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("Foo {}", self))
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
