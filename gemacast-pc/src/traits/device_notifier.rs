use async_trait::async_trait;
use gemacast_core::domain::types::DeviceId;
use std::net::SocketAddr;

/// Notifies a connected device to disconnect, using the best available transport.
///
/// Devices are notified through the authenticated WSS control channel.
///
/// **Production**: [`crate::adapters::MultiTransportDeviceNotifier`].
/// **Tests**: [`crate::testing::mocks::MockDeviceNotifier`] that records calls.
#[async_trait]
pub trait DeviceNotifier: Send + Sync {
    /// Notify a device to disconnect using all available transports.
    ///
    /// `addr` identifies a loopback peer eligible for the ADB fallback when
    /// WSS notification fails. Pass `None` to skip the fallback.
    async fn notify_disconnect(&self, device_id: &DeviceId, addr: Option<SocketAddr>);

    /// Signal all ADB TCP tasks to tear down their connections.
    fn signal_adb_shutdown(&self);
}
