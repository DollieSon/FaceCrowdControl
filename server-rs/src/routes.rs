//! HTTP routes.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::trace::TraceLayer;

use crate::{AppState, chat, games::ws::ws_handler};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/chat", post(post_chat))
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/games", get(list_games))
        .route("/ws/game", get(ws_handler))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({ "error": message.into() }))).into_response()
}

async fn index() -> String {
    format!(
        "FaceCrowdControl server {} — time is {}",
        env!("CARGO_PKG_VERSION"),
        chrono::Utc::now().to_rfc3339()
    )
}

/// The extension's popup reads `ok` and `db`.
async fn health(State(state): State<AppState>) -> Json<Value> {
    let db = state.store.ping().await.is_ok();
    Json(json!({
        "ok": true,
        "db": db,
        "backend": state.store.backend(),
        "games": state.games.status(),
    }))
}

async fn post_chat(
    State(state): State<AppState>,
    body: Result<Json<Value>, JsonRejection>,
) -> Response {
    // The Json extractor only accepts `Content-Type: application/json`, so a web page
    // can't send this request without a CORS preflight (which this server never approves).
    let Json(body) = match body {
        Ok(b) => b,
        Err(rejection) => return error(rejection.status(), rejection.body_text()),
    };
    let events = chat::events_from_body(&body);
    if events.is_empty() || events.len() > chat::MAX_EVENTS {
        return error(
            StatusCode::BAD_REQUEST,
            format!("Send 1-{} events", chat::MAX_EVENTS),
        );
    }

    let mut results = Vec::with_capacity(events.len());
    for event in events {
        match chat::handle_event(&state, event).await {
            Ok(r) => results.push(r),
            Err(e) => {
                tracing::error!("database error handling chat: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "Database error", "results": results })),
                )
                    .into_response();
            }
        }
    }
    Json(json!({ "results": results })).into_response()
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
}

/// Users with the most points first.
async fn list_users(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    match state.store.users(limit).await {
        Ok(users) => Json(users).into_response(),
        Err(e) => {
            tracing::error!("listing users: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
        }
    }
}

async fn get_user(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match state.store.user(id).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, format!("User #{id} doesn't exist")),
        Err(e) => {
            tracing::error!("loading user: {e}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
        }
    }
}

/// Connected games, plus every game command and its cost.
async fn list_games(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "connected": state.games.status(),
        "commands": state.catalog.all().collect::<Vec<_>>(),
    }))
}
