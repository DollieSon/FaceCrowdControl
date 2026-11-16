//! Chat commands: a message starting with `/`.
//!
//! Built-ins are matched in [`run`]; anything else is looked up in the game catalog
//! (games.toml). To add a built-in: write an `async fn(state, sender, args) -> CommandResult`
//! and add its name to the `match` and to `catalog::RESERVED`.

use serde::Serialize;
use serde_json::json;

use crate::{
    AppState,
    games::{self, RequestError},
    store::{StoreError, User},
};

/// The outcome of one command, as reported back in the `/chat` response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CommandReport {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// The viewer did something wrong (bad arguments, not enough points...). Shown as-is.
    #[error("{0}")]
    User(String),
    /// Our fault. Logged; the viewer gets a generic message.
    #[error(transparent)]
    Internal(#[from] StoreError),
}

pub type CommandResult = Result<serde_json::Value, CommandError>;

fn user_err(msg: impl Into<String>) -> CommandError {
    CommandError::User(msg.into())
}

/// Splits `/name arg arg` into a lowercase name and its arguments. None if it isn't a command.
pub fn parse(message: &str) -> Option<(String, Vec<&str>)> {
    let rest = message.trim().strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let name = parts.next()?.to_lowercase();
    Some((name, parts.collect()))
}

/// Runs the command in `message`, if there is one, for `sender` (their row after this message was recorded).
pub async fn run(state: &AppState, sender: &User, message: &str) -> Option<CommandReport> {
    let (name, args) = parse(message)?;
    tracing::info!(user = %sender.display_name, user_id = sender.id, command = %name, ?args, "command");

    let outcome = match name.as_str() {
        "give" => give(state, sender, &args).await,
        "points" | "balance" => points(sender, &args),
        "help" => help(state, &args),
        other => match state.catalog.get(other) {
            Some(cmd) if args.is_empty() => games::request_action(state, sender, cmd)
                .await
                .map_err(|e| match e {
                    RequestError::Store(e) => CommandError::Internal(e),
                    other => CommandError::User(other.to_string()),
                }),
            Some(_) => Err(user_err(format!("Usage: /{other}"))),
            None => Err(user_err(format!("Unknown command: /{other}"))),
        },
    };

    Some(match outcome {
        Ok(result) => CommandReport {
            name,
            ok: true,
            result: Some(result),
            error: None,
        },
        Err(CommandError::User(error)) => CommandReport {
            name,
            ok: false,
            result: None,
            error: Some(error),
        },
        Err(CommandError::Internal(e)) => {
            tracing::error!(command = %name, "command failed: {e}");
            CommandReport {
                name,
                ok: false,
                result: None,
                error: Some("Something went wrong on the server".into()),
            }
        }
    })
}

/// A whole positive number made only of digits (no sign, no decimals).
fn positive_int(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok().filter(|n| *n > 0)
}

/// `/give #42 10` (or `/give 42 10`): move points from the sender to user #42.
async fn give(state: &AppState, sender: &User, args: &[&str]) -> CommandResult {
    let [target, amount] = args else {
        return Err(user_err("Usage: /give #id amount"));
    };
    let target = positive_int(target.strip_prefix('#').unwrap_or(target))
        .ok_or_else(|| user_err("Target must be a user id number, e.g. /give #42 10"))?;
    let amount =
        positive_int(amount).ok_or_else(|| user_err("Amount must be a positive whole number"))?;
    if target == sender.id {
        return Err(user_err("You can't give points to yourself"));
    }

    match state.store.transfer_points(sender.id, target, amount).await {
        Ok(t) => Ok(json!({
            "message": format!("{amount} points transferred to #{target}"),
            "from": t.from.id,
            "to": t.to.id,
            "amount": amount,
            "balance": t.from.points,
        })),
        Err(StoreError::UserNotFound(id)) if id == target => {
            Err(user_err(format!("User #{target} doesn't exist")))
        }
        Err(StoreError::InsufficientPoints { have, .. }) => {
            Err(user_err(format!("Not enough points (you have {have})")))
        }
        Err(e) => Err(e.into()),
    }
}

/// `/points`: the sender's id and balance.
fn points(sender: &User, args: &[&str]) -> CommandResult {
    if !args.is_empty() {
        return Err(user_err("Usage: /points"));
    }
    Ok(json!({ "userId": sender.id, "points": sender.points }))
}

/// `/help`: every command and what it costs.
fn help(state: &AppState, args: &[&str]) -> CommandResult {
    if !args.is_empty() {
        return Err(user_err("Usage: /help"));
    }
    let games: Vec<_> = state
        .catalog
        .all()
        .map(|c| json!({ "command": format!("/{}", c.name), "game": c.game, "cost": c.cost, "description": c.description }))
        .collect();
    Ok(json!({
        "builtins": ["/give #id amount", "/points", "/help"],
        "games": games,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse("/"), None);
        assert_eq!(
            parse("  /Give  #42   10 "),
            Some(("give".into(), vec!["#42", "10"]))
        );
        assert_eq!(parse("/points"), Some(("points".into(), vec![])));
    }

    #[test]
    fn positive_ints_only() {
        assert_eq!(positive_int("10"), Some(10));
        for bad in [
            "",
            "0",
            "-5",
            "+5",
            "1.5",
            "1e3",
            "99999999999999999999",
            "١٢",
        ] {
            assert_eq!(positive_int(bad), None, "{bad:?}");
        }
    }
}
