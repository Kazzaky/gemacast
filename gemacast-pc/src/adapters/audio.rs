use crate::traits::AudioController;
use async_trait::async_trait;
use gemacast_core::audio::AudioSessionKey;
use gemacast_core::domain::types::{AudioSource, DeviceId};
use gemacast_core::stream::streamer::AudioStreamCommand;
use std::net::SocketAddr;
use tokio::sync::mpsc;

/// Sends [`AudioStreamCommand`]s to the audio engine via an `mpsc` channel.
pub struct ChannelAudioController {
    tx: mpsc::Sender<AudioStreamCommand>,
}

impl ChannelAudioController {
    pub fn new(tx: mpsc::Sender<AudioStreamCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl AudioController for ChannelAudioController {
    async fn subscribe(
        &self,
        device_id: DeviceId,
        generation: u64,
        target_addr: Option<SocketAddr>,
        source: Option<AudioSource>,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
    ) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.tx
            .send(AudioStreamCommand::Subscribe {
                device_id,
                generation,
                target_addr,
                source,
                bitrate,
                audio_key,
                reply,
            })
            .await
            .map_err(|_| "audio engine is unavailable".to_string())?;
        response
            .await
            .map_err(|_| "audio engine dropped the subscribe acknowledgement".to_string())?
    }

    async fn unsubscribe(&self, device_id: &DeviceId) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.tx
            .send(AudioStreamCommand::Unsubscribe {
                device_id: device_id.clone(),
                reply,
            })
            .await
            .map_err(|_| "audio engine is unavailable".to_string())?;
        response
            .await
            .map_err(|_| "audio engine dropped the unsubscribe acknowledgement".to_string())?
    }

    async fn change_source(&self, device_id: DeviceId, source: AudioSource) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.tx
            .send(AudioStreamCommand::ChangeSource {
                device_id,
                source,
                reply,
            })
            .await
            .map_err(|_| "audio engine is unavailable".to_string())?;
        response
            .await
            .map_err(|_| "audio engine dropped the source acknowledgement".to_string())?
    }

    async fn change_bitrate(
        &self,
        device_id: DeviceId,
        bitrate: Option<i32>,
    ) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.tx
            .send(AudioStreamCommand::ChangeBitrate {
                device_id,
                bitrate,
                reply,
            })
            .await
            .map_err(|_| "audio engine is unavailable".to_string())?;
        response
            .await
            .map_err(|_| "audio engine dropped the bitrate acknowledgement".to_string())?
    }

    async fn shutdown(&self) -> Result<(), String> {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.tx
            .send(AudioStreamCommand::Shutdown { reply })
            .await
            .map_err(|_| "audio engine is unavailable".to_string())?;
        response
            .await
            .map_err(|_| "audio engine dropped the shutdown acknowledgement".to_string())?
    }
}
