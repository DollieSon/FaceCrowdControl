//! Settings, from command-line flags or environment variables (a `.env` file in the
//! working directory is loaded first). Run `fcc-server --help` for the list.

use std::{path::PathBuf, time::Duration};

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "fcc-server",
    version,
    about = "FaceCrowdControl server: the hub between live-stream chat and games"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub settings: Settings,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the server (the default).
    Serve,
    /// Copy every row from one database into another, empty, one (e.g. SQLite -> Postgres).
    CopyDb {
        /// Source DATABASE_URL.
        #[arg(long)]
        from: String,
        /// Target DATABASE_URL. Its schema is created if needed; it must have no rows.
        #[arg(long)]
        to: String,
    },
}

#[derive(Debug, Clone, Args)]
pub struct Settings {
    /// Address to listen on. 127.0.0.1 = only this PC can connect.
    #[arg(long, env = "BIND_ADDRESS", default_value = "127.0.0.1")]
    pub bind_address: String,

    #[arg(long, env = "PORT", default_value_t = 3000)]
    pub port: u16,

    /// `sqlite://path/to/file.db` or `postgres://user:password@host:5432/dbname`.
    #[arg(long, env = "DATABASE_URL", default_value = "sqlite://data/fcc.db")]
    pub database_url: String,

    /// Game commands file.
    #[arg(long, env = "GAMES_CONFIG", default_value = "games.toml")]
    pub games_config: PathBuf,

    /// If set, games must connect to /ws/game?token=<this>.
    #[arg(long, env = "GAME_TOKEN")]
    pub game_token: Option<String>,

    /// How often active viewers earn points, in seconds.
    #[arg(long, env = "POINTS_INTERVAL_SECS", default_value_t = 180)]
    pub points_interval_secs: u64,

    /// Points each active viewer earns per interval.
    #[arg(long, env = "POINTS_PER_TICK", default_value_t = 1)]
    pub points_per_tick: i64,

    /// A viewer counts as active if they chatted within this many seconds.
    #[arg(long, env = "POINTS_ACTIVE_WINDOW_SECS", default_value_t = 1800)]
    pub points_active_window_secs: u64,

    /// Seconds a game has to answer an action before it's expired and refunded.
    #[arg(long, env = "ACTION_TIMEOUT_SECS", default_value_t = 60)]
    pub action_timeout_secs: u64,
}

impl Settings {
    /// The defaults above, ignoring the environment. Handy for tests.
    pub fn defaults() -> Self {
        let mut s = <Cli as clap::FromArgMatches>::from_arg_matches(
            &<Cli as clap::CommandFactory>::command()
                .mut_args(|a| a.env(None::<&str>))
                .get_matches_from(["fcc-server"]),
        )
        .expect("defaults parse")
        .settings;
        s.game_token = None;
        s
    }

    /// Empty values (`GAME_TOKEN=`) count as unset.
    pub fn normalized(mut self) -> Self {
        self.game_token = self.game_token.filter(|t| !t.trim().is_empty());
        self
    }

    pub fn points_interval(&self) -> Duration {
        Duration::from_secs(self.points_interval_secs.max(1))
    }

    pub fn points_active_window(&self) -> Duration {
        Duration::from_secs(self.points_active_window_secs)
    }

    pub fn action_timeout(&self) -> Duration {
        Duration::from_secs(self.action_timeout_secs.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_v1() {
        let s = Settings::defaults();
        assert_eq!(s.port, 3000);
        assert_eq!(s.bind_address, "127.0.0.1");
        assert_eq!(s.points_interval_secs, 180);
        assert_eq!(s.points_active_window_secs, 1800);
    }
}
