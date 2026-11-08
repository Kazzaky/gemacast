use super::stream::PlaybackOutput;
use crate::{
    domain::error::{AudioError, GemaCastError},
    domain::types::{JitterConfig, NetworkLink},
    jitter::RawPacket,
    network::Ports,
};
use cpal::StreamError;
use ringbuf::{HeapProd, HeapRb, traits::*};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering},
};
use tokio::sync::{mpsc, oneshot};

use super::heartbeat::KeepaliveHeartbeat;
use super::packet_receiver::PacketReceiver;
use super::playback_control::{PlaybackCommand, PlaybackControl};
use super::playback_worker::PlaybackWorker;
use super::stream::PlaybackStream;

const PACKET_CHANNEL_CAPACITY: usize = 1024;
// Xiaomi/HyperOS mutes an active AudioTrack after about 60 seconds of zero or
// "small" samples. A short grace avoids lifecycle churn during ordinary gaps,
// while still suspending the track well before the OEM detector can fire.
const SOURCE_IDLE_SUSPEND_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

pub struct AudioStreamPlayer {
    packet_producer: HeapProd<RawPacket>,
    playback_stream: PlaybackStream,
    stream_error_rx: mpsc::Receiver<StreamError>,
    playback_shutdown_rx: oneshot::Receiver<()>,
    latency_metric: Arc<AtomicU32>,
    jitter_metric: Arc<AtomicU32>,
    playback_control: PlaybackControl,
    playback_command_rx: mpsc::UnboundedReceiver<PlaybackCommand>,
    render_enabled: Arc<AtomicBool>,
    reset_requested: Arc<AtomicBool>,
    pub exclusive_granted: bool,
}

impl AudioStreamPlayer {
    pub fn new(
        config_ref: Arc<std::sync::RwLock<JitterConfig>>,
        is_tcp_mode: Arc<AtomicBool>,
        network_link: NetworkLink,
        volume: Arc<AtomicU32>,
        _exclusive_mode: bool,
        playback_shutdown_rx: oneshot::Receiver<()>,
    ) -> Result<Self, GemaCastError> {
        let (_stream_error_tx, stream_error_rx) = mpsc::channel::<StreamError>(1);
        let packet_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
        let (packet_producer, packet_consumer) = packet_rb.split();
        let latency_metric = Arc::new(AtomicU32::new(0));
        let jitter_metric = Arc::new(AtomicU32::new(0));
        let render_enabled = Arc::new(AtomicBool::new(true));
        let reset_requested = Arc::new(AtomicBool::new(false));
        let (playback_control, playback_command_rx) = PlaybackControl::channel();

        #[cfg(not(target_os = "android"))]
        let playback_stream = PlaybackOutput::build(
            packet_consumer,
            config_ref,
            is_tcp_mode,
            network_link,
            render_enabled.clone(),
            reset_requested.clone(),
            volume,
            latency_metric.clone(),
            jitter_metric.clone(),
            _stream_error_tx,
        )?;

        #[cfg(not(target_os = "android"))]
        let exclusive_granted = false;

        #[cfg(target_os = "android")]
        let (packet_producer, playback_stream, exclusive_granted) = {
            // Try Oboe first; if it fails, the consumer is consumed by the
            // failed callback so we must create a fresh ring buffer for cpal.
            match PlaybackOutput::build(
                packet_consumer,
                config_ref.clone(),
                is_tcp_mode.clone(),
                network_link,
                render_enabled.clone(),
                reset_requested.clone(),
                volume.clone(),
                latency_metric.clone(),
                jitter_metric.clone(),
                _exclusive_mode,
            ) {
                Ok((stream, granted)) => (packet_producer, stream, granted),
                Err(oboe_err) => {
                    tracing::warn!("Oboe failed ({}), retrying with cpal fallback", oboe_err);
                    let fallback_rb = HeapRb::<RawPacket>::new(PACKET_CHANNEL_CAPACITY);
                    let (fb_producer, fb_consumer) = fallback_rb.split();
                    let stream = PlaybackOutput::build_cpal_fallback(
                        fb_consumer,
                        config_ref,
                        is_tcp_mode,
                        network_link,
                        render_enabled.clone(),
                        reset_requested.clone(),
                        volume,
                        latency_metric.clone(),
                        jitter_metric.clone(),
                    )?;
                    (fb_producer, stream, false)
                }
            }
        };

        Ok(Self {
            packet_producer,
            playback_stream,
            stream_error_rx,
            playback_shutdown_rx,
            latency_metric,
            jitter_metric,
            playback_control,
            playback_command_rx,
            render_enabled,
            reset_requested,
            exclusive_granted,
        })
    }

    pub fn playback_control(&self) -> PlaybackControl {
        self.playback_control.clone()
    }

    pub async fn run_audio_receive_loop(
        mut self,
        streamer_ip_tx: Option<oneshot::Sender<String>>,
        latency_tx: Option<mpsc::Sender<(f32, f32, f32)>>,
        rtt_tx: Option<mpsc::Sender<f32>>,
        target_ip: Option<std::net::IpAddr>,
    ) -> Result<(), GemaCastError> {
        let (transport, heartbeat_socket) = super::transport::AudioTransportFactory::create(target_ip)?;
        let heartbeat_active = Arc::new(AtomicBool::new(true));
        let streamer_port = Arc::new(AtomicU16::new(Ports::AUDIO_UDP));

        let heartbeat_thread = match (target_ip, heartbeat_socket) {
            (Some(target), Some(hb_socket)) => Some(
                KeepaliveHeartbeat::new(
                    target,
                    streamer_port.clone(),
                    heartbeat_active.clone(),
                    hb_socket,
                )
                .spawn(),
            ),
            _ => None,
        };

        let (playback_control_error_tx, mut playback_control_error_rx) = mpsc::channel(1);
        let playback_control_thread = PlaybackWorker::spawn(
            self.playback_stream,
            self.playback_command_rx,
            self.render_enabled,
            self.reset_requested,
            playback_control_error_tx,
        );
        let player_active = Arc::new(AtomicBool::new(true));
        let (network_dropped_tx, mut network_dropped_rx) = mpsc::channel::<()>(1);

        let player_thread = PacketReceiver::spawn(
            transport,
            self.packet_producer,
            self.latency_metric.clone(),
            self.jitter_metric.clone(),
            streamer_ip_tx,
            latency_tx,
            rtt_tx,
            player_active.clone(),
            streamer_port,
            network_dropped_tx,
            target_ip,
            self.playback_control.clone(),
            cfg!(target_os = "android").then_some(SOURCE_IDLE_SUSPEND_AFTER),
        );

        struct ScopeGuard {
            heartbeat_active: Arc<AtomicBool>,
            player_active: Arc<AtomicBool>,
            heartbeat_thread: Option<std::thread::JoinHandle<()>>,
            player_thread: Option<std::thread::JoinHandle<()>>,
            playback_control: PlaybackControl,
            playback_control_thread: Option<std::thread::JoinHandle<()>>,
        }

        impl Drop for ScopeGuard {
            fn drop(&mut self) {
                self.heartbeat_active.store(false, Ordering::Relaxed);
                self.player_active.store(false, Ordering::Relaxed);
                self.playback_control.shutdown();
                if let Some(t) = self.heartbeat_thread.take() {
                    // Detach thread instead of blocking the Tokio worker
                    drop(t);
                }
                if let Some(t) = self.player_thread.take() {
                    // Detach thread instead of blocking the Tokio worker
                    drop(t);
                }
                if let Some(t) = self.playback_control_thread.take() {
                    // The control thread owns the stream so start/pause/drop are
                    // serialized. Joining prevents a new exclusive stream from
                    // racing the previous stream's close during reconnect.
                    let _ = t.join();
                }
            }
        }

        let mut _guard = ScopeGuard {
            heartbeat_active,
            player_active,
            heartbeat_thread,
            player_thread: Some(player_thread),
            playback_control: self.playback_control,
            playback_control_thread: Some(playback_control_thread),
        };

        tokio::select! {
            Some(stream_err) = self.stream_error_rx.recv() => {
                return Err(AudioError::StreamError(stream_err).into());
            }
            _ = network_dropped_rx.recv() => {
                return Err(crate::domain::error::NetworkError::ConnectionLost.into());
            }
            Some(message) = playback_control_error_rx.recv() => {
                return Err(AudioError::PlaybackControlFailed(message).into());
            }
            _ = &mut self.playback_shutdown_rx => {}
        }

        Ok(())
    }

    pub fn activate_playback_stream(&mut self) -> Result<(), GemaCastError> {
        PlaybackOutput::start(&mut self.playback_stream)
    }
}
