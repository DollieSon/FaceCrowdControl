//! JSON messages on the game WebSocket (`/ws/game`). Documented in docs/GAME_PROTOCOL.md.
//! Every message is a JSON object with a `"type"` field.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Server -> game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMessage {
    /// Reply to `hello`: the game is registered and will now receive actions.
    Welcome { protocol: u32, server: String },
    /// Do something. May be sent more than once (e.g. after a reconnect), so the
    /// game must ignore an `id` it already handled.
    Action {
        id: i64,
        action: String,
        params: serde_json::Value,
        cost: i64,
        user: ActionUser,
    },
    /// The last message from the game was not understood.
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionUser {
    pub id: i64,
    pub name: String,
}

/// Game -> server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GameMessage {
    /// Must be the first message. `actions` lists the action names this game build can perform.
    Hello {
        game: String,
        #[serde(default = "default_protocol")]
        protocol: u32,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        actions: Vec<String>,
    },
    /// What happened to an action.
    Result {
        id: i64,
        status: ResultStatus,
        #[serde(default)]
        message: Option<String>,
    },
}

fn default_protocol() -> u32 {
    PROTOCOL_VERSION
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    /// Applied in game.
    Done,
    /// Tried, but it went wrong. The viewer is refunded.
    Failed,
    /// Can't be done right now (e.g. not in a run / not in combat). The viewer is refunded.
    Rejected,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_hello_with_defaults() {
        let m: GameMessage =
            serde_json::from_value(json!({"type": "hello", "game": "sts2"})).unwrap();
        assert_eq!(
            m,
            GameMessage::Hello {
                game: "sts2".into(),
                protocol: 1,
                version: None,
                actions: vec![]
            }
        );
    }

    #[test]
    fn parses_result() {
        let m: GameMessage = serde_json::from_value(
            json!({"type": "result", "id": 7, "status": "rejected", "message": "not in combat"}),
        )
        .unwrap();
        assert_eq!(
            m,
            GameMessage::Result {
                id: 7,
                status: ResultStatus::Rejected,
                message: Some("not in combat".into())
            }
        );
    }

    #[test]
    fn serializes_action() {
        let m = ServerMessage::Action {
            id: 3,
            action: "heal".into(),
            params: json!({"amount": 5}),
            cost: 50,
            user: ActionUser {
                id: 42,
                name: "Maria".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            json!({"type": "action", "id": 3, "action": "heal", "params": {"amount": 5}, "cost": 50,
                   "user": {"id": 42, "name": "Maria"}})
        );
    }
}
