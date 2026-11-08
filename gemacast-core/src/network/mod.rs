//! Network utilities, port definitions, and interface classification.

mod classification;
mod interfaces;
mod link;
pub mod ports;

pub use crate::discovery::{PresenceBroadcaster, PresenceListener};
pub use crate::domain::types::{ConnectionModes, get_available_connection_modes};
pub use classification::{InterfaceCapabilities, InterfaceClassifier};
pub use interfaces::NetworkInterfaces;
pub use link::NetworkLinkDetector;
pub use ports::Ports;
