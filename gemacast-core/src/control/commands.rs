use std::net::SocketAddr;

use tokio::sync::oneshot;

use crate::control::auth::SessionGeneration;
use crate::control::types::{DeviceAuthRequest, PresenceResponse, SourcesResponse};
use crate::domain::types::{AudioSource, DeviceId};

/// Commands sent from the control HTTP surface to the application dispatcher.
#[derive(Debug)]
pub enum ControlCommand {
    Connect {
        device_id: DeviceId,
        device_name: String,
        source: Option<AudioSource>,
        remote_addr: SocketAddr,
        bitrate: Option<i32>,
        response_tx: oneshot::Sender<Result<PresenceResponse, String>>,
        authorized: bool,
        pending_request_id: Option<String>,
        device_auth: Option<DeviceAuthRequest>,
    },
    Disconnect {
        device_id: DeviceId,
        remote_addr: SocketAddr,
        generation: Option<SessionGeneration>,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    GetSources {
        response_tx: oneshot::Sender<SourcesResponse>,
    },
    ChangeSource {
        device_id: DeviceId,
        generation: SessionGeneration,
        source: AudioSource,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    ChangeBitrate {
        device_id: DeviceId,
        generation: SessionGeneration,
        bitrate: Option<i32>,
        response_tx: oneshot::Sender<Result<(), String>>,
    },
    SessionHeartbeat {
        device_id: DeviceId,
        generation: SessionGeneration,
    },
    Probe {
        device_id: Option<DeviceId>,
        response_tx: oneshot::Sender<PresenceResponse>,
    },
}
