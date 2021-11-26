//! # Crazyflie library
//!
//! This crate allows to connect, communicate with and control the Crazyflie using the [crazyflie-link] crate
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
//! | App channel | None |
//! | Commander | Partial (only RPYT) |
//! | Console | Full |
//! | High-level Commander | None |
//! | Localization | None |
//! | Log | Full (V2) |
//! | Memory | None |
//! | Param | Full(V2) |
//! | Platform | None |
//!
//! ## Compatibility
//!
//! This crate is compatible with Crazyflie protocol version > 4. This means Crazyflie firmware release >= 2018.08.
//!
//! The Crazyflie guarantees backward functionalities for one protocol version
//! so this lib will be compatible with version 4 (~2018-08) and 5 (future) of
//! the protocol.
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
//! # async fn test() -> Result<(), Box<dyn std::error::Error>> {
//! let link_context = crazyflie_link::LinkContext::new(async_executors::AsyncStd);
//!
//! // Scan for Crazyflies on the default address
//! let found = link_context.scan([0xE7; 5]).await?;
//!
//! if let Some(uri) = found.first() {
//!     let cf = crazyflie_lib::Crazyflie::connect_from_uri(async_executors::AsyncStd, &link_context, uri).await?;
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
//!     cf.disconnect().await;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! [crazyflie-link]: https://crates.io/crates/crazyflie-link

#![warn(missing_docs)]

mod crazyflie;
mod crtp_utils;
mod error;
mod value;

pub mod subsystems;

pub use crate::crazyflie::Crazyflie;
pub use crate::error::{Error, Result};
pub use crate::value::{Value, ValueType};

// Async executor selection
#[cfg(feature = "async-std")]
pub(crate) use async_std::task::spawn;

#[cfg(feature = "wasm-bindgen-futures")]
use wasm_bindgen_futures::spawn_local as spawn;

use async_executors::{LocalSpawnHandle, Timer};

/// Async executor trait
///
/// This trait is implemented in the `async_executors` crate for common async
/// executors. See example in the [crate root documentation](crate).
pub trait Executor: LocalSpawnHandle<()> + Timer + 'static {}

// Until trait alias makes it in stable rust, we need an empty implementation
// for this trait ...
impl<U> Executor for U where U: LocalSpawnHandle<()> + Timer + 'static {}

/// Supported protocol version
///
/// see [the crate documentation](crate#compatibility) for more information.
pub const SUPPORTED_PROTOCOL_VERSION: u8 = 4;
