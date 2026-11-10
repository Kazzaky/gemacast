use serde::{Deserialize, Serialize};

use crate::domain::types::{AudioSource, ConnectionMode, DeviceId, JitterConfig, NetworkLink};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectReq {
    pub device_id: DeviceId,
    pub device_name: String,
    pub mode: ConnectionMode,
    pub jitter_config: JitterConfig,
    #[serde(default = "default_bitrate")]
    pub bitrate: Option<i32>,

    #[serde(default)]
    pub source: Option<AudioSource>,

    #[serde(default)]
    pub network_link: Option<NetworkLink>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_request_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_auth: Option<DeviceAuthRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthRequest {
    pub public_key: String,
    pub phone_nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenge_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_confirmation: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthChallenge {
    pub challenge_id: String,
    pub challenge: String,
    pub pc_certificate_fingerprint: String,
    pub pairing_code: String,
    pub requires_approval: bool,
    pub expires_in_seconds: u64,
}

fn default_bitrate() -> Option<i32> {
    Some(128_000)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectReq {
    pub device_id: DeviceId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReq {
    #[serde(default)]
    pub device_id: Option<DeviceId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSourceReq {
    pub device_id: DeviceId,
    pub source: AudioSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeBitrateReq {
    pub device_id: DeviceId,
    pub bitrate: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcesResponse {
    pub sources: Vec<AudioSource>,
    pub capabilities: crate::domain::types::StreamerCapabilities,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceResponse {
    pub device_id: DeviceId,
    pub streamer_name: String,
    pub is_offline: bool,

    #[serde(default)]
    pub pc_network_link: Option<NetworkLink>,

    #[serde(default)]
    pub device_registered: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_generation: Option<crate::control::auth::SessionGeneration>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_request_id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_auth_challenge: Option<DeviceAuthChallenge>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc_certificate_fingerprint: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pc_output_volume: Option<f32>,
}

impl std::fmt::Debug for PresenceResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PresenceResponse")
            .field("device_id", &self.device_id)
            .field("streamer_name", &self.streamer_name)
            .field("is_offline", &self.is_offline)
            .field("pc_network_link", &self.pc_network_link)
            .field("device_registered", &self.device_registered)
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("session_generation", &self.session_generation)
            .field(
                "pending_request_id",
                &self.pending_request_id.as_ref().map(|_| "[REDACTED]"),
            )
            .field("device_auth_challenge", &self.device_auth_challenge)
            .field(
                "pc_certificate_fingerprint",
                &self.pc_certificate_fingerprint,
            )
            .field("pc_output_volume", &self.pc_output_volume)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListResponse {
    pub processes: Vec<crate::domain::types::ProcessInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlErrorResponse {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type", content = "payload")]
pub enum WsEvent {
    HeartbeatAcknowledged,
    Disconnect,
    Error { message: String },
    VolumeChanged { level: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type", content = "payload")]
pub enum WsCommand {
    Heartbeat,
    Disconnect,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{
        AudioSource, ConnectionMode, DeviceId, JitterConfig, StreamerCapabilities,
    };

    mod presence_response_debug {
        use super::*;

        #[test]
        fn debug_output_must_redact_session_tokens_and_pending_request_ids() {
            let token = "secret-session-token";
            let request_id = "secret-pending-request";
            let response = PresenceResponse {
                device_id: DeviceId("phone-1".into()),
                streamer_name: "PC".into(),
                is_offline: false,
                pc_network_link: None,
                device_registered: Some(true),
                session_token: Some(token.into()),
                session_generation: Some(crate::control::SessionGeneration(1)),
                pending_request_id: Some(request_id.into()),
                device_auth_challenge: None,
                pc_certificate_fingerprint: Some("ab".repeat(32)),
                pc_output_volume: None,
            };

            let debug = format!("{response:?}");
            assert!(!debug.contains(token));
            assert!(!debug.contains(request_id));
            assert_eq!(debug.matches("[REDACTED]").count(), 2);
        }
    }

    mod connect_req {
        use super::*;

        #[test]
        fn serde_should_default_bitrate_to_128k_when_omitted() {
            let json = r#"{
                "deviceId": "phone_1",
                "deviceName": "My Phone",
                "mode": "wifi",
                "jitterConfig": {
                    "minDepthMs": 0,
                    "comfortCapMs": 0,
                    "peakDecayHalflifeMs": 1000,
                    "resumeThresholdPct": 0.0
                }
            }"#;
            let req: ConnectReq = serde_json::from_str(json).unwrap();
            assert_eq!(
                req.bitrate,
                Some(128_000),
                "Expected default bitrate of 128000"
            );
        }

        #[test]
        fn serde_should_round_trip_with_explicit_source() {
            let req = ConnectReq {
                device_id: DeviceId("dev_1".to_string()),
                device_name: "Test".to_string(),
                mode: ConnectionMode::Wifi,
                jitter_config: JitterConfig::default(),
                bitrate: Some(256_000),
                source: Some(AudioSource::Process {
                    pid: 42,
                    name: "spotify.exe".to_string(),
                }),
                network_link: None,
                pending_request_id: None,
                device_auth: None,
            };
            let json = serde_json::to_string(&req).unwrap();
            let parsed: ConnectReq = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.device_id, req.device_id);
            assert_eq!(parsed.bitrate, Some(256_000));
            assert!(matches!(
                parsed.source,
                Some(AudioSource::Process { pid: 42, .. })
            ));
        }
    }

    mod sources_response {
        use super::*;

        #[test]
        fn serde_should_round_trip() {
            let resp = SourcesResponse {
                sources: vec![
                    AudioSource::Desktop,
                    AudioSource::Process {
                        pid: 100,
                        name: "app.exe".to_string(),
                    },
                ],
                capabilities: StreamerCapabilities {
                    supports_process_capture: true,
                    supports_volume_sync: true,
                },
            };
            let json = serde_json::to_string(&resp).unwrap();
            let parsed: SourcesResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.sources.len(), 2);
            assert!(parsed.capabilities.supports_process_capture);
            assert!(parsed.capabilities.supports_volume_sync);
        }

        #[test]
        fn volume_sync_support_defaults_to_false_when_the_key_is_absent() {
            let json = r#"{"sources":[],"capabilities":{"supportsProcessCapture":true}}"#;
            let parsed: SourcesResponse = serde_json::from_str(json).unwrap();
            assert!(!parsed.capabilities.supports_volume_sync);
        }
    }

    mod presence_response {
        use super::*;

        #[test]
        fn the_pc_output_volume_uses_a_camel_case_key() {
            let resp = PresenceResponse {
                device_id: DeviceId("pc_1".to_string()),
                streamer_name: "Desk PC".to_string(),
                is_offline: false,
                pc_network_link: None,
                device_registered: None,
                session_token: None,
                session_generation: None,
                pending_request_id: None,
                device_auth_challenge: None,
                pc_certificate_fingerprint: None,
                pc_output_volume: Some(0.4),
            };
            let json = serde_json::to_string(&resp).unwrap();
            assert!(
                json.contains("\"pcOutputVolume\":0.4"),
                "Expected camelCase pcOutputVolume key, got: {json}"
            );
            let parsed: PresenceResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.pc_output_volume, Some(0.4));
        }

        #[test]
        fn the_pc_output_volume_is_omitted_when_unknown() {
            let resp = PresenceResponse {
                device_id: DeviceId("pc_1".to_string()),
                streamer_name: "Desk PC".to_string(),
                is_offline: false,
                pc_network_link: None,
                device_registered: None,
                session_token: None,
                session_generation: None,
                pending_request_id: None,
                device_auth_challenge: None,
                pc_certificate_fingerprint: None,
                pc_output_volume: None,
            };
            let json = serde_json::to_string(&resp).unwrap();
            assert!(
                !json.contains("pcOutputVolume"),
                "Expected the key to be skipped, got: {json}"
            );
        }
    }

    mod ws_event {
        use super::*;

        #[test]
        fn heartbeat_acknowledgement_should_round_trip() {
            let event = WsEvent::HeartbeatAcknowledged;
            let json = serde_json::to_string(&event).unwrap();
            let parsed: WsEvent = serde_json::from_str(&json).unwrap();

            assert!(matches!(parsed, WsEvent::HeartbeatAcknowledged));
        }

        #[test]
        fn error_variant_should_serialize_with_screaming_snake_case_tag() {
            let event = WsEvent::Error {
                message: "something broke".to_string(),
            };
            let json = serde_json::to_string(&event).unwrap();
            assert!(
                json.contains("\"type\":\"ERROR\"") || json.contains("\"type\": \"ERROR\""),
                "Expected SCREAMING_SNAKE_CASE type tag, got: {json}"
            );
            let parsed: WsEvent = serde_json::from_str(&json).unwrap();
            assert!(matches!(parsed, WsEvent::Error { message } if message == "something broke"));
        }

        #[test]
        fn disconnect_variant_should_round_trip() {
            let event = WsEvent::Disconnect;
            let json = serde_json::to_string(&event).unwrap();
            let parsed: WsEvent = serde_json::from_str(&json).unwrap();
            assert!(matches!(parsed, WsEvent::Disconnect));
        }

        #[test]
        fn volume_changed_variant_should_serialize_with_screaming_snake_case_tag() {
            let event = WsEvent::VolumeChanged { level: 0.25 };
            let json = serde_json::to_string(&event).unwrap();
            assert!(
                json.contains("\"type\":\"VOLUME_CHANGED\""),
                "Expected SCREAMING_SNAKE_CASE type tag, got: {json}"
            );
            let parsed: WsEvent = serde_json::from_str(&json).unwrap();
            assert!(matches!(parsed, WsEvent::VolumeChanged { level } if level == 0.25));
        }

        #[test]
        fn volume_changed_carries_its_level_in_the_payload_field() {
            let json = serde_json::to_string(&WsEvent::VolumeChanged { level: 1.0 }).unwrap();
            assert!(
                json.contains("\"payload\":{\"level\":1.0}"),
                "Expected a nested payload object, got: {json}"
            );
        }
    }

    mod ws_command {
        use super::*;

        #[test]
        fn heartbeat_should_round_trip() {
            let command = WsCommand::Heartbeat;
            let json = serde_json::to_string(&command).unwrap();
            let parsed: WsCommand = serde_json::from_str(&json).unwrap();

            assert!(matches!(parsed, WsCommand::Heartbeat));
        }

        #[test]
        fn disconnect_should_round_trip() {
            let cmd = WsCommand::Disconnect;
            let json = serde_json::to_string(&cmd).unwrap();
            let parsed: WsCommand = serde_json::from_str(&json).unwrap();
            assert!(matches!(parsed, WsCommand::Disconnect));
        }
    }
}
