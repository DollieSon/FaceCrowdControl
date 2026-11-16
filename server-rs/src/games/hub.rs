//! In-memory registry of connected games, plus command cooldowns.
//!
//! Lives only in memory: after a restart games reconnect and resend `hello`.
//! The lock is a plain std Mutex and is never held across an `.await`.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::mpsc;

use super::protocol::ServerMessage;

#[derive(Default)]
pub struct GameHub {
    inner: Mutex<Inner>,
    next_conn_id: AtomicU64,
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<String, Session>,
    /// command name -> when it can be used again
    cooldowns: HashMap<String, Instant>,
}

struct Session {
    conn_id: u64,
    version: Option<String>,
    actions: BTreeSet<String>,
    connected_at: DateTime<Utc>,
    tx: mpsc::UnboundedSender<ServerMessage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameStatus {
    pub game: String,
    pub version: Option<String>,
    pub actions: Vec<String>,
    pub connected_at: DateTime<Utc>,
}

/// Why a game command can't be queued right now.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unavailable {
    #[error("{0} isn't connected")]
    NotConnected(String),
    #[error("{game} doesn't support {action:?} (update the mod?)")]
    Unsupported { game: String, action: String },
    #[error("on cooldown for {0}s")]
    Cooldown(u64),
}

impl GameHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a game connection, replacing (and so disconnecting) any older one for the same game.
    /// Messages for the game arrive on the returned receiver.
    pub fn register(
        &self,
        game: &str,
        version: Option<String>,
        actions: impl IntoIterator<Item = String>,
    ) -> (u64, mpsc::UnboundedReceiver<ServerMessage>) {
        let conn_id = self.next_conn_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        let session = Session {
            conn_id,
            version,
            actions: actions.into_iter().collect(),
            connected_at: Utc::now(),
            tx,
        };
        if self
            .lock()
            .sessions
            .insert(game.to_string(), session)
            .is_some()
        {
            tracing::info!(game, "a new connection replaced the previous one");
        }
        (conn_id, rx)
    }

    /// Removes the session if it's still the one with `conn_id` (a newer one may have replaced it).
    pub fn unregister(&self, game: &str, conn_id: u64) {
        let mut inner = self.lock();
        if inner
            .sessions
            .get(game)
            .is_some_and(|s| s.conn_id == conn_id)
        {
            inner.sessions.remove(game);
        }
    }

    pub fn status(&self) -> Vec<GameStatus> {
        let inner = self.lock();
        let mut list: Vec<_> = inner
            .sessions
            .iter()
            .map(|(game, s)| GameStatus {
                game: game.clone(),
                version: s.version.clone(),
                actions: s.actions.iter().cloned().collect(),
                connected_at: s.connected_at,
            })
            .collect();
        list.sort_by(|a, b| a.game.cmp(&b.game));
        list
    }

    pub fn supports(&self, game: &str, action: &str) -> bool {
        self.lock()
            .sessions
            .get(game)
            .is_some_and(|s| s.actions.contains(action))
    }

    /// Checks the game is connected and supports the action, and starts the command's
    /// cooldown. Call [`GameHub::release_cooldown`] if the command then doesn't go ahead.
    pub fn reserve(
        &self,
        command: &str,
        game: &str,
        action: &str,
        cooldown: Duration,
    ) -> Result<(), Unavailable> {
        let mut inner = self.lock();
        let session = inner
            .sessions
            .get(game)
            .ok_or_else(|| Unavailable::NotConnected(game.to_string()))?;
        if !session.actions.contains(action) {
            return Err(Unavailable::Unsupported {
                game: game.to_string(),
                action: action.to_string(),
            });
        }
        let now = Instant::now();
        if let Some(until) = inner.cooldowns.get(command)
            && *until > now
        {
            return Err(Unavailable::Cooldown((*until - now).as_secs().max(1)));
        }
        if !cooldown.is_zero() {
            inner.cooldowns.insert(command.to_string(), now + cooldown);
        }
        Ok(())
    }

    pub fn release_cooldown(&self, command: &str) {
        self.lock().cooldowns.remove(command);
    }

    /// Queues a message for the game. Returns false if it isn't connected.
    pub fn send(&self, game: &str, msg: ServerMessage) -> bool {
        self.lock()
            .sessions
            .get(game)
            .is_some_and(|s| s.tx.send(msg).is_ok())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding the lock can't leave Inner half-updated in a harmful way.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_checks_connection_support_and_cooldown() {
        let hub = GameHub::new();
        assert_eq!(
            hub.reserve("heal", "sts2", "heal", Duration::ZERO),
            Err(Unavailable::NotConnected("sts2".into()))
        );

        let (_id, _rx) = hub.register("sts2", None, ["heal".to_string()]);
        assert!(matches!(
            hub.reserve("gold", "sts2", "gold", Duration::ZERO),
            Err(Unavailable::Unsupported { .. })
        ));

        hub.reserve("heal", "sts2", "heal", Duration::from_secs(30))
            .unwrap();
        assert!(matches!(
            hub.reserve("heal", "sts2", "heal", Duration::from_secs(30)),
            Err(Unavailable::Cooldown(_))
        ));
        hub.release_cooldown("heal");
        hub.reserve("heal", "sts2", "heal", Duration::from_secs(30))
            .unwrap();
    }

    #[test]
    fn newer_connection_replaces_older() {
        let hub = GameHub::new();
        let (old, mut old_rx) = hub.register("sts2", None, Vec::<String>::new());
        let (new, _new_rx) = hub.register("sts2", None, Vec::<String>::new());
        // The old sender was dropped, so the old connection's writer sees the channel close.
        assert_eq!(
            old_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Disconnected)
        );
        hub.unregister("sts2", old); // stale: must not remove the new session
        assert_eq!(hub.status().len(), 1);
        hub.unregister("sts2", new);
        assert!(hub.status().is_empty());
    }
}
