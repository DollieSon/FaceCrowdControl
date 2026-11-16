-- SQLite schema. Timestamps are INTEGER milliseconds since the Unix epoch (UTC),
-- so they compare correctly as numbers. Keep in step with migrations/postgres.

CREATE TABLE users (
    id               INTEGER PRIMARY KEY,
    platform         TEXT    NOT NULL,
    platform_user_id TEXT    NOT NULL,
    display_name     TEXT    NOT NULL,
    points           INTEGER NOT NULL DEFAULT 0 CHECK (points >= 0),
    created_at       INTEGER NOT NULL,
    last_seen_at     INTEGER NOT NULL,
    UNIQUE (platform, platform_user_id)
);
CREATE INDEX users_last_seen_idx ON users (last_seen_at);

-- Every chat message received; the unique key is also how resends are dropped.
CREATE TABLE chat_messages (
    id               INTEGER PRIMARY KEY,
    platform         TEXT    NOT NULL,
    message_id       TEXT    NOT NULL,
    user_id          INTEGER REFERENCES users (id),
    platform_user_id TEXT    NOT NULL,
    display_name     TEXT    NOT NULL,
    message          TEXT    NOT NULL,
    sent_at          INTEGER,
    received_at      INTEGER NOT NULL,
    UNIQUE (platform, message_id)
);

-- Actions viewers paid for, and what the game did with them.
CREATE TABLE game_actions (
    id         INTEGER PRIMARY KEY,
    game       TEXT    NOT NULL,
    action     TEXT    NOT NULL,
    params     TEXT    NOT NULL DEFAULT '{}',
    user_id    INTEGER NOT NULL REFERENCES users (id),
    cost       INTEGER NOT NULL CHECK (cost >= 0),
    status     TEXT    NOT NULL CHECK (status IN ('pending', 'delivered', 'done', 'failed', 'rejected', 'expired')),
    error      TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX game_actions_open_idx ON game_actions (game, status);
