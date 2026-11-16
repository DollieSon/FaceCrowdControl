/*
 * Facebook live-chat comment parser.
 *
 * Pure DOM -> data functions, no browser-extension APIs, so it runs both as a
 * content script and under Node tests (jsdom).
 *
 * Facebook's CSS class names are minified and change often, so everything here
 * keys off accessibility attributes and link shapes instead. If Facebook changes
 * its markup, SELECTORS is the place to fix it.
 */
(function (root) {
    'use strict';

    const SELECTORS = {
        // Each comment (and reply) is rendered as an ARIA article.
        comment: '[role="article"]',
        // ...whose aria-label reads "Comment by <name> ..." / "Reply by <name> ...".
        // This is what tells a chat comment apart from the video post itself.
        // Locale dependent: add your Facebook language's wording here if it isn't English.
        commentLabel: /^(comment|reply) by /i,
        // Comment text lives in div[dir="auto"] blocks; the author name is a <span>.
        body: 'div[dir="auto"]',
    };

    // First path segments that are Facebook features, not usernames.
    const RESERVED_PATHS = new Set([
        'ads', 'bookmarks', 'business', 'events', 'friends', 'gaming', 'groups', 'hashtag',
        'help', 'home.php', 'l.php', 'live', 'login', 'marketplace', 'messages', 'notifications',
        'pages', 'permalink.php', 'photo', 'photo.php', 'photos', 'policies', 'privacy', 'reel',
        'reels', 'search', 'settings', 'share', 'sharer', 'sharer.php', 'stories', 'story.php',
        'ufi', 'video', 'videos', 'watch',
    ]);

    function toUrl(href) {
        if (!href) return null;
        try {
            return new URL(href, 'https://www.facebook.com/');
        } catch {
            return null;
        }
    }

    function isFacebookHost(url) {
        return /(^|\.)facebook\.com$/i.test(url.hostname);
    }

    /**
     * Stable account identifier from a profile link, or null if the link isn't a profile.
     *   /profile.php?id=1000123      -> "1000123"
     *   /juan.dc?comment_id=...      -> "juan.dc"
     *   /people/Juan/1000123/        -> "1000123"
     *   /groups/x/user/1000123/      -> "1000123"
     */
    function parseProfileId(href) {
        const url = toUrl(href);
        if (!url || !isFacebookHost(url)) return null;
        const segs = url.pathname.split('/').filter(Boolean);
        if (segs.length === 0) return null;

        if (segs[0] === 'profile.php') {
            const id = url.searchParams.get('id');
            return id && /^\d+$/.test(id) ? id : null;
        }
        if (segs[0] === 'people' && /^\d+$/.test(segs[2] || '')) return segs[2];
        if (segs[0] === 'groups' && segs[2] === 'user' && /^\d+$/.test(segs[3] || '')) return segs[3];
        if (segs.length === 1 && !RESERVED_PATHS.has(segs[0].toLowerCase()) && /^[A-Za-z0-9.-]+$/.test(segs[0])) {
            return segs[0].toLowerCase();
        }
        return null;
    }

    /** Facebook's own comment id, carried on the timestamp / reply links. */
    function parseCommentId(href) {
        const url = toUrl(href);
        if (!url || !isFacebookHost(url)) return null;
        return url.searchParams.get('reply_comment_id') || url.searchParams.get('comment_id') || null;
    }

    /** Visible text of an element, keeping emoji (rendered as <img alt="😀">). */
    function textOf(el) {
        let out = '';
        const walk = (node) => {
            if (node.nodeType === 3) {
                out += node.nodeValue;
            } else if (node.nodeType === 1) {
                const tag = node.tagName;
                if (tag === 'IMG') out += node.getAttribute('alt') || '';
                else if (tag === 'BR') out += '\n';
                else if (tag !== 'SCRIPT' && tag !== 'STYLE') node.childNodes.forEach(walk);
            }
        };
        walk(el);
        return out;
    }

    function clean(text) {
        return text.replace(/[ \t ]+/g, ' ').replace(/\s*\n\s*/g, '\n').trim();
    }

    function isComment(el) {
        if (!el || typeof el.matches !== 'function' || !el.matches(SELECTORS.comment)) return false;
        return SELECTORS.commentLabel.test(el.getAttribute('aria-label') || '');
    }

    /**
     * Parse one comment element.
     * Returns { platformUserId, displayName, message, commentId } or null when the
     * element isn't a usable comment yet (e.g. text not rendered, sticker-only).
     */
    function parseComment(article) {
        if (!isComment(article)) return null;
        // Only look at this comment's own nodes, not nested replies.
        const own = (el) => el.closest(SELECTORS.comment) === article;

        let platformUserId = null;
        let displayName = '';
        let authorLink = null;
        let commentId = null;

        for (const a of article.querySelectorAll('a[href]')) {
            if (!own(a)) continue;
            const href = a.getAttribute('href');
            if (!commentId) commentId = parseCommentId(href);
            const id = parseProfileId(href);
            if (!id) continue;
            // The avatar link comes first and has no text; the name link follows with the same id.
            if (!platformUserId) platformUserId = id;
            if (id === platformUserId && !displayName) {
                const name = clean(textOf(a));
                if (name) {
                    displayName = name;
                    authorLink = a;
                }
            }
        }
        if (!platformUserId) return null;

        const candidates = Array.from(article.querySelectorAll(SELECTORS.body)).filter((el) =>
            own(el) &&
            !el.closest('a') &&
            !el.closest('[role="button"]') &&
            !(authorLink && el.contains(authorLink)),
        );
        const outermost = candidates.filter((el) => !candidates.some((o) => o !== el && o.contains(el)));
        const message = clean(outermost.map(textOf).join('\n'));
        if (!message) return null;

        if (!displayName) {
            // Fall back to the aria-label: "Comment by Juan Dela Cruz 2 minutes ago".
            displayName = clean((article.getAttribute('aria-label') || '').replace(SELECTORS.commentLabel, ''));
        }

        return { platformUserId, displayName: displayName.slice(0, 100), message, commentId };
    }

    /** Small non-cryptographic hash (FNV-1a, 32-bit) for fallback message ids. */
    function hash(str) {
        let h = 0x811c9dc5;
        for (let i = 0; i < str.length; i++) {
            h ^= str.charCodeAt(i);
            h = Math.imul(h, 0x01000193);
        }
        return (h >>> 0).toString(16).padStart(8, '0');
    }

    const api = { SELECTORS, parseProfileId, parseCommentId, parseComment, isComment, textOf, hash };
    root.FCCFacebookParser = api;
    if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(globalThis);
