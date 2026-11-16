//! The game side of the hub: queuing paid actions, delivering them to connected games,
//! and settling the results (refunding when an action doesn't happen).
//!
//! Life of an action:
//!   chat `/heal` -> [`request_action`]: game connected? supports it? off cooldown?
//!     -> store charges the viewer and saves it as `pending`
//!     -> sent over the game's WebSocket -> `delivered`
//!     -> game answers `result` -> [`handle_result`]: `done`, or `failed`/`rejected` + refund
//!     -> no answer within ACTION_TIMEOUT_SECS -> [`expire_stale`]: `expired` + refund
//! Actions still pending/delivered when a game reconnects are sent again ([`deliver_open`]).

pub mod catalog;
pub mod hub;
pub mod protocol;
pub mod ws;

use std::time::Duration;

use serde_json::json;

use crate::{
    AppState,
    store::{self, ActionStatus, GameAction, NewGameAction, StoreError, User},
};
use catalog::GameCommand;
use protocol::{ActionUser, ResultStatus, ServerMessage};

/// What a viewer sees when their game command didn't go through.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("{0}")]
    Unavailable(#[from] hub::Unavailable),
    #[error("not enough points (you have {have}, {command} costs {need})")]
    NotEnoughPoints {
        command: String,
        have: i64,
        need: i64,
    },
    #[error(transparent)]
    Store(StoreError),
}

/// Charges `sender` for `cmd` and sends the action to its game.
pub async fn request_action(
    state: &AppState,
    sender: &User,
    cmd: &GameCommand,
) -> Result<serde_json::Value, RequestError> {
    state.games.reserve(
        &cmd.name,
        &cmd.game,
        &cmd.action,
        Duration::from_secs(cmd.cooldown_secs),
    )?;

    let new = NewGameAction {
        game: cmd.game.clone(),
        action: cmd.action.clone(),
        params: cmd.params.clone(),
        user_id: sender.id,
        cost: cmd.cost,
    };
    let action = match state.store.create_game_action(&new, store::now()).await {
        Ok(a) => a,
        Err(e) => {
            // Not charged, so don't make everyone else wait either.
            state.games.release_cooldown(&cmd.name);
            return Err(match e {
                StoreError::InsufficientPoints { have, need } => RequestError::NotEnoughPoints {
                    command: format!("/{}", cmd.name),
                    have,
                    need,
                },
                other => RequestError::Store(other),
            });
        }
    };

    let delivered = deliver(state, &action, &sender.display_name).await;
    tracing::info!(
        action_id = action.id,
        game = %action.game,
        action = %action.action,
        user = %sender.display_name,
        cost = action.cost,
        delivered,
        "game action queued"
    );
    Ok(json!({
        "actionId": action.id,
        "game": action.game,
        "action": action.action,
        "cost": action.cost,
        "balance": sender.points - action.cost,
        "status": if delivered { "delivered" } else { "pending" },
    }))
}

/// Sends one action to its game and marks it delivered. Returns false if the game isn't connected
/// (the action stays pending until it reconnects or the action expires).
async fn deliver(state: &AppState, action: &GameAction, user_name: &str) -> bool {
    let msg = ServerMessage::Action {
        id: action.id,
        action: action.action.clone(),
        params: action.params.clone(),
        cost: action.cost,
        user: ActionUser {
            id: action.user_id,
            name: user_name.to_string(),
        },
    };
    if !state.games.send(&action.game, msg) {
        return false;
    }
    if let Err(e) = state
        .store
        .mark_action_delivered(action.id, store::now())
        .await
    {
        tracing::error!(action_id = action.id, "couldn't mark action delivered: {e}");
    }
    true
}

/// Sends every unfinished action for `game` (e.g. right after it connects). Actions the
/// connected build doesn't support are rejected and refunded.
pub async fn deliver_open(state: &AppState, game: &str) -> Result<(), StoreError> {
    for action in state.store.open_actions(game).await? {
        if !state.games.supports(game, &action.action) {
            let reason = format!("{game} no longer supports {:?}", action.action);
            state
                .store
                .finish_action(
                    action.id,
                    ActionStatus::Rejected,
                    Some(&reason),
                    store::now(),
                )
                .await?;
            tracing::warn!(action_id = action.id, "{reason}; refunded");
            continue;
        }
        let name = match state.store.user(action.user_id).await? {
            Some(u) => u.display_name,
            None => format!("#{}", action.user_id),
        };
        deliver(state, &action, &name).await;
    }
    Ok(())
}

/// Settles a `result` message from `game`.
pub async fn handle_result(
    state: &AppState,
    game: &str,
    id: i64,
    status: ResultStatus,
    message: Option<&str>,
) -> Result<(), StoreError> {
    match state.store.game_action(id).await? {
        Some(a) if a.game == game => {}
        _ => {
            tracing::warn!(
                game,
                action_id = id,
                "result for an action that isn't this game's; ignored"
            );
            return Ok(());
        }
    }
    let status = match status {
        ResultStatus::Done => ActionStatus::Done,
        ResultStatus::Failed => ActionStatus::Failed,
        ResultStatus::Rejected => ActionStatus::Rejected,
    };
    match state
        .store
        .finish_action(id, status, message, store::now())
        .await?
    {
        Some(a) if status.is_refunded() => {
            tracing::info!(action_id = id, game, status = status.as_str(), reason = ?message, "refunded {} points to user #{}", a.cost, a.user_id)
        }
        Some(_) => tracing::info!(action_id = id, game, "action done"),
        None => tracing::warn!(
            action_id = id,
            game,
            "result for an action that was already finished; ignored"
        ),
    }
    Ok(())
}

/// Expires and refunds actions nobody answered within `timeout`. Returns how many expired.
pub async fn expire_stale(state: &AppState, timeout: Duration) -> Result<usize, StoreError> {
    let cutoff =
        store::now() - chrono::Duration::from_std(timeout).unwrap_or(chrono::Duration::MAX);
    let reason = format!("the game didn't answer within {}s", timeout.as_secs());
    let mut expired = 0;
    for id in state.store.stale_action_ids(cutoff).await? {
        if let Some(a) = state
            .store
            .finish_action(id, ActionStatus::Expired, Some(&reason), store::now())
            .await?
        {
            tracing::warn!(action_id = id, game = %a.game, "action expired; refunded {} points to user #{}", a.cost, a.user_id);
            expired += 1;
        }
    }
    Ok(expired)
}
