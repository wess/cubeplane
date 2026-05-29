//! The control/admin API: a small HTTP + WebSocket surface the Atlas admin
//! panel (or `curl`) drives to observe and steer the server.
//!
//! * `GET  /api/status`  — version, players online, uptime, mods, generator.
//! * `GET  /api/players` — connected player list with positions.
//! * `POST /api/say`     — broadcast a chat message.
//! * `POST /api/kick`    — disconnect a player.
//! * `POST /api/setblock`— place a block by name at world coordinates.
//! * `GET  /ws`          — live status stream (2s) accepting `say` commands.
//!
//! When `control.token` is configured, requests must carry
//! `Authorization: Bearer <token>` (or `?token=` for the WebSocket).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};

use cubeplane_world::block;

use crate::clientbound as cb;
use crate::state::Shared;
use crate::text;

#[derive(Clone)]
struct Control {
    shared: Arc<Shared>,
    token: Option<String>,
}

/// Start the control API server. Returns once it is listening (it runs until
/// the process exits).
pub async fn serve(shared: Arc<Shared>) -> anyhow::Result<()> {
    let cfg = shared.config.control.clone();
    let state = Control {
        shared: shared.clone(),
        token: cfg.token.clone(),
    };

    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/players", get(players))
        .route("/api/say", post(say))
        .route("/api/kick", post(kick))
        .route("/api/setblock", post(setblock))
        .route("/ws", get(ws_upgrade))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("control API listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn authorized(ctrl: &Control, headers: &HeaderMap) -> bool {
    match &ctrl.token {
        None => true,
        Some(expected) => headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t == expected)
            .unwrap_or(false),
    }
}

fn status_value(shared: &Arc<Shared>) -> Value {
    let players: Vec<String> = shared.players().iter().map(|p| p.name.clone()).collect();
    let mods = shared
        .mods
        .as_ref()
        .map(|m| m.loaded().to_vec())
        .unwrap_or_default();
    json!({
        "version": cubeplane_protocol::GAME_VERSION,
        "protocol": cubeplane_protocol::PROTOCOL_VERSION,
        "motd": shared.config.server.motd,
        "online": shared.player_count(),
        "max": shared.config.server.max_players,
        "uptimeSecs": shared.uptime_secs(),
        "totalJoins": shared.total_joins(),
        "generator": shared.world.lock().unwrap().generator_name(),
        "gamemode": shared.config.server.gamemode,
        "players": players,
        "mods": mods,
    })
}

async fn status(State(ctrl): State<Control>, headers: HeaderMap) -> impl IntoResponse {
    if !authorized(&ctrl, &headers) {
        return unauthorized();
    }
    Json(status_value(&ctrl.shared)).into_response()
}

async fn players(State(ctrl): State<Control>, headers: HeaderMap) -> impl IntoResponse {
    if !authorized(&ctrl, &headers) {
        return unauthorized();
    }
    let list: Vec<Value> = ctrl
        .shared
        .players()
        .iter()
        .map(|p| {
            let s = p.state();
            json!({
                "name": p.name,
                "uuid": p.uuid.to_string(),
                "entityId": p.entity_id,
                "x": s.x, "y": s.y, "z": s.z,
            })
        })
        .collect();
    Json(json!({ "players": list })).into_response()
}

#[derive(Deserialize)]
struct SayBody {
    message: String,
}

async fn say(
    State(ctrl): State<Control>,
    headers: HeaderMap,
    Json(body): Json<SayBody>,
) -> impl IntoResponse {
    if !authorized(&ctrl, &headers) {
        return unauthorized();
    }
    let msg = text::colored(format!("[Server] {}", body.message), "light_purple");
    ctrl.shared.broadcast(cb::system_chat(&msg, false));
    info!(target: "cubeplane::control", "say: {}", body.message);
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
struct KickBody {
    player: String,
    #[serde(default)]
    reason: Option<String>,
}

async fn kick(
    State(ctrl): State<Control>,
    headers: HeaderMap,
    Json(body): Json<KickBody>,
) -> impl IntoResponse {
    if !authorized(&ctrl, &headers) {
        return unauthorized();
    }
    let reason = body.reason.unwrap_or_else(|| "Kicked by an operator".into());
    match ctrl.shared.player_by_name(&body.player) {
        Some(p) => {
            p.send(cb::play_disconnect(&text::colored(reason, "red")));
            Json(json!({ "ok": true })).into_response()
        }
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "no such player" }))).into_response(),
    }
}

#[derive(Deserialize)]
struct SetBlockBody {
    x: i32,
    y: i32,
    z: i32,
    block: String,
}

async fn setblock(
    State(ctrl): State<Control>,
    headers: HeaderMap,
    Json(body): Json<SetBlockBody>,
) -> impl IntoResponse {
    if !authorized(&ctrl, &headers) {
        return unauthorized();
    }
    match block::by_name(&body.block) {
        Some(state) => {
            {
                let mut world = ctrl.shared.world.lock().unwrap();
                world.set_block(body.x, body.y, body.z, state);
            }
            ctrl.shared
                .broadcast(cb::block_update(body.x, body.y, body.z, state));
            Json(json!({ "ok": true })).into_response()
        }
        None => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown block", "block": body.block })),
        )
            .into_response(),
    }
}

fn unauthorized() -> axum::response::Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": "unauthorized" }))).into_response()
}

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

async fn ws_upgrade(
    State(ctrl): State<Control>,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    if let Some(expected) = &ctrl.token {
        if q.token.as_deref() != Some(expected.as_str()) {
            return unauthorized();
        }
    }
    ws.on_upgrade(move |socket| ws_loop(socket, ctrl.shared.clone()))
}

async fn ws_loop(mut socket: WebSocket, shared: Arc<Shared>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let payload = status_value(&shared).to_string();
                if socket.send(Message::Text(payload)).await.is_err() {
                    break;
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => handle_ws_command(&shared, &text),
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

fn handle_ws_command(shared: &Arc<Shared>, text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    match value.get("action").and_then(|a| a.as_str()) {
        Some("say") => {
            if let Some(message) = value.get("message").and_then(|m| m.as_str()) {
                let msg = crate::text::colored(format!("[Server] {message}"), "light_purple");
                shared.broadcast(cb::system_chat(&msg, false));
            }
        }
        other => warn!(target: "cubeplane::control", "unknown ws action: {other:?}"),
    }
}
