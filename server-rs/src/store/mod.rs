//! Database adapter.
//!
//! Everything outside this module talks to the database only through the
//! [`Store`] trait and the plain types defined here. Each backend
//! (`sqlite.rs`, `postgres.rs`) implements the trait with its own SQL and its
//! own migrations folder, so switching databases is a matter of changing
//! `DATABASE_URL` (and copying the data with `fcc-server copy-db` if you want
//! to keep it).
//!
//! Rules every implementation must keep (checked by `tests/store_conformance.rs`,
//! which runs the same tests against both backends):
//! - Methods that touch more than one row are atomic (one transaction).
//! - Points never go below zero.
//! - A game action's cost is taken when it is created and given back exactly
//!   once if it ends as failed / rejected / expired.

mod postgres;
mod sqlite;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use postgres::PgStore;
pub use sqlite::SqliteStore;

/// A viewer. Identity is (platform, platform_user_id); display_name is only a label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    pub platform: String,
    pub platform_user_id: String,
    pub display_name: String,
    pub points: i64,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// A validated chat message, ready to be stored.
#[derive(Debug, Clone, PartialEq)]
pub struct NewChatMessage {
    pub platform: String,
    pub platform_user_id: String,
    pub display_name: String,
    pub message_id: String,
    pub message: String,
    pub sent_at: Option<DateTime<Utc>>,
}

/// A stored chat message (used by export/import).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub platform: String,
    pub message_id: String,
    pub user_id: Option<i64>,
    pub platform_user_id: String,
    pub display_name: String,
    pub message: String,
    pub sent_at: Option<DateTime<Utc>>,
    pub received_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordOutcome {
    /// This (platform, message_id) was already stored: a resend. Nothing else was done.
    Duplicate,
    /// Stored; `user` is the sender after their last_seen/display name were updated.
    Recorded { user: User },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transfer {
    pub from: User,
    pub to: User,
    pub amount: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    /// Waiting to be sent to the game.
    Pending,
    /// Sent to the game, waiting for its result.
    Delivered,
    Done,
    /// The game tried and failed. Refunded.
    Failed,
    /// The game refused it (e.g. not in a run). Refunded.
    Rejected,
    /// Nobody answered in time. Refunded.
    Expired,
}

impl ActionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "delivered" => Self::Delivered,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "rejected" => Self::Rejected,
            "expired" => Self::Expired,
            _ => return None,
        })
    }

    /// Statuses that end in a refund.
    pub fn is_refunded(self) -> bool {
        matches!(self, Self::Failed | Self::Rejected | Self::Expired)
    }
}

/// Something a viewer paid for that a game should do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameAction {
    pub id: i64,
    pub game: String,
    pub action: String,
    pub params: serde_json::Value,
    pub user_id: i64,
    pub cost: i64,
    pub status: ActionStatus,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewGameAction {
    pub game: String,
    pub action: String,
    pub params: serde_json::Value,
    pub user_id: i64,
    pub cost: i64,
}

/// Every row of every table, for moving data between databases.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub users: Vec<User>,
    pub chat_messages: Vec<ChatMessage>,
    pub game_actions: Vec<GameAction>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("user #{0} doesn't exist")]
    UserNotFound(i64),
    #[error("not enough points (have {have}, need {need})")]
    InsufficientPoints { have: i64, need: i64 },
    #[error("the target database already has data; copy-db only fills an empty database")]
    NotEmpty,
    #[error("unsupported DATABASE_URL (use sqlite:... or postgres://...): {0}")]
    UnsupportedUrl(String),
    #[error("corrupt row: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type StoreResult<T> = Result<T, StoreError>;

#[async_trait]
pub trait Store: Send + Sync {
    /// "sqlite" or "postgres", for logs and /health.
    fn backend(&self) -> &'static str;

    /// Creates / upgrades the schema. Safe to run on every start.
    async fn migrate(&self) -> StoreResult<()>;

    async fn ping(&self) -> StoreResult<()>;

    /// Stores a chat message and creates/updates its sender, atomically.
    /// A message whose (platform, message_id) was already stored is a resend and changes nothing.
    async fn record_chat(
        &self,
        msg: &NewChatMessage,
        now: DateTime<Utc>,
    ) -> StoreResult<RecordOutcome>;

    async fn user(&self, id: i64) -> StoreResult<Option<User>>;

    /// Users with the most points first.
    async fn users(&self, limit: i64) -> StoreResult<Vec<User>>;

    /// Moves `amount` points between two users. Fails without changing anything if either
    /// user is missing or `from` has too few points.
    async fn transfer_points(&self, from: i64, to: i64, amount: i64) -> StoreResult<Transfer>;

    /// Gives `amount` points to everyone seen at or after `active_since`. Returns how many users got them.
    async fn award_active_users(
        &self,
        active_since: DateTime<Utc>,
        amount: i64,
    ) -> StoreResult<u64>;

    /// Charges the user `cost` and stores the action as pending, atomically.
    async fn create_game_action(
        &self,
        new: &NewGameAction,
        now: DateTime<Utc>,
    ) -> StoreResult<GameAction>;

    async fn game_action(&self, id: i64) -> StoreResult<Option<GameAction>>;

    /// pending -> delivered. Returns false if it wasn't pending.
    async fn mark_action_delivered(&self, id: i64, now: DateTime<Utc>) -> StoreResult<bool>;

    /// pending/delivered -> `status` (done / failed / rejected / expired), atomically refunding
    /// the cost when the new status is a refunded one. Returns None if the action was already
    /// finished (or doesn't exist), so an action can't be finished, or refunded, twice.
    async fn finish_action(
        &self,
        id: i64,
        status: ActionStatus,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<GameAction>>;

    /// Unfinished (pending or delivered) actions for a game, oldest first.
    async fn open_actions(&self, game: &str) -> StoreResult<Vec<GameAction>>;

    /// Ids of unfinished actions created before `created_before`, oldest first.
    async fn stale_action_ids(&self, created_before: DateTime<Utc>) -> StoreResult<Vec<i64>>;

    async fn export(&self) -> StoreResult<Snapshot>;

    /// Inserts a snapshot (keeping ids) into an empty database.
    async fn import(&self, snapshot: &Snapshot) -> StoreResult<()>;
}

/// The current time, cut to whole milliseconds: the precision both databases keep,
/// so a timestamp reads back exactly as it was written on either backend.
pub fn now() -> DateTime<Utc> {
    let t = Utc::now();
    DateTime::from_timestamp_millis(t.timestamp_millis()).unwrap_or(t)
}

/// Opens the database named by `url` and brings its schema up to date.
pub async fn connect(url: &str) -> StoreResult<Arc<dyn Store>> {
    let store: Arc<dyn Store> = if url.starts_with("sqlite:") {
        Arc::new(SqliteStore::connect(url).await?)
    } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Arc::new(PgStore::connect(url).await?)
    } else {
        return Err(StoreError::UnsupportedUrl(redact_url(url)));
    };
    store.migrate().await?;
    Ok(store)
}

/// Hides the password in a database URL so it can be logged.
pub fn redact_url(url: &str) -> String {
    // scheme://user:password@host/...  ->  scheme://user:***@host/...
    if let Some((scheme, rest)) = url.split_once("://")
        && let Some((auth, host)) = rest.split_once('@')
        && let Some((user, _)) = auth.split_once(':')
    {
        return format!("{scheme}://{user}:***@{host}");
    }
    url.to_string()
}

pub(crate) fn parse_params(text: &str) -> StoreResult<serde_json::Value> {
    serde_json::from_str(text).map_err(|e| StoreError::Corrupt(format!("game_actions.params: {e}")))
}

pub(crate) fn parse_status(text: &str) -> StoreResult<ActionStatus> {
    ActionStatus::parse(text)
        .ok_or_else(|| StoreError::Corrupt(format!("game_actions.status = {text:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_password() {
        assert_eq!(
            redact_url("postgres://user:secret@db:5432/x"),
            "postgres://user:***@db:5432/x"
        );
        assert_eq!(redact_url("sqlite://data/fcc.db"), "sqlite://data/fcc.db");
    }

    #[test]
    fn status_round_trips() {
        for s in [
            ActionStatus::Pending,
            ActionStatus::Delivered,
            ActionStatus::Done,
            ActionStatus::Failed,
            ActionStatus::Rejected,
            ActionStatus::Expired,
        ] {
            assert_eq!(ActionStatus::parse(s.as_str()), Some(s));
        }
    }
}
