use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::captured_frame::CapturedFrame;
use super::encode::AudioFrameEncoder;
use super::failure::StreamTaskFailure;
use super::udp_audio_sender::UdpAudioSender;
use crate::audio::{AudioSessionKey, MAX_OPUS_PACKET_SIZE, OPUS_FRAME_SAMPLES, SEQ_NUM_SIZE};
use crate::domain::error::{AudioError, GemaCastError};
use crate::domain::types::{AudioBitrate, AudioSource};
use crate::ports::capture::{CaptureBackend, CaptureCounters, CaptureHandle};

/// Tracks one per-target encoder task. Each connected player gets its own encoder
/// at its requested bitrate, running in a dedicated tokio task.
pub(crate) struct PerTargetEncoder {
    pub(crate) bitrate: AudioBitrate,
    pub(crate) audio_key: AudioSessionKey,
    pub(crate) generation: u64,
    pub(crate) shutdown_tx: oneshot::Sender<()>,
    pub(crate) join_handle: tokio::task::JoinHandle<()>,
}

/// Tracks a TCP encoder that publishes to the broadcast channel
/// instead of sending UDP packets.
pub(crate) struct TcpEncoder {
    pub(crate) bitrate: AudioBitrate,
    pub(crate) generation: u64,
    pub(crate) shutdown_tx: oneshot::Sender<()>,
    pub(crate) join_handle: tokio::task::JoinHandle<()>,
    pub(crate) audio_broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>,
}

pub(crate) struct AudioCaptureInstance {
    source: AudioSource,
    pub(crate) generation: u64,
    failure_tx: mpsc::UnboundedSender<StreamTaskFailure>,
    next_encoder_generation: u64,
    /// Per-target encoders keyed by socket address (for UDP/WiFi targets).
    pub(crate) per_target_encoders: HashMap<SocketAddr, PerTargetEncoder>,
    /// TCP encoders keyed by DeviceId.
    pub(crate) tcp_encoders: HashMap<crate::domain::types::DeviceId, TcpEncoder>,
    /// Broadcast channel for raw PCM frames from the capture thread.
    pcm_broadcast_tx: broadcast::Sender<Arc<CapturedFrame>>,
    pub capture_shutdown_tx: Option<oneshot::Sender<()>>,
    pub capture_join_handle: tokio::task::JoinHandle<()>,
}

impl AudioCaptureInstance {
    pub(crate) fn new<B: CaptureBackend + 'static>(
        capture: CaptureHandle<B>,
        source: AudioSource,
        generation: u64,
        failure_tx: mpsc::UnboundedSender<StreamTaskFailure>,
    ) -> Result<Self, GemaCastError> {
        // Capture fan-out is bounded independently from each phone's send deadline.
        let (pcm_broadcast_tx, _) = broadcast::channel(16);
        let (capture_shutdown_tx, capture_shutdown_rx) = oneshot::channel();
        let pcm_tx_clone = pcm_broadcast_tx.clone();

        let failure_source = source.clone();
        let capture_failure_tx = failure_tx.clone();
        let loop_source = source.clone();
        let join_handle = tokio::spawn(async move {
            if let Err(error) =
                Self::run_capture_loop(capture, capture_shutdown_rx, pcm_tx_clone, loop_source)
                    .await
            {
                let _ = capture_failure_tx.send(StreamTaskFailure::Capture {
                    source: failure_source,
                    generation,
                    message: error.to_string(),
                });
            }
        });

        Ok(Self {
            source,
            generation,
            failure_tx,
            next_encoder_generation: 0,
            per_target_encoders: HashMap::new(),
            tcp_encoders: HashMap::new(),
            pcm_broadcast_tx,
            capture_shutdown_tx: Some(capture_shutdown_tx),
            capture_join_handle: join_handle,
        })
    }

    /// Spawns a per-target encoder task that subscribes to raw PCM frames,
    /// encodes at the given bitrate, and sends UDP packets to the target.
    pub(crate) async fn spawn_target_encoder(
        &mut self,
        target_addr: SocketAddr,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
    ) -> Result<(), GemaCastError> {
        let bitrate = AudioBitrate::from_wire(bitrate)?;
        if self
            .per_target_encoders
            .get(&target_addr)
            .is_some_and(|encoder| encoder.bitrate == bitrate && encoder.audio_key == audio_key)
        {
            return Ok(());
        }

        let encoder = AudioFrameEncoder::create_codec(bitrate)?;
        self.remove_target_encoder(&target_addr).await;

        let pcm_rx = self.pcm_broadcast_tx.subscribe();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let failure_tx = self.failure_tx.clone();
        let source = self.source.clone();
        let generation = self.generation;
        self.next_encoder_generation = self.next_encoder_generation.wrapping_add(1).max(1);
        let encoder_generation = self.next_encoder_generation;

        let join_handle = tokio::spawn(async move {
            if let Err(error) =
                UdpAudioSender::run(pcm_rx, target_addr, encoder, bitrate, shutdown_rx, audio_key).await
            {
                tracing::error!("[PerTargetEncoder] {:?} failed: {}", target_addr, error);
                let _ = failure_tx.send(StreamTaskFailure::UdpEncoder {
                    source,
                    generation,
                    encoder_generation,
                    target: target_addr,
                    message: error.to_string(),
                });
            }
        });

        self.per_target_encoders.insert(
            target_addr,
            PerTargetEncoder {
                bitrate,
                audio_key,
                generation: encoder_generation,
                shutdown_tx,
                join_handle,
            },
        );

        Ok(())
    }

    pub(crate) async fn spawn_tcp_encoder_with_channel(
        &mut self,
        device_id: crate::domain::types::DeviceId,
        bitrate: Option<i32>,
        reusable_channel: Option<broadcast::Sender<Arc<Vec<u8>>>>,
    ) -> Result<broadcast::Sender<Arc<Vec<u8>>>, GemaCastError> {
        let bitrate = AudioBitrate::from_wire(bitrate)?;
        if let Some(existing) = self.tcp_encoders.get(&device_id)
            && existing.bitrate == bitrate
        {
            return Ok(existing.audio_broadcast_tx.clone());
        }

        let encoder = AudioFrameEncoder::create_codec(bitrate)?;
        let reusable_channel = reusable_channel.or_else(|| {
            self.tcp_encoders
                .get(&device_id)
                .map(|existing| existing.audio_broadcast_tx.clone())
        });
        self.remove_tcp_encoder(&device_id).await;

        let pcm_rx = self.pcm_broadcast_tx.subscribe();
        let audio_broadcast_tx = reusable_channel.unwrap_or_else(|| broadcast::channel(4000).0);
        let tcp_broadcast_tx = audio_broadcast_tx.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let failure_tx = self.failure_tx.clone();
        let source = self.source.clone();
        let generation = self.generation;
        self.next_encoder_generation = self.next_encoder_generation.wrapping_add(1).max(1);
        let encoder_generation = self.next_encoder_generation;
        let failure_device_id = device_id.clone();

        let join_handle = tokio::spawn(async move {
            if let Err(error) =
                TcpEncoder::run(pcm_rx, encoder, bitrate, tcp_broadcast_tx, shutdown_rx).await
            {
                tracing::error!("[TcpEncoder] failed: {}", error);
                let _ = failure_tx.send(StreamTaskFailure::TcpEncoder {
                    source,
                    generation,
                    encoder_generation,
                    device_id: failure_device_id,
                    message: error.to_string(),
                });
            }
        });

        self.tcp_encoders.insert(
            device_id,
            TcpEncoder {
                bitrate,
                generation: encoder_generation,
                shutdown_tx,
                join_handle,
                audio_broadcast_tx: audio_broadcast_tx.clone(),
            },
        );

        Ok(audio_broadcast_tx)
    }

    /// Removes a per-target encoder, shutting down its task.
    pub(crate) async fn remove_target_encoder(&mut self, target_addr: &SocketAddr) {
        if let Some(encoder) = self.per_target_encoders.remove(target_addr) {
            let _ = encoder.shutdown_tx.send(());
            let _ = encoder.join_handle.await;
        }
    }

    /// Removes a TCP encoder for a specific device, shutting down its task.
    pub(crate) async fn remove_tcp_encoder(&mut self, device_id: &crate::domain::types::DeviceId) {
        if let Some(encoder) = self.tcp_encoders.remove(device_id) {
            let _ = encoder.shutdown_tx.send(());
            let _ = encoder.join_handle.await;
        }
    }

    pub(crate) fn tcp_broadcaster(
        &self,
        device_id: &crate::domain::types::DeviceId,
    ) -> Option<broadcast::Sender<Arc<Vec<u8>>>> {
        self.tcp_encoders
            .get(device_id)
            .map(|encoder| encoder.audio_broadcast_tx.clone())
    }

    /// The capture loop: reads raw PCM from the audio backend and broadcasts
    /// raw frames. No encoding happens here.
    async fn run_capture_loop<B: CaptureBackend>(
        mut capture: CaptureHandle<B>,
        mut capture_shutdown_rx: oneshot::Receiver<()>,
        pcm_broadcast_tx: broadcast::Sender<Arc<CapturedFrame>>,
        source: AudioSource,
    ) -> Result<(), GemaCastError> {
        use ringbuf::traits::*;
        let mut sample_buf = Vec::<f32>::with_capacity(OPUS_FRAME_SAMPLES * 2);
        let mut frame_position = 0u64;

        // Watchdog interval to inject silence if WASAPI loopback goes idle (e.g. no apps playing audio)
        // 22ms is slightly longer than the standard 20ms Opus frame duration.
        let mut silence_interval = tokio::time::interval(tokio::time::Duration::from_millis(22));
        silence_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        // The capture counters are reported from here and never from the capture
        // callback: `snapshot()` allocates and `format!` is not something to run on a
        // real-time audio thread, which is the whole reason the counters are atomics
        // rather than log calls. A line is emitted only when a total actually moved,
        // so a healthy stream stays quiet; the unconditional totals line at every
        // exit path below is what proves the instrument was present at all.
        let mut counter_interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
        counter_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_reported = capture.counters.snapshot();

        capture.backend.play()?;

        loop {
            tokio::select! {
                _ = capture.notify.notified() => {
                    // We received real audio, reset the silence watchdog
                    silence_interval.reset();

                    let occupied = capture.consumer.occupied_len();
                    if occupied == 0 {
                        continue;
                    }

                    // Produce frames if any UDP targets exist OR any PCM subscribers
                    // (per-target or TCP encoder tasks) are listening.
                    let has_pcm_listeners = pcm_broadcast_tx.receiver_count() > 0;
                    if !has_pcm_listeners {
                        while capture.consumer.try_pop().is_some() {}
                        sample_buf.clear();
                        continue;
                    }

                    let prev_len = sample_buf.len();
                    sample_buf.resize(prev_len + occupied, 0.0);

                    let actually_read = capture.consumer.pop_slice(&mut sample_buf[prev_len..]);
                    sample_buf.truncate(prev_len + actually_read);

                    CaptureDiagnostics::note_ring_read_parity(actually_read, &source, &capture.counters);

                    let read_at = tokio::time::Instant::now();
                    while sample_buf.len() >= OPUS_FRAME_SAMPLES {
                        let remaining = sample_buf.len() - OPUS_FRAME_SAMPLES;
                        let buffered_time = std::time::Duration::from_secs_f64(
                            remaining as f64 / (crate::audio::OPUS_SAMPLE_RATE as f64 * crate::audio::OPUS_CHANNELS as f64),
                        );
                        let frame = Arc::new(CapturedFrame {
                            samples: sample_buf[..OPUS_FRAME_SAMPLES].to_vec(),
                            position: frame_position,
                            ready_at: read_at - buffered_time,
                        });
                        frame_position = frame_position.wrapping_add(1);
                        sample_buf.drain(..OPUS_FRAME_SAMPLES);

                        // Broadcast raw PCM frame to all encoder tasks
                        let _ = pcm_broadcast_tx.send(frame);
                    }
                },
                _ = silence_interval.tick() => {
                    // No real audio received for 22ms. Inject a silent frame if anyone is listening
                    // to prevent the mobile client from timing out and disconnecting.
                    let has_pcm_listeners = pcm_broadcast_tx.receiver_count() > 0;
                    if has_pcm_listeners {
                        let silent_frame = Arc::new(CapturedFrame {
                            samples: vec![0.0f32; OPUS_FRAME_SAMPLES],
                            position: frame_position,
                            ready_at: tokio::time::Instant::now(),
                        });
                        frame_position = frame_position.wrapping_add(1);
                        let _ = pcm_broadcast_tx.send(silent_frame);
                    }
                },
                _ = counter_interval.tick() => {
                    let current = capture.counters.snapshot();
                    if current != last_reported {
                        tracing::info!(
                            "[Capture] {:?} counters: {}",
                            source,
                            CaptureDiagnostics::format(&current)
                        );
                        last_reported = current;
                    }
                },
                Some(stream_error) = capture.stream_error_rx.recv() => {
                    CaptureDiagnostics::log_totals(&source, &capture.counters);
                    return Err(AudioError::StreamError(stream_error).into());
                },
                _ = &mut capture_shutdown_rx => {
                    let _ = capture.backend.pause();
                    break;
                }
                else => break,
            }
        }

        CaptureDiagnostics::log_totals(&source, &capture.counters);
        Ok(())
    }
}

struct CaptureDiagnostics;

impl CaptureDiagnostics {
    /// Format a counter snapshot as one `key=value` line.
    ///
    /// Allocates, so it belongs on the reporting path only; see the note in
    /// [`CaptureCounters`].
    fn format(snapshot: &[(&'static str, u64)]) -> String {
        snapshot
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Check that a ring read carried whole stereo pairs, and record it if it did not.
    ///
    /// This is the one seam where a broken [producer
    /// obligation 1](crate::ports::capture#producer-obligations) is still observable. The
    /// ring is a flat `f32` stream whose stereo pairing is implied by position, so an odd
    /// push shifts every later sample by one slot and swaps left and right for the rest of
    /// the session. Downstream there is nothing left to notice: the frame is still 960
    /// samples, the encoder still encodes it, and the phone still plays it.
    ///
    /// **Deliberately not a `debug_assert!`,** though that is what the audit prescribed
    /// here. Two reasons, and the first is the decisive one:
    ///
    /// 1. An odd read is a *platform observation*, not a local logic error. PipeWire can
    ///    produce one today: `push_pw_audio_to_ringbuf` pushes `n_samples` verbatim, and
    ///    nothing there enforces parity. A `debug_assert!` compiles out of release, which
    ///    is the only build a field capture ever runs, so it would assert loudly in exactly
    ///    the situation that cannot happen and stay silent in the one that does.
    /// 2. The assertions the audit named are tautologies at this site. `frame` is
    ///    `sample_buf[..OPUS_FRAME_SAMPLES].to_vec()`, so `frame.len() ==
    ///    OPUS_FRAME_SAMPLES` is guaranteed by the slicing expression, and 960 is even, so
    ///    the parity assertion on it holds twice over. Neither can fail; both would read as
    ///    coverage that does not exist.
    ///
    /// The frame length handed to the encoder is checked instead where it is genuinely
    /// unproven. `AudioFrameEncoder::encode` returns [`AudioError::InvalidFrameLength`], in
    /// release as well, and both call sites propagate that with `?`. A `debug_assert!`
    /// there would be strictly weaker than what is already in place.
    fn note_ring_read_parity(read: usize, source: &AudioSource, counters: &CaptureCounters) {
        if read.is_multiple_of(2) {
            return;
        }

        // `fetch_add` returns the previous value, so the first odd read on *this* stream
        // gets the prose and the rest are counted. Keying off the counter rather than a
        // `std::sync::Once` is what makes it per-stream: a `Once` is process-wide, so a
        // second capture session in the same process would be silent.
        let previously_seen = counters
            .odd_ring_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if previously_seen == 0 {
            tracing::error!(
                ?source,
                read,
                "[Capture] ring delivered an odd sample count; left and right are now \
             swapped for the rest of this stream"
            );
        }
    }

    /// Emit the final counter totals for a capture stream, including when all are zero.
    ///
    /// Unconditional on purpose. A run that logs nothing when nothing went wrong cannot
    /// be told apart from a run whose counters were never wired up, and that ambiguity
    /// has cost a session before: an earlier round had to infer which wire format a
    /// capture ran by reading the saturation behaviour of an unrelated heartbeat, because
    /// nothing recorded it directly.
    fn log_totals(source: &AudioSource, counters: &CaptureCounters) {
        tracing::info!(
            "[Capture] {:?} counter totals: {}",
            source,
            Self::format(&counters.snapshot())
        );
    }
}

impl TcpEncoder {
    /// TCP encoder loop: receives raw PCM frames, encodes at the configured
    /// bitrate, and publishes to the broadcast channel for TCP consumers.
    async fn run(
        mut pcm_rx: broadcast::Receiver<Arc<CapturedFrame>>,
        mut encoder: Option<opus::Encoder>,
        bitrate: AudioBitrate,
        tcp_broadcast_tx: broadcast::Sender<Arc<Vec<u8>>>,
        mut shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<(), GemaCastError> {
        let sequence_base: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut opus_output = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet_buf: Vec<u8> = Vec::with_capacity(
            SEQ_NUM_SIZE + crate::audio::FORMAT_FLAG_SIZE + MAX_OPUS_PACKET_SIZE,
        );

        loop {
            tokio::select! {
                result = pcm_rx.recv() => {
                    let frame = match result {
                        Ok(f) => f,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("[TcpEncoder] Lagged by {} frames", n);
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    };

                    AudioFrameEncoder::encode(
                        &frame.samples,
                        encoder.as_mut(),
                        bitrate,
                        sequence_base.wrapping_add(frame.position),
                        &mut opus_output,
                        &mut packet_buf,
                    )?;

                    let shared = Arc::new(packet_buf.clone());
                    let _ = tcp_broadcast_tx.send(shared);

                }
                _ = &mut shutdown_rx => break,
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::capture_pool::CapturePool;
    use super::*;

    fn test_audio_key() -> AudioSessionKey {
        AudioSessionKey::from_session_token(&"44".repeat(32)).unwrap()
    }
    use crate::domain::types::{DeviceId, TargetId};
    use crate::ports::capture::{CaptureBackend, CaptureCounters, CaptureFactory};
    use ringbuf::HeapRb;
    use ringbuf::traits::*;
    use tokio::sync::Notify;
    use tokio::sync::mpsc;

    struct MockBackend;
    impl CaptureBackend for MockBackend {
        fn play(&mut self) -> Result<(), GemaCastError> {
            Ok(())
        }
        fn pause(&mut self) -> Result<(), GemaCastError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn capture_instance_should_broadcast_pcm_and_encode_tcp() {
        // 1. Setup ringbuffer and mock handle
        let ring_buffer = HeapRb::<f32>::new(48000 * 2);
        let (mut producer, consumer) = ring_buffer.split();
        let notify = Arc::new(Notify::new());
        let (_err_tx, err_rx) = mpsc::channel(1);

        let capture_handle = CaptureHandle {
            backend: MockBackend,
            consumer,
            notify: notify.clone(),
            stream_error_rx: err_rx,
            counters: Arc::new(CaptureCounters::default()),
        };

        // 2. Create the AudioCaptureInstance
        let (failure_tx, _failure_rx) = mpsc::unbounded_channel();
        let mut instance =
            AudioCaptureInstance::new(capture_handle, AudioSource::Desktop, 1, failure_tx)
                .expect("Failed to create AudioCaptureInstance");

        // We can manually subscribe to the internal PCM broadcast channel to verify the capture loop
        let mut pcm_rx = instance.pcm_broadcast_tx.subscribe();

        // 3. Push fake PCM data
        // Opus stereo encoding expects exactly OPUS_FRAME_SAMPLES
        let frame_size = crate::audio::OPUS_FRAME_SAMPLES;
        let fake_audio = vec![0.5f32; frame_size];

        producer.push_slice(&fake_audio);
        notify.notify_one();

        // 4. Verify capture loop reads and broadcasts PCM (ignoring any silence watchdog frames)
        let mut received_pcm;
        loop {
            received_pcm = pcm_rx.recv().await.unwrap_or_else(|_| {
                tracing::error!("Fatal error: Failed to receive PCM");
                panic!("Failed to receive PCM");
            });
            if received_pcm.samples[0] != 0.0 {
                break;
            }
        }
        assert_eq!(received_pcm.samples.len(), frame_size);
        assert_eq!(received_pcm.samples[0], 0.5f32);

        // 5. Test Encoder spawning
        let device_id = DeviceId("test_dev".into());
        let audio_broadcast_tx = instance
            .spawn_tcp_encoder_with_channel(device_id.clone(), Some(128000), None)
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Fatal error: Failed to spawn TCP encoder: {}", e);
                panic!("Failed to spawn TCP encoder: {}", e);
            });

        let mut encoded_rx = audio_broadcast_tx.subscribe();

        // Push another frame so the encoder has something to encode
        producer.push_slice(&fake_audio);
        notify.notify_one();

        // The encoder should eventually emit an opus packet (ignore silence watchdog packets)
        let mut encoded_packet;
        loop {
            encoded_packet = encoded_rx.recv().await.unwrap_or_else(|_| {
                tracing::error!("Fatal error: Failed to receive Opus packet");
                panic!("Failed to receive Opus packet");
            });
            // Verify packet contains sequence number (8 bytes) + format flag (1 byte) + some opus payload
            if encoded_packet.len() > 9 {
                break;
            }
        }

        // 6. Test clean asynchronous teardown (simulating unsubscribe)
        instance.remove_tcp_encoder(&device_id).await;
        if let Some(stop_tx) = instance.capture_shutdown_tx.take() {
            stop_tx.send(()).unwrap();
            // Await the join handle like unsubscribe does, verifying no deadlocks!
            instance.capture_join_handle.await.unwrap_or_else(|e| {
                tracing::error!("Fatal error: Capture loop panicked: {}", e);
                panic!("Capture loop panicked");
            });
        }
    }

    #[tokio::test]
    async fn capture_instance_should_encode_and_send_udp_packets() {
        // 1. Setup ringbuffer and mock handle
        let ring_buffer = HeapRb::<f32>::new(48000 * 2);
        let (mut producer, consumer) = ring_buffer.split();
        let notify = Arc::new(Notify::new());
        let (_err_tx, err_rx) = mpsc::channel(1);

        let capture_handle = CaptureHandle {
            backend: MockBackend,
            consumer,
            notify: notify.clone(),
            stream_error_rx: err_rx,
            counters: Arc::new(CaptureCounters::default()),
        };

        // 2. Create the AudioCaptureInstance
        let (failure_tx, _failure_rx) = mpsc::unbounded_channel();
        let mut instance =
            AudioCaptureInstance::new(capture_handle, AudioSource::Desktop, 1, failure_tx)
                .expect("Failed to create AudioCaptureInstance");

        // Bind a local UDP socket to receive the encoded packets
        let player_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target_addr = player_socket.local_addr().unwrap();

        // 3. Spawn UDP target encoder
        instance
            .spawn_target_encoder(target_addr, Some(128000), test_audio_key())
            .await
            .expect("Failed to spawn UDP encoder");

        // Push fake audio frame
        let frame_size = crate::audio::OPUS_FRAME_SAMPLES;
        let fake_audio = vec![0.5f32; frame_size];

        producer.push_slice(&fake_audio);
        notify.notify_one();

        // The encoder should eventually emit an opus packet over UDP
        let mut buf = vec![0u8; 1500];
        let mut len;

        loop {
            // Keep pushing audio because UDP packets could be dropped (e.g. ARP/startup delays)
            producer.push_slice(&fake_audio);
            notify.notify_one();

            let recv_future = player_socket.recv_from(&mut buf);
            let (recv_len, _) =
                tokio::time::timeout(std::time::Duration::from_millis(500), recv_future)
                    .await
                    .expect("Timed out waiting for UDP packet")
                    .expect("Failed to receive UDP packet");

            len = recv_len;
            if len > 9 {
                break;
            }
        }

        // Verify packet contains sequence number (8 bytes) + format flag (1 byte) + some opus payload
        assert!(len > 9);

        // 4. Test clean teardown
        instance.remove_target_encoder(&target_addr).await;
        if let Some(stop_tx) = instance.capture_shutdown_tx.take() {
            stop_tx.send(()).unwrap();
            instance
                .capture_join_handle
                .await
                .expect("Capture loop panicked");
        }
    }

    #[tokio::test]
    async fn udp_encoder_should_reflect_an_echo_ping_back_to_its_source() {
        let ring_buffer = HeapRb::<f32>::new(48000 * 2);
        let (mut producer, consumer) = ring_buffer.split();
        let notify = Arc::new(Notify::new());
        let (_err_tx, err_rx) = mpsc::channel(1);

        let capture_handle = CaptureHandle {
            backend: MockBackend,
            consumer,
            notify: notify.clone(),
            stream_error_rx: err_rx,
            counters: Arc::new(CaptureCounters::default()),
        };

        let (failure_tx, _failure_rx) = mpsc::unbounded_channel();
        let mut instance =
            AudioCaptureInstance::new(capture_handle, AudioSource::Desktop, 1, failure_tx)
                .expect("Failed to create AudioCaptureInstance");

        // Stand in for the phone: the encoder sends audio to this socket, and
        // its own ephemeral source address is where we bounce a ping off.
        let phone_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target_addr = phone_socket.local_addr().unwrap();

        instance
            .spawn_target_encoder(target_addr, Some(128000))
            .await
            .expect("Failed to spawn UDP encoder");

        let frame_size = crate::audio::OPUS_FRAME_SAMPLES;
        let fake_audio = vec![0.5f32; frame_size];
        let mut buf = vec![0u8; 1500];

        // Learn the encoder's ephemeral source address from an audio packet.
        // Re-push each iteration because the broadcast subscription and a
        // localhost UDP datagram can both need a moment to catch the first
        // frame; bounded so a genuinely dead encoder fails instead of hanging.
        let mut encoder_src = None;
        for _ in 0..200 {
            producer.push_slice(&fake_audio);
            notify.notify_one();
            let recv_future = phone_socket.recv_from(&mut buf);
            if let Ok(Ok((_len, src))) =
                tokio::time::timeout(std::time::Duration::from_millis(500), recv_future).await
            {
                encoder_src = Some(src);
                break;
            }
        }
        let encoder_src = encoder_src.expect("encoder never sent an audio packet");

        // Bounce a ping off the encoder and expect the exact bytes back. Both
        // the ping and audio are re-sent each iteration because localhost UDP
        // can still drop a datagram, and most reads will be audio packets until
        // the reflected ping arrives.
        let ping = crate::stream::echo::EchoPacket::build();
        let reflected = loop {
            producer.push_slice(&fake_audio);
            notify.notify_one();
            phone_socket.send_to(&ping, encoder_src).await.unwrap();

            let recv_future = phone_socket.recv_from(&mut buf);
            let (len, src) =
                tokio::time::timeout(std::time::Duration::from_millis(500), recv_future)
                    .await
                    .expect("Timed out waiting for the reflected ping")
                    .expect("Failed to receive reflected ping");
            if src == encoder_src && crate::stream::echo::EchoPacket::matches(&buf, len) {
                break buf[..len].to_vec();
            }
        };

        assert_eq!(reflected, ping.to_vec());

        instance.remove_target_encoder(&target_addr).await;
        if let Some(stop_tx) = instance.capture_shutdown_tx.take() {
            stop_tx.send(()).unwrap();
            instance
                .capture_join_handle
                .await
                .expect("Capture loop panicked");
        }
    }

    struct MockCaptureFactory;

    impl CaptureFactory for MockCaptureFactory {
        type Backend = MockBackend;

        fn create_desktop_capture(&self) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
            let ring_buffer = HeapRb::<f32>::new(48000 * 2);
            let (_producer, consumer) = ring_buffer.split();
            let notify = Arc::new(Notify::new());
            let (_err_tx, err_rx) = mpsc::channel(1);

            Ok(CaptureHandle {
                backend: MockBackend,
                consumer,
                notify,
                stream_error_rx: err_rx,
                counters: Arc::new(CaptureCounters::default()),
            })
        }

        fn create_process_capture(
            &self,
            _pid: u32,
        ) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
            self.create_desktop_capture() // Just reuse the mock for tests
        }
    }

    #[tokio::test]
    async fn pool_should_create_and_teardown_instances_on_subscribe_unsubscribe() {
        let factory = MockCaptureFactory;
        let mut pool = CapturePool::new(factory, true);
        let target = TargetId::Tcp(DeviceId("dev1".into()));

        // 1. Subscribe to desktop
        let _tx = pool
            .subscribe(AudioSource::Desktop, target.clone(), Some(128000))
            .await
            .expect("Subscribe failed");
        assert_eq!(pool.instances.len(), 1);

        // 2. Subscribe again (should reuse the instance)
        let _tx2 = pool
            .subscribe(AudioSource::Desktop, target.clone(), Some(128000))
            .await
            .expect("Subscribe failed");
        assert_eq!(pool.instances.len(), 1);

        // 3. Unsubscribe (should teardown)
        pool.unsubscribe(&AudioSource::Desktop, target)
            .await
            .expect("Unsubscribe failed");
        assert_eq!(pool.instances.len(), 0);
    }

    #[tokio::test]
    async fn pool_should_migrate_target_when_changing_source() {
        let factory = MockCaptureFactory;
        let mut pool = CapturePool::new(factory, true);
        let target = TargetId::Tcp(DeviceId("dev1".into()));

        // Subscribe to desktop
        pool.subscribe(AudioSource::Desktop, target.clone(), Some(128000))
            .await
            .expect("Subscribe failed");
        assert_eq!(pool.instances.len(), 1);

        // Change source to process
        let new_source = AudioSource::Process {
            pid: 1234,
            name: "test".into(),
        };
        pool.change_source(
            &AudioSource::Desktop,
            new_source.clone(),
            target,
            Some(128000),
        )
        .await
        .expect("Change source failed");

        // Old instance should be gone, new instance should be created
        assert_eq!(pool.instances.len(), 1);
        assert!(pool.instances.contains_key(&new_source));
    }

    #[tokio::test]
    async fn pool_should_support_multiple_tcp_encoders_per_source() {
        let factory = MockCaptureFactory;
        let mut pool = CapturePool::new(factory, true);
        let target1 = TargetId::Tcp(DeviceId("dev1".into()));
        let target2 = TargetId::Tcp(DeviceId("dev2".into()));

        pool.subscribe(AudioSource::Desktop, target1.clone(), Some(128000))
            .await
            .expect("Subscribe 1 failed");
        pool.subscribe(AudioSource::Desktop, target2.clone(), Some(256000))
            .await
            .expect("Subscribe 2 failed");

        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.tcp_encoders.len(), 2);

        pool.unsubscribe(&AudioSource::Desktop, target1)
            .await
            .expect("Unsubscribe 1 failed");

        // Teardown should not happen yet
        assert_eq!(pool.instances.len(), 1);
        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.tcp_encoders.len(), 1);

        pool.unsubscribe(&AudioSource::Desktop, target2)
            .await
            .expect("Unsubscribe 2 failed");

        // Now it should teardown
        assert_eq!(pool.instances.len(), 0);
    }

    #[tokio::test]
    async fn tcp_broadcast_channel_should_survive_bitrate_changes() {
        let mut pool = CapturePool::new(MockCaptureFactory, true);
        let device_id = DeviceId("dev1".into());
        let target = TargetId::Tcp(device_id.clone());

        let original = pool
            .subscribe(AudioSource::Desktop, target.clone(), Some(128000))
            .await
            .unwrap()
            .unwrap();
        let replacement = pool
            .change_bitrate(&AudioSource::Desktop, target.clone(), Some(256000))
            .await
            .unwrap()
            .unwrap();

        assert!(original.same_channel(&replacement));
        assert!(
            original.same_channel(
                &pool
                    .tcp_broadcaster(&AudioSource::Desktop, &device_id)
                    .unwrap()
            )
        );

        pool.unsubscribe(&AudioSource::Desktop, target)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn stale_capture_failure_should_not_evict_a_recreated_source() {
        let mut pool = CapturePool::new(MockCaptureFactory, true);
        let first_target = TargetId::Tcp(DeviceId("dev1".into()));
        pool.subscribe(AudioSource::Desktop, first_target.clone(), Some(128000))
            .await
            .unwrap();
        let first_generation = pool
            .instances
            .get(&AudioSource::Desktop)
            .unwrap()
            .generation;
        pool.unsubscribe(&AudioSource::Desktop, first_target)
            .await
            .unwrap();

        let second_target = TargetId::Tcp(DeviceId("dev2".into()));
        pool.subscribe(AudioSource::Desktop, second_target.clone(), Some(128000))
            .await
            .unwrap();
        let second_generation = pool
            .instances
            .get(&AudioSource::Desktop)
            .unwrap()
            .generation;

        assert_ne!(first_generation, second_generation);
        assert!(
            !pool
                .evict_failed_source(&AudioSource::Desktop, first_generation)
                .await
        );
        assert!(pool.instances.contains_key(&AudioSource::Desktop));

        pool.unsubscribe(&AudioSource::Desktop, second_target)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn pool_should_support_multiple_udp_encoders_per_source() {
        let factory = MockCaptureFactory;
        let mut pool = CapturePool::new(factory, true);

        let target1 = TargetId::Udp("127.0.0.1:1111".parse().unwrap());
        let target2 = TargetId::Udp("127.0.0.1:2222".parse().unwrap());

        pool.subscribe(AudioSource::Desktop, target1.clone(), Some(128000))
            .await
            .expect("Subscribe 1 failed");
        pool.subscribe(AudioSource::Desktop, target2.clone(), Some(256000))
            .await
            .expect("Subscribe 2 failed");

        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.per_target_encoders.len(), 2);

        pool.unsubscribe(&AudioSource::Desktop, target1)
            .await
            .expect("Unsubscribe 1 failed");

        // Teardown should not happen yet
        assert_eq!(pool.instances.len(), 1);
        let instance = pool.instances.get(&AudioSource::Desktop).unwrap();
        assert_eq!(instance.per_target_encoders.len(), 1);

        pool.unsubscribe(&AudioSource::Desktop, target2)
            .await
            .expect("Unsubscribe 2 failed");

        // Now it should teardown
        assert_eq!(pool.instances.len(), 0);
    }

    mod capture_counter_reporting {
        use super::*;
        use std::sync::atomic::Ordering;

        #[test]
        fn should_name_every_counter_in_the_reported_line() {
            let counters = CaptureCounters::default();
            let line = CaptureDiagnostics::format(&counters.snapshot());

            // A field capture is grepped by counter name, so every name has to be
            // present even at zero; an absent key reads as "not instrumented".
            for name in [
                "dropped_samples",
                "unaligned_prefix_bytes",
                "truncated_samples",
                "odd_ring_reads",
                "corrupted_chunks",
                "silent_buffers",
                "unknown_format_buffers",
                "dropped_stream_errors",
            ] {
                assert!(
                    line.contains(&format!("{name}=0")),
                    "counter `{name}` missing from `{line}`"
                );
            }
        }

        #[test]
        fn should_change_the_snapshot_when_a_counter_moves() {
            // The 1 Hz arm decides whether to log by comparing snapshots, so snapshot
            // inequality is the change detector. If this ever held, a drop would be
            // counted and still never reported.
            let counters = CaptureCounters::default();
            let before = counters.snapshot();
            assert_eq!(before, counters.snapshot(), "a quiet stream must not log");

            CaptureCounters::add(&counters.dropped_samples, 960);
            let after = counters.snapshot();

            assert_ne!(before, after, "a counted drop must produce a new line");
            assert!(
                CaptureDiagnostics::format(&after).contains("dropped_samples=960"),
                "the reported line must carry the new total"
            );
        }

        #[tokio::test]
        async fn should_hand_the_capture_loop_the_same_counters_the_backend_writes() {
            // One `Arc`, two owners: the adapter's callback writes it and the loop
            // reports it. A backend that constructed its own copy would count
            // correctly and report zeros forever.
            let ring_buffer = HeapRb::<f32>::new(48000 * 2);
            let (_producer, consumer) = ring_buffer.split();
            let notify = Arc::new(Notify::new());
            let (_err_tx, err_rx) = mpsc::channel(1);
            let counters = Arc::new(CaptureCounters::default());

            let capture_handle = CaptureHandle {
                backend: MockBackend,
                consumer,
                notify,
                stream_error_rx: err_rx,
                counters: counters.clone(),
            };

            CaptureCounters::add(&capture_handle.counters.corrupted_chunks, 3);

            assert_eq!(
                counters.corrupted_chunks.load(Ordering::Relaxed),
                3,
                "the handle must share the caller's counters, not clone the values"
            );
            assert!(!counters.all_clear());
        }
    }

    /// The stereo-parity check at the ring pop.
    ///
    /// What makes these worth having is that the condition they detect is *silent by
    /// construction* everywhere else: an odd ring read still produces 960-sample
    /// frames, still encodes, and still plays. Only the sample count at this seam
    /// carries the evidence, and only for the one read that broke it.
    ///
    /// Falsified by making the check inert (`if true || read.is_multiple_of(2) { return }`):
    /// 3 of the 5 below fail: `should_count_a_read_that_ends_mid_stereo_pair`,
    /// `should_count_every_odd_read_and_not_only_the_first` and
    /// `should_not_confuse_an_odd_read_with_a_producer_holding_a_sample_back`. The
    /// other two assert the *absence* of a count and pass under that revert by
    /// construction; they are there to stop a future version counting clean reads,
    /// which would bury the signal in noise, not to discriminate this change.
    mod ring_read_parity {
        use super::*;
        use std::sync::atomic::Ordering;

        #[test]
        fn should_leave_the_counter_clear_for_a_read_of_whole_pairs() {
            let counters = CaptureCounters::default();

            CaptureDiagnostics::note_ring_read_parity(
                OPUS_FRAME_SAMPLES,
                &AudioSource::Desktop,
                &counters,
            );

            assert!(
                counters.all_clear(),
                "a healthy stream must report nothing at all"
            );
        }

        #[test]
        fn should_count_a_read_that_ends_mid_stereo_pair() {
            let counters = CaptureCounters::default();

            CaptureDiagnostics::note_ring_read_parity(
                OPUS_FRAME_SAMPLES - 1,
                &AudioSource::Desktop,
                &counters,
            );

            assert_eq!(counters.odd_ring_reads.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn should_count_every_odd_read_and_not_only_the_first() {
            // The prose is emitted once per stream, the count is not. Conflating the
            // two would make the tally useless for telling one glitch from a
            // producer that is broken on every buffer.
            let counters = CaptureCounters::default();

            for _ in 0..3 {
                CaptureDiagnostics::note_ring_read_parity(7, &AudioSource::Desktop, &counters);
            }

            assert_eq!(counters.odd_ring_reads.load(Ordering::Relaxed), 3);
        }

        #[test]
        fn should_not_confuse_an_odd_read_with_a_producer_holding_a_sample_back() {
            // `truncated_samples` means a producer honoured the parity obligation;
            // `odd_ring_reads` means one broke it. They are opposite readings and a
            // field capture is graded on which of the two moved.
            let counters = CaptureCounters::default();

            CaptureDiagnostics::note_ring_read_parity(1, &AudioSource::Desktop, &counters);

            assert_eq!(counters.odd_ring_reads.load(Ordering::Relaxed), 1);
            assert_eq!(
                counters.truncated_samples.load(Ordering::Relaxed),
                0,
                "the consumer-side observation must not be filed as a producer holdover"
            );
        }

        #[test]
        fn should_count_an_empty_read_as_clean() {
            // Zero is even, and a zero-length read is the normal outcome of a wake
            // with no data. Counting it would swamp the tally on every idle stream.
            let counters = CaptureCounters::default();

            CaptureDiagnostics::note_ring_read_parity(0, &AudioSource::Desktop, &counters);

            assert!(counters.all_clear());
        }
    }
}
