//! Adapter: Network transport implementations for audio packet I/O.
//!
//! Production implementations of [`AudioPacketTransport`](crate::ports::transport::AudioPacketTransport):
//! - [`UdpTransport`] — UDP socket for WiFi streaming

use std::net::{SocketAddr, UdpSocket};

use crate::ports::transport::AudioPacketTransport;

pub struct UdpTransport {
    pub socket: UdpSocket,
}

impl AudioPacketTransport for UdpTransport {
    fn receive_audio_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udp_transport_uses_socket_source_address() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = UdpSocket::bind("127.0.0.1:0").unwrap();
        target.send_to(b"test", socket.local_addr().unwrap()).unwrap();
        let mut transport = UdpTransport { socket };
        let mut buffer = [0u8; 16];
        let (len, addr) = transport.receive_audio_packet(&mut buffer).unwrap();
        assert_eq!(&buffer[..len], b"test");
        assert_eq!(addr, target.local_addr().unwrap());
    }
}
