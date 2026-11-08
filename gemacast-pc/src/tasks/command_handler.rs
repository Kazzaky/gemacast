//! Processes [`AppCommand`]s from the tray UI.
//!
//! Handles start/stop broadcasting, kicking individual devices,
//! and graceful shutdown of all streams. The [`CommandHandler`] struct
//! holds trait-based dependencies for full unit testability.

use std::net::SocketAddrV4;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gemacast_core::control::SessionAuthorizer;
use gemacast_core::control::messages::ControlMessage;
use gemacast_core::discovery::PresenceBroadcaster;
use gemacast_core::domain::types::DeviceId;
use gemacast_core::updater::{UpdateChecker, UpdateDownloader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::device_auth::DeviceAuthManager;
use crate::events::AppCommand;
use crate::traits::{AudioController, DeviceNotifier, DeviceRegistry, TrayNotifier};
use crate::trusted_devices::TrustedDeviceStore;

/// Holds the shutdown handle for an active presence broadcaster.
#[derive(Default)]
pub(crate) struct BroadcasterState {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

fn online_presence(streamer_id: &DeviceId, streamer_name: &str) -> ControlMessage {
    ControlMessage::Presence {
        device_id: streamer_id.clone(),
        streamer_name: streamer_name.to_string(),
        is_offline: false,
        transport: None,
    }
}

/// Handles [`AppCommand`]s from the tray UI.
///
/// Dependencies are injected as trait objects, making every command handler
/// independently unit-testable with mock implementations.
pub struct CommandHandler {
    pub is_broadcasting: Arc<AtomicBool>,
    pub streamer_id: DeviceId,
    pub streamer_name: String,
    pub registry: Arc<dyn DeviceRegistry>,
    pub tray: Arc<dyn TrayNotifier>,
    pub audio: Arc<dyn AudioController>,
    pub notifier: Arc<dyn DeviceNotifier>,
    pub authorizer: SessionAuthorizer,
    pub trusted_devices: TrustedDeviceStore,
    pub device_auth: DeviceAuthManager,
}

impl CommandHandler {
    /// Handle a single [`AppCommand`].
    ///
    /// Extracted from the receive loop for unit testing. The `broadcaster` state
    /// is passed mutably so the handler can start/stop the presence broadcaster.
    pub(crate) async fn handle(&self, command: AppCommand, broadcaster: &mut BroadcasterState) {
        match command {
            AppCommand::StartBroadcasting => {
                self.handle_start_broadcasting(broadcaster).await;
            }
            AppCommand::StopBroadcasting => {
                self.handle_stop_broadcasting(broadcaster).await;
            }
            AppCommand::KickDevice(device_id) => {
                self.handle_kick_device(device_id).await;
            }
            AppCommand::StopAllStreams => {
                self.handle_stop_all_streams(broadcaster).await;
            }
            AppCommand::ExitApp => {
                self.handle_stop_all_streams(broadcaster).await;
            }
            AppCommand::CheckForUpdates => {
                self.handle_check_for_updates();
            }
        }
    }

    async fn handle_start_broadcasting(&self, broadcaster: &mut BroadcasterState) {
        tracing::info!("Executing StartBroadcasting command");
        if broadcaster.shutdown_tx.is_some() {
            return;
        }

        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        let Ok(presence_broadcaster) = PresenceBroadcaster::new(stop_rx).await else {
            return;
        };

        self.is_broadcasting.store(true, Ordering::Relaxed);
        broadcaster.shutdown_tx = Some(stop_tx);

        let registry = self.registry.clone();
        let streamer_id = self.streamer_id.clone();
        let streamer_name = self.streamer_name.clone();

        tokio::spawn(async move {
            let sid = streamer_id;
            let sname = streamer_name;
            let factory = move || online_presence(&sid, &sname);
            let registry_ref = registry;
            let target_ips = move || {
                registry_ref
                    .all_devices()
                    .into_iter()
                    .filter_map(|(_, d)| {
                        if let std::net::SocketAddr::V4(v4) = d.addr {
                            Some(SocketAddrV4::new(
                                *v4.ip(),
                                gemacast_core::network::Ports::DISCOVERY,
                            ))
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            let _ = presence_broadcaster
                .run_broadcast_loop(factory, target_ips)
                .await;
        });
    }

    async fn handle_stop_broadcasting(&self, broadcaster: &mut BroadcasterState) {
        tracing::info!("Executing StopBroadcasting command");
        self.is_broadcasting.store(false, Ordering::Relaxed);

        if let Some(tx) = broadcaster.shutdown_tx.take() {
            let _ = tx.send(());
        }

        let all = self.registry.drain_all();
        self.authorizer.revoke_all();
        self.device_auth.clear_pending();
        for (device_id, device) in all {
            self.notifier
                .notify_disconnect(&device_id, Some(device.addr))
                .await;
            let _ = self.audio.unsubscribe(&device_id).await;
            self.tray.notify_device_lost(device_id, device.addr);
        }
    }

    async fn handle_kick_device(&self, device_id: DeviceId) {
        tracing::info!("Executing KickDevice command for device: {:?}", device_id);
        let addr = self.registry.get_addr(&device_id);
        self.registry.unregister(&device_id);
        self.authorizer.revoke(&device_id, None);
        self.authorizer.cancel_pending_for_device(&device_id);
        self.device_auth.cancel_pending_for_device(&device_id);
        if let Err(error) = self.trusted_devices.forget(&device_id) {
            tracing::error!("Failed to remove trusted device {:?}: {}", device_id, error);
            self.tray.notify_fatal_error(format!(
                "Could not persist the removal of {}. It remains revoked until restart; retry the kick before restarting: {error}",
                device_id.0
            ));
        }
        let _ = self.audio.unsubscribe(&device_id).await;
        self.notifier.notify_disconnect(&device_id, addr).await;
    }

    async fn handle_stop_all_streams(&self, broadcaster: &mut BroadcasterState) {
        tracing::info!("Executing StopAllStreams command");

        if let Some(tx) = broadcaster.shutdown_tx.take() {
            let _ = tx.send(());
        }

        let all = self.registry.drain_all();
        self.authorizer.revoke_all();
        self.device_auth.clear_pending();
        for (device_id, device) in all {
            self.tray.notify_device_lost(device_id.clone(), device.addr);
            self.notifier
                .notify_disconnect(&device_id, Some(device.addr))
                .await;
        }
        let _ = self.audio.shutdown().await;
    }

    fn handle_check_for_updates(&self) {
        tracing::info!("Executing manual CheckForUpdates command");
        let tray = self.tray.clone();
        tokio::spawn(async move {
            tray.notify_update_checking();

            let current_version = env!("CARGO_PKG_VERSION");
            let key = match crate::updater::platform_key() {
                Some(k) => k,
                None => {
                    tray.notify_update_failed(
                        "Auto-updates are not supported on this platform".to_string(),
                    );
                    return;
                }
            };

            let info = match UpdateChecker::latest_release()
                .find_available_update(current_version, key)
                .await
            {
                Ok(Some(info)) => info,
                Ok(None) => {
                    tracing::info!("Manual update check: already up to date.");
                    tray.notify_update_up_to_date();
                    return;
                }
                Err(error) => {
                    tracing::warn!("Manual update check failed: {}", error);
                    tray.notify_update_failed(error.to_string());
                    return;
                }
            };

            let filename = info
                .download_url
                .rsplit('/')
                .next()
                .unwrap_or("gemacast-update");

            let dir = std::env::temp_dir().join("gemacast-update");
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tray.notify_update_failed(format!("Failed to create update directory: {e}"));
                return;
            }
            let file_path = dir.join(filename);

            if file_path.exists() {
                tray.notify_update_ready(info.version, file_path);
                return;
            }

            let artifact = info.artifact();
            match UpdateDownloader::default()
                .download(&artifact, &file_path, None)
                .await
            {
                Ok(()) => {
                    tray.notify_update_ready(info.version, file_path);
                }
                Err(error) => {
                    tracing::warn!("Manual update download failed: {}", error);
                    tray.notify_update_failed(error.to_string());
                }
            }
        });
    }
}

/// Spawn the command handler as a background task.
pub fn spawn_command_handler(
    set: &mut JoinSet<()>,
    mut command_rx: mpsc::Receiver<AppCommand>,
    handler: Arc<CommandHandler>,
    shutdown_tx: tokio::sync::oneshot::Sender<()>,
) {
    set.spawn(async move {
        let mut broadcaster = BroadcasterState::default();
        let mut shutdown_tx = Some(shutdown_tx);
        while let Some(command) = command_rx.recv().await {
            let should_exit = matches!(command, AppCommand::ExitApp);
            handler.handle(command, &mut broadcaster).await;
            if should_exit {
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                break;
            }
        }
        if let Some(tx) = shutdown_tx.take() {
            let _ = tx.send(());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;

    fn make_handler(
        registry: Arc<MockDeviceRegistry>,
        tray: Arc<MockTrayNotifier>,
        audio: Arc<MockAudioController>,
        notifier: Arc<MockDeviceNotifier>,
    ) -> CommandHandler {
        CommandHandler {
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "Test PC".into(),
            registry,
            tray,
            audio,
            notifier,
            authorizer: SessionAuthorizer::default(),
            trusted_devices: TrustedDeviceStore::in_memory(),
            device_auth: DeviceAuthManager::default(),
        }
    }

    #[tokio::test]
    async fn stop_broadcasting_should_set_flag_and_unsubscribe_all() {
        let registry = Arc::new(MockDeviceRegistry::with_device(
            "phone-1",
            "192.168.1.5:9000",
        ));
        let tray = Arc::new(MockTrayNotifier::new());
        let audio = Arc::new(MockAudioController::new());
        let notifier = Arc::new(MockDeviceNotifier::new());
        let handler = make_handler(registry, tray, audio.clone(), notifier);

        let mut broadcaster = BroadcasterState::default();
        handler
            .handle(AppCommand::StopBroadcasting, &mut broadcaster)
            .await;

        assert!(!handler.is_broadcasting.load(Ordering::Relaxed));
        let calls = audio.take_calls();
        assert_eq!(calls.len(), 1);
        assert!(
            matches!(&calls[0], AudioCall::Unsubscribe { device_id } if device_id.0 == "phone-1")
        );
    }

    #[test]
    fn start_broadcasting_should_advertise_the_persistent_pc_identity() {
        let handler = make_handler(
            Arc::new(MockDeviceRegistry::new()),
            Arc::new(MockTrayNotifier::new()),
            Arc::new(MockAudioController::new()),
            Arc::new(MockDeviceNotifier::new()),
        );
        match online_presence(&handler.streamer_id, &handler.streamer_name) {
            ControlMessage::Presence {
                device_id,
                streamer_name,
                is_offline,
                transport,
            } => {
                assert_eq!(device_id, handler.streamer_id);
                assert_eq!(streamer_name, handler.streamer_name);
                assert!(!is_offline);
                assert!(transport.is_none());
            }
            _ => panic!("online presence helper returned the wrong message type"),
        }
    }

    #[tokio::test]
    async fn kick_device_should_unregister_and_notify_disconnect() {
        let registry = Arc::new(MockDeviceRegistry::with_device(
            "phone-1",
            "192.168.1.5:9000",
        ));
        let tray = Arc::new(MockTrayNotifier::new());
        let audio = Arc::new(MockAudioController::new());
        let notifier = Arc::new(MockDeviceNotifier::new());
        let handler = make_handler(registry.clone(), tray, audio.clone(), notifier.clone());
        let device_id = DeviceId("phone-1".into());
        handler
            .trusted_devices
            .trust(device_id.clone(), "Phone 1".into(), "key".into())
            .unwrap();
        let approval_request = handler
            .authorizer
            .create_pending(device_id.clone())
            .unwrap();
        handler
            .device_auth
            .hold_pending_pairing(
                approval_request.clone(),
                device_id.clone(),
                crate::device_auth::VerifiedDeviceIdentity {
                    public_key: "key".into(),
                    fingerprint: "fingerprint".into(),
                    pairing_code: "123456".into(),
                },
            )
            .unwrap();

        let mut broadcaster = BroadcasterState::default();
        handler
            .handle(AppCommand::KickDevice(device_id.clone()), &mut broadcaster)
            .await;

        assert!(!registry.contains("phone-1"));
        assert!(!handler.trusted_devices.is_trusted(&device_id, "key"));
        assert_eq!(
            handler
                .authorizer
                .pending_status(&approval_request, &device_id),
            None
        );
        assert!(
            handler
                .device_auth
                .pending_pairing(&approval_request, &device_id, "key")
                .is_none()
        );

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(
            matches!(&audio_calls[0], AudioCall::Unsubscribe { device_id } if device_id.0 == "phone-1")
        );

        let notifier_calls = notifier.take_calls();
        assert_eq!(notifier_calls.len(), 1);
        assert!(
            matches!(&notifier_calls[0], NotifierCall::Disconnect { device_id, addr } if device_id.0 == "phone-1" && addr.is_some())
        );
    }

    #[tokio::test]
    async fn stop_all_streams_should_drain_and_notify_all() {
        let registry = Arc::new(MockDeviceRegistry::new());
        // Add two devices
        {
            let d1 = gemacast_core::domain::types::DiscoveredDevice::from_presence(
                DeviceId("phone-1".into()),
                "Phone 1".into(),
                false,
                "192.168.1.5:9000".parse().unwrap(),
                None,
            );
            let d2 = gemacast_core::domain::types::DiscoveredDevice::from_presence(
                DeviceId("phone-2".into()),
                "Phone 2".into(),
                false,
                "192.168.1.6:9000".parse().unwrap(),
                None,
            );
            registry.register(d1);
            registry.register(d2);
        }

        let tray = Arc::new(MockTrayNotifier::new());
        let audio = Arc::new(MockAudioController::new());
        let notifier = Arc::new(MockDeviceNotifier::new());
        let handler = make_handler(
            registry.clone(),
            tray.clone(),
            audio.clone(),
            notifier.clone(),
        );

        let mut broadcaster = BroadcasterState::default();
        handler
            .handle(AppCommand::StopAllStreams, &mut broadcaster)
            .await;

        // Audio engine was shut down
        let audio_calls = audio.take_calls();
        assert!(audio_calls.iter().any(|c| matches!(c, AudioCall::Shutdown)));

        // All devices were drained and notified
        let tray_calls = tray.take_calls();
        assert_eq!(tray_calls.len(), 2);
        assert!(
            tray_calls
                .iter()
                .all(|c| matches!(c, TrayCall::Lost { .. }))
        );

        let notifier_calls = notifier.take_calls();
        assert_eq!(notifier_calls.len(), 3);
        let disconnects = notifier_calls
            .iter()
            .filter(|c| matches!(c, NotifierCall::Disconnect { .. }))
            .count();
        assert_eq!(disconnects, 2);

    }

}
