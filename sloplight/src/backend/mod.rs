//! Rust port of moonlight-qt's `app/backend`: GameStream/Sunshine/Apollo HTTP
//! protocol, pairing, host discovery and host state tracking.

pub mod apollo;
pub mod host;
pub mod http;
pub mod identity;
pub mod manager;
pub mod mdns;
pub mod pairing;
pub mod platform;
pub mod wol;
pub mod xml;

pub use host::{App, Host, HostState, PairState};
pub use identity::Identity;
pub use manager::{HostManager, ManagerEvent};
