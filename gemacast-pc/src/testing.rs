//! Hand-written mock implementations for unit testing.
//!
//! Each mock records calls in a `Mutex<Vec<...>>` so tests can assert
//! what was called and with which arguments.

pub mod mocks {
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use gemacast_core::domain::types::{AudioSource, DeviceId, DiscoveredDevice, TransportType};

    use crate::traits::{
        AudioController, ConnectionApprovalRequest, DeviceNotifier, DeviceRegistry,
        RegistrationOutcome, TrayNotifier,
    };

    // -----------------------------------------------------------------------
    // MockTrayNotifier
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone, PartialEq)]
    pub enum TrayCall {
        ApprovalRequested {
            request_id: String,
            device_id: DeviceId,
            name: String,
            addr: SocketAddr,
            key_fingerprint: String,
            pairing_code: String,
            replaces_existing_identity: bool,
        },
        Discovered {
            device_id: DeviceId,
            name: String,
            addr: SocketAddr,
            transport: Option<TransportType>,
        },
        Lost {
            device_id: DeviceId,
            addr: SocketAddr,
        },
        FatalError(String),
        FirewallWarning(String),
        ShutdownComplete,
        UpdateReady(String, std::path::PathBuf),
        UpdateFailed(String),
        UpdateChecking,
        UpdateUpToDate,
    }

    /// Records every tray notification for later assertion.
    pub struct MockTrayNotifier {
        pub calls: Mutex<Vec<TrayCall>>,
    }

    #[allow(clippy::new_without_default)]
    impl MockTrayNotifier {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn take_calls(&self) -> Vec<TrayCall> {
            self.calls.lock().unwrap().drain(..).collect()
        }
    }

    #[async_trait]
    impl TrayNotifier for MockTrayNotifier {
        async fn request_connection_approval(&self, request: ConnectionApprovalRequest) -> bool {
            let ConnectionApprovalRequest {
                request_id,
                device_id,
                name,
                addr,
                key_fingerprint,
                pairing_code,
                replaces_existing_identity,
            } = request;
            self.calls
                .lock()
                .unwrap()
                .push(TrayCall::ApprovalRequested {
                    request_id,
                    device_id,
                    name,
                    addr,
                    key_fingerprint,
                    pairing_code,
                    replaces_existing_identity,
                });
            true
        }

        fn notify_device_discovered(
            &self,
            device_id: DeviceId,
            name: String,
            addr: SocketAddr,
            transport: Option<TransportType>,
        ) {
            self.calls.lock().unwrap().push(TrayCall::Discovered {
                device_id,
                name,
                addr,
                transport,
            });
        }

        fn notify_device_lost(&self, device_id: DeviceId, addr: SocketAddr) {
            self.calls
                .lock()
                .unwrap()
                .push(TrayCall::Lost { device_id, addr });
        }

        fn notify_fatal_error(&self, message: String) {
            self.calls
                .lock()
                .unwrap()
                .push(TrayCall::FatalError(message));
        }

        fn notify_firewall_warning(&self, message: String) {
            self.calls
                .lock()
                .unwrap()
                .push(TrayCall::FirewallWarning(message));
        }

        fn notify_shutdown_complete(&self) {
            self.calls.lock().unwrap().push(TrayCall::ShutdownComplete);
        }

        fn notify_update_ready(&self, version: String, installer_path: std::path::PathBuf) {
            self.calls
                .lock()
                .unwrap()
                .push(TrayCall::UpdateReady(version, installer_path));
        }

        fn notify_update_failed(&self, message: String) {
            self.calls
                .lock()
                .unwrap()
                .push(TrayCall::UpdateFailed(message));
        }

        fn notify_update_checking(&self) {
            self.calls.lock().unwrap().push(TrayCall::UpdateChecking);
        }

        fn notify_update_up_to_date(&self) {
            self.calls.lock().unwrap().push(TrayCall::UpdateUpToDate);
        }
    }

    // -----------------------------------------------------------------------
    // MockAudioController
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone)]
    pub enum AudioCall {
        Subscribe {
            device_id: DeviceId,
            target_addr: Option<SocketAddr>,
            source: Option<AudioSource>,
            bitrate: Option<i32>,
        },
        Unsubscribe {
            device_id: DeviceId,
        },
        ChangeSource {
            device_id: DeviceId,
            source: AudioSource,
        },
        ChangeBitrate {
            device_id: DeviceId,
            bitrate: Option<i32>,
        },
        Shutdown,
    }

    /// Records every audio command for later assertion.
    pub struct MockAudioController {
        pub calls: Mutex<Vec<AudioCall>>,
    }

    #[allow(clippy::new_without_default)]
    impl MockAudioController {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn take_calls(&self) -> Vec<AudioCall> {
            self.calls.lock().unwrap().drain(..).collect()
        }
    }

    #[async_trait]
    impl AudioController for MockAudioController {
        async fn subscribe(
            &self,
            device_id: DeviceId,
            _generation: u64,
            target_addr: Option<SocketAddr>,
            source: Option<AudioSource>,
            bitrate: Option<i32>,
            _audio_key: gemacast_core::audio::AudioSessionKey,
        ) -> Result<(), String> {
            self.calls.lock().unwrap().push(AudioCall::Subscribe {
                device_id,
                target_addr,
                source,
                bitrate,
            });
            Ok(())
        }

        async fn unsubscribe(&self, device_id: &DeviceId) -> Result<(), String> {
            self.calls.lock().unwrap().push(AudioCall::Unsubscribe {
                device_id: device_id.clone(),
            });
            Ok(())
        }

        async fn change_source(
            &self,
            device_id: DeviceId,
            source: AudioSource,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(AudioCall::ChangeSource { device_id, source });
            Ok(())
        }

        async fn change_bitrate(
            &self,
            device_id: DeviceId,
            bitrate: Option<i32>,
        ) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(AudioCall::ChangeBitrate { device_id, bitrate });
            Ok(())
        }

        async fn shutdown(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push(AudioCall::Shutdown);
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // MockDeviceNotifier
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone)]
    pub enum NotifierCall {
        Disconnect {
            device_id: DeviceId,
            addr: Option<SocketAddr>,
        },
    }

    /// Records every disconnect notification for later assertion.
    pub struct MockDeviceNotifier {
        pub calls: Mutex<Vec<NotifierCall>>,
    }

    #[allow(clippy::new_without_default)]
    impl MockDeviceNotifier {
        pub fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }

        pub fn take_calls(&self) -> Vec<NotifierCall> {
            self.calls.lock().unwrap().drain(..).collect()
        }
    }

    #[async_trait]
    impl DeviceNotifier for MockDeviceNotifier {
        async fn notify_disconnect(&self, device_id: &DeviceId, addr: Option<SocketAddr>) {
            self.calls.lock().unwrap().push(NotifierCall::Disconnect {
                device_id: device_id.clone(),
                addr,
            });
        }
    }

    // -----------------------------------------------------------------------
    // MockDeviceRegistry
    // -----------------------------------------------------------------------

    use std::collections::HashMap;

    /// In-memory device registry backed by a simple `Mutex<HashMap>`.
    ///
    /// Behaves identically to `SharedMapDeviceRegistry` but is self-contained
    /// (no `Arc` wrapper needed — the mock itself holds the mutex).
    pub struct MockDeviceRegistry {
        inner: Mutex<HashMap<DeviceId, DiscoveredDevice>>,
    }

    #[allow(clippy::new_without_default)]
    impl MockDeviceRegistry {
        pub fn new() -> Self {
            Self {
                inner: Mutex::new(HashMap::new()),
            }
        }

        /// Pre-populate with a device for testing.
        pub fn with_device(device_id: &str, addr: &str) -> Self {
            let registry = Self::new();
            let device = DiscoveredDevice::from_presence(
                DeviceId(device_id.to_string()),
                device_id.to_string(),
                false,
                addr.parse().unwrap(),
                None,
            );
            registry
                .inner
                .lock()
                .unwrap()
                .insert(device.device_id.clone(), device);
            registry
        }

        /// Check if a device is currently registered.
        pub fn contains(&self, device_id: &str) -> bool {
            self.inner
                .lock()
                .unwrap()
                .contains_key(&DeviceId(device_id.to_string()))
        }

        /// Add a device with a specific `last_seen` time for watchdog testing.
        pub fn add_device_with_last_seen(
            &self,
            device_id: &str,
            addr: &str,
            last_seen: std::time::Instant,
        ) {
            let mut device = DiscoveredDevice::from_presence(
                DeviceId(device_id.to_string()),
                device_id.to_string(),
                false,
                addr.parse().unwrap(),
                None,
            );
            device.last_seen = last_seen;
            self.inner
                .lock()
                .unwrap()
                .insert(device.device_id.clone(), device);
        }
    }

    impl DeviceRegistry for MockDeviceRegistry {
        fn register(&self, device: DiscoveredDevice) -> RegistrationOutcome {
            let mut map = self.inner.lock().unwrap();
            if let Some(existing) = map.get(&device.device_id) {
                if existing.addr != device.addr {
                    let old_addr = existing.addr;
                    map.insert(device.device_id.clone(), device);
                    RegistrationOutcome::AddressChanged { old_addr }
                } else {
                    map.insert(device.device_id.clone(), device);
                    RegistrationOutcome::AlreadyRegistered
                }
            } else {
                map.insert(device.device_id.clone(), device);
                RegistrationOutcome::NewDevice
            }
        }

        fn unregister(&self, device_id: &DeviceId) -> Option<DiscoveredDevice> {
            self.inner.lock().unwrap().remove(device_id)
        }

        fn update_last_seen(&self, device_id: &DeviceId) -> bool {
            if let Some(device) = self.inner.lock().unwrap().get_mut(device_id) {
                device.last_seen = std::time::Instant::now();
                return true;
            }
            false
        }

        fn get_addr(&self, device_id: &DeviceId) -> Option<SocketAddr> {
            self.inner.lock().unwrap().get(device_id).map(|d| d.addr)
        }

        fn all_devices(&self) -> Vec<(DeviceId, DiscoveredDevice)> {
            self.inner
                .lock()
                .unwrap()
                .iter()
                .map(|(id, d)| (id.clone(), d.clone()))
                .collect()
        }

        fn drain_all(&self) -> Vec<(DeviceId, DiscoveredDevice)> {
            self.inner.lock().unwrap().drain().collect()
        }

        fn evict_stale(&self, timeout: Duration) -> Vec<(DeviceId, SocketAddr)> {
            let mut evicted = Vec::new();
            let now = std::time::Instant::now();
            self.inner.lock().unwrap().retain(|id, device| {
                if now.duration_since(device.last_seen) > timeout {
                    evicted.push((id.clone(), device.addr));
                    false
                } else {
                    true
                }
            });
            evicted
        }
    }
}
