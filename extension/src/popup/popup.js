'use strict';

const $ = (id) => document.getElementById(id);

const FACEBOOK_ORIGINS = ['https://*.facebook.com/*'];

function setDot(id, kind) {
    $(id).className = `dot ${kind || ''}`;
}

function timeAgo(iso) {
    if (!iso) return 'never';
    const s = Math.round((Date.now() - new Date(iso).getTime()) / 1000);
    if (s < 5) return 'just now';
    if (s < 60) return `${s}s ago`;
    if (s < 3600) return `${Math.round(s / 60)}m ago`;
    return new Date(iso).toLocaleTimeString();
}

/** Match pattern for the server host (match patterns can't carry a port; they match any port). */
function serverOriginPattern(serverUrl) {
    try {
        const u = new URL(serverUrl);
        return `${u.protocol}//${u.hostname}/*`;
    } catch {
        return null;
    }
}

let serverUrl = null;

async function refreshPermissions() {
    const origins = [...FACEBOOK_ORIGINS];
    const serverPattern = serverUrl && serverOriginPattern(serverUrl);
    if (serverPattern) origins.push(serverPattern);
    const granted = await browser.permissions.contains({ origins });
    $('permissions').hidden = granted;
    return origins;
}

async function refreshServer() {
    setDot('server-dot', '');
    $('server-status').textContent = 'Checking…';
    const h = await browser.runtime.sendMessage({ type: 'fcc:health' });
    serverUrl = h.serverUrl;
    $('server-url').textContent = h.serverUrl;
    if (!h.ok) {
        setDot('server-dot', 'bad');
        $('server-status').textContent = 'Not reachable';
        $('server-status').title = h.error || '';
    } else if (!h.db) {
        setDot('server-dot', 'warn');
        $('server-status').textContent = 'Running, database not connected';
    } else {
        setDot('server-dot', 'ok');
        $('server-status').textContent = 'Connected';
    }
}

async function refreshTab(enabled) {
    $('tab-stats').hidden = true;
    let status = null;
    try {
        const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
        if (tab) status = await browser.tabs.sendMessage(tab.id, { type: 'fcc:tabStatus' });
    } catch {
        status = null; // no content script in this tab
    }

    if (!status) {
        setDot('tab-dot', '');
        $('tab-status').textContent = 'Not a Facebook page (or reload the tab)';
        return;
    }
    if (!status.livePage) {
        setDot('tab-dot', '');
        $('tab-status').textContent = 'Facebook, but not a live video page';
        return;
    }
    if (!enabled || !status.capturing) {
        setDot('tab-dot', '');
        $('tab-status').textContent = 'Live page found, capture is off';
        return;
    }

    setDot('tab-dot', status.lastError ? 'bad' : 'ok');
    $('tab-status').textContent = status.captured === 0 && status.skipped === 0
        ? 'Watching… no comments seen yet'
        : 'Capturing live chat';
    $('tab-stats').hidden = false;
    $('tab-captured').textContent = status.captured;
    $('tab-sent').textContent = status.sent;
    $('tab-queued').textContent = status.queued;
    $('tab-skipped').textContent = status.skipped;
}

async function refreshStats() {
    const s = await browser.runtime.sendMessage({ type: 'fcc:stats' });
    $('sent-total').textContent = s.sentTotal;
    $('last-sent').textContent = timeAgo(s.lastSentAt);
    $('last-error').hidden = !s.lastError;
    $('last-error').textContent = s.lastError ? `Last error (${timeAgo(s.lastErrorAt)}): ${s.lastError}` : '';
}

async function refreshAll() {
    const { enabled } = await browser.storage.local.get({ enabled: false });
    await Promise.all([refreshTab(enabled), refreshStats()]);
}

async function init() {
    const settings = await browser.storage.local.get({ enabled: false, highlight: false });
    $('enabled').checked = settings.enabled;
    $('highlight').checked = settings.highlight;

    $('enabled').addEventListener('change', async (e) => {
        await browser.storage.local.set({ enabled: e.target.checked });
        setTimeout(refreshAll, 300);
    });
    $('highlight').addEventListener('change', (e) => browser.storage.local.set({ highlight: e.target.checked }));
    $('check').addEventListener('click', async () => {
        await browser.runtime.sendMessage({ type: 'fcc:resetError' });
        await refreshServer();
        await refreshAll();
    });
    $('grant').addEventListener('click', async () => {
        // Must be called directly from the click for Firefox to show the prompt.
        const origins = [...FACEBOOK_ORIGINS];
        const serverPattern = serverUrl && serverOriginPattern(serverUrl);
        if (serverPattern) origins.push(serverPattern);
        await browser.permissions.request({ origins });
        await refreshPermissions();
        await refreshServer();
    });

    await refreshServer();
    await refreshPermissions();
    await refreshAll();
    setInterval(refreshAll, 1000);
}

init();
