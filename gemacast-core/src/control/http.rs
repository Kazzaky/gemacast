//! HTTPS control server facade.
//!
//! The server is split by responsibility: [`super::state`] owns shared state,
//! [`super::handlers`] translates HTTP requests, and [`super::server`] owns
//! routing and TLS listener setup. This module keeps the historical public
//! paths stable for application crates.

pub use crate::control::commands::ControlCommand;
pub use crate::control::events::ControlEventBus;
pub use crate::control::events::{broadcast as broadcast_ws_event, send as send_ws_event};
#[cfg(test)]
pub(crate) use crate::control::handlers::connect_error_code;
#[cfg(test)]
pub(crate) use crate::control::server::PlainListener;
pub use crate::control::server::start_control_server;
#[cfg(test)]
pub(crate) use crate::control::server::{PeerAddr, build_router};
pub use crate::control::state::ControlServerState;

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    use super::*;
    use crate::control::types::{
        ChangeBitrateReq, ChangeSourceReq, ConnectReq, ControlErrorResponse, PresenceResponse,
        ProbeReq,
    };
    use crate::domain::types::{AudioSource, ConnectionMode, DeviceId};
    use crate::ports::process_lister::ProcessLister;

    #[derive(Clone)]
    struct MockProcessLister;

    impl ProcessLister for MockProcessLister {
        fn list_processes(&self) -> Vec<crate::domain::types::ProcessInfo> {
            Vec::new()
        }
    }

    #[test]
    fn connect_errors_should_map_to_stable_client_codes() {
        let cases = [
            ("pairing was cancelled on the phone", "pairing_cancelled"),
            (
                "connection request request-1 was rejected on the PC",
                "pairing_rejected",
            ),
            (
                "connection request request-1 is invalid or expired",
                "pairing_expired",
            ),
            ("device authentication challenge expired", "pairing_expired"),
            (
                "too many pending device-authentication requests",
                "pairing_capacity_exhausted",
            ),
            (
                "device authentication signature is invalid",
                "authentication_failed",
            ),
            (
                "failed to remember the approved device: disk full",
                "pairing_persistence_failed",
            ),
            ("failed to initialize audio capture", "stream_start_failed"),
        ];

        for (message, expected) in cases {
            assert_eq!(connect_error_code(message), expected, "message: {message}");
        }
    }

    async fn spawn_test_server() -> (
        String,
        mpsc::Receiver<ControlCommand>,
        crate::control::SessionAuthorizer,
    ) {
        spawn_test_server_with_broadcasting(true).await
    }

    async fn spawn_test_server_with_broadcasting(
        broadcasting: bool,
    ) -> (
        String,
        mpsc::Receiver<ControlCommand>,
        crate::control::SessionAuthorizer,
    ) {
        let (command_tx, command_rx) = mpsc::channel(10);
        let authorizer = crate::control::SessionAuthorizer::default();
        let state = ControlServerState {
            command_tx,
            is_broadcasting: Arc::new(AtomicBool::new(broadcasting)),
            streamer_id: DeviceId("test-streamer".to_string()),
            streamer_name: "Test Streamer".to_string(),
            ws_connections: Arc::new(Mutex::new(HashMap::new())),
            process_lister: MockProcessLister,
            authorizer: authorizer.clone(),
            pc_certificate_fingerprint: "test-certificate".to_string(),
        };

        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            axum::serve(
                PlainListener(listener),
                app.into_make_service_with_connect_info::<PeerAddr>(),
            )
            .await
            .unwrap();
        });

        (format!("http://127.0.0.1:{port}"), command_rx, authorizer)
    }

    fn connect_request(device_id: &str) -> ConnectReq {
        ConnectReq {
            device_id: DeviceId(device_id.to_string()),
            device_name: "Test Device".to_string(),
            source: None,
            bitrate: None,
            jitter_config: crate::domain::types::JitterConfig::default(),
            mode: ConnectionMode::Wifi,
            network_link: None,
            pending_request_id: None,
            device_auth: None,
        }
    }

    fn presence(device_id: DeviceId, pending_request_id: Option<&str>) -> PresenceResponse {
        PresenceResponse {
            device_id,
            streamer_name: "Test Streamer".to_string(),
            is_offline: false,
            pc_network_link: None,
            device_registered: Some(pending_request_id.is_none()),
            session_token: None,
            session_generation: None,
            pending_request_id: pending_request_id.map(str::to_string),
            device_auth_challenge: None,
            pc_certificate_fingerprint: None,
            pc_output_volume: None,
        }
    }

    #[tokio::test]
    async fn connect_endpoint_should_dispatch_command_and_return_presence() {
        let (base_url, mut command_rx, _) = spawn_test_server().await;
        let client = reqwest::Client::new();
        let req_body = connect_request("test-device");

        let request_task = tokio::spawn(async move {
            client
                .post(format!("{base_url}/connect"))
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        let cmd = command_rx.recv().await.unwrap();
        match cmd {
            ControlCommand::Connect {
                device_id,
                device_name,
                source,
                bitrate,
                response_tx,
                ..
            } => {
                assert_eq!(device_id.0, "test-device");
                assert_eq!(device_name, "Test Device");
                assert!(source.is_none());
                assert!(bitrate.is_none());
                let _ = response_tx.send(Ok(presence(device_id, None)));
            }
            _ => panic!("Expected ControlCommand::Connect"),
        }

        let response = request_task.await.unwrap();
        assert!(response.status().is_success());
    }

    #[tokio::test]
    async fn connect_endpoint_should_return_accepted_for_pending_approval() {
        let (base_url, mut command_rx, _) = spawn_test_server().await;
        let client = reqwest::Client::new();
        let req_body = connect_request("pending-device");

        let request_task = tokio::spawn(async move {
            client
                .post(format!("{base_url}/connect"))
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        match command_rx.recv().await.unwrap() {
            ControlCommand::Connect { response_tx, .. } => {
                let _ = response_tx.send(Ok(presence(
                    DeviceId("test-streamer".into()),
                    Some("request-1"),
                )));
            }
            _ => panic!("Expected ControlCommand::Connect"),
        }

        let response = request_task.await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        let presence: PresenceResponse = response.json().await.unwrap();
        assert_eq!(presence.pending_request_id.as_deref(), Some("request-1"));
    }

    #[tokio::test]
    async fn change_source_endpoint_should_dispatch_command() {
        let (base_url, mut command_rx, authorizer) = spawn_test_server().await;
        let client = reqwest::Client::new();
        let req_body = ChangeSourceReq {
            device_id: DeviceId("test-device-2".to_string()),
            source: AudioSource::Desktop,
        };
        let (token, _) = authorizer.issue(req_body.device_id.clone()).unwrap();

        let request_task = tokio::spawn(async move {
            client
                .post(format!("{base_url}/change-source"))
                .bearer_auth(token)
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        match command_rx.recv().await.unwrap() {
            ControlCommand::ChangeSource {
                device_id,
                source,
                response_tx,
            } => {
                assert_eq!(device_id.0, "test-device-2");
                assert_eq!(source, AudioSource::Desktop);
                let _ = response_tx.send(Ok(()));
            }
            _ => panic!("Expected ControlCommand::ChangeSource"),
        }

        assert!(request_task.await.unwrap().status().is_success());
    }

    #[tokio::test]
    async fn change_bitrate_endpoint_should_dispatch_command() {
        let (base_url, mut command_rx, authorizer) = spawn_test_server().await;
        let client = reqwest::Client::new();
        let req_body = ChangeBitrateReq {
            device_id: DeviceId("test-device-3".to_string()),
            bitrate: Some(192000),
        };
        let (token, _) = authorizer.issue(req_body.device_id.clone()).unwrap();

        let request_task = tokio::spawn(async move {
            client
                .post(format!("{base_url}/change-bitrate"))
                .bearer_auth(token)
                .json(&req_body)
                .send()
                .await
                .unwrap()
        });

        match command_rx.recv().await.unwrap() {
            ControlCommand::ChangeBitrate {
                device_id,
                bitrate,
                response_tx,
            } => {
                assert_eq!(device_id.0, "test-device-3");
                assert_eq!(bitrate, Some(192000));
                let _ = response_tx.send(Ok(()));
            }
            _ => panic!("Expected ControlCommand::ChangeBitrate"),
        }

        assert!(request_task.await.unwrap().status().is_success());
    }

    #[tokio::test]
    async fn connect_endpoint_should_reject_when_not_broadcasting() {
        let (base_url, _command_rx, _) = spawn_test_server_with_broadcasting(false).await;
        let response = reqwest::Client::new()
            .post(format!("{base_url}/connect"))
            .json(&connect_request("test-device"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        let body: ControlErrorResponse = response.json().await.unwrap();
        assert_eq!(body.code, "streamer_offline");
    }

    #[tokio::test]
    async fn unauthenticated_probe_should_return_only_public_presence() {
        let (base_url, _command_rx, _) = spawn_test_server().await;
        let response = reqwest::Client::new()
            .post(format!("{base_url}/probe"))
            .json(&ProbeReq { device_id: None })
            .send()
            .await
            .unwrap();

        assert!(response.status().is_success());
        let body: PresenceResponse = response.json().await.unwrap();
        assert_eq!(body.device_id.0, "test-streamer");
        assert!(!body.is_offline);
        assert_eq!(body.device_registered, None);
        assert_eq!(body.pc_output_volume, None);
    }

    #[tokio::test]
    async fn unauthenticated_device_probe_should_not_reveal_session_state() {
        let (base_url, _command_rx, _) = spawn_test_server().await;
        let response = reqwest::Client::new()
            .post(format!("{base_url}/probe"))
            .json(&ProbeReq {
                device_id: Some(DeviceId("phone-1".into())),
            })
            .send()
            .await
            .unwrap();

        assert!(response.status().is_success());
        let body: PresenceResponse = response.json().await.unwrap();
        assert_eq!(body.device_registered, None);
        assert_eq!(body.pc_output_volume, None);
    }

    #[tokio::test]
    async fn authenticated_device_probe_should_return_session_state() {
        let (base_url, mut command_rx, authorizer) = spawn_test_server().await;
        let device_id = DeviceId("phone-1".into());
        let (token, _) = authorizer.issue(device_id.clone()).unwrap();
        let request_task = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("{base_url}/probe"))
                .bearer_auth(token)
                .json(&ProbeReq {
                    device_id: Some(device_id.clone()),
                })
                .send()
                .await
                .unwrap()
        });

        match command_rx.recv().await.unwrap() {
            ControlCommand::Probe {
                device_id: Some(device_id),
                response_tx,
            } => {
                assert_eq!(device_id.0, "phone-1");
                let _ = response_tx.send(presence(DeviceId("test-streamer".into()), None));
            }
            _ => panic!("Expected authenticated device probe"),
        }

        let response = request_task.await.unwrap();
        assert!(response.status().is_success());
        let body: PresenceResponse = response.json().await.unwrap();
        assert_eq!(body.device_registered, Some(true));
    }
}
