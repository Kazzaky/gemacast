//! Player-side transport orchestration.
//!
//! Creates and configures the UDP audio transport used by the player.

use crate::adapters::transport::UdpTransport;
use crate::domain::error::NetworkError;
use crate::network::Ports;
use std::net::{Ipv4Addr, SocketAddrV4};

/// Creates the player-side UDP audio transport.
pub(crate) struct AudioTransportFactory;

impl AudioTransportFactory {
    pub(crate) fn create(
        target_ip: Option<std::net::IpAddr>,
    ) -> Result<(UdpTransport, Option<std::net::UdpSocket>), NetworkError> {
        let addr =
            std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, Ports::AUDIO_UDP));
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|source| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source,
        })?;

        socket
            .set_reuse_address(true)
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "reuse address",
                source,
            })?;
        #[cfg(not(windows))]
        socket
            .set_reuse_port(true)
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "reuse port",
                source,
            })?;

        socket
            .bind(&addr.into())
            .map_err(|source| NetworkError::SocketBindFailed {
                addr: addr.to_string(),
                source,
            })?;

        let _ = socket.set_recv_buffer_size(512 * 1024);

        let std_socket: std::net::UdpSocket = socket.into();

        let cloned_for_tos = std_socket
            .try_clone()
            .map_err(NetworkError::SocketCloneFailed)?;
        socket2::Socket::from(cloned_for_tos)
            .set_tos_v4(0xB8)
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "type of service",
                source,
            })?;

        std_socket
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "read timeout",
                source,
            })?;

        if let Some(target) = target_ip {
            let target_addr = std::net::SocketAddr::new(target, Ports::AUDIO_UDP);
            std_socket
                .send_to(&[0u8], target_addr)
                .map_err(NetworkError::SendFailed)?;
        }

        let heartbeat_socket = std_socket
            .try_clone()
            .map_err(NetworkError::SocketCloneFailed)?;

        Ok((UdpTransport { socket: std_socket }, Some(heartbeat_socket)))
    }
}
