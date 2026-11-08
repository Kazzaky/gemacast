use crate::{audio::AudioSessionKey, stream::echo::EchoPacketCipher};
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};

/// Owns the phone's UDP keepalive lifecycle.
pub(crate) struct KeepaliveHeartbeat {
    target: std::net::IpAddr,
    streamer_audio_port: Arc<AtomicU16>,
    active: Arc<AtomicBool>,
    socket: UdpSocket,
    echo_cipher: EchoPacketCipher,
}

impl KeepaliveHeartbeat {
    pub(crate) fn new(
        target: std::net::IpAddr,
        streamer_audio_port: Arc<AtomicU16>,
        active: Arc<AtomicBool>,
        socket: UdpSocket,
        audio_key: AudioSessionKey,
    ) -> Self {
        Self {
            target,
            streamer_audio_port,
            active,
            socket,
            echo_cipher: EchoPacketCipher::new(&audio_key),
        }
    }

    /// Spawns the phone's 500 ms keepalive thread.
    ///
    /// The packet doubles as an authenticated, encrypted UDP echo ping. The PC
    /// verifies it before reflecting it, and the receive loop verifies the reply
    /// before reporting raw wire RTT (see [`crate::stream::echo`]).
    pub(crate) fn spawn(self) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            #[cfg(target_os = "android")]
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, -19);
                libc::prctl(29, 1);
            }

            while self.active.load(Ordering::Relaxed) {
                let port = self.streamer_audio_port.load(Ordering::Relaxed);
                let target_addr = std::net::SocketAddr::new(self.target, port);
                if let Ok(packet) = self.echo_cipher.build() {
                    let _ = self.socket.send_to(&packet, target_addr);
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })
    }
}
