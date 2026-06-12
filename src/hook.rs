use axum::{extract::State, http::StatusCode, response::Json, routing::post, Router};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone)]
pub struct HookEvent {
    pub terminal_id: String,
    pub transcript_path: Option<String>,
}

#[derive(Deserialize)]
struct HookPayload {
    #[serde(rename = "type")]
    event_type: String,
    terminal_id: String,
    #[serde(default)]
    transcript_path: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct HookResponse {
    ok: bool,
}

#[derive(Clone)]
struct HookState {
    relay_tx: mpsc::Sender<HookEvent>,
    ping_tx: Option<mpsc::Sender<()>>,
}

pub async fn run_hook_server_on_listener(
    listener: TcpListener,
    mut shutdown: broadcast::Receiver<()>,
    relay_tx: mpsc::Sender<HookEvent>,
    ping_tx: Option<mpsc::Sender<()>>,
) {
    let state = HookState { relay_tx, ping_tx };
    let app = Router::new()
        .route("/hook", post(handle_hook))
        .with_state(state);

    let addr = listener
        .local_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());

    tracing::info!(target: "cduo::hook", "server listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.recv().await;
        })
        .await
        .unwrap_or_else(|e| tracing::error!(target: "cduo::hook", "server error: {e}"));
}

async fn handle_hook(
    State(state): State<HookState>,
    body: Result<Json<HookPayload>, axum::extract::rejection::JsonRejection>,
) -> (StatusCode, Json<HookResponse>) {
    let payload = match body {
        Ok(Json(p)) => p,
        Err(_) => return (StatusCode::OK, Json(HookResponse { ok: false })),
    };

    if !payload.event_type.eq_ignore_ascii_case("stop")
        || (payload.terminal_id != "a" && payload.terminal_id != "b")
    {
        return (StatusCode::OK, Json(HookResponse { ok: false }));
    }

    let event = HookEvent {
        terminal_id: payload.terminal_id,
        transcript_path: payload.transcript_path.filter(|path| !path.is_empty()),
    };

    if state.relay_tx.try_send(event).is_err() {
        return (StatusCode::OK, Json(HookResponse { ok: false }));
    }

    // Best-effort ping — never blocks.
    if let Some(ref ping_tx) = state.ping_tx {
        let _ = ping_tx.try_send(());
    }

    (StatusCode::OK, Json(HookResponse { ok: true }))
}

#[cfg(test)]
#[path = "hook_tests.rs"]
mod tests;
