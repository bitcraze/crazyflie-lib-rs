//! Error numbers sent by the Crazyflie firmware over CRTP.
//!
//! These values come from the firmware's ARM newlib errno definitions. They
//! are wire values, not host OS error numbers (for example, macOS EAGAIN is 35).
//! Keep unknown values intact when reporting firmware errors to callers.

/// No such entry.
pub(crate) const ENOENT: u8 = 2;
/// Firmware is not ready; retry the request.
pub(crate) const EAGAIN: u8 = 11;
