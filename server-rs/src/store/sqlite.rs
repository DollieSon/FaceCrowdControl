//! SQLite implementation of [`Store`]. Timestamps are stored as INTEGER Unix milliseconds.

use std::{path::Path, str::FromStr, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use sqlx::{
    Row, SqlitePool,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow},
};

use super::*;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");

const USER_COLS: &str =
    "id, platform, platform_user_id, display_name, points, created_at, last_seen_at";
const ACTION_COLS: &str =
    "id, game, action, params, user_id, cost, status, error, created_at, updated_at";
const OPEN: &str = "('pending', 'delivered')";

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// `url` is e.g. `sqlite://data/fcc.db` (file, created if missing) or `sqlite::memory:`.
    pub async fn connect(url: &str) -> StoreResult<Self> {
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));

        if !url.contains(":memory:")
            && let Some(dir) = Path::new(options.get_filename()).parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir).map_err(sqlx::Error::Io)?;
        }

        // One connection: SQLite allows a single writer anyway, and this makes every
        // transaction run one after another, so read-then-write logic (e.g. "check
        // points, then subtract") can't race. Plenty for one streamer's chat.
        // It also keeps an in-memory database alive (it's dropped with its last connection).
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }
}

fn ms(t: DateTime<Utc>) -> i64 {
    t.timestamp_millis()
}

fn from_ms(v: i64) -> StoreResult<DateTime<Utc>> {
    Utc.timestamp_millis_opt(v)
        .single()
        .ok_or_else(|| StoreError::Corrupt(format!("timestamp {v}")))
}

fn user_from(row: &SqliteRow) -> StoreResult<User> {
    Ok(User {
        id: row.try_get("id")?,
        platform: row.try_get("platform")?,
        platform_user_id: row.try_get("platform_user_id")?,
        display_name: row.try_get("display_name")?,
        points: row.try_get("points")?,
        created_at: from_ms(row.try_get("created_at")?)?,
        last_seen_at: from_ms(row.try_get("last_seen_at")?)?,
    })
}

fn action_from(row: &SqliteRow) -> StoreResult<GameAction> {
    Ok(GameAction {
        id: row.try_get("id")?,
        game: row.try_get("game")?,
        action: row.try_get("action")?,
        params: parse_params(row.try_get("params")?)?,
        user_id: row.try_get("user_id")?,
        cost: row.try_get("cost")?,
        status: parse_status(row.try_get("status")?)?,
        error: row.try_get("error")?,
        created_at: from_ms(row.try_get("created_at")?)?,
        updated_at: from_ms(row.try_get("updated_at")?)?,
    })
}

fn chat_from(row: &SqliteRow) -> StoreResult<ChatMessage> {
    let sent_at: Option<i64> = row.try_get("sent_at")?;
    Ok(ChatMessage {
        id: row.try_get("id")?,
        platform: row.try_get("platform")?,
        message_id: row.try_get("message_id")?,
        user_id: row.try_get("user_id")?,
        platform_user_id: row.try_get("platform_user_id")?,
        display_name: row.try_get("display_name")?,
        message: row.try_get("message")?,
        sent_at: sent_at.map(from_ms).transpose()?,
        received_at: from_ms(row.try_get("received_at")?)?,
    })
}

#[async_trait]
impl Store for SqliteStore {
    fn backend(&self) -> &'static str {
        "sqlite"
    }

    async fn migrate(&self) -> StoreResult<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    async fn ping(&self) -> StoreResult<()> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }

    async fn record_chat(
        &self,
        msg: &NewChatMessage,
        now: DateTime<Utc>,
    ) -> StoreResult<RecordOutcome> {
        let mut tx = self.pool.begin().await?;

        // Insert first: if this message id is already stored it's a resend, and nothing else happens.
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chat_messages
                (platform, message_id, platform_user_id, display_name, message, sent_at, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (platform, message_id) DO NOTHING
             RETURNING id",
        )
        .bind(&msg.platform)
        .bind(&msg.message_id)
        .bind(&msg.platform_user_id)
        .bind(&msg.display_name)
        .bind(&msg.message)
        .bind(msg.sent_at.map(ms))
        .bind(ms(now))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(chat_id) = inserted else {
            return Ok(RecordOutcome::Duplicate);
        };

        let row = sqlx::query(&format!(
            "INSERT INTO users (platform, platform_user_id, display_name, points, created_at, last_seen_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?4)
             ON CONFLICT (platform, platform_user_id)
             DO UPDATE SET display_name = excluded.display_name, last_seen_at = excluded.last_seen_at
             RETURNING {USER_COLS}"
        ))
        .bind(&msg.platform)
        .bind(&msg.platform_user_id)
        .bind(&msg.display_name)
        .bind(ms(now))
        .fetch_one(&mut *tx)
        .await?;
        let user = user_from(&row)?;

        sqlx::query("UPDATE chat_messages SET user_id = ?1 WHERE id = ?2")
            .bind(user.id)
            .bind(chat_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(RecordOutcome::Recorded { user })
    }

    async fn user(&self, id: i64) -> StoreResult<Option<User>> {
        let row = sqlx::query(&format!("SELECT {USER_COLS} FROM users WHERE id = ?1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(user_from).transpose()
    }

    async fn users(&self, limit: i64) -> StoreResult<Vec<User>> {
        let rows = sqlx::query(&format!(
            "SELECT {USER_COLS} FROM users ORDER BY points DESC, id LIMIT ?1"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(user_from).collect()
    }

    async fn transfer_points(&self, from: i64, to: i64, amount: i64) -> StoreResult<Transfer> {
        let mut tx = self.pool.begin().await?;

        let have: Option<i64> = sqlx::query_scalar("SELECT points FROM users WHERE id = ?1")
            .bind(from)
            .fetch_optional(&mut *tx)
            .await?;
        let have = have.ok_or(StoreError::UserNotFound(from))?;
        let target_exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?1")
            .bind(to)
            .fetch_optional(&mut *tx)
            .await?;
        if target_exists.is_none() {
            return Err(StoreError::UserNotFound(to));
        }
        if have < amount {
            return Err(StoreError::InsufficientPoints { have, need: amount });
        }

        let from_row = sqlx::query(&format!(
            "UPDATE users SET points = points - ?1 WHERE id = ?2 RETURNING {USER_COLS}"
        ))
        .bind(amount)
        .bind(from)
        .fetch_one(&mut *tx)
        .await?;
        let to_row = sqlx::query(&format!(
            "UPDATE users SET points = points + ?1 WHERE id = ?2 RETURNING {USER_COLS}"
        ))
        .bind(amount)
        .bind(to)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(Transfer {
            from: user_from(&from_row)?,
            to: user_from(&to_row)?,
            amount,
        })
    }

    async fn award_active_users(
        &self,
        active_since: DateTime<Utc>,
        amount: i64,
    ) -> StoreResult<u64> {
        let res = sqlx::query("UPDATE users SET points = points + ?1 WHERE last_seen_at >= ?2")
            .bind(amount)
            .bind(ms(active_since))
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    async fn create_game_action(
        &self,
        new: &NewGameAction,
        now: DateTime<Utc>,
    ) -> StoreResult<GameAction> {
        let mut tx = self.pool.begin().await?;

        let have: Option<i64> = sqlx::query_scalar("SELECT points FROM users WHERE id = ?1")
            .bind(new.user_id)
            .fetch_optional(&mut *tx)
            .await?;
        let have = have.ok_or(StoreError::UserNotFound(new.user_id))?;
        if have < new.cost {
            return Err(StoreError::InsufficientPoints {
                have,
                need: new.cost,
            });
        }
        sqlx::query("UPDATE users SET points = points - ?1 WHERE id = ?2")
            .bind(new.cost)
            .bind(new.user_id)
            .execute(&mut *tx)
            .await?;

        let row = sqlx::query(&format!(
            "INSERT INTO game_actions (game, action, params, user_id, cost, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?6)
             RETURNING {ACTION_COLS}"
        ))
        .bind(&new.game)
        .bind(&new.action)
        .bind(new.params.to_string())
        .bind(new.user_id)
        .bind(new.cost)
        .bind(ms(now))
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;
        action_from(&row)
    }

    async fn game_action(&self, id: i64) -> StoreResult<Option<GameAction>> {
        let row = sqlx::query(&format!(
            "SELECT {ACTION_COLS} FROM game_actions WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(action_from).transpose()
    }

    async fn mark_action_delivered(&self, id: i64, now: DateTime<Utc>) -> StoreResult<bool> {
        let res = sqlx::query("UPDATE game_actions SET status = 'delivered', updated_at = ?1 WHERE id = ?2 AND status = 'pending'")
            .bind(ms(now))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() == 1)
    }

    async fn finish_action(
        &self,
        id: i64,
        status: ActionStatus,
        error: Option<&str>,
        now: DateTime<Utc>,
    ) -> StoreResult<Option<GameAction>> {
        debug_assert!(!matches!(
            status,
            ActionStatus::Pending | ActionStatus::Delivered
        ));
        let mut tx = self.pool.begin().await?;

        let row = sqlx::query(&format!(
            "UPDATE game_actions SET status = ?1, error = ?2, updated_at = ?3
             WHERE id = ?4 AND status IN {OPEN}
             RETURNING {ACTION_COLS}"
        ))
        .bind(status.as_str())
        .bind(error)
        .bind(ms(now))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let action = action_from(&row)?;

        if status.is_refunded() && action.cost > 0 {
            sqlx::query("UPDATE users SET points = points + ?1 WHERE id = ?2")
                .bind(action.cost)
                .bind(action.user_id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(Some(action))
    }

    async fn open_actions(&self, game: &str) -> StoreResult<Vec<GameAction>> {
        let rows = sqlx::query(&format!(
            "SELECT {ACTION_COLS} FROM game_actions WHERE game = ?1 AND status IN {OPEN} ORDER BY id"
        ))
        .bind(game)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(action_from).collect()
    }

    async fn stale_action_ids(&self, created_before: DateTime<Utc>) -> StoreResult<Vec<i64>> {
        Ok(sqlx::query_scalar(&format!(
            "SELECT id FROM game_actions WHERE status IN {OPEN} AND created_at < ?1 ORDER BY id"
        ))
        .bind(ms(created_before))
        .fetch_all(&self.pool)
        .await?)
    }

    async fn export(&self) -> StoreResult<Snapshot> {
        let users = sqlx::query(&format!("SELECT {USER_COLS} FROM users ORDER BY id"))
            .fetch_all(&self.pool)
            .await?;
        let chats = sqlx::query("SELECT * FROM chat_messages ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        let actions = sqlx::query(&format!(
            "SELECT {ACTION_COLS} FROM game_actions ORDER BY id"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(Snapshot {
            users: users.iter().map(user_from).collect::<StoreResult<_>>()?,
            chat_messages: chats.iter().map(chat_from).collect::<StoreResult<_>>()?,
            game_actions: actions
                .iter()
                .map(action_from)
                .collect::<StoreResult<_>>()?,
        })
    }

    async fn import(&self, snap: &Snapshot) -> StoreResult<()> {
        let mut tx = self.pool.begin().await?;

        let existing: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM users) + (SELECT COUNT(*) FROM chat_messages) + (SELECT COUNT(*) FROM game_actions)",
        )
        .fetch_one(&mut *tx)
        .await?;
        if existing > 0 {
            return Err(StoreError::NotEmpty);
        }

        for u in &snap.users {
            sqlx::query(&format!(
                "INSERT INTO users ({USER_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
            ))
            .bind(u.id)
            .bind(&u.platform)
            .bind(&u.platform_user_id)
            .bind(&u.display_name)
            .bind(u.points)
            .bind(ms(u.created_at))
            .bind(ms(u.last_seen_at))
            .execute(&mut *tx)
            .await?;
        }
        for c in &snap.chat_messages {
            sqlx::query(
                "INSERT INTO chat_messages
                    (id, platform, message_id, user_id, platform_user_id, display_name, message, sent_at, received_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .bind(c.id)
            .bind(&c.platform)
            .bind(&c.message_id)
            .bind(c.user_id)
            .bind(&c.platform_user_id)
            .bind(&c.display_name)
            .bind(&c.message)
            .bind(c.sent_at.map(ms))
            .bind(ms(c.received_at))
            .execute(&mut *tx)
            .await?;
        }
        for a in &snap.game_actions {
            sqlx::query(&format!(
                "INSERT INTO game_actions ({ACTION_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
            ))
            .bind(a.id)
            .bind(&a.game)
            .bind(&a.action)
            .bind(a.params.to_string())
            .bind(a.user_id)
            .bind(a.cost)
            .bind(a.status.as_str())
            .bind(&a.error)
            .bind(ms(a.created_at))
            .bind(ms(a.updated_at))
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }
}
