use std::array::TryFromSliceError;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    ProtocolError(String),
    ParamError(String),
    LogError(String),
    ConversionError(String),
    LinkError(crazyflie_link::Error),
    Disconnected,
    VariableNotFound,
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
