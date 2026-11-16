/*
 * Content script for Facebook live pages.
 *
 * Watches the page for new chat comments, turns each into a normalized chat
 * event and hands batches to the background script, which posts them to the
 * local server. Queueing and retries live here because a content script lives
 * as long as the tab does, while Firefox may unload the background script.
 */
(() => {
    'use strict';

    const P = globalThis.FCCFacebookParser;
    const PLATFORM = 'facebook';

    const WARMUP_MS = 3000;       // comments that render in this window after attaching are treated as history and skipped
    const DEDUPE_TTL_MS = 60000;  // same user + same text within this window counts as one comment (re-renders)
    const MAX_AGE_MS = 60000;     // drop events the server couldn't take in time; stale commands shouldn't fire late
    const FLUSH_MS = 200;
    const BATCH_SIZE = 50;
    const PARSE_RETRIES = 3;      // comment text can render after its container; retry before giving up
    const PARSE_RETRY_MS = 300;

    // Pages where a live chat can appear: /Page/videos/123, /watch/live/?v=..., /live/producer/...
    const LIVE_PATH = /\/(videos|watch|live)(\/|$)/i;

    const HIGHLIGHT = {
        sent: '2px solid #16a34a',
        skipped: '2px dashed #9ca3af',
    };

    const state = {
        enabled: false,
        highlight: false,
        capturing: false,
        url: location.href,
        warmUntil: 0,
        observer: null,
        handled: new WeakSet(),
        retries: new WeakMap(),
        seen: new Map(),          // dedupe key -> time first seen
        queue: [],                // { event, t }
        inFlight: false,
        retryAt: 0,
        failures: 0,
        highlighted: new Set(),
        stats: { captured: 0, skipped: 0, sent: 0, dropped: 0, lastError: null, lastSentAt: null },
    };

    // ---- capture -------------------------------------------------------

    function shouldCapture() {
        return state.enabled && LIVE_PATH.test(location.pathname);
    }

    function start() {
        if (state.capturing) return;
        state.capturing = true;
        state.warmUntil = Date.now() + WARMUP_MS;
        state.observer = new MutationObserver(onMutations);
        state.observer.observe(document.body, { childList: true, subtree: true });
        scan(document);
        log('capturing', location.href);
    }

    function stop() {
        if (!state.capturing) return;
        state.capturing = false;
        state.observer.disconnect();
        state.observer = null;
        log('stopped');
    }

    function sync() {
        if (shouldCapture()) start();
        else stop();
    }

    let pendingRoots = [];
    let scanScheduled = false;

    function onMutations(mutations) {
        for (const m of mutations) {
            for (const node of m.addedNodes) {
                if (node.nodeType === 1) pendingRoots.push(node);
            }
        }
        if (!scanScheduled && pendingRoots.length) {
            scanScheduled = true;
            setTimeout(() => {
                scanScheduled = false;
                const roots = pendingRoots;
                pendingRoots = [];
                for (const r of roots) if (r.isConnected) scan(r);
            }, 50);
        }
    }

    function scan(rootNode) {
        if (rootNode.nodeType === 1 && rootNode.matches(P.SELECTORS.comment)) consider(rootNode);
        rootNode.querySelectorAll(P.SELECTORS.comment).forEach(consider);
        // A comment's text may render after its container already exists; re-check its ancestor.
        const parent = rootNode.nodeType === 1 && rootNode.parentElement && rootNode.parentElement.closest(P.SELECTORS.comment);
        if (parent) consider(parent);
    }

    function consider(article) {
        if (!state.capturing || state.handled.has(article) || !P.isComment(article)) return;

        const parsed = P.parseComment(article);
        if (!parsed) {
            const tries = (state.retries.get(article) || 0) + 1;
            state.retries.set(article, tries);
            if (tries < PARSE_RETRIES) setTimeout(() => consider(article), PARSE_RETRY_MS);
            return;
        }
        state.handled.add(article);

        const now = Date.now();
        const key = parsed.commentId
            ? `c:${parsed.commentId}`
            : `h:${parsed.platformUserId}\u0000${parsed.message}`;
        pruneSeen(now);

        if (now < state.warmUntil || state.seen.has(key)) {
            state.seen.set(key, state.seen.get(key) || now);
            state.stats.skipped++;
            mark(article, 'skipped');
            return;
        }
        state.seen.set(key, now);

        const messageId = parsed.commentId
            ? `fb:${parsed.commentId}`
            // No Facebook id on the page: hash + minute bucket, so the server can still drop resends.
            : `fbh:${P.hash(`${parsed.platformUserId}\u0000${parsed.message}`)}:${Math.floor(now / 60000)}`;

        state.queue.push({
            t: now,
            event: {
                platform: PLATFORM,
                platformUserId: parsed.platformUserId,
                displayName: parsed.displayName,
                message: parsed.message,
                messageId,
                timestamp: new Date(now).toISOString(),
            },
        });
        state.stats.captured++;
        mark(article, 'sent');
        log('captured', parsed.displayName, `(${parsed.platformUserId}):`, parsed.message);
    }

    function pruneSeen(now) {
        if (state.seen.size < 500) return;
        for (const [k, t] of state.seen) if (now - t > DEDUPE_TTL_MS) state.seen.delete(k);
    }

    // ---- sending -------------------------------------------------------

    async function flush() {
        if (state.inFlight || !state.queue.length || Date.now() < state.retryAt) return;

        const now = Date.now();
        const before = state.queue.length;
        state.queue = state.queue.filter((q) => now - q.t <= MAX_AGE_MS);
        state.stats.dropped += before - state.queue.length;
        if (!state.queue.length) return;

        const batch = state.queue.slice(0, BATCH_SIZE);
        state.inFlight = true;
        try {
            const res = await browser.runtime.sendMessage({ type: 'fcc:send', events: batch.map((q) => q.event) });
            if (!res || !res.ok) throw new Error((res && res.error) || 'No response from extension background');
            state.queue.splice(0, batch.length);
            state.stats.sent += batch.length;
            state.stats.lastSentAt = new Date().toISOString();
            state.stats.lastError = null;
            state.failures = 0;
        } catch (err) {
            state.failures++;
            state.retryAt = Date.now() + Math.min(10000, 1000 * 2 ** (state.failures - 1));
            state.stats.lastError = err.message;
            log('send failed, will retry:', err.message);
        } finally {
            state.inFlight = false;
        }
    }

    // ---- debug highlight ------------------------------------------------

    function mark(article, kind) {
        if (!state.highlight) return;
        article.style.outline = HIGHLIGHT[kind];
        state.highlighted.add(article);
    }

    function clearHighlights() {
        for (const el of state.highlighted) el.style.outline = '';
        state.highlighted.clear();
    }

    function log(...args) {
        if (state.highlight) console.debug('[FCC]', ...args);
    }

    // ---- wiring --------------------------------------------------------

    browser.storage.local.get({ enabled: false, highlight: false }).then((s) => {
        state.enabled = s.enabled;
        state.highlight = s.highlight;
        sync();
    });

    browser.storage.onChanged.addListener((changes, area) => {
        if (area !== 'local') return;
        if (changes.enabled) state.enabled = changes.enabled.newValue;
        if (changes.highlight) {
            state.highlight = changes.highlight.newValue;
            if (!state.highlight) clearHighlights();
        }
        sync();
    });

    // Facebook navigates without reloading the page; treat a new URL as a new chat.
    setInterval(() => {
        if (location.href !== state.url) {
            state.url = location.href;
            stop();
            sync();
        }
        for (const el of state.highlighted) if (!el.isConnected) state.highlighted.delete(el);
    }, 1000);

    setInterval(flush, FLUSH_MS);

    // Status for the popup.
    browser.runtime.onMessage.addListener((msg) => {
        if (msg && msg.type === 'fcc:tabStatus') {
            return Promise.resolve({
                platform: PLATFORM,
                livePage: LIVE_PATH.test(location.pathname),
                capturing: state.capturing,
                queued: state.queue.length,
                ...state.stats,
            });
        }
        return undefined;
    });
})();
