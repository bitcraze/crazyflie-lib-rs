//! # Crazyflie subsystems
//!
//! The Crazyflie firmware, as well as the CRTP protocol used to communicate with it, is organized in logical
//! subsystems. Subsystems are greatly independent and each have one logical role. The main design of the CRTP
//! communication protocol is to connect subsystem implementation in the Crazyflie one-to-one to implementation in the
//! lib on the ground.
//!
//! Modules here implement Rust API for the different Crazyflie subsystems, they are the main way to communicate and
//! interact with the Crazyflie.

pub mod commander;
pub mod high_level_commander;
pub mod console;
pub mod localization;
pub mod log;
pub mod param;
pub mod platform;
