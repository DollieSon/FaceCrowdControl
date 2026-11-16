//! FaceCrowdControl v2 server: the hub between live-stream chat and games.
//!
//! ```text
//!  browser extension --POST /chat--> [ chat -> commands ] --Store--> SQLite / Postgres
//!                                           |
//!                                           v
//!                      game mod <--WebSocket /ws/game-- [ games hub ]
//! ```
//!
//! - `store`    the database adapter (trait + one implementation per database)
//! - `chat`     validating and recording chat events
//! - `commands` `/give`, `/points`, `/help`, and game commands from games.toml
//! - `games`    connected games, delivering paid actions, refunds
//! - `routes`   HTTP endpoints

pub mod chat;
pub mod commands;
pub mod config;
pub mod games;
pub mod routes;
pub mod store;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use tokio::task::JoinHandle;

pub use config::Settings;
use games::{catalog::Catalog, hub::GameHub};
use store::Store;

/// Shared by every request handler and background task. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn Store>,
    pub games: Arc<GameHub>,
    pub catalog: Arc<Catalog>,
    pub settings: Arc<Settings>,
}

impl AppState {
    pub fn new(store: Arc<dyn Store>, catalog: Catalog, settings: Settings) -> Self {
        Self {
            store,
            games: Arc::new(GameHub::new()),
            catalog: Arc::new(catalog),
            settings: Arc::new(settings),
        }
    }
}

/// Connects to the database, retrying for a while (in Docker the database may still be starting).
pub async fn connect_store(url: &str) -> anyhow::Result<Arc<dyn Store>> {
    const ATTEMPTS: u32 = 30;
    let shown = store::redact_url(url);
    for attempt in 1..=ATTEMPTS {
        match store::connect(url).await {
            Ok(s) => {
                tracing::info!("connected to {} database at {shown}", s.backend());
                return Ok(s);
            }
            // Bad URLs and failed migrations won't fix themselves.
            Err(e @ (store::StoreError::UnsupportedUrl(_) | store::StoreError::Migrate(_))) => {
                return Err(e).with_context(|| format!("opening {shown}"));
            }
            Err(e) if attempt < ATTEMPTS => {
                tracing::warn!(
                    "database at {shown} not ready ({e}); retrying ({attempt}/{ATTEMPTS})"
                );
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e).with_context(|| format!("opening {shown}")),
        }
    }
    unreachable!()
}

/// Starts the timers: points for active viewers, and expiring unanswered game actions.
pub fn spawn_background_tasks(state: &AppState) -> Vec<JoinHandle<()>> {
    let points = {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(state.settings.points_interval());
            tick.tick().await; // the first tick is immediate; skip it
            loop {
                tick.tick().await;
                let window = chrono::Duration::from_std(state.settings.points_active_window())
                    .unwrap_or(chrono::Duration::MAX);
                match state
                    .store
                    .award_active_users(store::now() - window, state.settings.points_per_tick)
                    .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(
                        "gave {} point(s) to {n} active viewer(s)",
                        state.settings.points_per_tick
                    ),
                    Err(e) => tracing::error!("awarding points: {e}"),
                }
            }
        })
    };
    let expiry = {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                if let Err(e) = games::expire_stale(&state, state.settings.action_timeout()).await {
                    tracing::error!("expiring game actions: {e}");
                }
            }
        })
    };
    vec![points, expiry]
}

/// Runs the server until Ctrl+C.
pub async fn serve(settings: Settings) -> anyhow::Result<()> {
    let store = connect_store(&settings.database_url).await?;
    let catalog = Catalog::load(&settings.games_config)?;
    tracing::info!(
        "{} game command(s) loaded from {}",
        catalog.len(),
        settings.games_config.display()
    );

    let addr = format!("{}:{}", settings.bind_address, settings.port);
    let state = AppState::new(store, catalog, settings);
    let tasks = spawn_background_tasks(&state);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("listening on {addr}"))?;
    tracing::info!("server is running on http://{addr}");
    axum::serve(listener, routes::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await?;

    for t in tasks {
        t.abort();
    }
    Ok(())
}

/// `fcc-server copy-db`: moves all data between databases (either direction).
pub async fn copy_db(from: &str, to: &str) -> anyhow::Result<()> {
    let source = store::connect(from)
        .await
        .with_context(|| format!("opening source {}", store::redact_url(from)))?;
    let target = store::connect(to)
        .await
        .with_context(|| format!("opening target {}", store::redact_url(to)))?;
    let snapshot = source.export().await?;
    target.import(&snapshot).await?;
    tracing::info!(
        "copied {} users, {} chat messages, {} game actions from {} to {}",
        snapshot.users.len(),
        snapshot.chat_messages.len(),
        snapshot.game_actions.len(),
        source.backend(),
        target.backend()
    );
    Ok(())
}
