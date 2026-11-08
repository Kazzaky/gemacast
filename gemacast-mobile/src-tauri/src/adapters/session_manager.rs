use crate::traits::{
    FrontendNotifier, SessionInfo, SessionManager, SessionParams, StreamerControlClientFactory,
};
use async_trait::async_trait;
use gemacast_core::domain::types::{ConnectionMode, DeviceId, JitterConfig};
use gemacast_core::stream::player::PlaybackControl;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Internal session state, analogous to the old `state::ActiveSession`.
struct ActiveSession {
    exclusive_mode: bool,
    exclusive_granted: bool,
    mode: ConnectionMode,
    bitrate: Option<i32>,
    playback_control: PlaybackControl,
    volume: Arc<AtomicU32>,
    jitter_config: Arc<RwLock<JitterConfig>>,
    shutdown_tx: oneshot::Sender<()>,
    playback_task: JoinHandle<()>,
    probe_task: Option<JoinHandle<()>>,
    control_heartbeat_epoch: Arc<AtomicU64>,
    target_ip: Option<std::net::IpAddr>,
    device_id: String,
    network_link: gemacast_core::domain::types::NetworkLink,
}

/// Manages playback sessions and WebSocket client tasks using Tokio primitives.
pub struct TokioSessionManager {
    notifier: Arc<dyn FrontendNotifier>,
    client_factory: Arc<dyn StreamerControlClientFactory>,
    session: tokio::sync::Mutex<Option<ActiveSession>>,
    ws_client_task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl TokioSessionManager {
    pub fn new(
        notifier: Arc<dyn FrontendNotifier>,
        client_factory: Arc<dyn StreamerControlClientFactory>,
    ) -> Self {
        Self {
            notifier,
            client_factory,
            session: tokio::sync::Mutex::new(None),
            ws_client_task: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl SessionManager for TokioSessionManager {
    async fn start_session(&self, params: SessionParams) -> Result<(), String> {
        // Tear down any existing session first
        self.stop_session().await;

        let player = crate::services::audio::SessionPlayer::new(self.notifier.clone()).spawn(
            crate::services::audio::SessionPlayerRequest {
                jitter_config: params.jitter_config.clone(),
                exclusive_mode: params.exclusive_mode,
                target_ip: params.target_ip,
                network_link: params.network_link,
            },
        )?;

        let control_heartbeat_epoch = Arc::new(AtomicU64::new(0));
        let probe_task = match params.target_ip {
            Some(ip) if !ip.is_loopback() => {
                let client = self.client_factory.create(ip);
                let device_id = DeviceId(params.device_id.clone());
                Some(tokio::spawn(run_probe_loop(
                    client,
                    device_id,
                    control_heartbeat_epoch.clone(),
                )))
            }
            _ => None,
        };

        *self.session.lock().await = Some(ActiveSession {
            exclusive_mode: params.exclusive_mode,
            exclusive_granted: player.exclusive_granted,
            mode: params.mode,
            bitrate: params.bitrate,
            playback_control: player.playback_control,
            volume: player.volume,
            jitter_config: player.jitter_config,
            shutdown_tx: player.shutdown,
            playback_task: player.task,
            probe_task,
            control_heartbeat_epoch,
            target_ip: params.target_ip,
            device_id: params.device_id,
            network_link: params.network_link,
        });

        Ok(())
    }

    async fn stop_session(&self) {
        if let Some(session) = self.session.lock().await.take() {
            if let Some(probe_task) = session.probe_task {
                probe_task.abort();
            }
            let _ = session.shutdown_tx.send(());
            // Upper bound, not a fixed per-teardown cost: the receive loop's
            // `select!` wakes on `shutdown_tx` immediately and its `ScopeGuard`
            // detaches (does not join) the worker threads, so this await normally
            // returns in well under a frame. The 1.5 s only elapses if the Oboe
            // stream's `Drop`/close hangs — and force-proceeding earlier would just
            // re-open the device into that same stuck close (worse under exclusive
            // mode). Left as a safety ceiling.
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(1500),
                session.playback_task,
            )
            .await;
        }
        self.stop_ws_client().await;
    }

    async fn set_playing(&self, playing: bool) {
        let control = self
            .session
            .lock()
            .await
            .as_ref()
            .map(|session| session.playback_control.clone());
        if let Some(control) = control {
            let result = if playing {
                control.resume().await
            } else {
                control.pause().await
            };
            if let Err(error) = result {
                tracing::warn!("[Playback] Failed to change playback state: {error}");
            }
        }
    }

    async fn pause_playback(&self) -> Result<(), String> {
        let control = self
            .session
            .lock()
            .await
            .as_ref()
            .map(|session| session.playback_control.clone())
            .ok_or("No active session")?;
        control.pause().await.map_err(|error| error.to_string())
    }

    async fn resume_playback(&self) -> Result<(), String> {
        let control = self
            .session
            .lock()
            .await
            .as_ref()
            .map(|session| session.playback_control.clone())
            .ok_or("No active session")?;
        control.resume().await.map_err(|error| error.to_string())
    }

    async fn update_jitter_config(&self, config: JitterConfig) {
        if let Some(session) = self.session.lock().await.as_ref()
            && let Ok(mut guard) = session.jitter_config.write()
        {
            *guard = config;
        }
    }

    async fn session_info(&self) -> Option<SessionInfo> {
        let guard = self.session.lock().await;
        guard.as_ref().map(|s| SessionInfo {
            exclusive_mode: s.exclusive_mode,
            exclusive_granted: s.exclusive_granted,
            mode: s.mode,
            bitrate: s.bitrate,
            jitter_config: s
                .jitter_config
                .read()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default(),
            target_ip: s.target_ip,
            device_id: s.device_id.clone(),
            network_link: s.network_link,
        })
    }

    async fn update_bitrate(&self, bitrate: Option<i32>) {
        if let Some(session) = self.session.lock().await.as_mut() {
            session.bitrate = bitrate;
        }
    }

    async fn set_volume(&self, linear: f32) {
        if let Some(session) = self.session.lock().await.as_ref() {
            session
                .volume
                .store(f32::to_bits(linear), Ordering::Relaxed);
        }
    }

    async fn start_ws_client(&self, task: JoinHandle<()>) {
        let mut guard = self.ws_client_task.lock().await;
        if let Some(old_task) = guard.take() {
            old_task.abort();
        }
        *guard = Some(task);
    }

    async fn record_control_heartbeat(&self) {
        if let Some(session) = self.session.lock().await.as_ref() {
            session
                .control_heartbeat_epoch
                .fetch_add(1, Ordering::Release);
        }
    }

    async fn stop_ws_client(&self) {
        if let Some(task) = self.ws_client_task.lock().await.take() {
            task.abort();
        }
    }
}

/// Keep the PC session alive with HTTPS when WebSocket acknowledgements stop.
/// Errors are logged but never terminate the loop — probes are best-effort.
async fn run_probe_loop(
    client: Arc<dyn crate::traits::StreamerControlClient>,
    device_id: DeviceId,
    control_heartbeat_epoch: Arc<AtomicU64>,
) {
    let period = std::time::Duration::from_secs(5);
    let start = tokio::time::Instant::now() + period;
    let mut interval = tokio::time::interval_at(start, period);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut observed_heartbeat = control_heartbeat_epoch.load(Ordering::Acquire);

    loop {
        interval.tick().await;
        let current_heartbeat = control_heartbeat_epoch.load(Ordering::Acquire);
        if current_heartbeat != observed_heartbeat {
            observed_heartbeat = current_heartbeat;
            continue;
        }
        if let Err(e) = client.probe(Some(device_id.clone())).await {
            tracing::warn!("[Probe] WebSocket unavailable; HTTPS fallback failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::{ControlClientCall, MockStreamerControlClient};

    #[tokio::test(start_paused = true)]
    async fn https_probe_should_resume_when_websocket_acknowledgements_stop() {
        let client = Arc::new(MockStreamerControlClient::new());
        let heartbeat_epoch = Arc::new(AtomicU64::new(0));
        let task = tokio::spawn(run_probe_loop(
            client.clone(),
            DeviceId("phone-1".into()),
            heartbeat_epoch.clone(),
        ));
        tokio::task::yield_now().await;

        heartbeat_epoch.fetch_add(1, Ordering::Release);
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert!(client.take_calls().is_empty());

        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        tokio::task::yield_now().await;

        assert!(matches!(
            client.take_calls().as_slice(),
            [ControlClientCall::Probe {
                device_id: Some(DeviceId(device_id)),
            }] if device_id == "phone-1"
        ));
        task.abort();
    }
}
