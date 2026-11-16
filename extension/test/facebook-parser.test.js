'use strict';

const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { JSDOM } = require('jsdom');

const P = require('../src/sites/facebook-parser.js');

const html = readFileSync(join(__dirname, 'fixtures', 'facebook-live-chat.synthetic.html'), 'utf8');
const { document } = new JSDOM(html, { url: 'https://www.facebook.com/juanspage/videos/987/' }).window;
const byId = (id) => document.getElementById(id);

test('parseProfileId handles the link shapes Facebook uses', () => {
    assert.equal(P.parseProfileId('/profile.php?id=100012345678&comment_id=abc'), '100012345678');
    assert.equal(P.parseProfileId('https://www.facebook.com/Juan.DC?__cft__[0]=x'), 'juan.dc');
    assert.equal(P.parseProfileId('https://web.facebook.com/juan.dc'), 'juan.dc');
    assert.equal(P.parseProfileId('/people/Ana-Reyes/100099/'), '100099');
    assert.equal(P.parseProfileId('/groups/123/user/100055/'), '100055');
});

test('parseProfileId rejects non-profile links', () => {
    assert.equal(P.parseProfileId('/juanspage/videos/987/?comment_id=x'), null);
    assert.equal(P.parseProfileId('/watch/live/?v=1'), null);
    assert.equal(P.parseProfileId('/hashtag/isaac'), null);
    assert.equal(P.parseProfileId('/profile.php'), null);
    assert.equal(P.parseProfileId('https://evil.example.com/juan.dc'), null);
    assert.equal(P.parseProfileId('https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.com'), null);
    assert.equal(P.parseProfileId(''), null);
});

test('the video post itself is not treated as a comment', () => {
    assert.equal(P.isComment(byId('video-post')), false);
    assert.equal(P.parseComment(byId('video-post')), null);
});

test('comment with numeric id, Facebook comment id and action links', () => {
    assert.deepEqual(P.parseComment(byId('c1')), {
        platformUserId: '100012345678',
        displayName: 'Maria Santos',
        message: '/give #42 50',
        commentId: 'Y29tbWVudDoxMjM=',
    });
});

test('comment with username, emoji and line break, no comment id', () => {
    assert.deepEqual(P.parseComment(byId('c2')), {
        platformUserId: 'juan.dc',
        displayName: 'Juan Dela Cruz',
        message: 'gg 😂 nice run\nsecond line',
        commentId: null,
    });
});

test('mentions stay in the message and nested replies stay out', () => {
    const c3 = P.parseComment(byId('c3'));
    assert.equal(c3.platformUserId, '100099');
    assert.equal(c3.displayName, 'Ana Reyes');
    assert.equal(c3.message, 'hi Maria Santos thanks!');
    assert.equal(c3.commentId, null);
});

test('replies parse on their own, preferring reply_comment_id', () => {
    assert.deepEqual(P.parseComment(byId('r1')), {
        platformUserId: '100012345678',
        displayName: 'Maria Santos',
        message: 'welcome',
        commentId: 'cmVwbHk6OTk=',
    });
});

test('comments without text (stickers) are skipped', () => {
    assert.equal(P.parseComment(byId('c4')), null);
});

test('hash is stable and short', () => {
    assert.equal(P.hash('abc'), P.hash('abc'));
    assert.notEqual(P.hash('abc'), P.hash('abd'));
    assert.match(P.hash('anything'), /^[0-9a-f]{8}$/);
});
