use gemacast_core::domain::types::DeviceId;
use gemacast_core::network::Ports;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::UdpSocket;

pub struct SubnetProbe;

impl SubnetProbe {
    pub async fn run(
        socket: Arc<UdpSocket>,
        device_id: DeviceId,
        is_streaming: Arc<AtomicBool>,
    ) {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(5000));
        let payload = gemacast_core::control::messages::ControlMessage::Probe {
            device_id: Some(device_id),
        };

        let Ok(json_bytes) = serde_json::to_vec(&payload) else {
            return;
        };

        loop {
            interval.tick().await;

            // Skip subnet scan while streaming: the phone already knows the
            // streamer's IP, and the 254-packet burst floods the 2.4 GHz channel
            // causing 200ms+ jitter spikes on audio packets.
            if is_streaming.load(Ordering::Relaxed) {
                continue;
            }

            let subnets = Self::collect_local_subnets();
            for (b0, b1, b2) in subnets {
                for host in 1..=254u8 {
                    let target = std::net::SocketAddrV4::new(
                        std::net::Ipv4Addr::new(b0, b1, b2, host),
                        Ports::DISCOVERY,
                    );
                    let _ = socket.send_to(&json_bytes, target).await;
                }
            }
        }
    }

    /// Collects the /24 subnets for all non-loopback IPv4 interfaces.
    /// Falls back to common USB-tethering subnets if none are found.
    fn collect_local_subnets() -> Vec<(u8, u8, u8)> {
        let mut subnets = Vec::new();
        for iface in netdev::get_interfaces() {
            for ip_net in iface.ipv4 {
                let ip = ip_net.addr();
                if !ip.is_loopback() {
                    let o = ip.octets();
                    subnets.push((o[0], o[1], o[2]));
                }
            }
        }
        if subnets.is_empty() {
            subnets.push((192, 168, 42));
            subnets.push((192, 168, 43));
        }
        subnets
    }
}
