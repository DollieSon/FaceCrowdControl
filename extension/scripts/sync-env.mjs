#!/usr/bin/env node
/*
 * Writes src/config.json for the extension from the server's .env file.
 *
 * Browser extensions can't read files on disk at runtime, so the server address
 * is baked in at build time. Re-run (`npm run config`) after changing .env, then
 * reload the extension.
 *
 *   node scripts/sync-env.mjs                 # reads ../server/.env
 *   node scripts/sync-env.mjs --env path/.env
 *
 * Variables used (all optional):
 *   EXTENSION_SERVER_URL  full URL, wins over everything else
 *   BIND_ADDRESS          host the server is published on (0.0.0.0 -> 127.0.0.1)
 *   PORT                  server port (default 3000)
 */
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const argIdx = process.argv.indexOf('--env');
const envPath = resolve(argIdx > -1 ? process.argv[argIdx + 1] : resolve(here, '../../server/.env'));
const outPath = resolve(here, '../src/config.json');

export function parseEnv(text) {
    const env = {};
    for (const raw of text.split(/\r?\n/)) {
        const line = raw.trim();
        if (!line || line.startsWith('#')) continue;
        const m = line.match(/^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/);
        if (!m) continue;
        let value = m[2];
        const quoted = value.match(/^(['"])(.*)\1$/);
        if (quoted) value = quoted[2];
        else value = value.replace(/\s+#.*$/, '').trim();
        env[m[1]] = value;
    }
    return env;
}

export function serverUrlFrom(env) {
    if (env.EXTENSION_SERVER_URL) return env.EXTENSION_SERVER_URL.replace(/\/+$/, '');
    let host = env.BIND_ADDRESS || '127.0.0.1';
    if (host === '0.0.0.0' || host === '::') host = '127.0.0.1';
    const port = env.PORT || '3000';
    return `http://${host}:${port}`;
}

function main() {
    let env = {};
    if (existsSync(envPath)) {
        env = parseEnv(readFileSync(envPath, 'utf8'));
    } else {
        console.warn(`No .env at ${envPath}; using defaults.`);
    }
    const config = { serverUrl: serverUrlFrom(env) };
    writeFileSync(outPath, JSON.stringify(config, null, 2) + '\n');
    console.log(`Wrote ${outPath}: ${JSON.stringify(config)}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
