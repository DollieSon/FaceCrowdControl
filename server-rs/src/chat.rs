//! `POST /chat` events from the browser extension (and later Twitch/YouTube readers).
//!
//! Event shape (same as the v1 server, so the extension works unchanged):
//! `{ "platform": "facebook", "platformUserId": "1000123", "displayName": "Maria",
//!    "message": "/give #42 50", "messageId": "fb:abc", "timestamp": "2026-11-16T15:00:00Z" }`

use serde::Serialize;
use serde_json::Value;

use crate::{
    AppState,
    commands::{self, CommandReport},
    store::{self, NewChatMessage, RecordOutcome, StoreError},
};

pub const PLATFORMS: &[&str] = &["facebook", "youtube", "twitch"];
pub const MAX_EVENTS: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventResult {
    /// Echoed back so the sender can match results to events.
    pub message_id: Value,
    /// "ok", "duplicate" or "invalid".
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandReport>,
}

/// A trimmed, non-empty string field of at most `max` characters.
fn text_field(e: &Value, key: &str, max: usize) -> Result<Option<String>, String> {
    match e.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                Ok(None)
            } else if s.chars().count() > max {
                Err(format!("{key} is longer than {max} characters"))
            } else {
                Ok(Some(s.to_string()))
            }
        }
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn required(e: &Value, key: &str, max: usize) -> Result<String, String> {
    text_field(e, key, max)?.ok_or_else(|| format!("{key} is required"))
}

/// Checks one event and turns it into something storable, or says what's wrong with it.
pub fn validate(e: &Value) -> Result<NewChatMessage, String> {
    if !e.is_object() {
        return Err("event must be an object".into());
    }
    let platform = required(e, "platform", 32)?;
    if !PLATFORMS.contains(&platform.as_str()) {
        return Err("unknown platform".into());
    }
    let platform_user_id = required(e, "platformUserId", 200)?;
    // A display name that's missing or too long falls back to the id; it's only a label.
    let display_name = text_field(e, "displayName", 100)
        .ok()
        .flatten()
        .unwrap_or_else(|| platform_user_id.chars().take(100).collect());
    let message = required(e, "message", 2000)?;
    let message_id = required(e, "messageId", 300)?;
    // A bad timestamp isn't worth rejecting the message over.
    let sent_at = e
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&chrono::Utc));
    Ok(NewChatMessage {
        platform,
        platform_user_id,
        display_name,
        message_id,
        message,
        sent_at,
    })
}

/// Accepts `[event, ...]`, `{ "events": [...] }` or a single event.
pub fn events_from_body(body: &Value) -> &[Value] {
    match body {
        Value::Array(list) => list,
        Value::Object(map) => match map.get("events") {
            Some(Value::Array(list)) => list,
            _ => std::slice::from_ref(body),
        },
        other => std::slice::from_ref(other),
    }
}

/// Validates, stores and runs one event. Errs only on a database failure (the sender should retry;
/// already-stored messages then come back as duplicates).
pub async fn handle_event(state: &AppState, raw: &Value) -> Result<EventResult, StoreError> {
    let message_id = raw.get("messageId").cloned().unwrap_or(Value::Null);
    let msg = match validate(raw) {
        Ok(m) => m,
        Err(error) => {
            return Ok(EventResult {
                message_id,
                status: "invalid",
                user_id: None,
                error: Some(error),
                command: None,
            });
        }
    };
    tracing::info!(
        "[chat] {}:{} ({}): {}",
        msg.platform,
        msg.platform_user_id,
        msg.display_name,
        msg.message
    );

    let user = match state.store.record_chat(&msg, store::now()).await? {
        RecordOutcome::Duplicate => {
            return Ok(EventResult {
                message_id,
                status: "duplicate",
                user_id: None,
                error: None,
                command: None,
            });
        }
        RecordOutcome::Recorded { user } => user,
    };

    // Command failures are chat-level outcomes, not request errors.
    let command = commands::run(state, &user, &msg.message).await;
    Ok(EventResult {
        message_id,
        status: "ok",
        user_id: Some(user.id),
        error: None,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event() -> Value {
        json!({ "platform": "facebook", "platformUserId": "100012345678", "displayName": " Maria Santos ",
                "message": "/give #42 50", "messageId": "fb:Y29t", "timestamp": "2026-11-16T15:00:00.000Z" })
    }

    #[test]
    fn accepts_extension_event() {
        let m = validate(&event()).unwrap();
        assert_eq!(m.display_name, "Maria Santos");
        assert_eq!(m.message_id, "fb:Y29t");
        assert!(m.sent_at.is_some());
    }

    #[test]
    fn rejects_bad_events() {
        let cases = [
            (json!("nope"), "event must be an object"),
            (json!({"platform": "myspace"}), "unknown platform"),
            (
                json!({"platform": "facebook", "message": "hi", "messageId": "x"}),
                "platformUserId is required",
            ),
            (
                json!({"platform": "facebook", "platformUserId": "1", "messageId": "x", "message": "   "}),
                "message is required",
            ),
            (
                json!({"platform": "facebook", "platformUserId": "1", "message": "hi"}),
                "messageId is required",
            ),
            (
                json!({"platform": "facebook", "platformUserId": 1, "message": "hi", "messageId": "x"}),
                "platformUserId must be a string",
            ),
        ];
        for (e, want) in cases {
            assert_eq!(validate(&e).unwrap_err(), want, "{e}");
        }
        let mut long = event();
        long["message"] = json!("x".repeat(2001));
        assert_eq!(
            validate(&long).unwrap_err(),
            "message is longer than 2000 characters"
        );
    }

    #[test]
    fn display_name_falls_back_to_id() {
        let mut e = event();
        e["displayName"] = json!("n".repeat(101));
        assert_eq!(validate(&e).unwrap().display_name, "100012345678");
    }

    #[test]
    fn bad_timestamp_is_ignored() {
        let mut e = event();
        e["timestamp"] = json!("yesterday");
        assert_eq!(validate(&e).unwrap().sent_at, None);
    }

    #[test]
    fn body_shapes() {
        let e = event();
        assert_eq!(events_from_body(&json!([e.clone(), e.clone()])).len(), 2);
        assert_eq!(events_from_body(&json!({"events": [e.clone()]})).len(), 1);
        assert_eq!(events_from_body(&e).len(), 1);
    }
}
