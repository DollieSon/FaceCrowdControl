# FaceCrowdControl server v2 (Rust)

The hub between live-stream chat and games. The browser extension sends chat here.
The server keeps track of viewers and their points, runs chat commands, and sends paid
actions to connected games. The Node server in `../server` is v1, kept as is.

```
browser extension ──POST /chat──► chat ─► commands ──► Store ──► SQLite or Postgres
                                              │
                                              ▼
     game mod (StS2) ◄──WebSocket /ws/game── games hub
```

## Run it

Needs Rust 1.88 or newer (`rustup update`).

```sh
cd server-rs
cp .env.example .env        # optional: the defaults work
cargo run --release         # http://127.0.0.1:3000, SQLite file at data/fcc.db
```

Or with Docker: `docker compose up --build`.

Point the extension at it. It uses the same `PORT` / `BIND_ADDRESS` as v1, so if
nothing changed it already works. Otherwise:

```sh
cd ../extension
npm run config -- --env ../server-rs/.env
```

Don't run v1 and v2 on the same port at the same time.

## What it does

- **`POST /chat`** takes chat events from the extension. Same request and response
  format as v1, so the extension needs no changes. Resent messages are recognised by
  `messageId` and ignored.
- **Points**: viewers who chatted in the last 30 min earn 1 point every 3 min
  (configurable).
- **Chat commands**:
  - `/give #42 10` moves 10 of your points to user #42.
  - `/points` shows your id and balance.
  - `/help` lists the commands and their costs.
  - Game commands like `/heal` come from [`games.toml`](games.toml). They cost points,
    can have a cooldown, and are **refunded** if the game rejects them, fails, or
    doesn't answer.
- **Games** connect over a WebSocket and receive the actions viewers paid for. See
  [docs/GAME_PROTOCOL.md](docs/GAME_PROTOCOL.md) and the Slay the Spire 2 plan in
  [docs/STS2_MOD.md](docs/STS2_MOD.md).

Command results go back in the `/chat` response. They don't reach the live chat yet:
the extension only reads chat, it doesn't post to it.

### Endpoints

| Method | Path          | What                                                          |
|--------|---------------|---------------------------------------------------------------|
| GET    | `/health`     | `{ ok, db, backend, games }` (the extension popup uses this)  |
| POST   | `/chat`       | chat events: `{ "events": [...] }`, an array, or one event    |
| GET    | `/users`      | users, most points first (`?limit=`)                          |
| GET    | `/users/{id}` | one user                                                      |
| GET    | `/games`      | connected games and every game command                        |
| GET    | `/ws/game`    | WebSocket for game mods                                       |

## Database: SQLite or Postgres

Set `DATABASE_URL`. Nothing else changes:

```sh
DATABASE_URL=sqlite://data/fcc.db                                         # default
DATABASE_URL=postgres://user:password@localhost:5432/facecrowdcontrol_v2  # give v2 its own database
```

The schema is created and upgraded on start. To take your data with you when switching:

```sh
cargo run -- copy-db --from sqlite://data/fcc.db --to postgres://user:password@localhost:5432/facecrowdcontrol_v2
```

(It works in either direction. The target must be empty.)

**How the switch stays safe**: the rest of the code never writes SQL. It calls the
`Store` trait in [`src/store/mod.rs`](src/store/mod.rs), which has two implementations,
`sqlite.rs` and `postgres.rs`, each with its own `migrations/<backend>/` folder.
`tests/store_conformance.rs` runs **the same tests against both**, including concurrent
transfers and refunds, so a feature that passes on one passes on the other.

### Changing the schema

1. Add a new file to **both** `migrations/sqlite/` and `migrations/postgres/` with the
   same number, e.g. `0002_add_x.sql`. Never edit a migration that has already run.
2. Add or extend the `Store` method in `store/mod.rs`, then implement it in both
   `sqlite.rs` and `postgres.rs`.
3. Add a test to `tests/store_conformance.rs`. It runs on both backends automatically.

## Configuration

Flags or environment variables (a `.env` file in `server-rs/` is read on start).
`cargo run -- --help` lists them all.

| Variable                    | Default               |                                              |
|-----------------------------|-----------------------|----------------------------------------------|
| `PORT`                      | `3000`                |                                              |
| `BIND_ADDRESS`              | `127.0.0.1`           | only this PC can connect                     |
| `DATABASE_URL`              | `sqlite://data/fcc.db`|                                              |
| `GAMES_CONFIG`              | `games.toml`          | game commands                                |
| `GAME_TOKEN`                | (unset)               | require `?token=` from game mods             |
| `POINTS_INTERVAL_SECS`      | `180`                 |                                              |
| `POINTS_PER_TICK`           | `1`                   |                                              |
| `POINTS_ACTIVE_WINDOW_SECS` | `1800`                | "active" = chatted within this               |
| `ACTION_TIMEOUT_SECS`       | `60`                  | unanswered game actions are refunded after this |
| `RUST_LOG`                  | `fcc_server=info`     | log detail                                   |

## Try it without a game or Facebook

```sh
cargo run                               # terminal 1: the server
cargo run --example fake_game           # terminal 2: pretends to be the StS2 mod
```

```sh
# terminal 3: a viewer chats
curl -s localhost:3000/chat -H "content-type: application/json" \
  -d '{"events":[{"platform":"facebook","platformUserId":"1","displayName":"Maria","message":"/points","messageId":"m1"}]}'
```

New viewers start with 0 points, so `/heal` will say "not enough points" until they've
earned some. To test faster, set `POINTS_INTERVAL_SECS=5` and `POINTS_PER_TICK=50`.

## Tests

```sh
cargo test                     # unit tests, SQLite conformance tests, HTTP + WebSocket tests
cargo clippy --all-targets
```

To also run the conformance tests on Postgres (each test makes and drops its own database):

```sh
docker run --rm -d --name fcc-test-pg -p 55432:5432 -e POSTGRES_PASSWORD=test postgres:17-alpine
FCC_TEST_POSTGRES_URL=postgres://postgres:test@127.0.0.1:55432/postgres cargo test
```

## Code map

```
src/main.rs              CLI: serve (default) / copy-db
src/lib.rs               AppState, startup, background timers (points, action expiry)
src/config.rs            settings
src/routes.rs            HTTP endpoints
src/chat.rs              validating + recording chat events
src/commands.rs          /give, /points, /help, game commands
src/games/mod.rs         paid actions: charge, deliver, settle, refund
src/games/hub.rs         connected games + cooldowns (in memory)
src/games/ws.rs          the /ws/game WebSocket
src/games/protocol.rs    WebSocket message types
src/games/catalog.rs     games.toml
src/store/               database adapter: trait + SQLite + Postgres
migrations/{sqlite,postgres}/
tests/                   conformance (both databases) + end-to-end
examples/fake_game.rs    stand-in for the game mod
```

## Differences from v1

- `/give` to a user that doesn't exist no longer takes your points. `/give` to yourself
  is refused. Concurrent `/give`s can't spend the same points twice: the check and the
  transfer happen in one transaction.
- Old endpoints `/fb_message` and `POST /users` are gone (the extension uses `/chat`).
- New: game commands, refunds, `/points`, `/help`, `/games`, `/users/{id}`, SQLite.
- The v2 schema is different from v1's (snake_case columns, `points` instead of `point`),
  so use a separate database. v1 data isn't imported.
