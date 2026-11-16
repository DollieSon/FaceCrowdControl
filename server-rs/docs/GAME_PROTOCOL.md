# Game protocol (v1)

How a game mod talks to the FaceCrowdControl server. Any game that can open a
WebSocket can use it. Slay the Spire 2 is the first (see [STS2_MOD.md](STS2_MOD.md)).

## Connecting

The **game connects to the server**, the same direction as the browser extension:

```
ws://127.0.0.1:3000/ws/game            (add ?token=... if GAME_TOKEN is set)
```

- Don't send an `Origin` header. Browsers always send one, so the server refuses
  connections that have it. This stops any web page from posing as your game.
- Each message is one JSON text frame with a `"type"` field.
- If the connection drops, reconnect (wait 1 s, then up to 30 s between tries).
  Anything queued while the game was away is delivered after the new `hello`.
- Only one connection per game id. A new one replaces the old one.

## Messages

### game → server

**`hello`** must be the first message, sent within 10 s:

```json
{ "type": "hello", "game": "sts2", "protocol": 1, "version": "0.1.0",
  "actions": ["heal", "damage", "gain_gold", "gain_block", "random_potion", "add_curse"] }
```

- `game` is a lowercase id. It must match `game = "..."` in `games.toml`.
- `actions` lists the action names this build of the mod can perform. A chat command
  whose action isn't listed is refused before the viewer is charged.

**`result`** tells the server what happened to an action:

```json
{ "type": "result", "id": 17, "status": "done" }
{ "type": "result", "id": 18, "status": "rejected", "message": "not in combat" }
{ "type": "result", "id": 19, "status": "failed",   "message": "NullReferenceException ..." }
```

| status     | meaning                                           | viewer      |
|------------|---------------------------------------------------|-------------|
| `done`     | It happened.                                      | stays charged |
| `rejected` | Can't be done right now (menu, wrong room, ...).  | refunded    |
| `failed`   | Tried, but something broke.                       | refunded    |

### server → game

**`welcome`** is the reply to `hello`: `{ "type": "welcome", "protocol": 1, "server": "fcc-server 2.0.0" }`

**`action`** asks the game to do something:

```json
{ "type": "action", "id": 17, "action": "heal", "params": { "amount": 5 },
  "cost": 30, "user": { "id": 42, "name": "Maria Santos" } }
```

`params` come from `games.toml`. Viewers can't change them. `user` is there so the
game can show who did it.

**`error`** means the last message wasn't understood: `{ "type": "error", "message": "..." }`

## Delivery rules

- **At least once.** An action can arrive more than once, for example after a
  reconnect when the game never answered. Remember the ids you've handled (a set is
  enough) and ignore repeats. Don't send a second `result` for them.
- **Answer every action**, ideally within a second or two. After `ACTION_TIMEOUT_SECS`
  (default 60) without a `result`, the server marks it `expired` and refunds the viewer.
  A late result for an expired action is ignored.
- If the game can't do something right now, answer `rejected` straight away rather than
  holding the action. Viewers get their points back immediately.
- The server sends WebSocket pings every 20 s. Most WebSocket clients answer them
  automatically.

## Action lifecycle (server side)

```
/heal in chat ──► checks: game connected? action supported? off cooldown? enough points?
                       │ no → error in the /chat response, nothing charged
                       ▼ yes
                charge viewer, save as pending ──► send ──► delivered
                                                              │
                         result: done ◄───────────────────────┤
                         result: rejected/failed → refund ◄───┤
                         no answer in time → expired, refund ◄┘
```

## Trying it without the game

```sh
cargo run --example fake_game     # pretends to be the StS2 mod
```
