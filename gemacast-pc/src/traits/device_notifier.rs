use async_trait::async_trait;
use gemacast_core::domain::types::DeviceId;
use std::net::SocketAddr;

/// Notifies a connected device to disconnect, using the best available transport.
///
/// Devices are notified through the authenticated WSS control channel.
///
/// **Production**: [`crate::adapters::WsDeviceNotifier`].
/// **Tests**: [`crate::testing::mocks::MockDeviceNotifier`] that records calls.
#[async_trait]
pub trait DeviceNotifier: Send + Sync {
    /// Notify a device to disconnect through its authenticated control channel.
    async fn notify_disconnect(&self, device_id: &DeviceId, addr: Option<SocketAddr>);
}
