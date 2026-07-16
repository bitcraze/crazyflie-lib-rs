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
//! | App channel | Full |
//! | Commander | Full |
//! | Console | Full |
//! | High-level Commander | Full |
//! | Link Service | Full |
//! | Localization | Full |
//! | Log | Full (V2) |
//! | Memory | Partial |
//! | Param | Full(V2) |
//! | Platform | Full |
//! | Supervisor | Full (info and command channels) |
//!
//! ## Compatibility
//!
//! This crate is compatible with Crazyflie protocol versions [`MIN_SUPPORTED_PROTOCOL_VERSION`]
//! to [`MAX_SUPPORTED_PROTOCOL_VERSION`]. The Crazyflie guarantees backward compatibility for one
//! protocol version, so this library will work with both the current and next protocol version.
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
//! let link_context = crazyflie_lib::crazyflie_link::LinkContext::new();
//!
//! // Scan for Crazyflies on the default address
//! let found = link_context.scan([0xE7; 5]).await?;
//!
//! if let Some(uri) = found.first() {
//!     let cf = crazyflie_lib::Crazyflie::connect_from_uri(
//!         &link_context,
//!         uri,
//!         crazyflie_lib::NoTocCache
//!     ).await?;
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
//! ## Relation to the crazyflie-link and crazyradio crates
//!
//! Types from the [crazyflie-link] crate appear in this crate's public API (for example
//! [`crazyflie_link::LinkContext`] and [`crazyflie_link::Connection`] in
//! [`Crazyflie::connect_from_uri()`] and [`Crazyflie::connect_from_link()`]), which makes
//! crazyflie-link a *public dependency*: code using this crate needs to name its types. To
//! avoid a separate, possibly version-mismatched crazyflie-link dependency downstream, the
//! crate is re-exported at [`crazyflie_link`] — use it through this path instead of adding
//! a direct dependency. The crazyradio crate is reachable the same way, as
//! `crazyflie_lib::crazyflie_link::crazyradio`.
//!
//! This is a supported part of the API: the re-exported crates only move to
//! semver-incompatible versions in a semver-incompatible release of this crate.
//!
//! ## Cargo features
//!
//! - **packet_capture** - Enable CRTP packet capture in the link (Unix only). Forwards to
//!   the crazyflie-link feature of the same name, see `crazyflie_link::capture`.
//!
//! [crazyflie-link]: https://crates.io/crates/crazyflie-link

#![warn(missing_docs)]

mod crazyflie;
mod crtp_utils;
mod error;
mod value;

pub mod subsystems;

/// Re-export of the exact [`crazyflie_link`] crate version this crate was built against.
///
/// crazyflie-link is a public dependency of this crate (its types appear in our API,
/// e.g. [`crazyflie_link::LinkContext`]). Use this re-export instead of a direct
/// crazyflie-link dependency to guarantee a single, version-matched copy in your build;
/// the crazyradio crate is likewise available as
/// `crazyflie_lib::crazyflie_link::crazyradio`.
///
/// Supported API: the re-exported crates only change incompatibly in a
/// semver-incompatible release of this crate.
pub use crazyflie_link;

pub use crate::crazyflie::Crazyflie;
pub use crate::error::{Error, Result};
pub use crate::value::{Value, ValueType};
pub use crate::crtp_utils::TocCache;
pub use crate::crtp_utils::NoTocCache;

/// Minimum supported protocol version
///
/// see [the crate documentation](crate#compatibility) for more information.
pub const MIN_SUPPORTED_PROTOCOL_VERSION: u8 = 12;

/// Maximum supported protocol version
///
/// see [the crate documentation](crate#compatibility) for more information.
pub const MAX_SUPPORTED_PROTOCOL_VERSION: u8 = MIN_SUPPORTED_PROTOCOL_VERSION + 1;
