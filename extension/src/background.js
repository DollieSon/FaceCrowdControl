/*
 * Background script: the only part of the extension that talks to the local
 * server. Content scripts can't reliably fetch localhost from inside Facebook
 * (the page's Content-Security-Policy applies to them), so they hand events here.
 *
 * Kept stateless on purpose: Firefox may unload it when idle, so stats live in
 * storage.session and the server URL is re-read from config.json.
 */
'use strict';

const DEFAULT_CONFIG = { serverUrl: 'http://127.0.0.1:3000' };
const REQUEST_TIMEOUT_MS = 5000;

let configPromise = null;

/** config.json is generated from server/.env by `npm run config`; fall back to the default if it's missing. */
function getConfig() {
    if (!configPromise) {
        configPromise = fetch(browser.runtime.getURL('config.json'))
            .then((r) => (r.ok ? r.json() : {}))
            .catch(() => ({}))
            .then((c) => ({ ...DEFAULT_CONFIG, ...c, serverUrl: String(c.serverUrl || DEFAULT_CONFIG.serverUrl).replace(/\/+$/, '') }));
    }
    return configPromise;
}

async function request(path, options = {}) {
    const { serverUrl } = await getConfig();
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
    try {
        const res = await fetch(serverUrl + path, { ...options, signal: controller.signal });
        const body = await res.json().catch(() => null);
        if (!res.ok) throw new Error((body && body.error) || `Server answered ${res.status}`);
        return body;
    } catch (err) {
        if (err.name === 'AbortError') throw new Error(`Server at ${serverUrl} did not answer in time`);
        if (err instanceof TypeError) throw new Error(`Can't reach server at ${serverUrl}`);
        throw err;
    } finally {
        clearTimeout(timer);
    }
}

// ---- stats + badge ---------------------------------------------------

const EMPTY_STATS = { sentTotal: 0, lastSentAt: null, lastError: null, lastErrorAt: null };

async function getStats() {
    const { stats } = await browser.storage.session.get({ stats: EMPTY_STATS });
    return stats;
}

async function updateStats(patch) {
    const stats = { ...(await getStats()), ...patch };
    await browser.storage.session.set({ stats });
    await refreshBadge(stats);
    return stats;
}

async function refreshBadge(stats) {
    const { enabled } = await browser.storage.local.get({ enabled: false });
    stats = stats || (await getStats());
    if (!enabled) {
        await browser.action.setBadgeText({ text: '' });
        return;
    }
    if (stats.lastError) {
        await browser.action.setBadgeBackgroundColor({ color: '#dc2626' });
        await browser.action.setBadgeText({ text: 'ERR' });
        return;
    }
    await browser.action.setBadgeBackgroundColor({ color: '#16a34a' });
    const n = stats.sentTotal;
    await browser.action.setBadgeText({ text: n === 0 ? 'ON' : n > 999 ? '999+' : String(n) });
}

// ---- messages ----------------------------------------------------------

async function sendEvents(events) {
    try {
        const body = await request('/chat', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ events }),
        });
        const stats = await getStats();
        await updateStats({ sentTotal: stats.sentTotal + events.length, lastSentAt: new Date().toISOString(), lastError: null });
        return { ok: true, results: body && body.results };
    } catch (err) {
        await updateStats({ lastError: err.message, lastErrorAt: new Date().toISOString() });
        return { ok: false, error: err.message };
    }
}

async function checkHealth() {
    const { serverUrl } = await getConfig();
    try {
        const body = await request('/health');
        return { ok: true, serverUrl, db: !!(body && body.db) };
    } catch (err) {
        return { ok: false, serverUrl, error: err.message };
    }
}

browser.runtime.onMessage.addListener((msg, sender) => {
    if (!msg || typeof msg.type !== 'string') return undefined;
    switch (msg.type) {
        case 'fcc:send':
            // Only our own content scripts may send chat.
            if (!sender.tab || sender.id !== browser.runtime.id) return Promise.resolve({ ok: false, error: 'Rejected sender' });
            return sendEvents(Array.isArray(msg.events) ? msg.events : []);
        case 'fcc:health':
            return checkHealth();
        case 'fcc:stats':
            return Promise.all([getConfig(), getStats()]).then(([config, stats]) => ({ ...stats, serverUrl: config.serverUrl }));
        case 'fcc:resetError':
            return updateStats({ lastError: null, lastErrorAt: null });
        default:
            return undefined;
    }
});

browser.storage.onChanged.addListener((changes, area) => {
    if (area === 'local' && changes.enabled) refreshBadge();
});

refreshBadge();
