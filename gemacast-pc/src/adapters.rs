//! Production implementations of the trait abstractions.
//!
//! These adapters wrap the concrete I/O types (`EventLoopProxy`, `mpsc::Sender`,
//! `broadcast::Sender`, `HttpControlClient`) behind the traits defined in
//! [`crate::traits`]. The background engine creates these once at startup and
//! passes `Arc<dyn Trait>` to each task.

pub mod audio;
pub mod device;
pub mod tray;

pub use audio::ChannelAudioController;
pub use device::WsDeviceNotifier;
pub use tray::EventLoopTrayNotifier;
