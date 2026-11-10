use crate::control::{
    ControlServerState, SessionGeneration,
    types::{WsCommand, WsEvent},
};
use crate::domain::types::DeviceId;
use crate::ports::process_lister::ProcessLister;
use axum::extract::ws::{Message, WebSocket};
use futures::SinkExt;
use futures::stream::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;

pub async fn handle_ws<P: ProcessLister + 'static>(
    mut socket: WebSocket,
    device_id: DeviceId,
    generation: SessionGeneration,
    state: ControlServerState<P>,
) {
    // The HTTP upgrade and WebSocket task start asynchronously. A reconnect may
    // rotate the token between those two points; never register that stale socket.
    if !state.authorizer.is_current(&device_id, generation) {
        let _ = socket.close().await;
        return;
    }

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut session_check = tokio::time::interval(Duration::from_millis(250));
    session_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (event_tx, event_rx) = mpsc::channel::<WsEvent>(32);
    let mut event_rx = event_rx;

    {
        let mut connections = state.ws_connections.lock().unwrap();
        connections.insert(device_id.clone(), event_tx.clone());
    }

    loop {
        tokio::select! {
            _ = session_check.tick() => {
                if !state.authorizer.is_current(&device_id, generation) {
                    tracing::debug!("Closing stale WebSocket session for device {}", device_id);
                    break;
                }
            }
            msg_result = ws_receiver.next() => {
                if !state.authorizer.is_current(&device_id, generation) {
                    break;
                }
                let Some(msg_result) = msg_result else {
                    break;
                };
                match msg_result {
                    Ok(Message::Text(text)) => {
                        if let Err(e) = handle_ws_command(&text, &device_id, generation, &state).await {
                            tracing::error!("WebSocket command error for device {}: {}", device_id, e);
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Err(error) => {
                        if is_expected_disconnect(&error) {
                            tracing::debug!("WebSocket peer disconnected for device {}: {}", device_id, error);
                        } else {
                            tracing::warn!("WebSocket error for device {}: {}", device_id, error);
                        }
                        break;
                    }
                    _ => {}
                }
            }
            event = event_rx.recv() => {
                // The event registry is keyed by device ID, but the session is
                // keyed by both device ID and generation. Do not leak queued
                // events to a socket whose bearer token has been rotated/revoked.
                if !state.authorizer.is_current(&device_id, generation) {
                    break;
                }
                let Some(event) = event else {
                    break;
                };
                let msg = match serde_json::to_string(&event) {
                    Ok(json) => json,
                    Err(error) => {
                        tracing::error!("Failed to serialize WsEvent: {}", error);
                        continue;
                    }
                };
                match ws_sender.send(Message::Text(msg.clone().into())).await {
                    Ok(()) => tracing::info!("WS Event sent: {}", msg),
                    Err(error) => {
                        if is_expected_disconnect(&error) {
                            tracing::debug!("WebSocket peer disconnected while sending to device {}: {}", device_id, error);
                        } else {
                            tracing::warn!("WebSocket send error for device {}: {}", device_id, error);
                        }
                        break;
                    }
                }
            }
        }
    }

    let is_current = {
        let mut connections = state.ws_connections.lock().unwrap();
        let is_match = connections
            .get(&device_id)
            .is_some_and(|tx| tx.same_channel(&event_tx));

        if is_match {
            connections.remove(&device_id);
        }
        is_match
    };

    // HTTPS fallback owns liveness while this optional control channel reconnects.
    // The generation check prevents an old socket from removing a newer map entry.
    let _ = (is_current, generation);

    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), ws_sender.close()).await;
}

fn is_expected_disconnect(error: &axum::Error) -> bool {
    let mut current: &(dyn std::error::Error + 'static) = error;
    loop {
        if let Some(error) = current.downcast_ref::<tokio_tungstenite::tungstenite::Error>() {
            use tokio_tungstenite::tungstenite::Error;
            use tokio_tungstenite::tungstenite::error::ProtocolError;

            return match error {
                Error::ConnectionClosed => true,
                Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) => true,
                Error::Io(error) => matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::NotConnected
                ),
                _ => false,
            };
        }
        if let Some(error) = current.downcast_ref::<std::io::Error>() {
            return matches!(
                error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::NotConnected
            );
        }
        let Some(source) = current.source() else {
            return false;
        };
        current = source;
    }
}

async fn handle_ws_command<P: ProcessLister + 'static>(
    text: &str,
    device_id: &DeviceId,
    generation: SessionGeneration,
    state: &ControlServerState<P>,
) -> Result<(), String> {
    if !state.authorizer.is_current(device_id, generation) {
        return Err("session is no longer active".into());
    }
    let command: WsCommand =
        serde_json::from_str(text).map_err(|e| format!("Failed to parse WsCommand: {}", e))?;

    match command {
        WsCommand::Heartbeat => {
            state
                .command_tx
                .send(crate::control::ControlCommand::SessionHeartbeat {
                    device_id: device_id.clone(),
                    generation,
                })
                .await
                .map_err(|_| "control dispatcher is unavailable".to_string())?;
            crate::control::ControlEventBus::send(
                &state.ws_connections,
                device_id,
                WsEvent::HeartbeatAcknowledged,
            )
            .await
            .map_err(|error| error.to_string())?;
        }
        WsCommand::Disconnect => {
            tracing::info!("WebSocket disconnect requested by {device_id}");
            // Check if this command came from the current active WebSocket.
            // We can do this by checking if it's still in the map?
            // Actually, for explicit Disconnect from the client, we should probably
            // just process it. But to be safe against delayed packets, let's process it.
            let dummy_addr = "0.0.0.0:0".parse().unwrap();
            let (response_tx, _response_rx) = tokio::sync::oneshot::channel();
            let _ = state
                .command_tx
                .send(crate::control::ControlCommand::Disconnect {
                    device_id: device_id.clone(),
                    remote_addr: dummy_addr,
                    generation: Some(generation),
                    response_tx,
                })
                .await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct EmptyProcessLister;

    impl ProcessLister for EmptyProcessLister {
        fn list_processes(&self) -> Vec<crate::domain::types::ProcessInfo> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn heartbeat_should_carry_the_authenticated_session_generation() {
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel(1);
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(1);
        let device_id = DeviceId("phone-1".into());
        let ws_connections = Arc::new(Mutex::new(HashMap::from([(device_id.clone(), event_tx)])));
        let state = ControlServerState {
            command_tx,
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            streamer_id: DeviceId("pc-1".into()),
            streamer_name: "PC".into(),
            ws_connections,
            process_lister: EmptyProcessLister,
            authorizer: crate::control::SessionAuthorizer::default(),
            pc_certificate_fingerprint: "certificate".into(),
        };
        let generation = SessionGeneration(7);
        let heartbeat = serde_json::to_string(&WsCommand::Heartbeat).unwrap();

        handle_ws_command(&heartbeat, &device_id, generation, &state)
            .await
            .unwrap();

        assert!(matches!(
            command_rx.recv().await,
            Some(crate::control::ControlCommand::SessionHeartbeat {
                device_id: observed_device,
                generation: observed_generation,
            }) if observed_device == device_id && observed_generation == generation
        ));
        assert!(matches!(
            event_rx.recv().await,
            Some(WsEvent::HeartbeatAcknowledged)
        ));
    }

    #[test]
    fn abrupt_peer_eof_is_an_expected_control_channel_disconnect() {
        let error = axum::Error::new(tokio_tungstenite::tungstenite::Error::Io(
            std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
        ));

        assert!(is_expected_disconnect(&error));
    }

    #[test]
    fn protocol_corruption_is_not_an_expected_disconnect() {
        let error = axum::Error::new(tokio_tungstenite::tungstenite::Error::Protocol(
            tokio_tungstenite::tungstenite::error::ProtocolError::InvalidOpcode(3),
        ));

        assert!(!is_expected_disconnect(&error));
    }
}
