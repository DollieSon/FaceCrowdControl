const express = require('express');
const app = express();
const {givePointsToUser} = require('./commands');
const { Client } = require('pg');

// Settings come from the environment (.env via `npm start` or docker compose's env_file).
// Defaults match the original hard-coded values.
const client = new Client({
    user: process.env.PGUSER || 'user',
    host: process.env.PGHOST || 'db',
    database: process.env.PGDATABASE || 'facecrowdcontrol',
    password: process.env.PGPASSWORD || 'password',
    port: Number(process.env.PGPORT) || 5432,
})

let dbReady = false;

function incrementPoint(dbclient){
    const text = 'UPDATE users SET point = point + 1 WHERE LatestInteraction >= NOW() - INTERVAL \'30 minutes\' RETURNING *;';
    dbclient.query(text)
        .then(res => {
            console.log('Point incremented:', res.rows[0]);
        })
        .catch(err => {
            console.error('Error executing query', err.stack);
        });
}

// Idempotent schema setup. The ALTERs upgrade databases created by older versions.
const SCHEMA = `
    CREATE TABLE IF NOT EXISTS users (
        id SERIAL PRIMARY KEY,
        name VARCHAR(100) NOT NULL,
        point INTEGER,
        createdAt TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
        latestInteraction TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    );
    -- Identity is (platform, platform_user_id); name is only a display label.
    ALTER TABLE users ADD COLUMN IF NOT EXISTS platform VARCHAR(32);
    ALTER TABLE users ADD COLUMN IF NOT EXISTS platform_user_id VARCHAR(200);
    CREATE UNIQUE INDEX IF NOT EXISTS users_platform_user_uidx ON users (platform, platform_user_id);

    -- Every chat message received; also how resends are recognised and dropped.
    CREATE TABLE IF NOT EXISTS chat_messages (
        id BIGSERIAL PRIMARY KEY,
        platform VARCHAR(32) NOT NULL,
        message_id VARCHAR(300) NOT NULL,
        user_id INTEGER REFERENCES users(id),
        platform_user_id VARCHAR(200) NOT NULL,
        display_name VARCHAR(100) NOT NULL,
        message TEXT NOT NULL,
        sent_at TIMESTAMPTZ,
        received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
        UNIQUE (platform, message_id)
    );
`;

client.connect()
    .then(async () => {
        console.log('Connected to PostgreSQL')
        await client.query(SCHEMA);
        dbReady = true;
        setInterval(()=>{incrementPoint(client)}, 3 * 60 * 1000); // Increment points every 3 minutes
    })
    .catch(err => console.error('Connection error', err.stack));

app.get('/', (req, res) => {
    // give the currrent time
    const time = new Date();
    res.send(`Hello, Worldies! Current time: ${time}`);
});

app.get('/health', async (req, res) => {
    if (!dbReady) return res.status(200).json({ ok: true, db: false });
    try {
        await client.query('SELECT 1');
        res.status(200).json({ ok: true, db: true });
    } catch {
        res.status(200).json({ ok: true, db: false });
    }
});

app.post('/users', express.json(), async (req, res) => {
    const { name, age } = req.body;
    console.log(`Received user: ${name}, age: ${age}`);
    try {
        const text = `
            INSERT INTO users (name, point)
            VALUES ($1, $2)
        `;
        const values = [name, age];
        const result = await client.query(text, values);
        res.status(201).json(result.rows[0]);
    } catch (err) {
        console.error('Error executing query', err.stack);
        res.status(500).send('Internal Server Error');
    }
});

const commands = {
    'give': {
        parameters: ['targetId', 'amount'],
        function : givePointsToUser
    }
}

/**
 * Runs a chat command for `sender` (a users row).
 * Returns null when the message isn't a command, otherwise
 * { ok, command, result } or { ok: false, command, error, status }.
 */
async function runCommand(dbclient, sender, message) {
    const text = String(message).trim();
    if (!text.startsWith('/')) return null;

    const [command, ...args] = text.slice(1).split(/\s+/);
    // hasOwnProperty so "/toString" etc. don't resolve to Object.prototype
    if (!Object.prototype.hasOwnProperty.call(commands, command)) {
        return { ok: false, command, status: 400, error: `Unknown command: ${command}` };
    }
    const { parameters, function: comfunction } = commands[command];
    if (args.length !== parameters.length) {
        return { ok: false, command, status: 400, error: `Usage: /${command} ${parameters.join(' ')}` };
    }
    try {
        console.log(`Executing command: ${command} for ${sender.name} with args: ${args}`);
        const result = await comfunction(dbclient, sender, ...args);
        return { ok: true, command, result };
    } catch (error) {
        console.error('Command execution error:', error);
        return { ok: false, command, status: 500, error: `Error executing command: ${error.message}` };
    }
}

app.post('/fb_message',express.json(), async (req, res) => {
    const { fbname, message } = req.body;
    console.log(`Received message from ${fbname}: ${message}`);
    // check if the user exists
    try {
        const text = 'SELECT * FROM users WHERE name = $1';
        const values = [fbname];
        const result = await client.query(text, values);
        if (result.rows.length > 0) {
            // User exists, update latest interaction
            const updateText = 'UPDATE users SET latestInteraction = NOW() WHERE name = $1 RETURNING *';
            const updateValues = [fbname];
            const updateResult = await client.query(updateText, updateValues);

            const outcome = await runCommand(client, updateResult.rows[0], message);
            if (!outcome) {
                // If no command, just return the user info
                return res.status(200).json(updateResult.rows[0]);
            }
            if (outcome.ok) return res.status(200).json(outcome.result);
            return res.status(outcome.status).send(outcome.error);
        } else {
            // User does not exist, create new user
            const insertText = 'INSERT INTO users (name, point) VALUES ($1, 0) RETURNING *';
            const insertValues = [fbname];
            const insertResult = await client.query(insertText, insertValues);
            res.status(201).json(insertResult.rows[0]);
        }
    }catch (err) {
        console.error('Error executing query', err.stack);
        res.status(500).send('Internal Server Error: query failed');
    }
});

// ---- /chat: normalized chat events from the browser extension (and later Twitch/YouTube) ----

const PLATFORMS = new Set(['facebook', 'youtube', 'twitch']);

function str(value, max) {
    return typeof value === 'string' && value.trim() !== '' && value.length <= max ? value.trim() : null;
}

/** Returns a clean event or an error string. */
function validateEvent(e) {
    if (!e || typeof e !== 'object') return 'event must be an object';
    const platform = str(e.platform, 32);
    if (!platform || !PLATFORMS.has(platform)) return 'unknown platform';
    const platformUserId = str(e.platformUserId, 200);
    if (!platformUserId) return 'platformUserId is required';
    const displayName = str(e.displayName, 100) || platformUserId.slice(0, 100);
    const message = str(e.message, 2000);
    if (!message) return 'message is required';
    const messageId = str(e.messageId, 300);
    if (!messageId) return 'messageId is required';
    const sentAt = e.timestamp && !Number.isNaN(Date.parse(e.timestamp)) ? new Date(e.timestamp) : null;
    return { platform, platformUserId, displayName, message, messageId, sentAt };
}

async function handleChatEvent(e) {
    // Insert first: if this message id was already stored it's a resend, and nothing else happens.
    const inserted = await client.query(
        `INSERT INTO chat_messages (platform, message_id, platform_user_id, display_name, message, sent_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (platform, message_id) DO NOTHING
         RETURNING id`,
        [e.platform, e.messageId, e.platformUserId, e.displayName, e.message, e.sentAt],
    );
    if (inserted.rows.length === 0) return { messageId: e.messageId, status: 'duplicate' };

    const user = await client.query(
        `INSERT INTO users (name, point, platform, platform_user_id)
         VALUES ($1, 0, $2, $3)
         ON CONFLICT (platform, platform_user_id)
         DO UPDATE SET name = EXCLUDED.name, latestInteraction = NOW()
         RETURNING *`,
        [e.displayName, e.platform, e.platformUserId],
    );
    const sender = user.rows[0];
    await client.query('UPDATE chat_messages SET user_id = $1 WHERE id = $2', [sender.id, inserted.rows[0].id]);

    const outcome = await runCommand(client, sender, e.message);
    const result = { messageId: e.messageId, status: 'ok', userId: sender.id };
    if (outcome) {
        // Command failures are chat-level outcomes, not HTTP errors: the request itself succeeded.
        result.command = outcome.ok
            ? { name: outcome.command, ok: true, result: outcome.result }
            : { name: outcome.command, ok: false, error: outcome.error };
    }
    return result;
}

app.post('/chat', express.json({ limit: '256kb' }), async (req, res) => {
    // express.json() only parses application/json, so a page in the browser can't
    // forge this request without a CORS preflight (which this server never approves).
    if (!req.is('application/json')) {
        return res.status(415).json({ error: 'Content-Type must be application/json' });
    }
    if (!dbReady) return res.status(503).json({ error: 'Database not connected' });

    const raw = Array.isArray(req.body) ? req.body : Array.isArray(req.body.events) ? req.body.events : [req.body];
    if (raw.length === 0 || raw.length > 100) return res.status(400).json({ error: 'Send 1-100 events' });

    const results = [];
    try {
        for (const item of raw) {
            const event = validateEvent(item);
            if (typeof event === 'string') {
                results.push({ messageId: item && item.messageId, status: 'invalid', error: event });
                continue;
            }
            console.log(`[chat] ${event.platform}:${event.platformUserId} (${event.displayName}): ${event.message}`);
            results.push(await handleChatEvent(event));
        }
    } catch (err) {
        // Database failure: the extension retries, and already-stored messages come back as duplicates.
        console.error('Error handling chat events', err.stack);
        return res.status(500).json({ error: 'Database error', results });
    }
    res.status(200).json({ results });
});

app.get('/users', async (req, res) => {
    try {
        const result = await client.query('SELECT * FROM users');
        res.status(200).json(result.rows);
    } catch (err) {
        console.error('Error executing query', err.stack);
        res.status(500).send('Internal Server Error');
    }
});

const port = process.env.PORT || 3000;

app.listen(port, () => {
    console.log(`Server is runnin on http://localhost:${port}`);
});
