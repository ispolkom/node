//! Web API for media calls (voice/video)

use axum::{
    extract::{Path, State, WebSocketUpgrade},
    response::{Json, IntoResponse},
    http::StatusCode,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Serialize, Deserialize};
use crate::web::server::{AppState, IncomingCallInfo};
use crate::util::HashId;
use crate::p2p::{P2PPacket, P2PPacketType};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSignalEvent {
    pub from_peer_id: String,
    pub payload: String,
    pub packet_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StartCallRequest {
    pub peer_id: String,
    pub audio_enabled: bool,
    pub video_enabled: bool,
    #[serde(default)]
    pub display_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallResponse {
    pub call_id: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallInfo {
    pub call_id: String,
    pub peer_id: String,
    pub state: String,
    pub audio_active: bool,
    pub video_active: bool,
    pub duration_secs: u64,
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// Start a new outgoing voice call.
/// Sends VoiceCallRequest P2P packet to callee; returns call_id immediately.
/// Caller's browser should open WS to /api/media/ws/{peer_id} and wait for
/// a "call-accept" signal before creating WebRTC offer.
pub async fn start_call(
    State(app_state): State<AppState>,
    Json(req): Json<StartCallRequest>,
) -> Result<Json<CallResponse>, StatusCode> {
    let transport = app_state.p2p_transport.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let peer_hash = if let Ok(id) = HashId::from_hex(&req.peer_id) {
        id
    } else if let Some(id) = transport.find_peer_by_short_id(&req.peer_id).await {
        id
    } else {
        return Err(StatusCode::NOT_FOUND);
    };

    let call_id = format!("{:016x}", rand::random::<u64>());
    let my_short_id = hex::encode(&transport.node_id().0[..8]);

    let payload = serde_json::json!({
        "type": "call-request",
        "call_id": call_id,
        "from_short_id": my_short_id,
        "display_name": req.display_name,
    });

    let packet = P2PPacket::new(
        P2PPacketType::VoiceCallRequest,
        transport.node_id(),
        false,
        payload.to_string().into_bytes(),
    );

    if let Err(e) = transport.send_packet_dual_path(peer_hash, packet).await {
        eprintln!("❌ Failed to send VoiceCallRequest: {}", e);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(CallResponse {
        call_id,
        status: "ringing".to_string(),
    }))
}

/// End a specific call by ID.
pub async fn end_call(
    Path(call_id): Path<String>,
    State(app_state): State<AppState>,
) -> StatusCode {
    // Remove from incoming queue if present
    app_state.incoming_calls.lock().await.remove(&call_id);
    StatusCode::OK
}

/// End the currently active call (POST without call_id — for active call UI button).
pub async fn end_active_call(
    State(app_state): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let transport = match app_state.p2p_transport.as_ref() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"status": "error", "message": "no transport"})),
    };

    // Get peer_id and call_id from body
    let peer_id_str = body.as_ref()
        .and_then(|b| b.get("peer_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let call_id = body.as_ref()
        .and_then(|b| b.get("call_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(peer_hash) = if !peer_id_str.is_empty() {
        if let Ok(id) = HashId::from_hex(&peer_id_str) {
            Some(id)
        } else {
            transport.find_peer_by_short_id(&peer_id_str).await
        }
    } else {
        None
    } {
        let payload = serde_json::json!({
            "type": "hangup",
            "call_id": call_id,
        });
        let packet = P2PPacket::new(
            P2PPacketType::VoiceCallEnd,
            transport.node_id(),
            false,
            payload.to_string().into_bytes(),
        );
        let _ = transport.send_packet_dual_path(peer_hash, packet).await;
    }

    Json(serde_json::json!({"status": "ok"}))
}

/// Get a specific call's info.
pub async fn get_call_info(
    Path(call_id): Path<String>,
    State(app_state): State<AppState>,
) -> Result<Json<CallInfo>, StatusCode> {
    let calls = app_state.incoming_calls.lock().await;
    if let Some(info) = calls.get(&call_id) {
        return Ok(Json(CallInfo {
            call_id: info.call_id.clone(),
            peer_id: info.from_short_id.clone(),
            state: "incoming".to_string(),
            audio_active: true,
            video_active: false,
            duration_secs: 0,
        }));
    }
    Err(StatusCode::NOT_FOUND)
}

/// List all active calls.
pub async fn list_calls(
    State(_app_state): State<AppState>,
) -> Json<Vec<CallInfo>> {
    Json(vec![])
}

/// Get oldest pending incoming call (polling endpoint for callee's browser).
pub async fn get_incoming_call(
    State(app_state): State<AppState>,
) -> Json<serde_json::Value> {
    let calls = app_state.incoming_calls.lock().await;
    if let Some(info) = calls.values().next() {
        Json(serde_json::json!({
            "call_id": info.call_id,
            "from_short_id": info.from_short_id,
            "from_display_name": info.from_display_name,
            "received_at": info.received_at,
        }))
    } else {
        Json(serde_json::json!(null))
    }
}

/// Accept an incoming call: send VoiceCallAccept P2P packet back to caller.
pub async fn accept_call(
    Path(call_id): Path<String>,
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    let transport = match app_state.p2p_transport.as_ref() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"status": "error", "message": "no transport"})),
    };

    let info = {
        let mut calls = app_state.incoming_calls.lock().await;
        calls.remove(&call_id)
    };

    let info = match info {
        Some(i) => i,
        None => return Json(serde_json::json!({"status": "error", "message": "call not found"})),
    };

    let peer_hash = if let Some(id) = transport.find_peer_by_short_id(&info.from_short_id).await {
        id
    } else if let Ok(id) = HashId::from_hex(&info.from_short_id) {
        id
    } else {
        return Json(serde_json::json!({"status": "error", "message": "caller not found in peers"}));
    };

    let payload = serde_json::json!({
        "type": "call-accept",
        "call_id": call_id,
    });

    let packet = P2PPacket::new(
        P2PPacketType::VoiceCallAccept,
        transport.node_id(),
        false,
        payload.to_string().into_bytes(),
    );

    if let Err(e) = transport.send_packet_dual_path(peer_hash, packet).await {
        eprintln!("❌ Failed to send VoiceCallAccept: {}", e);
        return Json(serde_json::json!({"status": "error", "message": format!("send failed: {}", e)}));
    }

    Json(serde_json::json!({"status": "ok", "call_id": call_id}))
}

/// Reject an incoming call.
pub async fn reject_call(
    Path(call_id): Path<String>,
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    let transport = match app_state.p2p_transport.as_ref() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"status": "error", "message": "no transport"})),
    };

    let info = {
        let mut calls = app_state.incoming_calls.lock().await;
        calls.remove(&call_id)
    };

    let info = match info {
        Some(i) => i,
        None => return Json(serde_json::json!({"status": "error", "message": "call not found"})),
    };

    let peer_hash = if let Some(id) = transport.find_peer_by_short_id(&info.from_short_id).await {
        id
    } else if let Ok(id) = HashId::from_hex(&info.from_short_id) {
        id
    } else {
        return Json(serde_json::json!({"status": "ok"}));
    };

    let payload = serde_json::json!({
        "type": "call-reject",
        "call_id": call_id,
    });

    let packet = P2PPacket::new(
        P2PPacketType::VoiceCallReject,
        transport.node_id(),
        false,
        payload.to_string().into_bytes(),
    );

    let _ = transport.send_packet_dual_path(peer_hash, packet).await;
    Json(serde_json::json!({"status": "ok", "call_id": call_id}))
}

/// WebSocket handler for real-time media signaling (SDP offer/answer, ICE candidates).
pub async fn media_websocket_handler(
    State(app_state): State<AppState>,
    ws: WebSocketUpgrade,
    Path(peer_id): Path<String>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_media_socket(socket, peer_id, app_state))
}

async fn handle_media_socket(
    socket: axum::extract::ws::WebSocket,
    target_short_id: String,
    app_state: AppState,
) {
    use axum::extract::ws::Message;

    println!("📡 WebSocket session registered for peer: {}", target_short_id);

    let transport = match app_state.p2p_transport.as_ref() {
        Some(t) => t.clone(),
        None => {
            eprintln!("❌ No P2P transport for media signaling");
            return;
        }
    };

    let target_hash = match transport.find_peer_by_short_id(&target_short_id).await {
        Some(id) => id,
        None => match HashId::from_hex(&target_short_id) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("❌ Invalid peer ID for media WS: {} - {}", target_short_id, e);
                return;
            }
        }
    };

    let mut signal_rx = app_state.media_signal_bus.subscribe();
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let target_short = hex::encode(&target_hash.0[..8]);

    loop {
        tokio::select! {
            incoming = ws_receiver.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        // Browser → P2P: forward signaling as VoiceData
                        let packet = P2PPacket::new(
                            P2PPacketType::VoiceData,
                            transport.node_id(),
                            false,
                            text.as_bytes().to_vec(),
                        );
                        if let Err(e) = transport.send_packet_dual_path(target_hash, packet).await {
                            eprintln!("❌ Failed to forward media signal via P2P: {}", e);
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        eprintln!("❌ WS receive error: {}", e);
                        break;
                    }
                }
            }
            signal = signal_rx.recv() => {
                match signal {
                    Ok(event) if event.from_peer_id == target_short => {
                        // P2P → Browser: forward any signal from target peer
                        if ws_sender.send(Message::Text(event.payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        }
    }

    println!("📡 WebSocket session closed for peer: {}", target_short_id);
}

/// Сколько входящих вызовов может ждать ответа и как долго звонок «звонит».
pub const MAX_PENDING_CALLS: usize = 16;
pub const CALL_RING_MS: u64 = 120_000;

/// Входящий вызов из пакета. Кто звонит — ТОЛЬКО по проверенному отправителю пакета (ключ канала): раньше номер звонящего брался из
/// текста вызова, и любой узел со связью мог позвонить «от имени» другого. Номер вызова — до 64 латинских букв/цифр, имя — до 64 знаков.
pub fn incoming_from_packet(peer_id: &HashId, text: &str, now: u64) -> Option<IncomingCallInfo> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let call_id = json.get("call_id")?.as_str()?.to_string();
    if call_id.is_empty() || call_id.len() > 64 || !call_id.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let sender = hex::encode(&peer_id.0[..8]);
    if let Some(claimed) = json.get("from_short_id").and_then(|v| v.as_str()) {
        if claimed != sender {
            eprintln!("📞 вызов: в тексте указан звонящий {claimed}, но пакет от {sender} — показан настоящий");
        }
    }
    let name: String = json.get("display_name").and_then(|v| v.as_str()).unwrap_or("Unknown").chars().filter(|c| !c.is_control()).take(64).collect();
    Some(IncomingCallInfo { call_id, from_short_id: sender, from_display_name: name, received_at: now })
}

/// Поставить вызов в очередь: отзвонившие (дольше `CALL_RING_MS`) убираются, очередь не длиннее `MAX_PENDING_CALLS` (вытесняется
/// самый старый) — узел со связью не может завалить её вызовами.
pub fn queue_incoming(calls: &mut std::collections::HashMap<String, IncomingCallInfo>, info: IncomingCallInfo, now: u64) {
    calls.retain(|_, c| now.saturating_sub(c.received_at) < CALL_RING_MS);
    while calls.len() >= MAX_PENDING_CALLS {
        let Some(oldest) = calls.values().min_by_key(|c| c.received_at).map(|c| c.call_id.clone()) else { break };
        calls.remove(&oldest);
    }
    calls.insert(info.call_id.clone(), info);
}

/// Store an incoming call notification (called from main.rs signal loop).
pub fn store_incoming_call(
    incoming_calls: &std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<String, IncomingCallInfo>>>,
    info: IncomingCallInfo,
) {
    let calls = incoming_calls.clone();
    let call_id = info.call_id.clone();
    tokio::spawn(async move {
        calls.lock().await.insert(call_id, info);
    });
}

// ── Video call handlers ──────────────────────────────────────────────────────

/// Start a new outgoing video call.
pub async fn start_video_call(
    State(app_state): State<AppState>,
    Json(req): Json<StartCallRequest>,
) -> Result<Json<CallResponse>, StatusCode> {
    let transport = app_state.p2p_transport.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    let peer_hash = if let Ok(id) = HashId::from_hex(&req.peer_id) {
        id
    } else if let Some(id) = transport.find_peer_by_short_id(&req.peer_id).await {
        id
    } else {
        return Err(StatusCode::NOT_FOUND);
    };

    let call_id = format!("{:016x}", rand::random::<u64>());
    let my_short_id = hex::encode(&transport.node_id().0[..8]);

    let payload = serde_json::json!({
        "type": "video-call-request",
        "call_id": call_id,
        "from_short_id": my_short_id,
        "display_name": req.display_name,
    });

    let packet = P2PPacket::new(
        P2PPacketType::VideoCallRequest,
        transport.node_id(),
        false,
        payload.to_string().into_bytes(),
    );

    if let Err(e) = transport.send_packet_dual_path(peer_hash, packet).await {
        eprintln!("❌ Failed to send VideoCallRequest: {}", e);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    Ok(Json(CallResponse {
        call_id,
        status: "ringing".to_string(),
    }))
}

/// Get oldest pending incoming video call (polling endpoint for callee's browser).
pub async fn get_incoming_video_call(
    State(app_state): State<AppState>,
) -> Json<serde_json::Value> {
    let calls = app_state.incoming_video_calls.lock().await;
    if let Some(info) = calls.values().next() {
        Json(serde_json::json!({
            "call_id": info.call_id,
            "from_short_id": info.from_short_id,
            "from_display_name": info.from_display_name,
            "received_at": info.received_at,
        }))
    } else {
        Json(serde_json::json!(null))
    }
}

/// Accept an incoming video call.
pub async fn accept_video_call(
    Path(call_id): Path<String>,
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    let transport = match app_state.p2p_transport.as_ref() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"status": "error", "message": "no transport"})),
    };

    let info = {
        let mut calls = app_state.incoming_video_calls.lock().await;
        calls.remove(&call_id)
    };

    let info = match info {
        Some(i) => i,
        None => return Json(serde_json::json!({"status": "error", "message": "call not found"})),
    };

    let peer_hash = if let Some(id) = transport.find_peer_by_short_id(&info.from_short_id).await {
        id
    } else if let Ok(id) = HashId::from_hex(&info.from_short_id) {
        id
    } else {
        return Json(serde_json::json!({"status": "error", "message": "caller not found in peers"}));
    };

    let payload = serde_json::json!({
        "type": "call-accept",
        "call_id": call_id,
        "call_type": "video",
    });

    let packet = P2PPacket::new(
        P2PPacketType::VideoCallAccept,
        transport.node_id(),
        false,
        payload.to_string().into_bytes(),
    );

    if let Err(e) = transport.send_packet_dual_path(peer_hash, packet).await {
        eprintln!("❌ Failed to send VideoCallAccept: {}", e);
        return Json(serde_json::json!({"status": "error", "message": format!("send failed: {}", e)}));
    }

    Json(serde_json::json!({"status": "ok", "call_id": call_id}))
}

/// Reject an incoming video call.
pub async fn reject_video_call(
    Path(call_id): Path<String>,
    State(app_state): State<AppState>,
) -> impl IntoResponse {
    let transport = match app_state.p2p_transport.as_ref() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"status": "error", "message": "no transport"})),
    };

    let info = {
        let mut calls = app_state.incoming_video_calls.lock().await;
        calls.remove(&call_id)
    };

    let info = match info {
        Some(i) => i,
        None => return Json(serde_json::json!({"status": "ok"})),
    };

    let peer_hash = if let Some(id) = transport.find_peer_by_short_id(&info.from_short_id).await {
        id
    } else if let Ok(id) = HashId::from_hex(&info.from_short_id) {
        id
    } else {
        return Json(serde_json::json!({"status": "ok"}));
    };

    let payload = serde_json::json!({
        "type": "call-reject",
        "call_id": call_id,
        "call_type": "video",
    });

    let packet = P2PPacket::new(
        P2PPacketType::VideoCallReject,
        transport.node_id(),
        false,
        payload.to_string().into_bytes(),
    );

    let _ = transport.send_packet_dual_path(peer_hash, packet).await;
    Json(serde_json::json!({"status": "ok", "call_id": call_id}))
}

/// End the active video call.
pub async fn end_active_video_call(
    State(app_state): State<AppState>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let transport = match app_state.p2p_transport.as_ref() {
        Some(t) => t.clone(),
        None => return Json(serde_json::json!({"status": "error", "message": "no transport"})),
    };

    let peer_id_str = body.as_ref()
        .and_then(|b| b.get("peer_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let call_id = body.as_ref()
        .and_then(|b| b.get("call_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(peer_hash) = if !peer_id_str.is_empty() {
        if let Ok(id) = HashId::from_hex(&peer_id_str) { Some(id) }
        else { transport.find_peer_by_short_id(&peer_id_str).await }
    } else { None } {
        let payload = serde_json::json!({"type": "hangup", "call_id": call_id, "call_type": "video"});
        let packet = P2PPacket::new(
            P2PPacketType::VideoCallEnd,
            transport.node_id(),
            false,
            payload.to_string().into_bytes(),
        );
        let _ = transport.send_packet_dual_path(peer_hash, packet).await;
    }

    Json(serde_json::json!({"status": "ok"}))}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caller_is_who_signed_the_packet_not_who_the_text_claims() {
        let real = HashId([0xab; 32]);
        let text = serde_json::json!({"type": "call-request", "call_id": "0123abcd", "from_short_id": "cdcdcdcdcdcdcdcd", "display_name": "Мама\u{7}"}).to_string();
        let c = incoming_from_packet(&real, &text, 5).unwrap();
        assert_eq!((c.from_short_id.as_str(), c.from_display_name.as_str(), c.received_at), ("abababababababab", "Мама", 5));
        assert!(incoming_from_packet(&real, r#"{"call_id": ""}"#, 0).is_none());
        assert!(incoming_from_packet(&real, &serde_json::json!({"call_id": "x".repeat(65)}).to_string(), 0).is_none());
        assert!(incoming_from_packet(&real, r#"{"call_id": "../etc"}"#, 0).is_none());
        assert!(incoming_from_packet(&real, "not json", 0).is_none());
        let long = incoming_from_packet(&real, &serde_json::json!({"call_id": "a1", "display_name": "я".repeat(100)}).to_string(), 0).unwrap();
        assert_eq!(long.from_display_name.chars().count(), 64);
    }

    #[test]
    fn the_queue_forgets_old_rings_and_never_grows_past_its_limit() {
        let who = HashId([1; 32]);
        let mut q = std::collections::HashMap::new();
        for i in 0..100u64 {
            let c = incoming_from_packet(&who, &serde_json::json!({"call_id": format!("c{i}")}).to_string(), 1_000 + i).unwrap();
            queue_incoming(&mut q, c, 1_000 + i);
        }
        assert_eq!(q.len(), MAX_PENDING_CALLS);
        assert!(q.contains_key("c99") && !q.contains_key("c83"), "the oldest are pushed out");
        let late = incoming_from_packet(&who, r#"{"call_id": "late"}"#, 1_000 + 99 + CALL_RING_MS).unwrap();
        queue_incoming(&mut q, late, 1_000 + 99 + CALL_RING_MS);
        assert_eq!(q.keys().cloned().collect::<Vec<_>>(), vec!["late".to_string()], "rings older than two minutes are gone");
    }
}
