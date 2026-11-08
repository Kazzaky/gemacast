use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::{Query, State, ws::WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, response::Response};
use tokio::sync::oneshot;

use crate::control::auth::SessionGeneration;
use crate::control::commands::ControlCommand;
use crate::control::server::PeerAddr;
use crate::control::state::ControlServerState;
use crate::control::types::{
    ChangeBitrateReq, ChangeSourceReq, ConnectReq, ControlErrorResponse, DisconnectReq,
    PresenceResponse, ProbeReq, ProcessListResponse, SourcesResponse,
};
use crate::domain::types::{AudioSource, DeviceId, StreamerCapabilities};
use crate::ports::process_lister::ProcessLister;

pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(65);

pub(crate) fn control_error(
    status: StatusCode,
    code: &str,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(ControlErrorResponse {
            code: code.to_string(),
            message: message.into(),
        }),
    )
        .into_response()
}

pub(crate) fn connect_error_code(message: &str) -> &'static str {
    let message = message.to_ascii_lowercase();
    if message.contains("cancelled on the phone") {
        "pairing_cancelled"
    } else if message.contains("rejected on the pc") {
        "pairing_rejected"
    } else if message.contains("invalid or expired") || message.contains("challenge expired") {
        "pairing_expired"
    } else if message.contains("too many pending") {
        "pairing_capacity_exhausted"
    } else if message.contains("challenge")
        || message.contains("authentication")
        || message.contains("signature")
        || message.contains("device key")
    {
        "authentication_failed"
    } else if message.contains("remember the approved") {
        "pairing_persistence_failed"
    } else {
        "stream_start_failed"
    }
}

async fn await_mutation(response_rx: oneshot::Receiver<Result<(), String>>) -> Response {
    match tokio::time::timeout(COMMAND_TIMEOUT, response_rx).await {
        Ok(Ok(Ok(()))) => StatusCode::OK.into_response(),
        Ok(Ok(Err(message))) => control_error(StatusCode::CONFLICT, "operation_failed", message),
        Ok(Err(_)) => control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher dropped the acknowledgement",
        ),
        Err(_) => control_error(
            StatusCode::GATEWAY_TIMEOUT,
            "operation_timeout",
            "control operation timed out",
        ),
    }
}

fn unauthorized() -> Response {
    control_error(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "a valid device session token is required",
    )
}

pub(crate) async fn handle_probe<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    headers: HeaderMap,
    Json(req): Json<ProbeReq>,
) -> Json<PresenceResponse> {
    tracing::info!("HTTP POST /probe from {:?}", req.device_id);

    // Unauthenticated probes may discover only public streamer presence. The
    // device registration state and PC output volume are session information.
    let Some(device_id) = req.device_id.as_ref() else {
        let mut presence = state.build_presence();
        presence.pc_certificate_fingerprint = Some(state.pc_certificate_fingerprint.clone());
        return Json(presence);
    };
    if state.authenticate_device(&headers, device_id).is_none() {
        let mut presence = state.build_presence();
        presence.pc_certificate_fingerprint = Some(state.pc_certificate_fingerprint.clone());
        return Json(presence);
    }

    let (response_tx, response_rx) = oneshot::channel();
    let _ = state
        .command_tx
        .send(ControlCommand::Probe {
            device_id: Some(device_id.clone()),
            response_tx,
        })
        .await;

    let mut presence = match response_rx.await {
        Ok(presence) => presence,
        Err(_) => state.build_presence(),
    };
    presence.pc_certificate_fingerprint = Some(state.pc_certificate_fingerprint.clone());

    Json(presence)
}

pub(crate) async fn handle_connect<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<ConnectReq>,
) -> Response {
    tracing::info!("HTTP POST /connect from {:?}", req.device_id);

    let pc_link = Some(crate::network::NetworkLinkDetector::detect(
        req.mode,
        addr.ip(),
    ));

    if !state.is_broadcasting.load(Ordering::Relaxed) {
        return control_error(
            StatusCode::FORBIDDEN,
            "streamer_offline",
            format!("streamer {} is offline", state.streamer_name),
        );
    }

    let authorized = state
        .authenticate_device(&headers, &req.device_id)
        .is_some();
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::Connect {
            device_id: req.device_id,
            device_name: req.device_name,
            source: req.source.clone(),
            remote_addr: addr,
            bitrate: req.bitrate,
            response_tx,
            authorized,
            pending_request_id: req.pending_request_id,
            device_auth: req.device_auth,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }

    let mut presence = match tokio::time::timeout(CONNECT_TIMEOUT, response_rx).await {
        Ok(Ok(Ok(presence))) => presence,
        Ok(Ok(Err(message))) => {
            let code = connect_error_code(&message);
            return control_error(StatusCode::CONFLICT, code, message);
        }
        Ok(Err(_)) => {
            return control_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "dispatcher_unavailable",
                "control dispatcher dropped the acknowledgement",
            );
        }
        Err(_) => {
            return control_error(
                StatusCode::GATEWAY_TIMEOUT,
                "operation_timeout",
                "stream start timed out",
            );
        }
    };

    presence.pc_network_link = pc_link;
    presence.pc_certificate_fingerprint = Some(state.pc_certificate_fingerprint.clone());

    let status = if presence.session_token.is_none() && presence.pending_request_id.is_some() {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    (status, Json(presence)).into_response()
}

pub(crate) async fn handle_disconnect<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<DisconnectReq>,
) -> Response {
    tracing::info!("HTTP POST /disconnect from {:?}", req.device_id);
    let Some(session) = state.authenticate_device(&headers, &req.device_id) else {
        return unauthorized();
    };
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::Disconnect {
            device_id: req.device_id,
            remote_addr: addr,
            generation: Some(session.generation),
            response_tx,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }
    await_mutation(response_rx).await
}

pub(crate) async fn handle_get_sources<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
) -> Response {
    tracing::info!("HTTP GET /sources");
    let _ = addr;
    if state.authenticate_token(&headers).is_none() {
        return unauthorized();
    }
    let (response_tx, response_rx) = oneshot::channel();
    let _ = state
        .command_tx
        .send(ControlCommand::GetSources { response_tx })
        .await;

    let response = match response_rx.await {
        Ok(response) => response,
        Err(_) => SourcesResponse {
            sources: vec![AudioSource::Desktop],
            capabilities: StreamerCapabilities {
                supports_process_capture: false,
                supports_volume_sync: false,
            },
        },
    };

    Json(response).into_response()
}

pub(crate) async fn handle_change_source<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<ChangeSourceReq>,
) -> Response {
    tracing::info!("HTTP POST /change-source from {:?}", req.device_id);
    let _ = addr;
    if state
        .authenticate_device(&headers, &req.device_id)
        .is_none()
    {
        return unauthorized();
    }
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::ChangeSource {
            device_id: req.device_id,
            source: req.source,
            response_tx,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }
    await_mutation(response_rx).await
}

pub(crate) async fn handle_change_bitrate<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
    Json(req): Json<ChangeBitrateReq>,
) -> Response {
    tracing::info!("HTTP POST /change-bitrate from {:?}", req.device_id);
    let _ = addr;
    if state
        .authenticate_device(&headers, &req.device_id)
        .is_none()
    {
        return unauthorized();
    }
    let (response_tx, response_rx) = oneshot::channel();
    if state
        .command_tx
        .send(ControlCommand::ChangeBitrate {
            device_id: req.device_id,
            bitrate: req.bitrate,
            response_tx,
        })
        .await
        .is_err()
    {
        return control_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "dispatcher_unavailable",
            "control dispatcher is unavailable",
        );
    }
    await_mutation(response_rx).await
}

pub(crate) async fn handle_get_processes<P: ProcessLister + 'static>(
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
) -> Response {
    tracing::info!("HTTP GET /processes");
    let _ = addr;
    if state.authenticate_token(&headers).is_none() {
        return unauthorized();
    }
    let processes = state.process_lister.list_processes();
    Json(ProcessListResponse { processes }).into_response()
}

pub(crate) async fn handle_ws_upgrade<P: ProcessLister + 'static>(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    State(state): State<ControlServerState<P>>,
    axum::extract::ConnectInfo(PeerAddr(addr)): axum::extract::ConnectInfo<PeerAddr>,
    headers: HeaderMap,
) -> Response {
    tracing::info!("HTTP GET /ws upgrade request with params: {:?}", params);
    let device_id = match params.get("device_id") {
        Some(id) => DeviceId(id.clone()),
        None => {
            return (StatusCode::BAD_REQUEST, "Missing device_id query parameter").into_response();
        }
    };

    let _ = addr;
    let Some(session) = state.authenticate_device(&headers, &device_id) else {
        return unauthorized();
    };
    let generation: SessionGeneration = session.generation;

    ws.on_upgrade(move |socket| crate::control::ws::handle_ws(socket, device_id, generation, state))
        .into_response()
}
