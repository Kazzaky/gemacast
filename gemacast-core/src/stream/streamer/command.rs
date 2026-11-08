use crate::audio::AudioSessionKey;
use crate::domain::types::{AudioSource, DeviceId};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSessionFailure {
    pub device_id: DeviceId,
    pub generation: u64,
}

#[cfg(test)]
pub(crate) type SessionInspection = (Option<SocketAddr>, AudioSource, Option<i32>);

#[derive(Clone)]
pub struct TcpBroadcastLease {
    pub broadcaster: broadcast::Sender<Arc<Vec<u8>>>,
    pub session_generation: u64,
}

/// Commands accepted by the streamer audio engine.
pub enum AudioStreamCommand {
    Subscribe {
        device_id: DeviceId,
        generation: u64,
        target_addr: Option<SocketAddr>,
        source: Option<AudioSource>,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Unsubscribe {
        device_id: DeviceId,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ChangeSource {
        device_id: DeviceId,
        source: AudioSource,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ChangeBitrate {
        device_id: DeviceId,
        bitrate: Option<i32>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    GetTcpBroadcaster {
        device_id: DeviceId,
        reply: oneshot::Sender<Option<TcpBroadcastLease>>,
    },
    TransportClosed {
        device_id: DeviceId,
        generation: u64,
    },
    #[cfg(test)]
    InspectSession {
        device_id: DeviceId,
        reply: oneshot::Sender<Option<SessionInspection>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), String>>,
    },
}
