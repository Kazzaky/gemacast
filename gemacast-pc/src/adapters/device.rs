use async_trait::async_trait;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

use gemacast_core::control::ControlEventBus;
use gemacast_core::control::types::WsEvent;
use gemacast_core::domain::types::DeviceId;

use crate::traits::DeviceNotifier;

/// WebSocket connection map type alias for readability.
pub type WsConnectionMap = Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>;

/// Notifies devices to disconnect through the authenticated control channel.
///
/// Uses the authenticated WSS control channel.
///
pub struct MultiTransportDeviceNotifier {
    ws_connections: WsConnectionMap,
}

impl MultiTransportDeviceNotifier {
    pub fn new(ws_connections: WsConnectionMap) -> Self {
        Self { ws_connections }
    }
}

#[async_trait]
impl DeviceNotifier for MultiTransportDeviceNotifier {
    async fn notify_disconnect(&self, device_id: &DeviceId, addr: Option<SocketAddr>) {
        // Try WSS first.
        let ws_ok = ControlEventBus::send(&self.ws_connections, device_id, WsEvent::Disconnect)
            .await
            .is_ok();

        if !ws_ok {
            tracing::debug!(
                %device_id,
                ?addr,
                "WSS disconnect notification unavailable; remote liveness cleanup remains authoritative"
            );
        }
    }
}
