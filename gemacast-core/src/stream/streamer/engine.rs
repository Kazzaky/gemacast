use std::collections::HashMap;

use crate::audio::AudioSessionKey;
use tokio::sync::mpsc;

use crate::domain::error::GemaCastError;
use crate::domain::types::{DeviceId, TargetId};
use crate::ports::capture::CaptureFactory;
use crate::ports::error_notifier::ErrorNotifier;

use super::capture_pool::CapturePool;
use super::command::{AudioStreamCommand, StreamSessionFailure, TcpBroadcastLease};
use super::failure::StreamTaskFailure;
use super::stream_session::StreamSession;

pub struct AudioStreamEngine<F: CaptureFactory, N: ErrorNotifier> {
    pool: CapturePool<F>,
    active_player_sessions: HashMap<DeviceId, StreamSession>,
    session_generations: HashMap<DeviceId, u64>,
    error_notifier: N,
    session_failure_tx: Option<mpsc::UnboundedSender<StreamSessionFailure>>,
}

impl<F: CaptureFactory, N: ErrorNotifier> AudioStreamEngine<F, N> {
    pub fn new(factory: F, supports_process_capture: bool, error_notifier: N) -> Self {
        Self {
            pool: CapturePool::new(factory, supports_process_capture),
            active_player_sessions: HashMap::new(),
            session_generations: HashMap::new(),
            error_notifier,
            session_failure_tx: None,
        }
    }

    pub fn with_session_failure_sender(
        mut self,
        session_failure_tx: mpsc::UnboundedSender<StreamSessionFailure>,
    ) -> Self {
        self.session_failure_tx = Some(session_failure_tx);
        self
    }

    pub async fn run_command_loop(
        &mut self,
        mut audio_engine_command_rx: mpsc::Receiver<AudioStreamCommand>,
    ) -> Result<(), GemaCastError> {
        loop {
            tokio::select! {
                command = audio_engine_command_rx.recv() => {
                    let Some(command) = command else {
                        self.pool.shutdown_all().await;
                        self.active_player_sessions.clear();
                        self.session_generations.clear();
                        break;
                    };
                    match command {
                AudioStreamCommand::Subscribe {
                    device_id,
                    generation,
                    target_addr,
                    source,
                    bitrate,
                    audio_key,
                    reply,
                } => {
                    let mut final_source = source.unwrap_or_default();
                    if let Some(session) = self.active_player_sessions.get(&device_id) {
                        final_source = session.source.clone();
                    }

                    tracing::info!(
                        "[Engine] Subscribe device={:?} source={:?} target_addr={:?} bitrate={:?}",
                        device_id,
                        final_source,
                        target_addr,
                        bitrate
                    );

                    let Some(target_addr) = target_addr else {
                        let _ = reply.send(Err(
                            "UDP audio target is required; legacy TCP/ADB audio is disabled".into()
                        ));
                        continue;
                    };
                    let target = TargetId::Udp(target_addr);

                    let old_session = self.active_player_sessions.get(&device_id).cloned();
                    let result = match self
                        .pool
                        .subscribe(final_source.clone(), target.clone(), bitrate, audio_key)
                        .await
                    {
                        Ok(_) => {
                            if let Some(old_session) = old_session {
                                let old_target = old_session.target(&device_id);
                                if old_session.source != final_source || old_target != target {
                                    let _ = self
                                        .pool
                                        .unsubscribe(&old_session.source, old_target)
                                        .await;
                                }
                            }
                            self.active_player_sessions.insert(
                                device_id.clone(),
                                StreamSession::new(Some(target_addr), final_source, bitrate, audio_key),
                            );
                            self.session_generations.insert(device_id, generation);
                            Ok(())
                        }
                        Err(e) => {
                            let msg = format!("Audio capture failed: {}", e);
                            tracing::error!("[Engine] Subscribe failed: {}", msg);
                            self.error_notifier.notify_error(&device_id, msg);
                            Err(e.to_string())
                        }
                    };
                    let _ = reply.send(result);
                }
                AudioStreamCommand::Unsubscribe { device_id, reply } => {
                    tracing::info!("[Engine] Unsubscribe device={:?}", device_id);
                    if let Some(session) = self.active_player_sessions.remove(&device_id) {
                        self.session_generations.remove(&device_id);
                        let target = session.target(&device_id);
                        let result = self
                            .pool
                            .unsubscribe(&session.source, target)
                            .await
                            .map_err(|error| error.to_string());
                        let _ = reply.send(result);
                    } else {
                        let _ = reply.send(Ok(()));
                    }
                }
                AudioStreamCommand::ChangeSource {
                    device_id,
                    source,
                    reply,
                } => {
                    tracing::info!(
                        "[Engine] ChangeSource device={:?} new_source={:?}",
                        device_id,
                        source
                    );

                    tracing::info!(
                        "[Engine] Active sessions: {:?}",
                        self.active_player_sessions.keys().collect::<Vec<_>>()
                    );

                    if let Some(session) = self.active_player_sessions.get(&device_id) {
                        let old_source = session.source.clone();
                        let target_addr = session.target_addr;
                        let bitrate = session.bitrate;
                        let audio_key = session.audio_key;
                        tracing::info!(
                            "[Engine] Found session: old_source={:?} target_addr={:?}",
                            old_source,
                            target_addr
                        );

                        let target = session.target(&device_id);

                        match self
                            .pool
                            .change_source(&old_source, source.clone(), target, bitrate, audio_key)
                            .await
                        {
                            Ok(_broadcast_tx) => {
                                tracing::info!("[Engine] Source changed successfully");
                                self.active_player_sessions
                                    .insert(device_id, StreamSession::new(target_addr, source, bitrate, audio_key));
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let msg = format!("Failed to change audio source: {}", e);
                                tracing::error!(
                                    "[Engine] Failed to change source from {:?} to {:?}: {}",
                                    old_source,
                                    source,
                                    msg
                                );
                                self.error_notifier.notify_error(&device_id, msg);
                                let _ = reply.send(Err(e.to_string()));
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[Engine] ChangeSource: device {:?} not found in active sessions",
                            device_id
                        );
                        let _ =
                            reply.send(Err(format!("device {} has no active session", device_id)));
                    }
                }
                AudioStreamCommand::ChangeBitrate {
                    device_id,
                    bitrate,
                    reply,
                } => {
                    tracing::info!(
                        "[Engine] ChangeBitrate device={:?} new_bitrate={:?}",
                        device_id,
                        bitrate
                    );

                    if let Some(session) = self.active_player_sessions.get(&device_id) {
                        if session.bitrate == bitrate {
                            tracing::info!("[Engine] Bitrate unchanged, skipping.");
                            let _ = reply.send(Ok(()));
                            continue;
                        }

                        let source_clone = session.source.clone();
                        let target_addr_clone = session.target_addr;
                        let audio_key = session.audio_key;

                        tracing::info!(
                            "[Engine] Found session to update bitrate: source={:?} target_addr={:?}",
                            source_clone,
                            target_addr_clone
                        );

                        let target = session.target(&device_id);

                        match self
                            .pool
                            .change_bitrate(&source_clone, target, bitrate, audio_key)
                            .await
                        {
                            Ok(_broadcast_tx) => {
                                tracing::info!("[Engine] Bitrate changed successfully");
                                self.active_player_sessions
                                    .insert(device_id, StreamSession::new(target_addr_clone, source_clone, bitrate, audio_key));
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                let msg = format!("Failed to change bitrate: {}", e);
                                tracing::error!("[Engine] Bitrate change failed: {}", msg);
                                self.error_notifier.notify_error(&device_id, msg);
                                let _ = reply.send(Err(e.to_string()));
                            }
                        }
                    } else {
                        let _ =
                            reply.send(Err(format!("device {} has no active session", device_id)));
                    }
                }
                AudioStreamCommand::GetTcpBroadcaster { device_id, reply } => {
                    tracing::info!("[Engine] GetTcpBroadcaster for device={:?}", device_id);
                    if let Some(session) = self.active_player_sessions.get(&device_id) {
                        if session.target_addr.is_some() {
                            tracing::warn!(
                                "[Engine] GetTcpBroadcaster requested for UDP device={:?}",
                                device_id
                            );
                            let _ = reply.send(None);
                        } else {
                            match self.pool.tcp_broadcaster(&session.source, &device_id) {
                                Some(broadcast_tx) => {
                                    let session_generation = self
                                        .session_generations
                                        .get(&device_id)
                                        .copied()
                                        .unwrap_or_default();
                                    let _ = reply.send(Some(TcpBroadcastLease {
                                        broadcaster: broadcast_tx,
                                        session_generation,
                                    }));
                                }
                                None => {
                                    tracing::warn!(
                                        "[Engine] GetTcpBroadcaster: no active TCP encoder (device={:?})",
                                        device_id
                                    );
                                    let _ = reply.send(None);
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[Engine] GetTcpBroadcaster: No active session for device={:?}",
                            device_id
                        );
                        let _ = reply.send(None);
                    }
                }
                AudioStreamCommand::TransportClosed { device_id, generation } => {
                    if self.session_generations.get(&device_id).copied() != Some(generation) {
                        continue;
                    }
                    if let Some(session) = self.active_player_sessions.remove(&device_id) {
                        self.session_generations.remove(&device_id);
                        let target = session.target(&device_id);
                        let _ = self.pool.unsubscribe(&session.source, target).await;
                        self.report_session_failure(device_id, generation);
                    }
                }
                #[cfg(test)]
                AudioStreamCommand::InspectSession { device_id, reply } => {
                    let _ = reply.send(
                        self.active_player_sessions
                            .get(&device_id)
                            .map(StreamSession::inspection),
                    );
                }
                AudioStreamCommand::Shutdown { reply } => {
                    tracing::info!("[Engine] Shutdown");
                    self.pool.shutdown_all().await;
                    self.active_player_sessions.clear();
                    self.session_generations.clear();
                    let _ = reply.send(Ok(()));
                    break;
                }
                    }
                }
                failure = self.pool.recv_failure() => {
                    let Some(failure) = failure else {
                        continue;
                    };
                    self.handle_stream_failure(failure).await;
                }
            }
        }
        Ok(())
    }

    async fn handle_stream_failure(&mut self, failure: StreamTaskFailure) {
        match failure {
            StreamTaskFailure::Capture {
                source,
                generation,
                message,
            } => {
                if !self.pool.evict_failed_source(&source, generation).await {
                    return;
                }
                let affected: Vec<_> = self
                    .active_player_sessions
                    .iter()
                    .filter(|(_, session)| session.source == source)
                    .map(|(device_id, _)| device_id.clone())
                    .collect();
                for device_id in affected {
                    let generation = self.session_generations.remove(&device_id);
                    self.active_player_sessions.remove(&device_id);
                    self.error_notifier
                        .notify_error(&device_id, format!("Audio source failed: {message}"));
                    if let Some(generation) = generation {
                        self.report_session_failure(device_id, generation);
                    }
                }
            }
            StreamTaskFailure::UdpEncoder {
                target,
                ref message,
                ..
            } => {
                if !self.pool.remove_failed_target(&failure).await {
                    return;
                }
                let device_id = self
                    .active_player_sessions
                    .iter()
                    .find(|(_, session)| session.target_addr == Some(target))
                    .map(|(device_id, _)| device_id.clone());
                if let Some(device_id) = device_id {
                    let generation = self.session_generations.remove(&device_id);
                    self.active_player_sessions.remove(&device_id);
                    self.error_notifier
                        .notify_error(&device_id, format!("Audio encoder failed: {message}"));
                    if let Some(generation) = generation {
                        self.report_session_failure(device_id, generation);
                    }
                }
            }
            StreamTaskFailure::TcpEncoder {
                ref device_id,
                ref message,
                ..
            } => {
                if !self.pool.remove_failed_target(&failure).await {
                    return;
                }
                let generation = self.session_generations.remove(device_id);
                self.active_player_sessions.remove(device_id);
                self.error_notifier
                    .notify_error(device_id, format!("Audio encoder failed: {message}"));
                if let Some(generation) = generation {
                    self.report_session_failure(device_id.clone(), generation);
                }
            }
        }
    }

    fn report_session_failure(&self, device_id: DeviceId, generation: u64) {
        if let Some(tx) = &self.session_failure_tx {
            let _ = tx.send(StreamSessionFailure {
                device_id,
                generation,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_audio_key() -> AudioSessionKey {
        AudioSessionKey::from_session_token(&"55".repeat(32)).unwrap()
    }
    use crate::domain::types::AudioSource;
    use crate::ports::capture::{CaptureBackend, CaptureCounters, CaptureFactory, CaptureHandle};
    use ringbuf::HeapRb;
    use ringbuf::traits::*;
    use std::sync::Arc;
    use tokio::sync::Notify;

    struct MockBackend;
    impl CaptureBackend for MockBackend {
        fn play(&mut self) -> Result<(), GemaCastError> {
            Ok(())
        }
        fn pause(&mut self) -> Result<(), GemaCastError> {
            Ok(())
        }
    }

    struct MockCaptureFactory;
    impl CaptureFactory for MockCaptureFactory {
        type Backend = MockBackend;

        fn create_desktop_capture(&self) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
            let ring_buffer = HeapRb::<f32>::new(48000 * 2);
            let (_producer, consumer) = ring_buffer.split();
            let notify = Arc::new(Notify::new());
            let (_err_tx, err_rx) = mpsc::channel(1);

            Ok(CaptureHandle {
                backend: MockBackend,
                consumer,
                notify,
                stream_error_rx: err_rx,
                counters: Arc::new(CaptureCounters::default()),
            })
        }

        fn create_process_capture(
            &self,
            _pid: u32,
        ) -> Result<CaptureHandle<Self::Backend>, GemaCastError> {
            self.create_desktop_capture()
        }
    }

    struct MockErrorNotifier;
    impl ErrorNotifier for MockErrorNotifier {
        fn notify_error(&self, _device_id: &DeviceId, _message: String) {
            // No-op for tests
        }
    }

    #[tokio::test]
    async fn should_register_session_on_subscribe() {
        let mut engine = AudioStreamEngine::new(MockCaptureFactory, true, MockErrorNotifier);
        let (tx, rx) = mpsc::channel(10);
        let device_id = DeviceId("test-device".to_string());

        let target_addr = Some("127.0.0.1:1234".parse().unwrap());
        let source = AudioSource::Desktop;

        let (subscribe_reply, subscribe_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::Subscribe {
            device_id: device_id.clone(),
            generation: 1,
            target_addr,
            source: Some(source.clone()),
            bitrate: None,
            audio_key: test_audio_key(),
            reply: subscribe_reply,
        })
        .await
        .unwrap();

        let engine_task = tokio::spawn(async move {
            engine.run_command_loop(rx).await.unwrap();
        });
        subscribe_result.await.unwrap().unwrap();

        let (inspect_reply, inspect_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::InspectSession {
            device_id: device_id.clone(),
            reply: inspect_reply,
        })
        .await
        .unwrap();
        let (actual_target, actual_source, _) = inspect_result.await.unwrap().unwrap();

        assert_eq!(actual_target, target_addr);
        assert_eq!(actual_source, source);

        drop(tx);
        engine_task.await.unwrap();
    }

    #[tokio::test]
    async fn should_update_session_source_on_change_source() {
        let mut engine = AudioStreamEngine::new(MockCaptureFactory, true, MockErrorNotifier);
        let (tx, rx) = mpsc::channel(10);
        let device_id = DeviceId("test-device-2".to_string());

        let (subscribe_reply, subscribe_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::Subscribe {
            device_id: device_id.clone(),
            generation: 1,
            target_addr: Some("127.0.0.1:23556".parse().unwrap()),
            source: Some(AudioSource::Desktop),
            bitrate: None,
            audio_key: test_audio_key(),
            reply: subscribe_reply,
        })
        .await
        .unwrap();

        let engine_task = tokio::spawn(async move {
            engine.run_command_loop(rx).await.unwrap();
        });
        subscribe_result.await.unwrap().unwrap();

        let (change_reply, change_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::ChangeSource {
            device_id: device_id.clone(),
            source: AudioSource::Process {
                pid: 1234,
                name: "test".to_string(),
            },
            reply: change_reply,
        })
        .await
        .unwrap();
        change_result.await.unwrap().unwrap();

        let (inspect_reply, inspect_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::InspectSession {
            device_id: device_id.clone(),
            reply: inspect_reply,
        })
        .await
        .unwrap();
        let (_, actual_source, _) = inspect_result.await.unwrap().unwrap();

        assert_eq!(
            actual_source,
            AudioSource::Process {
                pid: 1234,
                name: "test".to_string()
            }
        );

        drop(tx);
        engine_task.await.unwrap();
    }

    #[tokio::test]
    async fn stale_tcp_transport_close_should_not_remove_a_newer_session() {
        let (failure_tx, mut failure_rx) = mpsc::unbounded_channel();
        let mut engine = AudioStreamEngine::new(MockCaptureFactory, true, MockErrorNotifier)
            .with_session_failure_sender(failure_tx);
        let (tx, rx) = mpsc::channel(10);
        let device_id = DeviceId("tcp-device".to_string());
        let current_generation = 2;

        let engine_task = tokio::spawn(async move {
            engine.run_command_loop(rx).await.unwrap();
        });
        let (subscribe_reply, subscribe_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::Subscribe {
            device_id: device_id.clone(),
            generation: current_generation,
            target_addr: Some("127.0.0.1:23556".parse().unwrap()),
            source: Some(AudioSource::Desktop),
            bitrate: None,
            audio_key: test_audio_key(),
            reply: subscribe_reply,
        })
        .await
        .unwrap();
        subscribe_result.await.unwrap().unwrap();

        tx.send(AudioStreamCommand::TransportClosed {
            device_id: device_id.clone(),
            generation: 1,
        })
        .await
        .unwrap();
        let (inspect_reply, inspect_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::InspectSession {
            device_id: device_id.clone(),
            reply: inspect_reply,
        })
        .await
        .unwrap();
        assert!(inspect_result.await.unwrap().is_some());
        assert!(failure_rx.try_recv().is_err());

        tx.send(AudioStreamCommand::TransportClosed {
            device_id: device_id.clone(),
            generation: current_generation,
        })
        .await
        .unwrap();
        assert_eq!(
            failure_rx.recv().await,
            Some(StreamSessionFailure {
                device_id: device_id.clone(),
                generation: current_generation,
            })
        );
        let (inspect_reply, inspect_result) = tokio::sync::oneshot::channel();
        tx.send(AudioStreamCommand::InspectSession {
            device_id,
            reply: inspect_reply,
        })
        .await
        .unwrap();
        assert!(inspect_result.await.unwrap().is_none());

        drop(tx);
        engine_task.await.unwrap();
    }

    #[tokio::test]
    async fn should_remove_session_on_unsubscribe() {
        let mut engine = AudioStreamEngine::new(MockCaptureFactory, true, MockErrorNotifier);
        let (tx, rx) = mpsc::channel(10);
        let device_id = DeviceId("test-device-3".to_string());

        tx.send(AudioStreamCommand::Subscribe {
            device_id: device_id.clone(),
            generation: 1,
            target_addr: None,
            source: Some(AudioSource::Desktop),
            bitrate: None,
            audio_key: test_audio_key(),
            reply: tokio::sync::oneshot::channel().0,
        })
        .await
        .unwrap();

        tx.send(AudioStreamCommand::Unsubscribe {
            device_id: device_id.clone(),
            reply: tokio::sync::oneshot::channel().0,
        })
        .await
        .unwrap();

        drop(tx);

        engine.run_command_loop(rx).await.unwrap();

        assert!(!engine.active_player_sessions.contains_key(&device_id));
    }
}
