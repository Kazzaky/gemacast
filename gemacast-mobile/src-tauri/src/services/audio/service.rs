use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gemacast_core::control::types::ConnectReq;
use gemacast_core::domain::types::{AudioSource, ConnectionMode, DeviceId, JitterConfig, LinkPair};

use crate::services::audio::volume::{VolumeController, VolumeMix};
use crate::traits::{
    ConnectParams, FrontendNotifier, PlatformService, PlaybackState, ResumeParams, SessionManager,
    SessionParams, StreamerControlClientFactory,
};

pub struct AudioService {
    session: Arc<dyn SessionManager>,
    client_factory: Arc<dyn StreamerControlClientFactory>,
    notifier: Arc<dyn FrontendNotifier>,
    platform: Arc<dyn PlatformService>,
    is_streaming: Arc<AtomicBool>,
    cached_link_pair: std::sync::Mutex<Option<LinkPair>>,
    recovery_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    volume_mix: Arc<std::sync::Mutex<VolumeMix>>,
}

const RECOVERY_PROBE_INTERVAL: Duration = Duration::from_secs(2);

const RECOVERY_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

const RECOVERY_BUDGET: Duration = Duration::from_secs(60);

async fn run_link_recovery(
    client: Arc<dyn crate::traits::StreamerControlClient>,
    device_id: DeviceId,
    notifier: Arc<dyn FrontendNotifier>,
    interval: Duration,
    budget: Duration,
) {
    let started = tokio::time::Instant::now();
    let mut ticker = tokio::time::interval(interval);
    let mut attempts: u32 = 0;

    loop {
        ticker.tick().await;

        if started.elapsed() >= budget {
            tracing::warn!(
                "[AudioService] Link recovery gave up after {} attempts in {:?}",
                attempts,
                budget,
            );
            notifier.emit_link_recovery_gave_up();
            return;
        }

        attempts += 1;
        match client.probe(Some(device_id.clone())).await {
            Ok(presence) => {
                tracing::info!(
                    "[AudioService] Link recovered after {} attempts: streamer={}, offline={}, registered={:?}",
                    attempts,
                    presence.streamer_name,
                    presence.is_offline,
                    presence.device_registered,
                );
                notifier.emit_link_recovered(presence.device_registered);
                return;
            }
            Err(e) => {
                tracing::debug!(
                    "[AudioService] Link recovery attempt {} failed: {}",
                    attempts,
                    e
                );
            }
        }
    }
}

impl AudioService {
    pub fn new(
        session: Arc<dyn SessionManager>,
        client_factory: Arc<dyn StreamerControlClientFactory>,
        notifier: Arc<dyn FrontendNotifier>,
        platform: Arc<dyn PlatformService>,
        is_streaming: Arc<AtomicBool>,
    ) -> Self {
        Self {
            session,
            client_factory,
            notifier,
            platform,
            is_streaming,
            cached_link_pair: std::sync::Mutex::new(None),
            recovery_task: std::sync::Mutex::new(None),
            volume_mix: Arc::new(std::sync::Mutex::new(VolumeMix::default())),
        }
    }

    pub async fn shutdown(&self) {
        self.session.stop_session().await;
    }

    pub async fn connect_to_streamer(&self, params: ConnectParams) -> Result<(), String> {
        tracing::info!(
            "[AudioService] Connect: ip={}, device={:?}, mode={:?}, jitter_preset=min_{}ms/cap_{}ms",
            params.ip,
            params.device_id,
            params.mode,
            params.jitter_config.min_depth_ms,
            params.jitter_config.comfort_cap_ms,
        );
        self.stop_link_recovery();

        let ip_addr: IpAddr = params
            .ip
            .parse()
            .map_err(|e: std::net::AddrParseError| e.to_string())?;
        let client = self.client_factory.create(ip_addr);

        let response = client
            .connect(ConnectReq {
                device_id: params.device_id.clone(),
                device_name: params.device_name.clone(),
                source: None,
                mode: params.mode,
                jitter_config: params.jitter_config.clone(),
                bitrate: params.bitrate,
                network_link: params.phone_network_link,
                pending_request_id: None,
                device_auth: None,
            })
            .await?;

        let phone_link = params
            .phone_network_link
            .unwrap_or(gemacast_core::domain::types::NetworkLink::Unknown);
        let pc_link = response
            .pc_network_link
            .unwrap_or(gemacast_core::domain::types::NetworkLink::Unknown);
        let link_pair = LinkPair {
            phone: phone_link,
            pc: pc_link,
        };
        *self.cached_link_pair.lock().unwrap() = Some(link_pair);

        if let Some(level) = response.pc_output_volume {
            self.apply_pc_output_volume(level).await?;
            self.notifier.emit_pc_volume_changed(level);
        }

        tracing::info!(
            "Network link pair: phone={:?}, pc={:?}, effective={:?}",
            link_pair.phone,
            link_pair.pc,
            link_pair.effective_link()
        );

        let effective_jitter_config = if params.jitter_config.is_auto_sentinel() {
            JitterConfig::for_link_pair(link_pair)
        } else {
            params.jitter_config
        };

        if let Err(error) = self
            .session
            .start_session(SessionParams {
                jitter_config: effective_jitter_config,
                exclusive_mode: params.exclusive_mode,
                target_ip: Some(ip_addr),
                mode: params.mode,
                device_id: params.device_id.to_string(),
                bitrate: params.bitrate,
                network_link: link_pair.effective_link(),
            })
            .await
        {
            let _ = client.disconnect(params.device_id.clone()).await;
            *self.cached_link_pair.lock().unwrap() = None;
            return Err(error);
        }

        self.is_streaming.store(true, Ordering::Relaxed);
        self.platform.set_streaming_flag(true);
        self.platform
            .sync_service(PlaybackState::Playing, params.exclusive_mode);

        Ok(())
    }

    pub async fn disconnect_from_streamer(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
    ) -> Result<(), String> {
        tracing::info!(
            "[AudioService] Disconnect: ip={}, device={:?}",
            ip,
            device_id
        );
        self.stop_link_recovery();

        let client = self.client_factory.create(ip);
        let _ = client.disconnect(device_id).await;

        self.session.stop_session().await;

        *self.cached_link_pair.lock().unwrap() = None;

        self.is_streaming.store(false, Ordering::Relaxed);
        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
        Ok(())
    }

    pub async fn start_audio_playback(&self, _resume: Option<ResumeParams>) -> Result<(), String> {
        tracing::info!("[AudioService] Resume playback");
        self.session.resume_playback().await?;
        let info = self.session.session_info().await;
        let exclusive = info.as_ref().is_some_and(|i| i.exclusive_mode);

        self.platform
            .sync_service(PlaybackState::Playing, exclusive);
        Ok(())
    }

    pub async fn stop_audio_playback(
        &self,
        _ip: Option<IpAddr>,
        _device_id: Option<DeviceId>,
    ) -> Result<(), String> {
        tracing::info!("[AudioService] Pause playback");
        self.session.pause_playback().await?;

        self.platform.sync_service(PlaybackState::Paused, false);
        Ok(())
    }

    pub async fn kill_playback(&self) -> Result<(), String> {
        tracing::warn!("[AudioService] Kill playback (forced teardown)");
        self.stop_link_recovery();
        self.session.stop_session().await;

        *self.cached_link_pair.lock().unwrap() = None;

        self.is_streaming.store(false, Ordering::Relaxed);
        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
        Ok(())
    }

    pub fn notify_streaming_stopped(&self) {
        self.stop_link_recovery();
        self.is_streaming.store(false, Ordering::Relaxed);
        self.platform.set_streaming_flag(false);
        self.platform.sync_service(PlaybackState::Stopped, false);
    }

    pub fn start_link_recovery(&self, ip: IpAddr, device_id: DeviceId) {
        self.start_link_recovery_paced(ip, device_id, RECOVERY_PROBE_INTERVAL, RECOVERY_BUDGET);
    }

    pub fn start_link_recovery_paced(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
        interval: Duration,
        budget: Duration,
    ) {
        self.stop_link_recovery();

        let client = self
            .client_factory
            .create_with_timeout(ip, RECOVERY_PROBE_TIMEOUT);
        let notifier = self.notifier.clone();

        tracing::warn!(
            "[AudioService] Link lost — probing {} every {:?} for up to {:?}",
            ip,
            interval,
            budget,
        );

        let handle = tokio::spawn(async move {
            run_link_recovery(client, device_id, notifier, interval, budget).await;
        });

        *self.recovery_task.lock().unwrap() = Some(handle);
    }

    pub fn stop_link_recovery(&self) {
        if let Some(handle) = self.recovery_task.lock().unwrap().take() {
            handle.abort();
            tracing::info!("[AudioService] Link recovery cancelled");
        }
    }

    pub async fn restart_session(&self, exclusive_mode: bool) -> Result<(), String> {
        let info = self
            .session
            .session_info()
            .await
            .ok_or("No active session to restart")?;

        let jitter_config = if info.jitter_config.is_auto_sentinel() {
            if let Some(pair) = *self.cached_link_pair.lock().unwrap() {
                JitterConfig::for_link_pair(pair)
            } else {
                info.jitter_config
            }
        } else {
            info.jitter_config
        };

        tracing::info!(
            "[AudioService] Restart session: exclusive_mode={} (was {})",
            exclusive_mode,
            info.exclusive_mode,
        );

        self.session
            .start_session(SessionParams {
                jitter_config,
                exclusive_mode,
                target_ip: info.target_ip,
                mode: info.mode,
                device_id: info.device_id,
                bitrate: info.bitrate,
                network_link: info.network_link,
            })
            .await?;

        self.platform
            .sync_service(PlaybackState::Playing, exclusive_mode);

        Ok(())
    }

    pub async fn update_jitter_config(&self, config: JitterConfig) -> Result<(), String> {
        let effective_config = if config.is_auto_sentinel() {
            if let Some(pair) = *self.cached_link_pair.lock().unwrap() {
                tracing::info!(
                    "[AudioService] Jitter config update: Auto sentinel → link-pair override ({:?})",
                    pair.effective_link(),
                );
                JitterConfig::for_link_pair(pair)
            } else {
                tracing::info!(
                    "[AudioService] Jitter config update: Auto sentinel (no cached link pair)"
                );
                config
            }
        } else {
            tracing::info!(
                "[AudioService] Jitter config update: min_depth={}ms, comfort_cap={}ms, static={:?}",
                config.min_depth_ms,
                config.comfort_cap_ms,
                config.static_target_ms,
            );
            config
        };

        self.session.update_jitter_config(effective_config).await;
        Ok(())
    }

    pub async fn set_volume(&self, linear: f32) -> Result<(), String> {
        let effective = {
            let mut mix = self.volume_mix.lock().unwrap();
            mix.user_gain = linear;
            mix.effective()
        };
        self.session.set_volume(effective).await;
        Ok(())
    }

    pub async fn set_match_pc_volume(&self, enabled: bool) -> Result<(), String> {
        let effective = {
            let mut mix = self.volume_mix.lock().unwrap();
            mix.match_pc = enabled;
            mix.effective()
        };
        self.session.set_volume(effective).await;
        Ok(())
    }

    pub async fn apply_pc_output_volume(&self, level: f32) -> Result<(), String> {
        VolumeController::apply_pc_level(&self.volume_mix, self.session.as_ref(), level).await;
        Ok(())
    }

    #[cfg(test)]
    pub fn volume_mix(&self) -> VolumeMix {
        *self.volume_mix.lock().unwrap()
    }

    pub fn get_cached_link_pair(&self) -> Option<LinkPair> {
        *self.cached_link_pair.lock().unwrap()
    }

    pub async fn get_audio_sources(
        &self,
        ip: IpAddr,
    ) -> Result<
        (
            Vec<AudioSource>,
            gemacast_core::domain::types::StreamerCapabilities,
        ),
        String,
    > {
        let client = self.client_factory.create(ip);
        client.get_audio_sources().await
    }

    pub async fn probe_streamer(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
    ) -> Result<gemacast_core::control::types::PresenceResponse, String> {
        let client = self.client_factory.create(ip);
        client.probe(Some(device_id)).await
    }

    pub async fn change_audio_source(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
        source: AudioSource,
    ) -> Result<(), String> {
        let client = self.client_factory.create(ip);
        client.change_source(device_id, source).await
    }

    pub async fn change_audio_bitrate(
        &self,
        ip: IpAddr,
        device_id: DeviceId,
        bitrate: Option<i32>,
    ) -> Result<(), String> {
        let client = self.client_factory.create(ip);
        client.change_bitrate(device_id, bitrate).await?;
        self.session.update_bitrate(bitrate).await;
        Ok(())
    }

    pub async fn get_process_list(
        &self,
        ip: IpAddr,
    ) -> Result<Vec<gemacast_core::domain::types::ProcessInfo>, String> {
        let client = self.client_factory.create(ip);
        client.get_process_list().await
    }

    pub async fn establish_websocket(
        &self,
        streamer_ip: IpAddr,
        device_id: String,
    ) -> Result<(), String> {
        let client_factory = self.client_factory.clone();
        let session = self.session.clone();
        let notifier = self.notifier.clone();
        let volume_mix = self.volume_mix.clone();
        let task = tokio::spawn(async move {
            const RETRY_DELAYS: [std::time::Duration; 4] = [
                std::time::Duration::from_millis(250),
                std::time::Duration::from_millis(500),
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(2),
            ];

            for retry_delay in RETRY_DELAYS
                .into_iter()
                .chain(std::iter::once(std::time::Duration::ZERO))
            {
                if session.session_info().await.is_none() {
                    return;
                }
                let credentials =
                    client_factory.session_credentials(streamer_ip, &DeviceId(device_id.clone()));
                let ws_client = match gemacast_core::control::WsControlClient::new_with_credentials(
                    streamer_ip,
                    &device_id,
                    credentials
                        .as_ref()
                        .map(|credentials| credentials.token.as_str()),
                    credentials
                        .as_ref()
                        .map(|credentials| credentials.pc_certificate_fingerprint.as_str()),
                )
                .await
                {
                    Ok(client) => client,
                    Err(error) => {
                        tracing::warn!("WebSocket connection failed: {error}");
                        if retry_delay.is_zero() {
                            return;
                        }
                        tokio::time::sleep(retry_delay).await;
                        continue;
                    }
                };

                loop {
                    match ws_client.recv_event().await {
                        Ok(gemacast_core::control::types::WsEvent::HeartbeatAcknowledged) => {
                            session.record_control_heartbeat().await;
                        }
                        Ok(gemacast_core::control::types::WsEvent::Disconnect) => {
                            notifier.emit_ws_disconnect();
                            return;
                        }
                        Ok(gemacast_core::control::types::WsEvent::Error { message }) => {
                            notifier.emit_ws_error(message);
                            notifier.emit_ws_disconnect();
                            return;
                        }
                        Ok(gemacast_core::control::types::WsEvent::VolumeChanged { level }) => {
                            VolumeController::apply_pc_level(&volume_mix, session.as_ref(), level)
                                .await;
                            notifier.emit_pc_volume_changed(level);
                        }
                        Err(error) => {
                            tracing::warn!("WebSocket control channel dropped: {error}");
                            if retry_delay.is_zero() {
                                return;
                            }
                            tokio::time::sleep(retry_delay).await;
                            break;
                        }
                    }
                }
            }
        });

        self.session.start_ws_client(task).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;
    use crate::traits::SessionInfo;

    fn make_service(
        session: Arc<MockSessionManager>,
        client: Arc<MockStreamerControlClient>,
        platform: Arc<MockPlatformService>,
    ) -> AudioService {
        make_service_with_notifier(
            session,
            client,
            platform,
            Arc::new(MockFrontendNotifier::new()),
        )
    }

    fn make_service_with_notifier(
        session: Arc<MockSessionManager>,
        client: Arc<MockStreamerControlClient>,
        platform: Arc<MockPlatformService>,
        notifier: Arc<MockFrontendNotifier>,
    ) -> AudioService {
        let factory = Arc::new(MockStreamerControlClientFactory::new(client));
        AudioService::new(
            session,
            factory,
            notifier,
            platform,
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[tokio::test]
    async fn connect_should_send_http_then_start_session() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .connect_to_streamer(ConnectParams {
                ip: "192.168.1.5".to_string(),
                device_id: DeviceId("phone-1".into()),
                device_name: "My Phone".into(),
                mode: ConnectionMode::Wifi,
                exclusive_mode: false,
                jitter_config: JitterConfig::default(),
                bitrate: None,
                phone_network_link: None,
            })
            .await
            .unwrap();

        let client_calls = client.take_calls();
        assert_eq!(client_calls.len(), 1);
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::Connect { device_id } if device_id.0 == "phone-1"
        ));

        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StartSession { .. }))
        );

        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: true }))
        );
        assert!(platform_calls.iter().any(|c| matches!(
            c,
            PlatformCall::SyncService {
                state: PlaybackState::Playing,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn connect_should_disconnect_streamer_when_local_playback_start_fails() {
        let session =
            Arc::new(MockSessionManager::new().with_start_error("audio output failed".into()));
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session, client.clone(), platform);

        let result = service
            .connect_to_streamer(ConnectParams {
                ip: "192.168.1.5".to_string(),
                device_id: DeviceId("phone-1".into()),
                device_name: "My Phone".into(),
                mode: ConnectionMode::Wifi,
                exclusive_mode: false,
                jitter_config: JitterConfig::default(),
                bitrate: Some(128000),
                phone_network_link: None,
            })
            .await;

        assert_eq!(result.unwrap_err(), "audio output failed");
        assert!(client.take_calls().iter().any(|call| matches!(
            call,
            ControlClientCall::Disconnect { device_id } if device_id.0 == "phone-1"
        )));
        assert!(!service.is_streaming.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn disconnect_should_stop_session_and_sync() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .disconnect_from_streamer("192.168.1.5".parse().unwrap(), DeviceId("phone-1".into()))
            .await
            .unwrap();

        let client_calls = client.take_calls();
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::Disconnect { device_id } if device_id.0 == "phone-1"
        ));

        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StopSession))
        );

        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false }))
        );
    }

    #[tokio::test]
    async fn start_playback_should_call_resume_playback() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.start_audio_playback(None).await.unwrap();

        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::ResumePlayback))
        );
    }

    #[tokio::test]
    async fn start_playback_should_not_send_http_reconnect() {
        let session = Arc::new(MockSessionManager::new().with_session_info(SessionInfo {
            exclusive_mode: false,
            exclusive_granted: false,
            mode: ConnectionMode::Wifi,
            bitrate: Some(128000),
            jitter_config: JitterConfig::default(),
            target_ip: Some("192.168.1.5".parse().unwrap()),
            device_id: "phone-1".into(),
            network_link: gemacast_core::domain::types::NetworkLink::Unknown,
        }));
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .start_audio_playback(Some(ResumeParams {
                ip: "192.168.1.5".parse().unwrap(),
                device_id: DeviceId("phone-1".into()),
                device_name: "My Phone".into(),
            }))
            .await
            .unwrap();

        let client_calls = client.take_calls();
        assert_eq!(client_calls.len(), 0);
    }

    #[tokio::test]
    async fn stop_playback_should_pause_not_stop_session() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .stop_audio_playback(
                Some("192.168.1.5".parse().unwrap()),
                Some(DeviceId("phone-1".into())),
            )
            .await
            .unwrap();

        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::PausePlayback))
        );
        assert!(
            !session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StopSession))
        );

        let client_calls = client.take_calls();
        assert_eq!(client_calls.len(), 0);

        let platform_calls = platform.take_calls();
        assert!(platform_calls.iter().any(|c| matches!(
            c,
            PlatformCall::SyncService {
                state: PlaybackState::Paused,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn kill_playback_should_stop_everything() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.kill_playback().await.unwrap();

        let session_calls = session.take_calls();
        assert!(
            session_calls
                .iter()
                .any(|c| matches!(c, SessionCall::StopSession))
        );

        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false }))
        );
        assert!(platform_calls.iter().any(|c| matches!(
            c,
            PlatformCall::SyncService {
                state: PlaybackState::Stopped,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn notify_streaming_stopped_should_sync_the_service_to_stopped() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service.notify_streaming_stopped();

        let platform_calls = platform.take_calls();
        assert!(
            platform_calls
                .iter()
                .any(|c| matches!(c, PlatformCall::SetStreamingFlag { active: false })),
            "the streaming flag must still be cleared"
        );
        assert!(
            platform_calls.iter().any(|c| matches!(
                c,
                PlatformCall::SyncService {
                    state: PlaybackState::Stopped,
                    ..
                }
            )),
            "notify_streaming_stopped must sync the service, got {platform_calls:?}"
        );
        assert!(!service.is_streaming.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn every_stop_path_should_sync_the_service() {
        fn synced_stopped(calls: &[PlatformCall]) -> bool {
            calls.iter().any(|c| {
                matches!(
                    c,
                    PlatformCall::SyncService {
                        state: PlaybackState::Stopped,
                        ..
                    }
                )
            })
        }

        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(
            Arc::new(MockSessionManager::new()),
            Arc::new(MockStreamerControlClient::new()),
            platform.clone(),
        );
        service
            .disconnect_from_streamer("192.168.1.5".parse().unwrap(), DeviceId("phone-1".into()))
            .await
            .unwrap();
        assert!(
            synced_stopped(&platform.take_calls()),
            "disconnect_from_streamer must sync the service"
        );

        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(
            Arc::new(MockSessionManager::new()),
            Arc::new(MockStreamerControlClient::new()),
            platform.clone(),
        );
        service.kill_playback().await.unwrap();
        assert!(
            synced_stopped(&platform.take_calls()),
            "kill_playback must sync the service"
        );

        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(
            Arc::new(MockSessionManager::new()),
            Arc::new(MockStreamerControlClient::new()),
            platform.clone(),
        );
        service.notify_streaming_stopped();
        assert!(
            synced_stopped(&platform.take_calls()),
            "notify_streaming_stopped must sync the service"
        );
    }

    #[tokio::test]
    async fn change_bitrate_should_update_session_and_send_http() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        service
            .change_audio_bitrate(
                "192.168.1.5".parse().unwrap(),
                DeviceId("phone-1".into()),
                Some(256000),
            )
            .await
            .unwrap();

        let session_calls = session.take_calls();
        assert!(session_calls.iter().any(|c| matches!(
            c,
            SessionCall::UpdateBitrate {
                bitrate: Some(256000)
            }
        )));

        let client_calls = client.take_calls();
        assert!(matches!(
            &client_calls[0],
            ControlClientCall::ChangeBitrate {
                bitrate: Some(256000),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn failed_bitrate_change_should_preserve_session_bitrate() {
        let session = Arc::new(MockSessionManager::new());
        let client =
            Arc::new(MockStreamerControlClient::new().with_change_bitrate_error("rejected".into()));
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client, platform);

        let result = service
            .change_audio_bitrate(
                "192.168.1.5".parse().unwrap(),
                DeviceId("phone-1".into()),
                Some(256000),
            )
            .await;

        assert_eq!(result.unwrap_err(), "rejected");
        assert!(
            !session
                .take_calls()
                .iter()
                .any(|call| matches!(call, SessionCall::UpdateBitrate { .. }))
        );
    }

    #[tokio::test]
    async fn update_jitter_config_should_reapply_cached_link_pair_for_auto_sentinel() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        let pair = LinkPair {
            phone: NetworkLink::Wifi5Ghz,
            pc: NetworkLink::Wifi5Ghz,
        };
        *service.cached_link_pair.lock().unwrap() = Some(pair);

        let auto_config = JitterConfig {
            min_depth_ms: 25,
            comfort_cap_ms: 1000,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.25,
            static_target_ms: None,
        };

        service.update_jitter_config(auto_config).await.unwrap();

        let calls = session.take_calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, SessionCall::UpdateJitterConfig)),
            "Expected UpdateJitterConfig call"
        );
    }

    #[tokio::test]
    async fn update_jitter_config_should_passthrough_non_auto_config() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        let pair = LinkPair {
            phone: NetworkLink::Wifi5Ghz,
            pc: NetworkLink::Wifi5Ghz,
        };
        *service.cached_link_pair.lock().unwrap() = Some(pair);

        let balanced_config = JitterConfig {
            min_depth_ms: 10,
            comfort_cap_ms: 200,
            peak_decay_halflife_ms: 3500,
            resume_threshold_pct: 0.75,
            static_target_ms: None,
        };

        service.update_jitter_config(balanced_config).await.unwrap();

        let calls = session.take_calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, SessionCall::UpdateJitterConfig))
        );
    }

    #[tokio::test]
    async fn update_jitter_config_should_passthrough_auto_when_no_cache() {
        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        assert!(service.cached_link_pair.lock().unwrap().is_none());

        let auto_config = JitterConfig {
            min_depth_ms: 25,
            comfort_cap_ms: 1000,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.25,
            static_target_ms: None,
        };

        service.update_jitter_config(auto_config).await.unwrap();

        let calls = session.take_calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, SessionCall::UpdateJitterConfig))
        );
    }

    #[tokio::test]
    async fn disconnect_should_clear_cached_link_pair() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        *service.cached_link_pair.lock().unwrap() = Some(LinkPair {
            phone: NetworkLink::Wifi5Ghz,
            pc: NetworkLink::Ethernet,
        });

        service
            .disconnect_from_streamer("192.168.1.5".parse().unwrap(), DeviceId("phone-1".into()))
            .await
            .unwrap();

        assert!(service.cached_link_pair.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn kill_playback_should_clear_cached_link_pair() {
        use gemacast_core::domain::types::NetworkLink;

        let session = Arc::new(MockSessionManager::new());
        let client = Arc::new(MockStreamerControlClient::new());
        let platform = Arc::new(MockPlatformService::new());
        let service = make_service(session.clone(), client.clone(), platform.clone());

        *service.cached_link_pair.lock().unwrap() = Some(LinkPair {
            phone: NetworkLink::UsbTether,
            pc: NetworkLink::Ethernet,
        });

        service.kill_playback().await.unwrap();

        assert!(service.cached_link_pair.lock().unwrap().is_none());
    }

    mod link_recovery {
        use super::*;

        const IP: &str = "192.168.1.5";
        const INTERVAL: Duration = Duration::from_secs(2);
        const BUDGET: Duration = Duration::from_secs(60);

        fn probe_count(calls: &[ControlClientCall]) -> usize {
            calls
                .iter()
                .filter(|c| matches!(c, ControlClientCall::Probe { .. }))
                .count()
        }

        async fn drain(service: &AudioService) {
            let handle = service.recovery_task.lock().unwrap().take();
            if let Some(handle) = handle {
                let _ = handle.await;
            }
        }

        #[tokio::test(start_paused = true)]
        async fn link_recovery_should_report_the_pc_registration_once_it_answers() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(
                MockStreamerControlClient::new()
                    .with_probe_failures(3)
                    .with_probe_registration(Some(true)),
            );
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service =
                make_service_with_notifier(session, client.clone(), platform, notifier.clone());

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            assert_eq!(probe_count(&client.take_calls()), 4);
            assert!(matches!(
                notifier.take_events().as_slice(),
                [FrontendEvent::LinkRecovered {
                    device_registered: Some(true)
                }]
            ));
        }

        #[tokio::test(start_paused = true)]
        async fn a_recovery_probe_should_carry_our_device_id() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockStreamerControlClient::new());
            let platform = Arc::new(MockPlatformService::new());
            let service = make_service(session, client.clone(), platform);

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            let calls = client.take_calls();
            assert!(matches!(
                calls.as_slice(),
                [ControlClientCall::Probe { device_id: Some(id) }] if id.0 == "phone-1"
            ));
        }

        #[tokio::test(start_paused = true)]
        async fn link_recovery_should_give_up_once_its_budget_is_spent() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockStreamerControlClient::new().with_unreachable_probe());
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service =
                make_service_with_notifier(session, client.clone(), platform, notifier.clone());

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            assert_eq!(probe_count(&client.take_calls()), 30);
            assert!(matches!(
                notifier.take_events().as_slice(),
                [FrontendEvent::LinkRecoveryGaveUp]
            ));
        }

        #[tokio::test(start_paused = true)]
        async fn a_forced_teardown_should_cancel_link_recovery() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockStreamerControlClient::new().with_unreachable_probe());
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service =
                make_service_with_notifier(session, client.clone(), platform, notifier.clone());

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );

            tokio::time::sleep(INTERVAL * 3).await;
            let probes_before = probe_count(&client.take_calls());
            assert!(probes_before > 0);

            service.kill_playback().await.unwrap();

            tokio::time::sleep(BUDGET * 2).await;
            assert_eq!(probe_count(&client.take_calls()), 0);
            assert!(
                !notifier
                    .take_events()
                    .iter()
                    .any(|e| matches!(e, FrontendEvent::LinkRecoveryGaveUp)),
            );
            assert!(service.recovery_task.lock().unwrap().is_none());
        }

        #[tokio::test(start_paused = true)]
        async fn a_second_link_loss_should_not_stack_a_second_prober() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockStreamerControlClient::new().with_unreachable_probe());
            let platform = Arc::new(MockPlatformService::new());
            let service = make_service(session, client.clone(), platform);

            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            service.start_link_recovery_paced(
                IP.parse().unwrap(),
                DeviceId("phone-1".into()),
                INTERVAL,
                BUDGET,
            );
            drain(&service).await;

            assert_eq!(probe_count(&client.take_calls()), 30);
        }
    }

    mod pc_volume_matching {
        use super::*;

        fn volumes(calls: &[SessionCall]) -> Vec<f32> {
            calls
                .iter()
                .filter_map(|c| match c {
                    SessionCall::SetVolume { linear } => Some(*linear),
                    _ => None,
                })
                .collect()
        }

        fn service() -> (Arc<MockSessionManager>, AudioService) {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockStreamerControlClient::new());
            let platform = Arc::new(MockPlatformService::new());
            let service = make_service(session.clone(), client, platform);
            (session, service)
        }

        #[tokio::test]
        async fn the_user_gain_reaches_the_session_untouched_while_matching_is_off() {
            let (session, service) = service();

            service.set_volume(0.5).await.unwrap();

            assert_eq!(volumes(&session.take_calls()), vec![0.5]);
        }

        #[tokio::test]
        async fn a_pc_level_arriving_while_matching_is_off_is_remembered_but_not_applied() {
            let (session, service) = service();

            service.set_volume(1.0).await.unwrap();
            session.take_calls();
            service.apply_pc_output_volume(0.25).await.unwrap();

            assert!(volumes(&session.take_calls()).is_empty());
            assert_eq!(service.volume_mix().pc_level, Some(0.25));
        }

        #[tokio::test]
        async fn turning_matching_on_applies_the_remembered_pc_level_immediately() {
            let (session, service) = service();

            service.set_volume(1.0).await.unwrap();
            service.apply_pc_output_volume(0.25).await.unwrap();
            session.take_calls();
            service.set_match_pc_volume(true).await.unwrap();

            assert_eq!(volumes(&session.take_calls()), vec![0.25]);
        }

        #[tokio::test]
        async fn a_pc_level_arriving_while_matching_is_on_scales_the_user_gain() {
            let (session, service) = service();

            service.set_volume(0.8).await.unwrap();
            service.set_match_pc_volume(true).await.unwrap();
            session.take_calls();
            service.apply_pc_output_volume(0.5).await.unwrap();

            assert_eq!(volumes(&session.take_calls()), vec![0.4]);
        }

        #[tokio::test]
        async fn turning_matching_off_restores_the_unscaled_user_gain() {
            let (session, service) = service();

            service.set_volume(0.8).await.unwrap();
            service.apply_pc_output_volume(0.5).await.unwrap();
            service.set_match_pc_volume(true).await.unwrap();
            session.take_calls();
            service.set_match_pc_volume(false).await.unwrap();

            assert_eq!(volumes(&session.take_calls()), vec![0.8]);
        }

        async fn connect(service: &AudioService) {
            service
                .connect_to_streamer(ConnectParams {
                    ip: "192.168.1.5".to_string(),
                    device_id: DeviceId("phone-1".into()),
                    device_name: "My Phone".into(),
                    mode: ConnectionMode::Wifi,
                    exclusive_mode: false,
                    jitter_config: JitterConfig::default(),
                    bitrate: None,
                    phone_network_link: None,
                })
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn the_handshake_seeds_the_pc_level_and_announces_it_to_the_frontend() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockStreamerControlClient::new().with_pc_output_volume(0.35));
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service = make_service_with_notifier(session, client, platform, notifier.clone());

            connect(&service).await;

            assert_eq!(service.volume_mix().pc_level, Some(0.35));
            assert!(
                notifier
                    .take_events()
                    .iter()
                    .any(|e| matches!(e, FrontendEvent::PcVolumeChanged(level) if *level == 0.35))
            );
        }

        #[tokio::test]
        async fn a_streamer_that_reports_no_pc_level_leaves_the_mix_unseeded() {
            let session = Arc::new(MockSessionManager::new());
            let client = Arc::new(MockStreamerControlClient::new());
            let platform = Arc::new(MockPlatformService::new());
            let notifier = Arc::new(MockFrontendNotifier::new());
            let service = make_service_with_notifier(session, client, platform, notifier.clone());

            connect(&service).await;

            assert_eq!(service.volume_mix().pc_level, None);
            assert!(
                !notifier
                    .take_events()
                    .iter()
                    .any(|e| matches!(e, FrontendEvent::PcVolumeChanged(_)))
            );
        }

        #[tokio::test]
        async fn moving_the_gain_slider_while_matching_keeps_the_pc_scaling() {
            let (session, service) = service();

            service.apply_pc_output_volume(0.5).await.unwrap();
            service.set_match_pc_volume(true).await.unwrap();
            session.take_calls();
            service.set_volume(0.6).await.unwrap();

            assert_eq!(volumes(&session.take_calls()), vec![0.3]);
        }
    }
}
