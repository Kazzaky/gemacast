use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

use super::capture_instance::AudioCaptureInstance;
use super::failure::StreamTaskFailure;
use crate::audio::AudioSessionKey;
use crate::domain::error::{AudioError, GemaCastError};
use crate::domain::types::{AudioSource, TargetId};
use crate::ports::capture::CaptureFactory;

pub struct CapturePool<F: CaptureFactory> {
    pub(crate) instances: HashMap<AudioSource, AudioCaptureInstance>,
    max_instances: usize,
    pub supports_process_capture: bool,
    factory: F,
    next_generation: u64,
    failure_tx: mpsc::UnboundedSender<StreamTaskFailure>,
    failure_rx: mpsc::UnboundedReceiver<StreamTaskFailure>,
}

impl<F: CaptureFactory> CapturePool<F> {
    pub fn new(factory: F, supports_process_capture: bool) -> Self {
        let (failure_tx, failure_rx) = mpsc::unbounded_channel();
        Self {
            instances: HashMap::new(),
            max_instances: 8,
            supports_process_capture,
            factory,
            next_generation: 0,
            failure_tx,
            failure_rx,
        }
    }

    pub(crate) async fn recv_failure(&mut self) -> Option<StreamTaskFailure> {
        self.failure_rx.recv().await
    }

    pub async fn subscribe(
        &mut self,
        source: AudioSource,
        target: TargetId,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
    ) -> Result<Option<broadcast::Sender<Arc<Vec<u8>>>>, GemaCastError> {
        self.subscribe_with_channel(source, target, bitrate, audio_key, None)
            .await
    }

    async fn subscribe_with_channel(
        &mut self,
        source: AudioSource,
        target: TargetId,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
        reusable_tcp_channel: Option<broadcast::Sender<Arc<Vec<u8>>>>,
    ) -> Result<Option<broadcast::Sender<Arc<Vec<u8>>>>, GemaCastError> {
        if !self.instances.contains_key(&source) {
            if self.instances.len() >= self.max_instances {
                return Err(AudioError::CapturePoolExhausted {
                    max: self.max_instances,
                }
                .into());
            }

            let handle = match &source {
                AudioSource::Desktop => self.factory.create_desktop_capture()?,
                AudioSource::Process { pid, .. } => {
                    if !self.supports_process_capture {
                        return Err(AudioError::ProcessCaptureUnavailable.into());
                    }
                    self.factory.create_process_capture(*pid)?
                }
            };

            self.next_generation = self.next_generation.wrapping_add(1).max(1);
            let instance = AudioCaptureInstance::new(
                handle,
                source.clone(),
                self.next_generation,
                self.failure_tx.clone(),
            )?;
            self.instances.insert(source.clone(), instance);
        }

        let instance = self.instances.get_mut(&source).unwrap();
        let ret = match target {
            TargetId::Udp(addr) => {
                instance.spawn_target_encoder(addr, bitrate, audio_key).await?;
                None
            }
            TargetId::Tcp(device_id) => Some(
                instance
                    .spawn_tcp_encoder_with_channel(device_id, bitrate, reusable_tcp_channel)
                    .await?,
            ),
        };

        Ok(ret)
    }

    pub async fn unsubscribe(
        &mut self,
        source: &AudioSource,
        target: TargetId,
    ) -> Result<(), GemaCastError> {
        if let Some(instance) = self.instances.get_mut(source) {
            match target {
                TargetId::Udp(addr) => {
                    instance.remove_target_encoder(&addr).await;
                }
                TargetId::Tcp(device_id) => {
                    instance.remove_tcp_encoder(&device_id).await;
                }
            }

            if instance.per_target_encoders.is_empty()
                && instance.tcp_encoders.is_empty()
                && let Some(mut removed) = self.instances.remove(source)
                && let Some(stop_tx) = removed.capture_shutdown_tx.take()
            {
                let _ = stop_tx.send(());
                let _ = removed.capture_join_handle.await;
            }
        }
        Ok(())
    }

    pub async fn change_source(
        &mut self,
        old_source: &AudioSource,
        new_source: AudioSource,
        target: TargetId,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
    ) -> Result<Option<broadcast::Sender<Arc<Vec<u8>>>>, GemaCastError> {
        if old_source == &new_source {
            return self.subscribe(new_source, target, bitrate, audio_key).await;
        }

        let reusable_tcp_channel = match &target {
            TargetId::Tcp(device_id) => self
                .instances
                .get(old_source)
                .and_then(|instance| instance.tcp_broadcaster(device_id)),
            TargetId::Udp(_) => None,
        };
        let tx = self
            .subscribe_with_channel(new_source, target.clone(), bitrate, audio_key, reusable_tcp_channel)
            .await?;
        let _ = self.unsubscribe(old_source, target).await;
        Ok(tx)
    }

    pub async fn change_bitrate(
        &mut self,
        source: &AudioSource,
        target: TargetId,
        bitrate: Option<i32>,
        audio_key: AudioSessionKey,
    ) -> Result<Option<broadcast::Sender<Arc<Vec<u8>>>>, GemaCastError> {
        if let Some(instance) = self.instances.get_mut(source) {
            match target {
                TargetId::Udp(addr) => {
                    instance.spawn_target_encoder(addr, bitrate, audio_key).await?;
                    Ok(None)
                }
                TargetId::Tcp(device_id) => {
                    // `spawn_tcp_encoder_with_channel` creates and validates the
                    // replacement encoder before removing the old task, while
                    // reusing its broadcast channel. A failed bitrate change
                    // therefore leaves the old stream intact.
                    let tx = instance
                        .spawn_tcp_encoder_with_channel(device_id, bitrate, None)
                        .await?;
                    Ok(Some(tx))
                }
            }
        } else {
            Err(AudioError::SourceNotSubscribed.into())
        }
    }

    pub fn tcp_broadcaster(
        &self,
        source: &AudioSource,
        device_id: &crate::domain::types::DeviceId,
    ) -> Option<broadcast::Sender<Arc<Vec<u8>>>> {
        self.instances
            .get(source)
            .and_then(|instance| instance.tcp_broadcaster(device_id))
    }

    pub async fn shutdown_all(&mut self) {
        let sources: Vec<_> = self.instances.keys().cloned().collect();
        for source in sources {
            if let Some(mut instance) = self.instances.remove(&source) {
                for (_, encoder) in instance.per_target_encoders.drain() {
                    let _ = encoder.shutdown_tx.send(());
                    let _ = encoder.join_handle.await;
                }
                for (_, encoder) in instance.tcp_encoders.drain() {
                    let _ = encoder.shutdown_tx.send(());
                    let _ = encoder.join_handle.await;
                }
                if let Some(stop_tx) = instance.capture_shutdown_tx.take() {
                    let _ = stop_tx.send(());
                }
                let _ = instance.capture_join_handle.await;
            }
        }
    }

    pub async fn evict_failed_source(&mut self, source: &AudioSource, generation: u64) -> bool {
        if self
            .instances
            .get(source)
            .is_none_or(|instance| instance.generation != generation)
        {
            return false;
        }

        if let Some(mut instance) = self.instances.remove(source) {
            for (_, encoder) in instance.per_target_encoders.drain() {
                let _ = encoder.shutdown_tx.send(());
                let _ = encoder.join_handle.await;
            }
            for (_, encoder) in instance.tcp_encoders.drain() {
                let _ = encoder.shutdown_tx.send(());
                let _ = encoder.join_handle.await;
            }
            if let Some(stop_tx) = instance.capture_shutdown_tx.take() {
                let _ = stop_tx.send(());
            }
            let _ = instance.capture_join_handle.await;
        }
        true
    }

    pub(crate) async fn remove_failed_target(&mut self, failure: &StreamTaskFailure) -> bool {
        match failure {
            StreamTaskFailure::UdpEncoder {
                source,
                generation,
                encoder_generation,
                target,
                ..
            } => {
                let Some(instance) = self.instances.get_mut(source) else {
                    return false;
                };
                if instance.generation != *generation {
                    return false;
                }
                if instance
                    .per_target_encoders
                    .get(target)
                    .is_none_or(|encoder| encoder.generation != *encoder_generation)
                {
                    return false;
                }
                instance.remove_target_encoder(target).await;
                true
            }
            StreamTaskFailure::TcpEncoder {
                source,
                generation,
                encoder_generation,
                device_id,
                ..
            } => {
                let Some(instance) = self.instances.get_mut(source) else {
                    return false;
                };
                if instance.generation != *generation {
                    return false;
                }
                if instance
                    .tcp_encoders
                    .get(device_id)
                    .is_none_or(|encoder| encoder.generation != *encoder_generation)
                {
                    return false;
                }
                instance.remove_tcp_encoder(device_id).await;
                true
            }
            StreamTaskFailure::Capture { .. } => false,
        }
    }
}
