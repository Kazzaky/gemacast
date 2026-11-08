use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gemacast_core::domain::types::{ConnectionMode, DeviceId, DiscoveredDevice, TransportType};

use crate::STREAMER_HEARTBEAT_TIMEOUT_SECS;
use crate::traits::FrontendNotifier;

pub struct DispatchContext {
    pub streamer_last_seen: Arc<Mutex<HashMap<DeviceId, Instant>>>,
    pub active_usb_streamers: Arc<Mutex<HashMap<DeviceId, Instant>>>,
    pub notifier: Arc<dyn FrontendNotifier>,
}

impl Clone for DispatchContext {
    fn clone(&self) -> Self {
        Self {
            streamer_last_seen: self.streamer_last_seen.clone(),
            active_usb_streamers: self.active_usb_streamers.clone(),
            notifier: self.notifier.clone(),
        }
    }
}

impl DispatchContext {
    pub fn new(notifier: Arc<dyn FrontendNotifier>) -> Self {
        Self {
            streamer_last_seen: Arc::new(Mutex::new(HashMap::new())),
            active_usb_streamers: Arc::new(Mutex::new(HashMap::new())),
            notifier,
        }
    }

    pub fn dispatch(
        &self,
        message: gemacast_core::control::messages::ControlMessage,
        addr: std::net::SocketAddr,
        mode: ConnectionMode,
    ) {
        match message {
            gemacast_core::control::messages::ControlMessage::Presence {
                device_id,
                streamer_name,
                is_offline,
                transport,
            } => {
                self.handle_presence(device_id, streamer_name, is_offline, transport, addr, mode);
            }
            gemacast_core::control::messages::ControlMessage::Disconnect { .. } => {
                self.notifier.emit_force_disconnect();
            }
            _ => {}
        }
    }

    fn handle_presence(
        &self,
        device_id: DeviceId,
        streamer_name: String,
        is_offline: bool,
        transport: Option<TransportType>,
        addr: std::net::SocketAddr,
        mode: ConnectionMode,
    ) {
        if is_offline {
            self.streamer_last_seen.lock().unwrap().remove(&device_id);
            self.active_usb_streamers.lock().unwrap().remove(&device_id);
        } else {
            self.streamer_last_seen
                .lock()
                .unwrap()
                .insert(device_id.clone(), Instant::now());
        }

        let mut audio_addr = addr;
        audio_addr.set_port(gemacast_core::network::Ports::AUDIO_UDP);

        let mut is_usb = transport == Some(TransportType::Usb);
        if !is_usb {
            is_usb = gemacast_core::network::NetworkInterfaces::is_usb_tether_ip(&addr.ip());
        }

        match mode {
            ConnectionMode::Wifi => {
                if is_usb {
                    return;
                }
            }
            ConnectionMode::Usb => {
                if !is_usb {
                    return;
                }
            }
        }

        if !is_offline {
            if is_usb {
                self.active_usb_streamers
                    .lock()
                    .unwrap()
                    .insert(device_id.clone(), Instant::now());
            } else {
                let has_recent_usb = self
                    .active_usb_streamers
                    .lock()
                    .unwrap()
                    .get(&device_id)
                    .is_some_and(|ts| {
                        Instant::now().duration_since(*ts).as_secs()
                            < STREAMER_HEARTBEAT_TIMEOUT_SECS
                    });
                if has_recent_usb {
                    return;
                }
            }
        }

        let device = DiscoveredDevice::from_presence(
            device_id,
            streamer_name,
            is_offline,
            audio_addr,
            transport,
        );
        self.notifier.emit_streamer_discovered(device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;

    fn make_ctx() -> (DispatchContext, Arc<MockFrontendNotifier>) {
        let notifier = Arc::new(MockFrontendNotifier::new());
        let ctx = DispatchContext::new(notifier.clone());
        (ctx, notifier)
    }

    #[test]
    fn wifi_mode_should_emit_wifi_streamer() {
        let (ctx, notifier) = make_ctx();
        let msg = gemacast_core::control::messages::ControlMessage::Presence {
            device_id: DeviceId("pc1".into()),
            streamer_name: "PC".into(),
            is_offline: false,
            transport: None,
        };
        // Non-USB IP address (10.99.99.x avoids matching real host interfaces)
        let addr: std::net::SocketAddr = "10.99.99.5:23555".parse().unwrap();
        ctx.dispatch(msg, addr, ConnectionMode::Wifi);

        let events = notifier.take_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], FrontendEvent::StreamerDiscovered(_)));
    }

    #[test]
    fn wifi_mode_should_ignore_usb_streamer() {
        let (ctx, notifier) = make_ctx();
        let msg = gemacast_core::control::messages::ControlMessage::Presence {
            device_id: DeviceId("pc1".into()),
            streamer_name: "PC".into(),
            is_offline: false,
            transport: Some(TransportType::Usb),
        };
        let addr: std::net::SocketAddr = "192.168.42.1:23555".parse().unwrap();
        ctx.dispatch(msg, addr, ConnectionMode::Wifi);

        assert!(notifier.take_events().is_empty());
    }

    #[test]
    fn disconnect_message_should_emit_force_disconnect() {
        let (ctx, notifier) = make_ctx();
        let msg = gemacast_core::control::messages::ControlMessage::Disconnect {
            device_id: DeviceId("phone".into()),
        };
        let addr: std::net::SocketAddr = "10.99.99.5:23555".parse().unwrap();
        ctx.dispatch(msg, addr, ConnectionMode::Wifi);

        let events = notifier.take_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], FrontendEvent::ForceDisconnect));
    }

    #[test]
    fn offline_presence_should_clean_heartbeat_tracker() {
        let (ctx, notifier) = make_ctx();
        // First, register the streamer
        ctx.streamer_last_seen
            .lock()
            .unwrap()
            .insert(DeviceId("pc1".into()), Instant::now());

        let msg = gemacast_core::control::messages::ControlMessage::Presence {
            device_id: DeviceId("pc1".into()),
            streamer_name: "PC".into(),
            is_offline: true,
            transport: None,
        };
        let addr: std::net::SocketAddr = "10.99.99.5:23555".parse().unwrap();
        ctx.dispatch(msg, addr, ConnectionMode::Wifi);

        // Streamer was removed from tracker
        assert!(
            !ctx.streamer_last_seen
                .lock()
                .unwrap()
                .contains_key(&DeviceId("pc1".into()))
        );

        // Offline event was still emitted
        let events = notifier.take_events();
        assert_eq!(events.len(), 1);
    }

}
