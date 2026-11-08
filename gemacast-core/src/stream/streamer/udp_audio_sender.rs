use super::packet_pacer::PacketPacer;
use super::{captured_frame::CapturedFrame, encode::AudioFrameEncoder};
use crate::{
    adapters::audio_priority::AudioPriority,
    audio::{AudioSessionKey, encryption::{AudioPacketCipher, AUDIO_TAG_LEN}, FORMAT_UNCOMPRESSED, MAX_OPUS_PACKET_SIZE, pcm_datagram::PcmDatagram},
    domain::{
        error::{GemaCastError, NetworkError},
        types::AudioBitrate,
    },
    stream::echo::{EchoPacketCipher, EchoReplayWindow},
};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::{broadcast, oneshot},
    time::Instant,
};

static LAST_GENERATION: AtomicU64 = AtomicU64::new(0);

// Every UDP sender reserves a disjoint sequence-number interval. A sender may
// be recreated during bitrate/source changes while the TLS session key stays
// the same; using wall-clock milliseconds as the nonce sequence base could
// therefore repeat a ChaCha20-Poly1305 nonce after a clock adjustment or a
// rapid restart. At 100 frames/s this interval lasts over 348 years.
const SEQUENCE_RESERVATION_SIZE: u64 = 1 << 40;
static NEXT_SEQUENCE_BASE: AtomicU64 = AtomicU64::new(0);

fn reserve_sequence_base() -> Result<u64, NetworkError> {
    NEXT_SEQUENCE_BASE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(SEQUENCE_RESERVATION_SIZE)
        })
        .map_err(|_| {
            NetworkError::SendFailed(std::io::Error::other(
                "UDP audio sequence space exhausted; cannot reserve a nonce-safe range",
            ))
        })
}

#[derive(Default)]
struct SendCounters {
    packets: u64,
    bytes: u64,
    pcm_frames: u64,
    incomplete_pcm_frames: u64,
    expired_frames: u64,
    lagged_frames: u64,
    would_block: u64,
    send_errors: u64,
    max_age_us: u64,
}

/// One phone owns its pacing clock, encoder, and socket priority.
pub(crate) struct UdpAudioSender {
    socket: UdpSocket,
    _priority: Option<AudioPriority>,
    target: SocketAddr,
    generation: u64,
    sequence_base: u64,
    counters: SendCounters,
}

impl UdpAudioSender {
    fn new(target: SocketAddr) -> Result<Self, NetworkError> {
        let socket =
            Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|source| {
                NetworkError::SocketBindFailed {
                    addr: "0.0.0.0:0".into(),
                    source,
                }
            })?;
        socket
            .bind(&"0.0.0.0:0".parse::<SocketAddr>().unwrap().into())
            .map_err(|source| NetworkError::SocketBindFailed {
                addr: "0.0.0.0:0".into(),
                source,
            })?;
        socket
            .set_nonblocking(true)
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "nonblocking audio sender",
                source,
            })?;
        let priority = match AudioPriority::attach(&socket, target) {
            Ok(priority) => {
                tracing::info!(%target, "[AudioSend] OS audio priority requested");
                Some(priority)
            }
            Err(error) => {
                tracing::warn!(%target, %error, "[AudioSend] OS audio priority unavailable");
                None
            }
        };
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let generation = LAST_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |previous| {
                Some((previous + 1).max(epoch.as_micros() as u64))
            })
            .unwrap();
        let generation = (generation + 1).max(epoch.as_micros() as u64);
        let sequence_base = reserve_sequence_base()?;
        let socket = UdpSocket::from_std(socket.into()).map_err(NetworkError::RecvFailed)?;
        Ok(Self {
            socket,
            _priority: priority,
            target,
            generation,
            sequence_base,
            counters: SendCounters::default(),
        })
    }

    fn send(&mut self, bytes: &[u8]) -> bool {
        match self.socket.try_send_to(bytes, self.target) {
            Ok(len) if len == bytes.len() => {
                self.counters.packets += 1;
                self.counters.bytes += len as u64;
                true
            }
            Ok(len) => {
                if self.counters.send_errors == 0 {
                    tracing::warn!(target = %self.target, expected = bytes.len(), sent = len,
                        "[AudioSend] Incomplete datagram send");
                }
                self.counters.send_errors += 1;
                false
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                self.counters.would_block += 1;
                false
            }
            Err(error) => {
                if self.counters.send_errors == 0 {
                    tracing::warn!(target = %self.target, %error, "[AudioSend] Datagram send failed");
                }
                self.counters.send_errors += 1;
                false
            }
        }
    }

    fn report(&mut self) {
        let counters = std::mem::take(&mut self.counters);
        tracing::info!(target = %self.target, packets = counters.packets, bytes = counters.bytes,
            pcm_frames = counters.pcm_frames, incomplete_pcm_frames = counters.incomplete_pcm_frames,
            expired_frames = counters.expired_frames, lagged_frames = counters.lagged_frames,
            would_block = counters.would_block, send_errors = counters.send_errors,
            max_frame_age_us = counters.max_age_us, "[AudioSend] Window");
    }

    pub async fn run(
        frames: broadcast::Receiver<Arc<CapturedFrame>>,
        target: SocketAddr,
        encoder: Option<opus::Encoder>,
        bitrate: AudioBitrate,
        shutdown: oneshot::Receiver<()>,
        audio_key: AudioSessionKey,
    ) -> Result<(), GemaCastError> {
        Self::new(target)?
            .run_session(frames, encoder, bitrate, shutdown, audio_key)
            .await
    }

    async fn run_session(
        mut self,
        mut frames: broadcast::Receiver<Arc<CapturedFrame>>,
        mut encoder: Option<opus::Encoder>,
        bitrate: AudioBitrate,
        mut shutdown: oneshot::Receiver<()>,
        audio_key: AudioSessionKey,
    ) -> Result<(), GemaCastError> {
        let cipher = AudioPacketCipher::new(&audio_key);
        let echo_cipher = EchoPacketCipher::new(&audio_key);
        let mut echo_replay = EchoReplayWindow::default();
        let sender = &mut self;
        let target = sender.target;
        tracing::info!(%target, ?bitrate, pcm_datagram_bytes = PcmDatagram::SIZE, "[AudioSend] Session started");
        let mut opus_output = vec![0; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::with_capacity(9 + MAX_OPUS_PACKET_SIZE);
        let mut chunk = [0; PcmDatagram::SIZE];
        let mut encrypted_chunk = Vec::with_capacity(PcmDatagram::SIZE + AUDIO_TAG_LEN);
        let mut inbound = [0; 64];
        let mut pending: Option<Arc<CapturedFrame>> = None;
        let mut chunked = false;
        let mut pacer = PacketPacer::new(Instant::now());
        let mut report = tokio::time::interval_at(
            Instant::now() + Duration::from_secs(1),
            Duration::from_secs(1),
        );
        report.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = report.tick() => sender.report(),
                result = sender.socket.recv_from(&mut inbound) => {
                    match result {
                        Ok((len, source)) => {
                            if source == target
                                && EchoPacketCipher::matches(&inbound, len)
                                && let Ok((sequence, _)) = echo_cipher.open(&inbound[..len])
                                && echo_replay.accept(sequence)
                            {
                                // Reflect only authenticated, fresh pings from the paired phone.
                                let _ = sender.socket.try_send_to(&inbound[..len], source);
                            }
                        }
                        Err(error) => return Err(NetworkError::RecvFailed(error).into()),
                    }
                }
                result = frames.recv(), if pending.is_none() => {
                    let frame = match result {
                        Ok(frame) => frame,
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            sender.counters.lagged_frames += count;
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    if frame.expired(Instant::now()) {
                        sender.counters.expired_frames += 1;
                        continue;
                    }
                    // Capture positions survive queue drops; the receiver observes the discontinuity.
                    // Refuse to cross this sender's reserved interval: reusing a
                    // sequence value would also reuse its ChaCha20-Poly1305 nonce.
                    if frame.position >= SEQUENCE_RESERVATION_SIZE {
                        return Err(NetworkError::SendFailed(std::io::Error::other(
                            "UDP audio sender exhausted its reserved nonce sequence range",
                        )).into());
                    }
                    let sequence = sender.sequence_base.checked_add(frame.position)
                        .ok_or_else(|| NetworkError::SendFailed(std::io::Error::other(
                            "UDP audio sequence number overflow",
                        )))?;
                    AudioFrameEncoder::encode(&frame.samples, encoder.as_mut(), bitrate,
                        sequence, &mut opus_output, &mut packet)?;
                    chunked = packet[8] == FORMAT_UNCOMPRESSED;
                    pending = Some(frame);
                }
                _ = tokio::time::sleep_until(pacer.deadline), if pending.is_some() => {
                    let now = Instant::now();
                    let frame = pending.as_ref().unwrap();
                    sender.counters.max_age_us = sender.counters.max_age_us.max(now.saturating_duration_since(frame.ready_at).as_micros() as u64);
                    if frame.expired(now) {
                        sender.counters.expired_frames += 1;
                        pending = None;
                        continue;
                    }
                    if chunked {
                        let mut complete = true;
                        for chunk_index in 0..PcmDatagram::CHUNKS {
                            PcmDatagram::encode(&packet, sender.generation, chunk_index, &mut chunk);
                            encrypted_chunk.clear();
                            encrypted_chunk.extend_from_slice(&chunk);
                            cipher.seal(&mut encrypted_chunk).map_err(|reason| {
                                NetworkError::SendFailed(std::io::Error::other(reason))
                            })?;
                            if !sender.send(&encrypted_chunk) {
                                complete = false;
                                break;
                            }
                        }
                        sender.counters.pcm_frames += 1;
                        if !complete {
                            sender.counters.incomplete_pcm_frames += 1;
                        }
                    } else {
                        cipher.seal(&mut packet).map_err(|reason| {
                            NetworkError::SendFailed(std::io::Error::other(reason))
                        })?;
                        let _ = sender.send(&packet);
                    }
                    pending = None;
                    pacer.sent(now, CapturedFrame::DURATION);
                }
            }
        }
        sender.report();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_audio_key() -> AudioSessionKey {
        AudioSessionKey::from_session_token(&"22".repeat(32)).unwrap()
    }

    #[test]
    fn sender_instances_reserve_disjoint_nonce_sequence_ranges() {
        let target = "127.0.0.1:0".parse().unwrap();
        let first = UdpAudioSender::new(target).unwrap();
        let second = UdpAudioSender::new(target).unwrap();

        let first_end = first.sequence_base + SEQUENCE_RESERVATION_SIZE - 1;
        let second_end = second.sequence_base + SEQUENCE_RESERVATION_SIZE - 1;
        assert!(
            first_end < second.sequence_base || second_end < first.sequence_base,
            "sender sequence intervals must never overlap"
        );
    }

    #[test]
    fn sequence_reservations_are_large_enough_for_a_sender_lifetime() {
        // At the 10 ms capture cadence, one interval is over 348 years.
        assert_eq!(SEQUENCE_RESERVATION_SIZE, 1u64 << 40);
        assert!(SEQUENCE_RESERVATION_SIZE > 100 * 60 * 60 * 24 * 365);
    }

    #[tokio::test]
    async fn pcm_should_cross_udp_as_four_lossless_mtu_safe_chunks_without_negotiation() {
        let phone = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpAudioSender::new(phone.local_addr().unwrap()).unwrap();
        let mut server = sender.socket.local_addr().unwrap();
        server.set_ip(std::net::Ipv4Addr::LOCALHOST.into());
        let base = sender.sequence_base;
        let (tx, rx) = broadcast::channel(16);
        let (stop, shutdown) = oneshot::channel();
        let task = tokio::spawn(sender.run_session(rx, None, AudioBitrate::Uncompressed, shutdown, test_audio_key()));
        let exchange = async {
            let ping = EchoPacketCipher::new(&test_audio_key()).build().unwrap();
            phone.send_to(&ping, server).await.unwrap();
            let mut buffer = [0; 8192];
            let (len, _) = phone.recv_from(&mut buffer).await.unwrap();
            assert_eq!(&buffer[..len], &ping);
            // An expired capture is skipped but its position remains visible on the wire.
            let now = Instant::now();
            tx.send(Arc::new(CapturedFrame {
                samples: vec![0.25; 960],
                position: 4,
                ready_at: now - CapturedFrame::MAX_SEND_AGE,
            }))
            .unwrap();
            tx.send(Arc::new(CapturedFrame {
                samples: (0..960).map(|index| index as f32 / 960.0).collect(),
                position: 5,
                ready_at: now,
            }))
            .unwrap();
            let cipher = AudioPacketCipher::new(&test_audio_key());
            let mut bytes = vec![0; 3840];
            let mut mask = 0;
            for index in 0..4 {
                let (len, _) = if index == 0 {
                    phone.recv_from(&mut buffer).await.unwrap()
                } else {
                    phone
                        .try_recv_from(&mut buffer)
                        .expect("PCM chunks must be sent in one frame batch")
                };
                assert_eq!(len, PcmDatagram::SIZE + AUDIO_TAG_LEN);
                let mut encrypted = buffer[..len].to_vec();
                let plain_len = cipher.open(&mut encrypted).unwrap();
                assert_eq!(plain_len, PcmDatagram::SIZE);
                let chunk = PcmDatagram::decode(&encrypted[..plain_len]).unwrap();
                assert_eq!(chunk.sequence, base + 5);
                mask |= 1 << chunk.index;
                let offset = chunk.index * PcmDatagram::PAYLOAD;
                bytes[offset..offset + PcmDatagram::PAYLOAD].copy_from_slice(chunk.payload);
            }
            assert_eq!(mask, 0b1111);
            for (index, sample) in bytes.chunks_exact(4).enumerate() {
                assert_eq!(
                    f32::from_ne_bytes(sample.try_into().unwrap()),
                    index as f32 / 960.0
                );
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(3), exchange).await;
        let _ = stop.send(());
        task.await.unwrap().unwrap();
        result.expect("UDP PCM delivery timed out");
    }

    #[tokio::test]
    async fn silence_should_use_a_single_marker_instead_of_pcm_chunks() {
        let phone = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpAudioSender::new(phone.local_addr().unwrap()).unwrap();
        let (tx, rx) = broadcast::channel(16);
        let (stop, shutdown) = oneshot::channel();
        let task = tokio::spawn(sender.run_session(rx, None, AudioBitrate::Uncompressed, shutdown, test_audio_key()));
        tx.send(Arc::new(CapturedFrame {
            samples: vec![0.0; 960],
            position: 0,
            ready_at: Instant::now(),
        }))
        .unwrap();
        let mut bytes = [0; 4096];
        let result =
            tokio::time::timeout(Duration::from_secs(3), phone.recv_from(&mut bytes)).await;
        let _ = stop.send(());
        task.await.unwrap().unwrap();
        let (len, _) = result.unwrap().unwrap();
        assert_eq!(len, 9 + AUDIO_TAG_LEN);
        let cipher = AudioPacketCipher::new(&test_audio_key());
        let plain_len = cipher.open(&mut bytes[..len]).unwrap();
        assert_eq!(plain_len, 9);
        assert_eq!(bytes[8], crate::audio::FORMAT_SILENCE);
    }
}
