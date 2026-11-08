use crate::audio::AudioSessionKey;
use crate::domain::types::{AudioSource, DeviceId, TargetId};
use std::net::SocketAddr;

/// Describes one player's active capture subscription.
#[derive(Clone)]
pub(crate) struct StreamSession {
    pub(crate) target_addr: Option<SocketAddr>,
    pub(crate) source: AudioSource,
    pub(crate) bitrate: Option<i32>,
    pub(crate) audio_key: AudioSessionKey,
}

impl StreamSession {
    pub(crate) fn new(
        target_addr: Option<SocketAddr>,
        source: AudioSource,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
    ) -> Self {
        Self {
            target_addr,
            source,
            bitrate,
            audio_key,
        }
    }

    pub(crate) fn target(&self, device_id: &DeviceId) -> TargetId {
        self.target_addr
            .map(TargetId::Udp)
            .unwrap_or_else(|| TargetId::Tcp(device_id.clone()))
    }

    #[cfg(test)]
    pub(crate) fn inspection(&self) -> (Option<SocketAddr>, AudioSource, Option<i32>) {
        (self.target_addr, self.source.clone(), self.bitrate)
    }
}
