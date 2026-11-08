use super::packet::AudioPacketDecoder;
use super::pcm_assembly::PcmAssembly;
use super::playback_control::PlaybackControl;
use super::receive_diagnostics::ReceiveDiagnostics;
use super::source_idle::SourceIdleDetector;
use crate::audio::pcm_datagram::PcmDatagram;
use crate::audio::encryption::{AudioPacketCipher, AudioReplayWindow, AUDIO_TAG_LEN};
use crate::audio::AudioSessionKey;
use crate::audio::{MAX_OPUS_PACKET_SIZE, SEQ_NUM_SIZE};
use crate::jitter::RawPacket;
use crate::ports::transport::AudioPacketTransport;
use ringbuf::{HeapProd, traits::*};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use tokio::sync::{mpsc, oneshot};

pub(crate) struct PacketReceiver;

impl PacketReceiver {
    #[expect(
        clippy::too_many_arguments,
        reason = "receiver construction wires one stream session"
    )]
    pub(crate) fn spawn<T: AudioPacketTransport + 'static>(
        mut transport: T,
        mut packet_producer: HeapProd<RawPacket>,
        latency_metric: Arc<AtomicU32>,
        jitter_metric: Arc<AtomicU32>,
        mut streamer_ip_tx: Option<oneshot::Sender<String>>,
        latency_tx: Option<mpsc::Sender<(f32, f32, f32)>>,
        rtt_tx: Option<mpsc::Sender<f32>>,
        active: Arc<AtomicBool>,
        streamer_port: Arc<AtomicU16>,
        network_dropped_tx: mpsc::Sender<()>,
        allowed_streamer_ip: Option<std::net::IpAddr>,
        playback_control: PlaybackControl,
        source_idle_after: Option<std::time::Duration>,
        audio_key: AudioSessionKey,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            #[cfg(target_os = "android")]
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, -19);
                libc::prctl(29, 1);
            }

            let mut recv_buff =
                vec![0u8; SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE + AUDIO_TAG_LEN];
            let mut last_packet_time = std::time::Instant::now();
            let mut first_packet_received = false;
            let mut source_idle_detector = source_idle_after.map(SourceIdleDetector::new);
            let mut pcm_assembly = PcmAssembly::new();
            let mut diagnostics = ReceiveDiagnostics::new(std::time::Instant::now());
            let cipher = AudioPacketCipher::new(&audio_key);
            let mut replay_window = AudioReplayWindow::default();

            while active.load(Ordering::Relaxed) {
                let now = std::time::Instant::now();
                pcm_assembly.expire(now);
                diagnostics.report(
                    now,
                    pcm_assembly.completed_frames,
                    pcm_assembly.expired_frames,
                );
                let result = transport.receive_audio_packet(&mut recv_buff);
                let (mut len, streamer_addr) = match result {
                    Ok(r) => {
                        if allowed_streamer_ip.is_some_and(|allowed| allowed != r.1.ip()) {
                            tracing::debug!(
                                allowed = %allowed_streamer_ip.unwrap(),
                                observed = %r.1.ip(),
                                "[Player] Ignoring audio packet from an unexpected streamer"
                            );
                            continue;
                        }
                        // An echo ping the PC reflected back: record the wire RTT and
                        // skip the audio path entirely. This must run before the
                        // bookkeeping below. An echo is liveness, not audio, so it
                        // must not seed the streamer IP, reset the audio-arrival
                        // timeout, or trip the "first packet" log.
                        if crate::stream::echo::EchoPacket::matches(&recv_buff, r.0) {
                            if let Some(ref tx) = rtt_tx {
                                let _ = tx.try_send(
                                    crate::stream::echo::EchoPacket::round_trip_ms(&recv_buff),
                                );
                            }
                            continue;
                        }
                        r
                    }
                    Err(e) => {
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) {
                            diagnostics.timeout();
                        }
                        if e.kind() == std::io::ErrorKind::UnexpectedEof
                            || e.kind() == std::io::ErrorKind::ConnectionReset
                        {
                            let _ = network_dropped_tx.try_send(());
                            break;
                        }
                        let timeout = 10;
                        let elapsed = last_packet_time.elapsed().as_secs();
                        if elapsed >= timeout {
                            tracing::warn!(
                                "[Player] Network timeout: no packets for {}s (threshold={}s), disconnecting",
                                elapsed,
                                timeout,
                            );
                            let _ = network_dropped_tx.try_send(());
                            break;
                        }
                        // The transport already waits on the socket. Sleeping after a
                        // timeout would delay recovery and manufacture arrival bursts.
                        if !matches!(
                            e.kind(),
                            std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::Interrupted
                        ) {
                            tracing::warn!(error = %e, "[Player] Audio receive failed");
                            let _ = network_dropped_tx.try_send(());
                            break;
                        }
                        continue;
                    }
                };

                // Authenticate before decoding, metrics, liveness updates, or replay bookkeeping.
                // Echo packets were deliberately handled above and remain outside the audio AEAD.
                let Ok(plain_len) = cipher.open(&mut recv_buff[..len]) else {
                    continue;
                };
                len = plain_len;
                if !replay_window.accept(&recv_buff[..len]) {
                    continue;
                }
                diagnostics.received(len, std::time::Instant::now());
                if !first_packet_received {
                    tracing::info!("[Player] First authenticated audio packet received from {}", streamer_addr);
                }
                last_packet_time = std::time::Instant::now();
                first_packet_received = true;

                streamer_port.store(streamer_addr.port(), Ordering::Relaxed);

                if let Some(tx) = streamer_ip_tx.take() {
                    let _ = tx.send(streamer_addr.ip().to_string());
                }

                let packet = if len > SEQ_NUM_SIZE && recv_buff[SEQ_NUM_SIZE] == PcmDatagram::FORMAT
                {
                    let Ok(chunk) = PcmDatagram::decode(&recv_buff[..len]) else {
                        continue;
                    };
                    let Some(packet) = pcm_assembly.push(chunk, std::time::Instant::now()) else {
                        continue;
                    };
                    packet
                } else {
                    let Ok(packet) = AudioPacketDecoder::decode(&recv_buff, len) else {
                        continue;
                    };
                    packet
                };

                let seq_num = packet.seq_num;
                let is_silence = packet.is_silence;
                let is_uncompressed = packet.is_uncompressed;
                let report_rms = latency_tx
                    .as_ref()
                    .filter(|_| seq_num.is_multiple_of(100))
                    .map(|_| {
                        AudioPacketDecoder::estimate_rms(
                            &packet.payload_data[..packet.payload_len],
                            is_silence,
                            is_uncompressed,
                        )
                    });

                let source_idle_edge = source_idle_detector
                    .as_mut()
                    .and_then(|detector| detector.observe(is_silence, std::time::Instant::now()));
                let source_is_suspended = source_idle_detector
                    .as_ref()
                    .is_some_and(SourceIdleDetector::is_suspended);

                // Manual pause drops every packet. Automatic source-idle suspension
                // drops only silence markers. The first real packet raises the wake
                // edge; following packets populate the freshly reset jitter buffer.
                let should_enqueue =
                    playback_control.user_wants_playing() && !(source_is_suspended && is_silence);

                if should_enqueue && packet_producer.try_push(packet).is_err() {
                    tracing::warn!(
                        "[WARN] SPSC ring buffer full, dropped seq {}. Audio callback may be stalled.",
                        seq_num
                    );
                }

                if let Some(idle) = source_idle_edge {
                    tracing::info!(idle, "[Playback] Remote source idle state changed");
                    playback_control.set_source_idle(idle);
                }

                if let Some(ref tx) = latency_tx
                    && let Some(rms) = report_rms
                {
                    let buffer_delay_ms = latency_metric.load(Ordering::Relaxed) as f32;
                    let jitter_ms = jitter_metric.load(Ordering::Relaxed) as f32;
                    let _ = tx.try_send((buffer_delay_ms, rms, jitter_ms));
                }
            }
            tracing::info!(
                completed_pcm_frames = pcm_assembly.completed_frames,
                expired_pcm_frames = pcm_assembly.expired_frames,
                "[Player] PCM assembly totals"
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::transport::AudioPacketTransport;
    use ringbuf::HeapRb;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32};
    use tokio::sync::mpsc;

    fn test_audio_key() -> AudioSessionKey {
        AudioSessionKey::from_session_token(&"66".repeat(32)).unwrap()
    }

    fn test_playback_control() -> PlaybackControl {
        PlaybackControl::channel().0
    }

    fn receive_sequence(results: Vec<Result<Vec<u8>, std::io::ErrorKind>>) -> Vec<RawPacket> {
        let cipher = AudioPacketCipher::new(&test_audio_key());
        let results = results
            .into_iter()
            .map(|result| result.and_then(|mut packet| {
                cipher.seal(&mut packet).map_err(|_| std::io::ErrorKind::InvalidData)?;
                Ok(packet)
            }))
            .collect();
        let (producer, mut consumer) = HeapRb::<RawPacket>::new(8).split();
        let (drop_tx, _) = mpsc::channel(1);
        PacketReceiver::spawn(
            crate::testing::mocks::PacketSequence::new(results),
            producer,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            None,
            None,
            None,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicU16::new(0)),
            drop_tx,
            Some("127.0.0.1".parse().unwrap()),
            test_playback_control(),
            None,
            test_audio_key(),
        )
        .join()
        .unwrap();
        let mut packets = Vec::new();
        while let Some(packet) = consumer.try_pop() {
            packets.push(packet);
        }
        packets
    }

    #[test]
    fn the_receive_loop_should_reassemble_pcm_after_transient_errors() {
        let mut frame = vec![0; 3849];
        frame[..8].copy_from_slice(&100u64.to_be_bytes());
        frame[8] = crate::audio::FORMAT_UNCOMPRESSED;
        for sample in frame[9..].chunks_exact_mut(4) {
            sample.copy_from_slice(&0.5f32.to_ne_bytes());
        }
        let mut results = vec![
            Err(std::io::ErrorKind::TimedOut),
            Err(std::io::ErrorKind::WouldBlock),
            Err(std::io::ErrorKind::Interrupted),
        ];
        for index in [2, 0, 3, 1] {
            let mut chunk = [0; PcmDatagram::SIZE];
            PcmDatagram::encode(&frame, 123, index, &mut chunk);
            results.push(Ok(chunk.to_vec()));
        }
        let packets = receive_sequence(results);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].seq_num, 100);
        assert_eq!(&packets[0].payload_data[..3840], &frame[9..]);
    }

    #[test]
    fn a_permanent_receive_failure_should_stop_before_processing_more_audio() {
        let mut frame = vec![0; 9];
        frame[8] = crate::audio::FORMAT_SILENCE;
        assert!(
            receive_sequence(vec![Err(std::io::ErrorKind::PermissionDenied), Ok(frame)]).is_empty()
        );
    }

    struct MockTransport {
        packet_to_send: Option<Vec<u8>>,
        streamer_addr: SocketAddr,
    }

    impl AudioPacketTransport for MockTransport {
        fn receive_audio_packet(
            &mut self,
            buffer: &mut [u8],
        ) -> std::io::Result<(usize, SocketAddr)> {
            if let Some(data) = self.packet_to_send.take() {
                let len = data.len();
                buffer[..len].copy_from_slice(&data);
                Ok((len, self.streamer_addr))
            } else {
                // Return EOF to terminate the loop
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "Done",
                ))
            }
        }
    }

    #[tokio::test]
    async fn should_push_parsed_packet_to_ring_buffer_and_signal_network_drop() {
        let packet_rb = HeapRb::<RawPacket>::new(1024);
        let (producer, mut consumer) = packet_rb.split();
        let latency_metric = Arc::new(AtomicU32::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(0));
        let (network_dropped_tx, mut network_dropped_rx) = mpsc::channel(1);

        // Construct a dummy Opus packet
        let mut dummy_packet =
            vec![0u8; crate::audio::SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + 10];
        // Seq num = 42 (Big Endian)
        dummy_packet[0..8].copy_from_slice(&42u64.to_be_bytes());
        // Opus format flag
        dummy_packet[8] = crate::audio::FORMAT_OPUS;
        // payload = some data
        dummy_packet[9..19].copy_from_slice(&[0x1; 10]);

        let cipher = AudioPacketCipher::new(&test_audio_key());
        cipher.seal(&mut dummy_packet).unwrap();

        let transport = MockTransport {
            packet_to_send: Some(dummy_packet),
            streamer_addr: "127.0.0.1:1234".parse().unwrap(),
        };

        let handle = PacketReceiver::spawn(
            transport,
            producer,
            latency_metric,
            Arc::new(AtomicU32::new(0)),
            None,
            None,
            None,
            active,
            streamer_port,
            network_dropped_tx,
            Some("127.0.0.1".parse().unwrap()),
            test_playback_control(),
            None,
            test_audio_key(),
        );

        // Wait for thread to exit
        let _ = handle.join();

        // Ensure network drop was signaled due to EOF
        assert!(network_dropped_rx.recv().await.is_some());

        // Check if the packet was pushed to the consumer
        let received_packet = consumer.try_pop().expect("Packet should have been pushed");
        assert_eq!(received_packet.seq_num, 42);
        assert!(!received_packet.is_silence);
        assert!(!received_packet.is_uncompressed);
        assert_eq!(received_packet.payload_len, 10);
        assert_eq!(received_packet.payload_data[0], 0x1);
    }

    #[tokio::test]
    async fn should_ignore_udp_packets_from_a_different_streamer_ip() {
        let packet_rb = HeapRb::<RawPacket>::new(8);
        let (producer, mut consumer) = packet_rb.split();
        let active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(0));
        let (network_dropped_tx, mut network_dropped_rx) = mpsc::channel(1);
        let mut packet = vec![0u8; crate::audio::SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + 1];
        packet[0..8].copy_from_slice(&1u64.to_be_bytes());
        packet[8] = crate::audio::FORMAT_OPUS;

        let handle = PacketReceiver::spawn(
            MockTransport {
                packet_to_send: Some(packet),
                streamer_addr: "10.0.0.2:23558".parse().unwrap(),
            },
            producer,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            None,
            None,
            None,
            active,
            streamer_port.clone(),
            network_dropped_tx,
            Some("10.0.0.1".parse().unwrap()),
            test_playback_control(),
            None,
            test_audio_key(),
        );

        handle.join().unwrap();
        assert!(network_dropped_rx.recv().await.is_some());
        assert!(consumer.try_pop().is_none());
        assert_eq!(streamer_port.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn should_reflect_an_echo_ping_to_the_rtt_channel_and_not_the_ring_buffer() {
        let packet_rb = HeapRb::<RawPacket>::new(8);
        let (producer, mut consumer) = packet_rb.split();
        let active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(0));
        let (network_dropped_tx, _network_dropped_rx) = mpsc::channel(1);
        let (rtt_tx, mut rtt_rx) = mpsc::channel::<f32>(4);

        let ping = crate::stream::echo::EchoPacket::build().to_vec();

        let handle = PacketReceiver::spawn(
            MockTransport {
                packet_to_send: Some(ping),
                streamer_addr: "10.0.0.1:50000".parse().unwrap(),
            },
            producer,
            Arc::new(AtomicU32::new(0)),
            Arc::new(AtomicU32::new(0)),
            None,
            None,
            Some(rtt_tx),
            active,
            streamer_port.clone(),
            network_dropped_tx,
            Some("10.0.0.1".parse().unwrap()),
            test_playback_control(),
            None,
            test_audio_key(),
        );

        handle.join().unwrap();

        // An echo is liveness, not audio: nothing reaches the jitter ring and
        // the audio bookkeeping (streamer port) is untouched...
        assert!(consumer.try_pop().is_none());
        assert_eq!(streamer_port.load(Ordering::Relaxed), 0);
        // ...but a wire-RTT sample was emitted.
        let rtt = rtt_rx
            .try_recv()
            .expect("an RTT sample should have been sent");
        assert!(rtt >= 0.0, "rtt should be non-negative, got {rtt}");
    }
}
