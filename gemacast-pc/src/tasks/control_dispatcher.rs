use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gemacast_core::audio::AudioSessionKey;
use gemacast_core::control::messages::ControlMessage;
use gemacast_core::control::types::PresenceResponse;
use gemacast_core::control::{ControlCommand, SessionAuthorizer};
use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::device_auth::{DeviceAuthManager, VerifiedDeviceIdentity};
use crate::traits::{
    AudioController, ConnectionApprovalRequest, DeviceNotifier, DeviceRegistry,
    RegistrationOutcome, TrayNotifier,
};
use crate::trusted_devices::TrustedDeviceStore;

pub struct ControlDispatcher {
    pub registry: Arc<dyn DeviceRegistry>,
    pub tray: Arc<dyn TrayNotifier>,
    pub audio: Arc<dyn AudioController>,
    pub notifier: Arc<dyn DeviceNotifier>,
    pub streamer_id: DeviceId,
    pub streamer_name: String,
    pub pc_certificate_fingerprint: String,
    pub is_broadcasting: Arc<AtomicBool>,
    pub authorizer: SessionAuthorizer,
    pub device_auth: DeviceAuthManager,
    pub trusted_devices: TrustedDeviceStore,
}

impl ControlDispatcher {
    pub async fn handle_command(&self, cmd: ControlCommand) {
        match cmd {
            ControlCommand::Connect {
                device_id,
                device_name,
                source,
                remote_addr,
                bitrate,
                response_tx,
                authorized_generation,
                pending_request_id,
                device_auth,
            } => {
                tracing::info!(
                    "ControlCommand::Connect from {:?} at {}",
                    device_id.clone(),
                    remote_addr
                );
                let mut audio_addr = remote_addr;
                audio_addr.set_port(gemacast_core::network::Ports::AUDIO_UDP);

                let (approved_request_id, identity_to_trust) =
                    if let Some(generation) = authorized_generation {
                        if !self.authorizer.is_current(&device_id, generation) {
                            let _ = response_tx.send(Err("session is no longer active".into()));
                            return;
                        }
                        (None, None)
                    } else if let Some(request_id) = pending_request_id {
                        let auth = match device_auth.as_ref() {
                            Some(auth) => auth,
                            None => {
                                let _ = response_tx
                                    .send(Err("LAN device authentication proof is missing".into()));
                                return;
                                }
                        };
                        let identity = match self.device_auth.pending_pairing(
                            &request_id,
                            &device_id,
                            &auth.public_key,
                        ) {
                            Some(identity) => identity,
                            None => {
                                let _ = response_tx.send(Err(format!(
                                    "connection request {request_id} does not match this device key"
                                )));
                                return;
                            }
                        };
                        match self.authorizer.pending_status(&request_id, &device_id) {
                            Some(gemacast_core::control::PendingApprovalStatus::Pending) => {
                                let _ = response_tx.send(Ok(PresenceResponse {
                                    device_id: self.streamer_id.clone(),
                                    streamer_name: self.streamer_name.clone(),
                                    is_offline: false,
                                    pc_network_link: None,
                                    device_registered: Some(false),
                                    session_token: None,
                                    session_generation: None,
                                    pending_request_id: Some(request_id),
                                    device_auth_challenge: None,
                                    pc_certificate_fingerprint: Some(
                                        self.pc_certificate_fingerprint.clone(),
                                    ),
                                    pc_output_volume: read_pc_output_volume(),
                                }));
                                return;
                            }
                            Some(gemacast_core::control::PendingApprovalStatus::Approved) => {
                                (Some(request_id), Some(identity))
                            }
                            Some(gemacast_core::control::PendingApprovalStatus::Rejected) => {
                                self.authorizer.remove_pending(&request_id);
                                self.device_auth.remove_pending_pairing(&request_id);
                                let _ = response_tx.send(Err(format!(
                                    "connection request {request_id} was rejected on the PC"
                                )));
                                return;
                            }
                            None => {
                                let _ = response_tx.send(Err(format!(
                                    "connection request {request_id} is invalid or expired"
                                )));
                                return;
                            }
                        }
                    } else {
                        let Some(auth) = device_auth else {
                            let _ = response_tx
                                .send(Err("LAN device authentication identity is missing".into()));
                            return;
                        };
                        if auth.challenge_id.is_none() && auth.signature.is_none() {
                            let requires_approval = !self
                                .trusted_devices
                                .is_trusted(&device_id, &auth.public_key);
                            let challenge = match self.device_auth.begin(
                                device_id.clone(),
                                self.streamer_id.clone(),
                                self.pc_certificate_fingerprint.clone(),
                                requires_approval,
                                auth.public_key,
                                auth.phone_nonce,
                            ) {
                                Ok(challenge) => challenge,
                                Err(error) => {
                                    let _ = response_tx.send(Err(error));
                                    return;
                                }
                            };
                            let _ = response_tx.send(Ok(PresenceResponse {
                                device_id: self.streamer_id.clone(),
                                streamer_name: self.streamer_name.clone(),
                                is_offline: false,
                                pc_network_link: None,
                                device_registered: Some(false),
                                session_token: None,
                                session_generation: None,
                                pending_request_id: None,
                                device_auth_challenge: Some(challenge),
                                pc_certificate_fingerprint: Some(
                                    self.pc_certificate_fingerprint.clone(),
                                ),
                                pc_output_volume: read_pc_output_volume(),
                            }));
                            return;
                        }
                        let identity = match self.device_auth.verify(
                            &device_id,
                            &self.streamer_id,
                            &self.pc_certificate_fingerprint,
                            &auth,
                        ) {
                            Ok(identity) => identity,
                            Err(error) => {
                                let _ = response_tx.send(Err(error));
                                return;
                            }
                        };
                        let already_trusted = self
                            .trusted_devices
                            .is_trusted(&device_id, &identity.public_key);
                        if already_trusted && auth.phone_confirmation.is_none() {
                            (None, None)
                        } else {
                            match auth.phone_confirmation {
                                Some(true) => {}
                                Some(false) => {
                                    let _ = response_tx
                                        .send(Err("pairing was cancelled on the phone".into()));
                                    return;
                                }
                                None => {
                                    let _ = response_tx
                                        .send(Err("phone confirmation is required for pairing".into()));
                                    return;
                                }
                            }
                            let replaces_existing_identity = self
                                .trusted_devices
                                .public_key(&device_id)
                                .is_some_and(|trusted_key| trusted_key != identity.public_key);
                            if replaces_existing_identity {
                                tracing::warn!(
                                    ?device_id,
                                    "Device presented a new public key; waiting for explicit re-pair approval"
                                );
                            }
                            let request_id = match self.authorizer.create_pending(device_id.clone()) {
                                Ok(request_id) => request_id,
                                Err(error) => {
                                    let _ = response_tx.send(Err(error));
                                    return;
                                }
                            };
                            if let Err(error) = self.device_auth.hold_pending_pairing(
                                request_id.clone(),
                                device_id.clone(),
                                identity.clone(),
                            ) {
                                self.authorizer.remove_pending(&request_id);
                                let _ = response_tx.send(Err(error));
                                return;
                            }
                            let tray = self.tray.clone();
                            let authorizer = self.authorizer.clone();
                            let approval_device_id = device_id.clone();
                            let approval_device_name = device_name.clone();
                            let approval_request_id = request_id.clone();
                            let key_fingerprint = identity.fingerprint.clone();
                            let pairing_code = identity.pairing_code.clone();
                            tokio::spawn(async move {
                                let approved = tray
                                    .request_connection_approval(ConnectionApprovalRequest {
                                        request_id: approval_request_id.clone(),
                                        device_id: approval_device_id,
                                        name: approval_device_name,
                                        addr: remote_addr,
                                        key_fingerprint,
                                        pairing_code,
                                        replaces_existing_identity,
                                    })
                                    .await;
                                authorizer.resolve_pending(&approval_request_id, approved);
                            });
                            let _ = response_tx.send(Ok(PresenceResponse {
                                device_id: self.streamer_id.clone(),
                                streamer_name: self.streamer_name.clone(),
                                is_offline: false,
                                pc_network_link: None,
                                device_registered: Some(false),
                                session_token: None,
                                session_generation: None,
                                pending_request_id: Some(request_id),
                                device_auth_challenge: None,
                                pc_certificate_fingerprint: Some(
                                    self.pc_certificate_fingerprint.clone(),
                                ),
                                pc_output_volume: read_pc_output_volume(),
                            }));
                            return;
                        }
                };

                let pending_session = match self.authorizer.prepare(device_id.clone()) {
                    Ok(session) => session,
                    Err(error) => {
                        let _ = response_tx.send(Err(error));
                        return;
                    }
                };
                let generation = pending_session.generation().0;
                let audio_key = pending_session.audio_key();
                let result = register_device_with_audio_key(
                    self.registry.as_ref(),
                    self.tray.as_ref(),
                    self.audio.as_ref(),
                    device_id.clone(),
                    generation,
                    device_name.clone(),
                    audio_addr,
                    remote_addr,
                    None,
                    source,
                    bitrate,
                    audio_key,
                )
                .await;

                let result = match result {
                    Err(error) => Err(error),
                    Ok(()) => {
                        if let Some(VerifiedDeviceIdentity { public_key, .. }) = identity_to_trust
                            && let Err(error) = self.trusted_devices.trust(
                                device_id.clone(),
                                device_name,
                                public_key,
                            )
                        {
                            let _ = unregister_device(
                                self.registry.as_ref(),
                                self.tray.as_ref(),
                                self.audio.as_ref(),
                                self.notifier.as_ref(),
                                device_id.clone(),
                            )
                            .await;
                            Err(format!("failed to remember the approved device: {error}"))
                        } else {
                            self.authorizer.commit(pending_session).map(
                                |(session_token, session_generation)| PresenceResponse {
                                    device_id: self.streamer_id.clone(),
                                    streamer_name: self.streamer_name.clone(),
                                    is_offline: false,
                                    pc_network_link: None,
                                    device_registered: Some(true),
                                    session_token: Some(session_token),
                                    session_generation: Some(session_generation),
                                    pending_request_id: approved_request_id.clone(),
                                    device_auth_challenge: None,
                                    pc_certificate_fingerprint: Some(
                                        self.pc_certificate_fingerprint.clone(),
                                    ),
                                    pc_output_volume: read_pc_output_volume(),
                                },
                            )
                        }
                    }
                };
                if let Some(request_id) = approved_request_id {
                    self.authorizer.remove_pending(&request_id);
                    self.device_auth.remove_pending_pairing(&request_id);
                }
                let _ = response_tx.send(result);
            }
            ControlCommand::Disconnect {
                device_id,
                remote_addr: _,
                generation,
                response_tx,
            } => {
                tracing::info!("ControlCommand::Disconnect from {:?}", device_id);
                if generation
                    .is_some_and(|generation| !self.authorizer.is_current(&device_id, generation))
                {
                    let _ = response_tx.send(Ok(()));
                    return;
                }
                let result = unregister_device(
                    self.registry.as_ref(),
                    self.tray.as_ref(),
                    self.audio.as_ref(),
                    self.notifier.as_ref(),
                    device_id.clone(),
                )
                .await;
                if result.is_ok() {
                    self.authorizer.revoke(&device_id, generation);
                }
                let _ = response_tx.send(result);
            }
            ControlCommand::GetSources { response_tx } => {
                let (sources, caps) = get_platform_sources();

                let _ = response_tx.send(gemacast_core::control::types::SourcesResponse {
                    sources,
                    capabilities: caps,
                });
            }
            ControlCommand::ChangeSource {
                device_id,
                generation,
                source,
                response_tx,
            } => {
                tracing::info!(
                    "ControlCommand::ChangeSource for {:?} to {:?}",
                    device_id,
                    source
                );
                if !self.authorizer.is_current(&device_id, generation) {
                    let _ = response_tx.send(Err("session is no longer active".into()));
                    return;
                }
                let _ = response_tx.send(self.audio.change_source(device_id, source).await);
            }
            ControlCommand::ChangeBitrate {
                device_id,
                generation,
                bitrate,
                response_tx,
            } => {
                tracing::info!(
                    "ControlCommand::ChangeBitrate for {:?} to {:?}",
                    device_id,
                    bitrate
                );
                if !self.authorizer.is_current(&device_id, generation) {
                    let _ = response_tx.send(Err("session is no longer active".into()));
                    return;
                }
                let _ = response_tx.send(self.audio.change_bitrate(device_id, bitrate).await);
            }
            ControlCommand::SessionHeartbeat {
                device_id,
                generation,
            } => {
                if self.authorizer.is_current(&device_id, generation) {
                    let _ = self.registry.update_last_seen(&device_id);
                }
            }
            ControlCommand::Probe {
                device_id,
                response_tx,
            } => {
                let device_registered = device_id.map(|id| self.registry.update_last_seen(&id));

                let _ = response_tx.send(PresenceResponse {
                    device_id: self.streamer_id.clone(),
                    streamer_name: self.streamer_name.clone(),
                    is_offline: !self.is_broadcasting.load(Ordering::Relaxed),
                    pc_network_link: None,
                    device_registered,
                    session_token: None,
                    session_generation: None,
                    pending_request_id: None,
                    device_auth_challenge: None,
                    pc_certificate_fingerprint: Some(self.pc_certificate_fingerprint.clone()),
                    pc_output_volume: read_pc_output_volume(),
                });
            }
        }
    }
}

pub fn spawn_control_dispatcher(
    set: &mut JoinSet<()>,
    mut inbound_control_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    mut control_command_rx: mpsc::Receiver<ControlCommand>,
    dispatcher: Arc<ControlDispatcher>,
    registry_for_probes: Arc<dyn DeviceRegistry>,
) {
    set.spawn(async move {
        while let Some((message, _remote_addr)) = inbound_control_rx.recv().await {
            if let ControlMessage::Probe {
                device_id: Some(id),
            } = message
            {
                let _ = registry_for_probes.update_last_seen(&id);
            }
        }
    });

    set.spawn(async move {
        while let Some(cmd) = control_command_rx.recv().await {
            dispatcher.handle_command(cmd).await;
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub async fn register_device_with_audio_key(
    registry: &dyn DeviceRegistry,
    tray: &dyn TrayNotifier,
    audio: &dyn AudioController,
    device_id: DeviceId,
    generation: u64,
    device_name: String,
    audio_addr: SocketAddr,
    _remote_addr: SocketAddr,
    transport: Option<gemacast_core::domain::types::TransportType>,
    source: Option<gemacast_core::domain::types::AudioSource>,
    bitrate: Option<i32>,
    audio_key: AudioSessionKey,
) -> Result<(), String> {
    tracing::debug!(
        "Registering device: {} ({:?}) at {}",
        device_name,
        device_id,
        audio_addr
    );

    // Wi-Fi and USB tethering both use the encrypted UDP audio transport.
    // Loopback is a valid UDP address in local tests; it is not a TCP/ADB fallback.
    let effective_addr = Some(audio_addr);

    if let Err(error) = audio
        .subscribe(
            device_id.clone(),
            generation,
            effective_addr,
            source,
            bitrate,
            audio_key,
        )
        .await
    {
        tracing::warn!("Audio subscription failed: {error}");
        return Err(error);
    }

    let device = DiscoveredDevice::from_presence(
        device_id.clone(),
        device_name.clone(),
        false,
        audio_addr,
        transport,
    );
    match registry.register(device) {
        RegistrationOutcome::AddressChanged { old_addr } => {
            tray.notify_device_lost(device_id.clone(), old_addr);
            tray.notify_device_discovered(device_id, device_name, audio_addr, transport);
        }
        RegistrationOutcome::NewDevice => {
            tray.notify_device_discovered(device_id, device_name, audio_addr, transport);
        }
        RegistrationOutcome::AlreadyRegistered => {}
    }
    Ok(())
}

pub async fn unregister_device(
    registry: &dyn DeviceRegistry,
    tray: &dyn TrayNotifier,
    audio: &dyn AudioController,
    notifier: &dyn DeviceNotifier,
    device_id: DeviceId,
) -> Result<(), String> {
    tracing::debug!("Unregistering device: {:?}", device_id);

    if let Some(removed) = registry.unregister(&device_id) {
        tray.notify_device_lost(device_id.clone(), removed.addr);
        notifier
            .notify_disconnect(&device_id, Some(removed.addr))
            .await;
    }
    audio.unsubscribe(&device_id).await
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub async fn register_device(
    registry: &dyn DeviceRegistry,
    tray: &dyn TrayNotifier,
    audio: &dyn AudioController,
    device_id: DeviceId,
    generation: u64,
    device_name: String,
    audio_addr: SocketAddr,
    remote_addr: SocketAddr,
    transport: Option<gemacast_core::domain::types::TransportType>,
    source: Option<gemacast_core::domain::types::AudioSource>,
    bitrate: Option<i32>,
) -> Result<(), String> {
    let test_key = AudioSessionKey::from_session_token(&"11".repeat(32)).unwrap();
    register_device_with_audio_key(
        registry, tray, audio, device_id, generation, device_name, audio_addr,
        remote_addr, transport, source, bitrate, test_key,
    ).await
}

fn read_pc_output_volume() -> Option<f32> {
    #[cfg(not(target_os = "android"))]
    {
        use gemacast_core::ports::output_volume::OutputVolumeReader;
        gemacast_core::adapters::output_volume::PlatformOutputVolumeReader::new().read()
    }
    #[cfg(target_os = "android")]
    {
        None
    }
}

fn platform_supports_process_capture() -> bool {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        true
    }
    #[cfg(target_os = "linux")]
    {
        gemacast_core::adapters::capture::pipewire_common::is_pipewire_available()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

fn get_platform_sources() -> (
    Vec<gemacast_core::domain::types::AudioSource>,
    gemacast_core::domain::types::StreamerCapabilities,
) {
    let supports_volume_sync = {
        #[cfg(not(target_os = "android"))]
        {
            gemacast_core::adapters::output_volume::PlatformOutputVolumeReader::new().is_supported()
        }
        #[cfg(target_os = "android")]
        {
            false
        }
    };

    (
        vec![gemacast_core::domain::types::AudioSource::Desktop],
        gemacast_core::domain::types::StreamerCapabilities {
            supports_process_capture: platform_supports_process_capture(),
            supports_volume_sync,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;
    use base64::Engine;
    use gemacast_core::control::device_auth::build_device_auth_transcript;
    use gemacast_core::control::types::DeviceAuthRequest;
    use ring::rand::SystemRandom;
    use ring::signature::{EcdsaKeyPair, KeyPair};

    fn make_addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn register_should_notify_tray_for_new_device() {
        let registry = MockDeviceRegistry::new();
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            1,
            "My Phone".into(),
            make_addr("192.168.1.5:9000"),
            make_addr("192.168.1.5:23559"),
            None,
            None,
            None,
        )
        .await;

        let tray_calls = tray.take_calls();
        assert_eq!(tray_calls.len(), 1);
        assert!(
            matches!(&tray_calls[0], TrayCall::Discovered { device_id, name, .. } if device_id.0 == "phone-1" && name == "My Phone")
        );

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(
            matches!(&audio_calls[0], AudioCall::Subscribe { device_id, .. } if device_id.0 == "phone-1")
        );
    }

    #[tokio::test]
    async fn register_should_handle_ip_change_correctly() {
        let registry = MockDeviceRegistry::with_device("phone-1", "192.168.1.1:9000");
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            1,
            "My Phone".into(),
            make_addr("192.168.1.2:9000"),
            make_addr("192.168.1.2:23559"),
            None,
            None,
            None,
        )
        .await;

        let tray_calls = tray.take_calls();
        assert_eq!(tray_calls.len(), 2);
        assert!(
            matches!(&tray_calls[0], TrayCall::Lost { device_id, addr } if device_id.0 == "phone-1" && *addr == make_addr("192.168.1.1:9000"))
        );
        assert!(
            matches!(&tray_calls[1], TrayCall::Discovered { device_id, .. } if device_id.0 == "phone-1")
        );

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(matches!(
            &audio_calls[0],
            AudioCall::Subscribe { device_id, .. } if device_id.0 == "phone-1"
        ));
    }

    #[tokio::test]
    async fn register_should_use_udp_addr_for_loopback() {
        let registry = MockDeviceRegistry::new();
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("loopback-dev".into()),
            1,
            "Loopback Phone".into(),
            make_addr("127.0.0.1:9000"),
            make_addr("127.0.0.1:23559"),
            None,
            None,
            None,
        )
        .await;

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(matches!(
            &audio_calls[0],
            AudioCall::Subscribe {
                target_addr: Some(addr),
                ..
            } if *addr == make_addr("127.0.0.1:9000")
        ));
    }

    #[tokio::test]
    async fn register_should_not_re_notify_for_existing_device_at_same_addr() {
        let registry = MockDeviceRegistry::with_device("phone-1", "192.168.1.1:9000");
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            1,
            "My Phone".into(),
            make_addr("192.168.1.1:9000"),
            make_addr("192.168.1.1:23559"),
            None,
            None,
            None,
        )
        .await;

        assert!(tray.take_calls().is_empty());
        assert_eq!(audio.take_calls().len(), 1);
    }

    #[tokio::test]
    async fn unregister_should_notify_tray_and_unsubscribe() {
        let registry = MockDeviceRegistry::with_device("phone-1", "192.168.1.1:9000");
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();
        let notifier = MockDeviceNotifier::new();

        let _ = unregister_device(
            &registry,
            &tray,
            &audio,
            &notifier,
            DeviceId("phone-1".into()),
        )
        .await;

        let tray_calls = tray.take_calls();
        assert_eq!(tray_calls.len(), 1);
        assert!(
            matches!(&tray_calls[0], TrayCall::Lost { device_id, .. } if device_id.0 == "phone-1")
        );

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(
            matches!(&audio_calls[0], AudioCall::Unsubscribe { device_id } if device_id.0 == "phone-1")
        );

        assert!(!registry.contains("phone-1"));
    }

    #[tokio::test]
    async fn unregister_should_do_nothing_for_unknown_device() {
        let registry = MockDeviceRegistry::new();
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();
        let notifier = MockDeviceNotifier::new();

        let _ = unregister_device(
            &registry,
            &tray,
            &audio,
            &notifier,
            DeviceId("ghost".into()),
        )
        .await;

        assert!(tray.take_calls().is_empty());
        assert_eq!(audio.take_calls().len(), 1);
    }

    #[tokio::test]
    async fn approved_lan_request_should_register_and_issue_a_bound_session() {
        let registry = Arc::new(MockDeviceRegistry::new());
        let tray = Arc::new(MockTrayNotifier::new());
        let audio = Arc::new(MockAudioController::new());
        let authorizer = SessionAuthorizer::default();
        let device_auth = DeviceAuthManager::default();
        let trusted_devices = TrustedDeviceStore::in_memory();
        let dispatcher = ControlDispatcher {
            registry: registry.clone(),
            tray,
            audio: audio.clone(),
            notifier: Arc::new(MockDeviceNotifier::new()),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            pc_certificate_fingerprint: "pc-certificate".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer: authorizer.clone(),
            device_auth: device_auth.clone(),
            trusted_devices: trusted_devices.clone(),
        };
        let device_id = DeviceId("phone-1".into());
        let remote_addr = make_addr("192.168.1.5:23559");
        let request_id = authorizer.create_pending(device_id.clone()).unwrap();
        let public_key = "verified-public-key".to_string();
        device_auth
            .hold_pending_pairing(
                request_id.clone(),
                device_id.clone(),
                VerifiedDeviceIdentity {
                    public_key: public_key.clone(),
                    fingerprint: "fingerprint".into(),
                    pairing_code: "123456".into(),
                },
            )
            .unwrap();
        assert!(authorizer.resolve_pending(&request_id, true));

        let (approved_tx, approved_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr,
                bitrate: Some(128_000),
                response_tx: approved_tx,
                authorized_generation: None,
                pending_request_id: Some(request_id.clone()),
                device_auth: Some(gemacast_core::control::types::DeviceAuthRequest {
                    public_key: public_key.clone(),
                    phone_nonce: "nonce".into(),
                    challenge_id: None,
                    signature: None,
                    phone_confirmation: Some(true),
                }),
            })
            .await;
        let approved = approved_rx.await.unwrap().unwrap();
        let token = approved
            .session_token
            .expect("approval must issue a device session token");
        let generation = approved
            .session_generation
            .expect("approval must issue a session generation");

        assert_eq!(approved.device_registered, Some(true));
        assert_eq!(
            approved.pending_request_id.as_deref(),
            Some(request_id.as_str())
        );
        assert!(registry.contains("phone-1"));
        assert!(authorizer.authenticate(&device_id, &token).is_some());
        assert!(authorizer.is_current(&device_id, generation));
        assert!(trusted_devices.is_trusted(&device_id, &public_key));
        assert_eq!(authorizer.pending_status(&request_id, &device_id), None);
        assert!(matches!(
            audio.take_calls().as_slice(),
            [AudioCall::Subscribe { device_id: subscribed, bitrate: Some(128_000), .. }]
                if subscribed == &device_id
        ));
    }

    #[tokio::test]
    async fn trusted_lan_key_should_reconnect_silently_but_phone_repair_requires_approval() {
        let registry = Arc::new(MockDeviceRegistry::new());
        let tray = Arc::new(MockTrayNotifier::new());
        let audio = Arc::new(MockAudioController::new());
        let device_auth = DeviceAuthManager::default();
        let trusted_devices = TrustedDeviceStore::in_memory();
        let dispatcher = ControlDispatcher {
            registry: registry.clone(),
            tray: tray.clone(),
            audio: audio.clone(),
            notifier: Arc::new(MockDeviceNotifier::new()),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            pc_certificate_fingerprint: "pc-certificate".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer: SessionAuthorizer::default(),
            device_auth: device_auth.clone(),
            trusted_devices: trusted_devices.clone(),
        };
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &random)
                .unwrap();
        let key_pair = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8.as_ref(),
            &random,
        )
        .unwrap();
        let public_key = base64::engine::general_purpose::STANDARD.encode(key_pair.public_key());
        let phone_nonce = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
        let device_id = DeviceId("phone-1".into());
        trusted_devices
            .trust(device_id.clone(), "My Phone".into(), public_key.clone())
            .unwrap();

        let (challenge_tx, challenge_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr: make_addr("192.168.1.5:23559"),
                bitrate: Some(128_000),
                response_tx: challenge_tx,
                authorized_generation: None,
                pending_request_id: None,
                device_auth: Some(DeviceAuthRequest {
                    public_key: public_key.clone(),
                    phone_nonce: phone_nonce.clone(),
                    challenge_id: None,
                    signature: None,
                    phone_confirmation: None,
                }),
            })
            .await;
        let challenge = challenge_rx
            .await
            .unwrap()
            .unwrap()
            .device_auth_challenge
            .expect("LAN authentication must issue a proof challenge");
        assert!(!challenge.requires_approval);
        let transcript = build_device_auth_transcript(
            &device_id,
            &dispatcher.streamer_id,
            &dispatcher.pc_certificate_fingerprint,
            &public_key,
            &phone_nonce,
            &challenge.challenge_id,
            &challenge.challenge,
        );
        let signature = key_pair.sign(&random, &transcript).unwrap();

        let (connect_tx, connect_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr: make_addr("192.168.1.5:23559"),
                bitrate: Some(128_000),
                response_tx: connect_tx,
                authorized_generation: None,
                pending_request_id: None,
                device_auth: Some(DeviceAuthRequest {
                    public_key: public_key.clone(),
                    phone_nonce: phone_nonce.clone(),
                    challenge_id: Some(challenge.challenge_id),
                    signature: Some(
                        base64::engine::general_purpose::STANDARD.encode(signature.as_ref()),
                    ),
                    phone_confirmation: None,
                }),
            })
            .await;
        let connected = connect_rx.await.unwrap().unwrap();

        assert_eq!(connected.device_registered, Some(true));
        assert!(connected.session_token.is_some());
        assert!(connected.pending_request_id.is_none());
        assert!(registry.contains("phone-1"));
        assert!(
            tray.take_calls()
                .iter()
                .all(|call| !matches!(call, TrayCall::ApprovalRequested { .. }))
        );
        assert!(matches!(
            audio.take_calls().as_slice(),
            [AudioCall::Subscribe { device_id: subscribed, .. }] if subscribed == &device_id
        ));

        let repair_registry = Arc::new(MockDeviceRegistry::new());
        let repair_tray = Arc::new(MockTrayNotifier::new());
        let repair_audio = Arc::new(MockAudioController::new());
        let repair_authorizer = SessionAuthorizer::default();
        let repair_device_auth = DeviceAuthManager::default();
        let repair_dispatcher = ControlDispatcher {
            registry: repair_registry,
            tray: repair_tray.clone(),
            audio: repair_audio.clone(),
            notifier: Arc::new(MockDeviceNotifier::new()),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            pc_certificate_fingerprint: "pc-certificate".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer: repair_authorizer,
            device_auth: repair_device_auth,
            trusted_devices: trusted_devices.clone(),
        };
        let repair_nonce = base64::engine::general_purpose::STANDARD.encode([8_u8; 32]);
        let (repair_challenge_tx, repair_challenge_rx) = tokio::sync::oneshot::channel();
        repair_dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr: make_addr("192.168.1.5:23559"),
                bitrate: Some(128_000),
                response_tx: repair_challenge_tx,
                authorized_generation: None,
                pending_request_id: None,
                device_auth: Some(DeviceAuthRequest {
                    public_key: public_key.clone(),
                    phone_nonce: repair_nonce.clone(),
                    challenge_id: None,
                    signature: None,
                    phone_confirmation: None,
                }),
            })
            .await;
        let repair_challenge = repair_challenge_rx
            .await
            .unwrap()
            .unwrap()
            .device_auth_challenge
            .expect("trusted repair must issue a proof challenge");
        assert!(!repair_challenge.requires_approval);
        let repair_transcript = build_device_auth_transcript(
            &device_id,
            &repair_dispatcher.streamer_id,
            &repair_dispatcher.pc_certificate_fingerprint,
            &public_key,
            &repair_nonce,
            &repair_challenge.challenge_id,
            &repair_challenge.challenge,
        );
        let repair_signature = key_pair.sign(&random, &repair_transcript).unwrap();
        let (repair_tx, repair_rx) = tokio::sync::oneshot::channel();
        repair_dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr: make_addr("192.168.1.5:23559"),
                bitrate: Some(128_000),
                response_tx: repair_tx,
                authorized_generation: None,
                pending_request_id: None,
                device_auth: Some(DeviceAuthRequest {
                    public_key,
                    phone_nonce: repair_nonce,
                    challenge_id: Some(repair_challenge.challenge_id),
                    signature: Some(
                        base64::engine::general_purpose::STANDARD.encode(repair_signature.as_ref()),
                    ),
                    phone_confirmation: Some(true),
                }),
            })
            .await;
        let repair_pending = repair_rx.await.unwrap().unwrap();

        assert!(repair_pending.pending_request_id.is_some());
        assert_eq!(repair_pending.device_registered, Some(false));
        assert!(repair_audio.take_calls().is_empty());
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if repair_tray.calls.lock().unwrap().iter().any(|call| {
                    matches!(
                        call,
                        TrayCall::ApprovalRequested {
                            replaces_existing_identity: false,
                            ..
                        }
                    )
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("phone-requested re-pair must show PC approval");
        assert!(repair_tray.take_calls().iter().any(|call| matches!(
            call,
            TrayCall::ApprovalRequested {
                replaces_existing_identity: false,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn rotated_lan_key_should_require_approval_and_replace_the_old_key() {
        let registry = Arc::new(MockDeviceRegistry::new());
        let tray = Arc::new(MockTrayNotifier::new());
        let audio = Arc::new(MockAudioController::new());
        let authorizer = SessionAuthorizer::default();
        let device_auth = DeviceAuthManager::default();
        let trusted_devices = TrustedDeviceStore::in_memory();
        let dispatcher = ControlDispatcher {
            registry: registry.clone(),
            tray: tray.clone(),
            audio: audio.clone(),
            notifier: Arc::new(MockDeviceNotifier::new()),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            pc_certificate_fingerprint: "pc-certificate".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer: authorizer.clone(),
            device_auth: device_auth.clone(),
            trusted_devices: trusted_devices.clone(),
        };
        let device_id = DeviceId("phone-1".into());
        let old_public_key = "old-public-key".to_string();
        trusted_devices
            .trust(device_id.clone(), "My Phone".into(), old_public_key.clone())
            .unwrap();

        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &random)
                .unwrap();
        let key_pair = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8.as_ref(),
            &random,
        )
        .unwrap();
        let new_public_key =
            base64::engine::general_purpose::STANDARD.encode(key_pair.public_key());
        let phone_nonce = base64::engine::general_purpose::STANDARD.encode([11_u8; 32]);
        let remote_addr = make_addr("192.168.1.5:23559");

        let (challenge_tx, challenge_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr,
                bitrate: Some(128_000),
                response_tx: challenge_tx,
                authorized_generation: None,
                pending_request_id: None,
                device_auth: Some(DeviceAuthRequest {
                    public_key: new_public_key.clone(),
                    phone_nonce: phone_nonce.clone(),
                    challenge_id: None,
                    signature: None,
                    phone_confirmation: None,
                }),
            })
            .await;
        let challenge = challenge_rx
            .await
            .unwrap()
            .unwrap()
            .device_auth_challenge
            .expect("a rotated key must receive a proof challenge");
        assert!(challenge.requires_approval);

        let transcript = build_device_auth_transcript(
            &device_id,
            &dispatcher.streamer_id,
            &dispatcher.pc_certificate_fingerprint,
            &new_public_key,
            &phone_nonce,
            &challenge.challenge_id,
            &challenge.challenge,
        );
        let signature = key_pair.sign(&random, &transcript).unwrap();
        let verified_auth = DeviceAuthRequest {
            public_key: new_public_key.clone(),
            phone_nonce,
            challenge_id: Some(challenge.challenge_id),
            signature: Some(base64::engine::general_purpose::STANDARD.encode(signature.as_ref())),
            phone_confirmation: Some(true),
        };

        let (pair_tx, pair_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr,
                bitrate: Some(128_000),
                response_tx: pair_tx,
                authorized_generation: None,
                pending_request_id: None,
                device_auth: Some(verified_auth.clone()),
            })
            .await;
        let pending = pair_rx.await.unwrap().unwrap();
        let request_id = pending
            .pending_request_id
            .expect("rotated identity must wait for PC approval");
        assert!(trusted_devices.is_trusted(&device_id, &old_public_key));

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if matches!(
                    authorizer.pending_status(&request_id, &device_id),
                    Some(gemacast_core::control::PendingApprovalStatus::Approved)
                ) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mock PC approval should resolve");

        let (approved_tx, approved_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr,
                bitrate: Some(128_000),
                response_tx: approved_tx,
                authorized_generation: None,
                pending_request_id: Some(request_id),
                device_auth: Some(verified_auth),
            })
            .await;
        let approved = approved_rx.await.unwrap().unwrap();

        assert_eq!(approved.device_registered, Some(true));
        assert!(approved.session_token.is_some());
        assert!(registry.contains("phone-1"));
        assert!(!trusted_devices.is_trusted(&device_id, &old_public_key));
        assert!(trusted_devices.is_trusted(&device_id, &new_public_key));
        assert!(tray.take_calls().iter().any(|call| matches!(
            call,
            TrayCall::ApprovalRequested {
                replaces_existing_identity: true,
                ..
            }
        )));
        assert!(matches!(
            audio.take_calls().as_slice(),
            [AudioCall::Subscribe { device_id: subscribed, .. }] if subscribed == &device_id
        ));
    }

    #[tokio::test]
    async fn forgotten_lan_key_should_require_approval_again() {
        let trusted_devices = TrustedDeviceStore::in_memory();
        let dispatcher = ControlDispatcher {
            registry: Arc::new(MockDeviceRegistry::new()),
            tray: Arc::new(MockTrayNotifier::new()),
            audio: Arc::new(MockAudioController::new()),
            notifier: Arc::new(MockDeviceNotifier::new()),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            pc_certificate_fingerprint: "pc-certificate".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer: SessionAuthorizer::default(),
            device_auth: DeviceAuthManager::default(),
            trusted_devices: trusted_devices.clone(),
        };
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &random)
                .unwrap();
        let key_pair = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8.as_ref(),
            &random,
        )
        .unwrap();
        let public_key = base64::engine::general_purpose::STANDARD.encode(key_pair.public_key());
        let device_id = DeviceId("phone-1".into());
        trusted_devices
            .trust(device_id.clone(), "My Phone".into(), public_key.clone())
            .unwrap();
        assert!(trusted_devices.forget(&device_id).unwrap());

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id,
                device_name: "My Phone".into(),
                source: None,
                remote_addr: make_addr("192.168.1.5:23559"),
                bitrate: Some(128_000),
                response_tx,
                authorized_generation: None,
                pending_request_id: None,
                device_auth: Some(DeviceAuthRequest {
                    public_key,
                    phone_nonce: base64::engine::general_purpose::STANDARD.encode([9_u8; 32]),
                    challenge_id: None,
                    signature: None,
                    phone_confirmation: None,
                }),
            })
            .await;
        let challenge = response_rx
            .await
            .unwrap()
            .unwrap()
            .device_auth_challenge
            .expect("forgotten LAN identity must issue a proof challenge");

        assert!(challenge.requires_approval);
    }

    #[tokio::test]
    async fn loopback_connect_should_not_create_persistent_trust() {
        let registry = Arc::new(MockDeviceRegistry::new());
        let tray = Arc::new(MockTrayNotifier::new());
        let trusted_devices = TrustedDeviceStore::in_memory();
        let device_id = DeviceId("loopback-phone".into());
        let authorizer = SessionAuthorizer::default();
        let (_, authorized_generation) = authorizer.issue(device_id.clone()).unwrap();
        let dispatcher = ControlDispatcher {
            registry: registry.clone(),
            tray: tray.clone(),
            audio: Arc::new(MockAudioController::new()),
            notifier: Arc::new(MockDeviceNotifier::new()),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            pc_certificate_fingerprint: "pc-certificate".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer,
            device_auth: DeviceAuthManager::default(),
            trusted_devices: trusted_devices.clone(),
        };
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        dispatcher
            .handle_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "Loopback Phone".into(),
                source: None,
                remote_addr: make_addr("127.0.0.1:23559"),
                bitrate: Some(128_000),
                response_tx,
                authorized_generation: Some(authorized_generation),
                pending_request_id: None,
                device_auth: None,
            })
            .await;
        let connected = response_rx.await.unwrap().unwrap();

        assert_eq!(connected.device_registered, Some(true));
        assert!(connected.session_token.is_some());
        assert!(registry.contains("loopback-phone"));
        assert!(trusted_devices.public_key(&device_id).is_none());
        assert!(
            tray.take_calls()
                .iter()
                .all(|call| !matches!(call, TrayCall::ApprovalRequested { .. }))
        );
    }

    #[tokio::test]
    async fn ordinary_disconnect_should_preserve_persistent_device_trust() {
        let registry = Arc::new(MockDeviceRegistry::with_device(
            "phone-1",
            "192.168.1.5:9000",
        ));
        let trusted_devices = TrustedDeviceStore::in_memory();
        let device_id = DeviceId("phone-1".into());
        trusted_devices
            .trust(device_id.clone(), "Phone".into(), "public-key".into())
            .unwrap();
        let authorizer = SessionAuthorizer::default();
        let (_, generation) = authorizer.issue(device_id.clone()).unwrap();
        let dispatcher = ControlDispatcher {
            registry,
            tray: Arc::new(MockTrayNotifier::new()),
            audio: Arc::new(MockAudioController::new()),
            notifier: Arc::new(MockDeviceNotifier::new()),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            pc_certificate_fingerprint: "pc-certificate".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer,
            device_auth: DeviceAuthManager::default(),
            trusted_devices: trusted_devices.clone(),
        };
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        dispatcher
            .handle_command(ControlCommand::Disconnect {
                device_id: device_id.clone(),
                remote_addr: make_addr("192.168.1.5:23559"),
                generation: Some(generation),
                response_tx,
            })
            .await;

        response_rx.await.unwrap().unwrap();
        assert!(trusted_devices.is_trusted(&device_id, "public-key"));
    }

    mod stale_session_commands {
        use super::*;
        use tokio::sync::oneshot;

        #[tokio::test]
        async fn stale_session_must_not_reconnect_or_mutate_audio() {
            let device_id = DeviceId("phone-1".into());
            let authorizer = SessionAuthorizer::default();
            let (_, stale_generation) = authorizer.issue(device_id.clone()).unwrap();
            let _ = authorizer.issue(device_id.clone()).unwrap();

            let audio = Arc::new(MockAudioController::new());
            let dispatcher = ControlDispatcher {
                registry: Arc::new(MockDeviceRegistry::new()),
                tray: Arc::new(MockTrayNotifier::new()),
                audio: audio.clone(),
                notifier: Arc::new(MockDeviceNotifier::new()),
                streamer_id: DeviceId("pc-1".into()),
                streamer_name: "Test PC".into(),
                pc_certificate_fingerprint: "pc-certificate".into(),
                is_broadcasting: Arc::new(AtomicBool::new(true)),
                authorizer,
                device_auth: DeviceAuthManager::default(),
                trusted_devices: TrustedDeviceStore::in_memory(),
            };

            let (connect_tx, connect_rx) = oneshot::channel();
            dispatcher
                .handle_command(ControlCommand::Connect {
                    device_id: device_id.clone(),
                    device_name: "Phone".into(),
                    source: None,
                    remote_addr: "127.0.0.1:23559".parse().unwrap(),
                    bitrate: Some(128_000),
                    response_tx: connect_tx,
                    authorized_generation: Some(stale_generation),
                    pending_request_id: None,
                    device_auth: None,
                })
                .await;
            assert_eq!(
                connect_rx.await.unwrap().unwrap_err(),
                "session is no longer active"
            );

            let (source_tx, source_rx) = oneshot::channel();
            dispatcher
                .handle_command(ControlCommand::ChangeSource {
                    device_id: device_id.clone(),
                    generation: stale_generation,
                    source: gemacast_core::domain::types::AudioSource::Desktop,
                    response_tx: source_tx,
                })
                .await;
            assert_eq!(
                source_rx.await.unwrap(),
                Err("session is no longer active".into())
            );

            let (bitrate_tx, bitrate_rx) = oneshot::channel();
            dispatcher
                .handle_command(ControlCommand::ChangeBitrate {
                    device_id,
                    generation: stale_generation,
                    bitrate: Some(192_000),
                    response_tx: bitrate_tx,
                })
                .await;
            assert_eq!(
                bitrate_rx.await.unwrap(),
                Err("session is no longer active".into())
            );

            assert!(
                audio.take_calls().is_empty(),
                "stale commands must not reach the active audio stream"
            );
        }
    }

    mod session_heartbeat {
        use super::*;
        use std::time::{Duration, Instant};

        fn make_dispatcher(
            registry: Arc<dyn DeviceRegistry>,
            authorizer: SessionAuthorizer,
        ) -> ControlDispatcher {
            ControlDispatcher {
                registry,
                tray: Arc::new(MockTrayNotifier::new()),
                audio: Arc::new(MockAudioController::new()),
                notifier: Arc::new(MockDeviceNotifier::new()),
                streamer_id: DeviceId("pc-1".into()),
                streamer_name: "Test PC".into(),
                pc_certificate_fingerprint: "pc-certificate".into(),
                is_broadcasting: Arc::new(AtomicBool::new(true)),
                authorizer,
                device_auth: DeviceAuthManager::default(),
                trusted_devices: TrustedDeviceStore::in_memory(),
            }
        }

        #[tokio::test]
        async fn current_session_heartbeat_should_refresh_device_liveness() {
            let registry = Arc::new(MockDeviceRegistry::new());
            registry.add_device_with_last_seen(
                "phone-1",
                "192.168.1.5:9000",
                Instant::now() - Duration::from_secs(60),
            );
            let device_id = DeviceId("phone-1".into());
            let authorizer = SessionAuthorizer::default();
            let (_, generation) = authorizer.issue(device_id.clone()).unwrap();
            let dispatcher = make_dispatcher(registry.clone(), authorizer);

            dispatcher
                .handle_command(ControlCommand::SessionHeartbeat {
                    device_id,
                    generation,
                })
                .await;

            assert!(registry.evict_stale(Duration::from_secs(30)).is_empty());
        }

        #[tokio::test]
        async fn stale_session_heartbeat_should_not_refresh_device_liveness() {
            let registry = Arc::new(MockDeviceRegistry::new());
            registry.add_device_with_last_seen(
                "phone-1",
                "192.168.1.5:9000",
                Instant::now() - Duration::from_secs(60),
            );
            let device_id = DeviceId("phone-1".into());
            let authorizer = SessionAuthorizer::default();
            let (_, stale_generation) = authorizer.issue(device_id.clone()).unwrap();
            let _ = authorizer.issue(device_id.clone()).unwrap();
            let dispatcher = make_dispatcher(registry.clone(), authorizer);

            dispatcher
                .handle_command(ControlCommand::SessionHeartbeat {
                    device_id,
                    generation: stale_generation,
                })
                .await;

            assert_eq!(registry.evict_stale(Duration::from_secs(30)).len(), 1);
        }
    }

    mod probe_registration_status {
        use super::*;
        use tokio::sync::oneshot;

        fn make_dispatcher(registry: Arc<dyn DeviceRegistry>) -> ControlDispatcher {
            ControlDispatcher {
                registry,
                tray: Arc::new(MockTrayNotifier::new()),
                audio: Arc::new(MockAudioController::new()),
                notifier: Arc::new(MockDeviceNotifier::new()),
                streamer_id: DeviceId("test-streamer".into()),
                streamer_name: "Test Streamer".into(),
                pc_certificate_fingerprint: "pc-certificate".into(),
                is_broadcasting: Arc::new(AtomicBool::new(true)),
                authorizer: SessionAuthorizer::default(),
                device_auth: DeviceAuthManager::default(),
                trusted_devices: TrustedDeviceStore::in_memory(),
            }
        }

        async fn probe(
            dispatcher: &ControlDispatcher,
            device_id: Option<DeviceId>,
        ) -> PresenceResponse {
            let (response_tx, response_rx) = oneshot::channel();
            dispatcher
                .handle_command(ControlCommand::Probe {
                    device_id,
                    response_tx,
                })
                .await;
            response_rx.await.expect("probe must answer")
        }

        #[tokio::test]
        async fn the_probe_response_should_report_a_registered_device_as_registered() {
            let registry = Arc::new(MockDeviceRegistry::with_device(
                "phone-1",
                "192.168.1.5:9000",
            ));
            let dispatcher = make_dispatcher(registry);

            let presence = probe(&dispatcher, Some(DeviceId("phone-1".into()))).await;

            assert_eq!(presence.device_registered, Some(true));
            assert!(!presence.is_offline);
        }

        #[tokio::test]
        async fn the_probe_response_should_report_an_evicted_device_as_unregistered() {
            let registry = Arc::new(MockDeviceRegistry::new());
            let dispatcher = make_dispatcher(registry);

            let presence = probe(&dispatcher, Some(DeviceId("phone-1".into()))).await;

            assert_eq!(presence.device_registered, Some(false));
            assert!(!presence.is_offline);
        }

        #[tokio::test]
        async fn a_probe_without_a_device_id_should_make_no_registration_claim() {
            let registry = Arc::new(MockDeviceRegistry::with_device(
                "phone-1",
                "192.168.1.5:9000",
            ));
            let dispatcher = make_dispatcher(registry);

            let presence = probe(&dispatcher, None).await;

            assert_eq!(presence.device_registered, None);
        }

        #[tokio::test]
        async fn updating_last_seen_on_an_evicted_device_should_not_reregister_it() {
            let registry = MockDeviceRegistry::new();

            assert!(!registry.update_last_seen(&DeviceId("phone-1".into())));
            assert!(!registry.contains("phone-1"));
            assert!(registry.all_devices().is_empty());

            let live = MockDeviceRegistry::with_device("phone-1", "192.168.1.5:9000");
            assert!(live.update_last_seen(&DeviceId("phone-1".into())));
            assert!(live.contains("phone-1"));
        }
    }
}
