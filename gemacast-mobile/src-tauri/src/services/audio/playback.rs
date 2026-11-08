use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, RwLock};

use gemacast_core::domain::error::{GemaCastError, NetworkError};
use gemacast_core::domain::types::{JitterConfig, NetworkLink};
use gemacast_core::stream::player::{AudioStreamPlayer, PlaybackControl};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::traits::FrontendNotifier;

pub struct SessionPlayerRequest {
    pub jitter_config: JitterConfig,
    pub exclusive_mode: bool,
    pub target_ip: Option<IpAddr>,
    pub network_link: NetworkLink,
}

pub struct SessionPlayerHandle {
    pub playback_control: PlaybackControl,
    pub jitter_config: Arc<RwLock<JitterConfig>>,
    pub volume: Arc<AtomicU32>,
    pub shutdown: oneshot::Sender<()>,
    pub task: JoinHandle<()>,
    pub exclusive_granted: bool,
}

pub struct SessionPlayer {
    notifier: Arc<dyn FrontendNotifier>,
}

impl SessionPlayer {
    pub fn new(notifier: Arc<dyn FrontendNotifier>) -> Self {
        Self { notifier }
    }

    pub fn spawn(&self, request: SessionPlayerRequest) -> Result<SessionPlayerHandle, String> {
        let jitter_config = Arc::new(RwLock::new(request.jitter_config.clone()));
        let transport_mode = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(AtomicU32::new(f32::to_bits(1.0)));
        let (shutdown, shutdown_rx) = oneshot::channel();

        let player = AudioStreamPlayer::new(
            jitter_config.clone(),
            transport_mode.clone(),
            request.network_link,
            volume.clone(),
            request.exclusive_mode,
            shutdown_rx,
        )
        .map_err(|error| error.to_string())?;

        let exclusive_granted = player.exclusive_granted;
        let playback_control = player.playback_control();
        let notifier = self.notifier.clone();
        let channels = PlaybackEvents::new(notifier.clone());

        let task = tokio::spawn(async move {
            Self::run(player, request, channels, notifier).await;
        });

        Ok(SessionPlayerHandle {
            playback_control,
            jitter_config,
            volume,
            shutdown,
            task,
            exclusive_granted,
        })
    }

    async fn run(
        mut player: AudioStreamPlayer,
        request: SessionPlayerRequest,
        channels: PlaybackEvents,
        notifier: Arc<dyn FrontendNotifier>,
    ) {
        if let Err(error) = player.activate_playback_stream() {
            notifier.emit_playback_error(error.to_string());
            return;
        }

        let result = player
            .run_audio_receive_loop(
                Some(channels.streamer_ip),
                Some(channels.telemetry),
                Some(channels.network_rtt),
                request.target_ip,
            )
            .await;

        match result {
            Err(GemaCastError::Network(NetworkError::ConnectionLost)) => notifier.emit_link_lost(),
            Err(error) => notifier.emit_playback_error(error.to_string()),
            Ok(()) => {}
        }
    }
}

struct PlaybackEvents {
    streamer_ip: oneshot::Sender<String>,
    telemetry: mpsc::Sender<(f32, f32, f32)>,
    network_rtt: mpsc::Sender<f32>,
}

impl PlaybackEvents {
    fn new(notifier: Arc<dyn FrontendNotifier>) -> Self {
        let (streamer_ip, streamer_ip_rx) = oneshot::channel();
        let connection_notifier = notifier.clone();
        tokio::spawn(async move {
            if let Ok(ip) = streamer_ip_rx.await {
                connection_notifier.emit_streamer_connected(ip);
            }
        });

        let (network_rtt, mut network_rtt_rx) = mpsc::channel(10);
        let network_notifier = notifier.clone();
        tokio::spawn(async move {
            while let Some(rtt_ms) = network_rtt_rx.recv().await {
                network_notifier.emit_network_rtt(rtt_ms);
            }
        });

        let (telemetry, mut telemetry_rx) = mpsc::channel(10);
        tokio::spawn(async move {
            while let Some((latency, rms, jitter)) = telemetry_rx.recv().await {
                notifier.emit_audio_telemetry(latency, rms > 0.0001, jitter);
            }
        });

        Self {
            streamer_ip,
            telemetry,
            network_rtt,
        }
    }
}
