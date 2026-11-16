//! Chat commands that trigger game actions, loaded from games.toml.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// Built-in chat commands; a game command can't use these names.
pub const RESERVED: &[&str] = &["give", "points", "balance", "help"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameCommand {
    /// Chat command without the slash: `heal` means `/heal`.
    pub name: String,
    /// Which connected game receives it (the `game` the mod sends in `hello`).
    pub game: String,
    /// Action name the mod understands.
    pub action: String,
    /// Points it costs. Refunded if the game rejects it, fails, or doesn't answer.
    pub cost: i64,
    /// Seconds before anyone can use this command again. 0 = no cooldown.
    #[serde(default)]
    pub cooldown_secs: u64,
    /// Fixed parameters sent with the action. Viewers can't change them.
    #[serde(default = "empty_object")]
    pub params: serde_json::Value,
    #[serde(default)]
    pub description: String,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    #[serde(default, rename = "command")]
    commands: Vec<GameCommand>,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    commands: BTreeMap<String, GameCommand>,
}

impl Catalog {
    /// Reads games.toml. A missing file just means no game commands.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            tracing::warn!(
                "{} not found: no game commands are available",
                path.display()
            );
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_toml(&text).with_context(|| format!("in {}", path.display()))
    }

    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        let file: CatalogFile = toml::from_str(text)?;
        Self::from_commands(file.commands)
    }

    pub fn from_commands(list: Vec<GameCommand>) -> anyhow::Result<Self> {
        let mut commands = BTreeMap::new();
        for mut c in list {
            c.name = c.name.to_ascii_lowercase();
            if !is_simple_name(&c.name) {
                bail!(
                    "command name {:?}: use 1-32 lowercase letters, digits or _",
                    c.name
                );
            }
            if RESERVED.contains(&c.name.as_str()) {
                bail!("command name {:?} is a built-in command", c.name);
            }
            if !is_simple_name(&c.game) || c.action.trim().is_empty() {
                bail!(
                    "command {:?}: game must be a simple name and action can't be empty",
                    c.name
                );
            }
            if c.cost < 0 {
                bail!("command {:?}: cost can't be negative", c.name);
            }
            if !c.params.is_object() {
                bail!("command {:?}: params must be a table", c.name);
            }
            let name = c.name.clone();
            if commands.insert(name.clone(), c).is_some() {
                bail!("command {name:?} is defined twice");
            }
        }
        Ok(Self { commands })
    }

    pub fn get(&self, name: &str) -> Option<&GameCommand> {
        self.commands.get(name)
    }

    pub fn all(&self) -> impl Iterator<Item = &GameCommand> {
        self.commands.values()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

/// 1-32 chars of a-z, 0-9, `_` or `-`.
pub fn is_simple_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_commands() {
        let c = Catalog::from_toml(
            r#"
            [[command]]
            name = "Heal"
            game = "sts2"
            action = "heal"
            cost = 50
            cooldown_secs = 30
            params = { amount = 5 }
            "#,
        )
        .unwrap();
        let heal = c.get("heal").unwrap();
        assert_eq!(heal.cost, 50);
        assert_eq!(heal.params, serde_json::json!({"amount": 5}));
    }

    #[test]
    fn rejects_bad_catalogs() {
        let bad = [
            r#"[[command]]
               name = "give"
               game = "sts2"
               action = "x"
               cost = 1"#,
            r#"[[command]]
               name = "a"
               game = "sts2"
               action = "x"
               cost = -1"#,
            r#"[[command]]
               name = "has space"
               game = "sts2"
               action = "x"
               cost = 1"#,
            r#"[[command]]
               name = "a"
               game = "sts2"
               action = "x"
               cost = 1
               [[command]]
               name = "a"
               game = "sts2"
               action = "y"
               cost = 1"#,
            r#"[[command]]
               name = "a"
               game = "sts2"
               action = "x"
               cost = 1
               typo = true"#,
        ];
        for text in bad {
            assert!(Catalog::from_toml(text).is_err(), "should reject:\n{text}");
        }
    }

    #[test]
    fn example_games_toml_is_valid() {
        let c = Catalog::from_toml(include_str!("../../games.toml")).unwrap();
        assert!(!c.is_empty());
    }
}
