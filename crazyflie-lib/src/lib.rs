//! # Crazyflie library
//! 
//! This crate allows to connect, communicate with and control the Crazylfie using the [crazyflie-link] crate
//! to open a communication link. The link implementation only supports radio for now, but more will be implemented
//! in the future (at least USB).
//! 
//! ## Status 
//! 
//! The crate aims at implementing a Rust API to control the Crazyflie. The Crazyflie functionalities are implemented in 
//! subsystems. The current status is:
//! 
//! | Subsystem | Support |
//! |-----------|---------|
//! | Platform | None |
//! | Log | Full (V2) |
//! | Param | Full(V2) |
//! | Commander | Partial (only RPYT) |
//! | Memory | None |
//! | High-level Commander | None |
//! 
//! ## Compatibility
//! 
//! This crate is compatible with Crazyflie protocol vertion > 4, ie. >= 2018.08.
//! 
//! ## Usage
//! 
//! The basic procedure to use the lib is:
//!  - Find the link URI to connect, either by scanning or as a config or user input
//!  - Create a Crazyflie object from the URI or a connected Link, this will connect to the Crazyflie and initializes
//!    the subsystems
//!  - Subsystems are available as public fields of the [Crazyflie] struct.
//!  - Use the subsystems in the Crazyflie object to control the Crazyflie
//!  - Drop the Crazyflie object or call [crazyflie::Crazyflie::disconnect()]
//! 
//! All subsystems functions are only taking an un-mutable reference to self (`&self`), the intention is for the
//! Crazyflie object to be shared between tasks using `Arc<>` or `Rc<>`.
//! 
//! For example:
//! ``` no_run
//! # use std::sync::Arc;
//! # async fn test() -> Result<(), Box<dyn std::error::Error>> {
//! let link_context = crazyflie_link::LinkContext::new(Arc::new(async_executors::AsyncStd));
//! 
//! // Scann for Crazyflies on the default address
//! let found = link_context.scan([0xE7; 5]).await?;
//! 
//! if !found.is_empty() {
//!     let cf = crazyflie_lib::Crazyflie::connect_from_uri(async_executors::AsyncStd, &link_context, &found[0]).await?;
//! 
//!     println!("List of params variables: ");
//!     for name in cf.param.names() {
//!         println!(" - {}", name);
//!     }
//! 
//!     println!("List of log variables: ");
//!     for name in cf.param.names() {
//!         println!(" - {}", name);
//!     }
//! 
//!     cf.disconnect();
//! }
//! # Ok(())
//! # }
//! ```

mod error;
mod value;
mod crazyflie;
mod crtp_utils;

pub mod subsystems;

pub use crate::crazyflie::Crazyflie;
pub use crate::error::{Error, Result};
pub use crate::value::{Value, ValueType};

// Async executor selection
#[cfg(feature = "async-std")]
pub(crate) use async_std::task::spawn;

#[cfg(feature = "wasm-bindgen-futures")]
use wasm_bindgen_futures::spawn_local as spawn;

use trait_set::trait_set;
use async_executors::{LocalSpawnHandle, Timer};

trait_set! {
    pub trait Executor = LocalSpawnHandle<()> + Timer + 'static
}
