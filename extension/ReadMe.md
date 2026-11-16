Firefox Extension for reading comments in Facebook,Twitch And Youtube

Currently supports **Facebook** live chat. It reads each new comment's author ID,
display name and text, and sends them to the local server's `POST /chat`.

## Setup

```sh
cd extension
npm install
npm run config   # writes src/config.json (server URL) from ../server/.env
npm start        # opens a Firefox with the extension loaded, reloads on save
```

`npm run config` reads `PORT` / `BIND_ADDRESS` (or `EXTENSION_SERVER_URL`) from
`server/.env`, the same file the server uses. Re-run it and reload the extension
after changing `.env`. Without a `.env` it uses `http://127.0.0.1:3000`.

To load it into your normal Firefox instead: `about:debugging` → *This Firefox* →
*Load Temporary Add-on…* → pick `src/manifest.json`. Temporary add-ons are removed
when Firefox restarts; for a permanent install, sign it (below).

## Using it

1. Start the server (`docker compose up` or `npm start` in `server/`).
2. Open your Facebook live video page.
3. Click the extension icon → turn **Capture** on.

The popup shows whether the server and database are reachable, whether this tab
is a live page being captured, and counts of captured / sent / skipped comments.
The toolbar badge shows `ON`, the number sent, or `ERR`.

- Comments already on screen when capture starts (first 3 s) are skipped, so old
  commands aren't replayed.
- Capture only runs on `/videos/`, `/watch/` and `/live/` pages.
- If the server is down, comments are retried for up to 60 s, then dropped.

## Checking the parser works

Facebook changes its markup often, and the parser was written without a real
sample of the live chat HTML. To verify:

1. Turn on **Highlight captured comments** in the popup.
2. New comments get a **green** outline when sent; comments on screen at start
   get a **grey dashed** one. Details are logged to the console with `[FCC]`.
3. If comments get no outline, right-click one → *Inspect*, right-click its outer
   element in the inspector → *Copy → Outer HTML*, and add it as a test fixture.
   Selectors live at the top of `src/sites/facebook-parser.js` (`SELECTORS`).
   The aria-label match (`Comment by …`) is English; other Facebook languages
   need their wording added.

```sh
npm test     # parser + config tests (jsdom)
npm run lint # Mozilla's extension linter
```

## Permanent install (signing)

Regular Firefox only keeps signed extensions. Signing as *unlisted* is free and
doesn't publish it anywhere:

1. Get API keys at <https://addons.mozilla.org/developers/addon/api/key/>.
2. `npx web-ext sign --source-dir src --channel=unlisted --api-key=... --api-secret=...`
3. Install the `.xpi` from `web-ext-artifacts/`.

## Layout

```
src/manifest.json             Firefox MV3 manifest
src/background.js             posts to the server, badge, stats
src/sites/facebook-parser.js  DOM → {platformUserId, displayName, message, commentId}
src/sites/facebook.js         watches the page, skips history, dedupes, queues + retries
src/popup/                    on/off, status, highlight toggle
scripts/sync-env.mjs          .env → src/config.json
test/                         node:test + jsdom
```

Event sent to `POST /chat` (as `{ "events": [...] }`):

```json
{ "platform": "facebook", "platformUserId": "100012345678", "displayName": "Maria Santos",
  "message": "/give #42 50", "messageId": "fb:Y29tbWVudDoxMjM=", "timestamp": "2026-11-16T15:00:00.000Z" }
```
