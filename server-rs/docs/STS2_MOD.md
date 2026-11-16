# Slay the Spire 2 mod: plan

**Status: design only. The mod isn't written yet.** The server side is done and tested
with `examples/fake_game.rs`, which speaks the same protocol the mod will.

## Can a StS2 mod talk to our server? Yes.

- StS2 is built on **Godot 4 with C# (.NET 9)**. Mods are ordinary .NET class libraries
  (`<id>.dll`, plus an optional Godot `<id>.pck` for assets), so they can use the full
  .NET library, including `System.Net.WebSockets.ClientWebSocket`.
- The game has a **built-in mod loader**. It scans `<StS2 install>/mods/` recursively for
  `<ModId>.json` manifests and calls the method named by `[ModInitializer(...)]`.
- Mods already do this kind of thing. For example, `sts2-ai-mod` runs an HTTP server
  inside the game on `localhost:57541` so an outside AI can play.
- Mods can change game code with **Harmony** patches. **BaseLib** (Alchyr) is the
  common shared library, and `Alchyr/ModTemplate-StS2` is the starter project.

Caveat: StS2 is in Early Access and its modding API still changes between patches.
Pin `min_game_version` in the manifest and expect to adjust the mod after game updates.

## Why the mod connects to us (and not the other way round)

`sts2-ai-mod` has the game host a server. We do the opposite, and the mod is a
**WebSocket client**:

- The FaceCrowdControl server is the hub. The extension already connects in, and games
  do the same. There's no extra port open inside the game and nothing to configure in
  the server.
- If the game restarts, the mod reconnects and the server re-sends queued actions.
  Points stay safe in the database the whole time.
- The same protocol works for the next game ([GAME_PROTOCOL.md](GAME_PROTOCOL.md)).

## Mod layout

```
<StS2>/mods/FaceCrowdControl/
  FaceCrowdControl.json    manifest
  FaceCrowdControl.dll     the code below
```

`FaceCrowdControl.json`:

```json
{
  "id": "FaceCrowdControl",
  "name": "FaceCrowdControl",
  "author": "DollieSon",
  "description": "Lets live-stream chat trigger events in your run.",
  "version": "0.1.0",
  "has_dll": true,
  "has_pck": false,
  "dependencies": [],
  "min_game_version": "<the version you build against>",
  "affects_gameplay": true
}
```

Project: a `net9.0` class library that references `sts2.dll` and `GodotSharp.dll` from
the game's install folder (`Copy Local = false`). The community tutorials
(`fresh-milkshake/Modding-Tutorial`, `jiegec/STS2FirstMod`) have working `.csproj` files.

## Code sketch

Untested. The networking and threading parts are plain .NET and Godot. **The game API
calls are placeholders**: find the real ones by opening `sts2.dll` in ILSpy (look at
`PlayerCmd`, `CreatureCmd`, `CardPileCmd`, `RelicCmd`) or from BaseLib.

The one rule that matters: **only touch game state on Godot's main thread.** The
socket runs on a background task and puts actions in a queue. The queue is emptied on
the main thread every frame through `SceneTree.ProcessFrame`. That's a plain C# event,
so the mod doesn't need to register a Godot `Node` subclass of its own.

```csharp
using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.IO;
using System.Net.WebSockets;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Threading;
using System.Threading.Tasks;
using Godot;
using MegaCrit.Sts2.Core.Modding;

[ModInitializer(nameof(Initialize))]
public static class FaceCrowdControl
{
    const string ServerUrl = "ws://127.0.0.1:3000/ws/game";
    static readonly string[] Supported = { "heal", "damage", "gain_gold", "gain_block", "random_potion", "add_curse" };

    static readonly ConcurrentQueue<JsonObject> Inbox = new();
    static readonly HashSet<long> Handled = new();          // main thread only
    static readonly SemaphoreSlim SendLock = new(1, 1);
    static ClientWebSocket? _ws;

    public static void Initialize()
    {
        var tree = (SceneTree)Engine.GetMainLoop();
        tree.ProcessFrame += DrainInbox;                    // runs on the main thread each frame
        _ = Task.Run(ConnectForever);                       // networking stays off the main thread
    }

    // ---- main thread ---------------------------------------------------------

    static void DrainInbox()
    {
        while (Inbox.TryDequeue(out var msg))
        {
            long id = msg["id"]!.GetValue<long>();
            if (!Handled.Add(id)) continue;                 // redelivered: already handled
            string status; string? message = null;
            try { (status, message) = Apply(msg["action"]!.GetValue<string>(), msg["params"]!.AsObject(), msg["user"]!["name"]!.GetValue<string>()); }
            catch (Exception e) { status = "failed"; message = e.Message; }
            _ = Send(new { type = "result", id, status, message });
        }
    }

    /// Returns ("done" | "rejected" | "failed", message).
    static (string, string?) Apply(string action, JsonObject p, string viewer)
    {
        // TODO(placeholders): replace with the real StS2 API.
        // if (!RunManager.IsInRun) return ("rejected", "not in a run");
        switch (action)
        {
            case "heal":
                // CreatureCmd.Heal(player, p["amount"]!.GetValue<int>());
                return ("done", null);
            case "gain_block":
                // if (!InCombat) return ("rejected", "not in combat");
                return ("done", null);
            default:
                return ("rejected", $"unknown action {action}");
        }
        // Also show "<viewer> healed you!" on screen so the stream sees who did it.
    }

    // ---- background ----------------------------------------------------------

    static async Task ConnectForever()
    {
        var wait = TimeSpan.FromSeconds(1);
        while (true)
        {
            try
            {
                using var ws = new ClientWebSocket();
                await ws.ConnectAsync(new Uri(ServerUrl), CancellationToken.None);
                _ws = ws;
                wait = TimeSpan.FromSeconds(1);
                await Send(new { type = "hello", game = "sts2", protocol = 1, version = "0.1.0", actions = Supported });
                await ReadLoop(ws);
            }
            catch (Exception e) { GD.PrintErr($"[FCC] {e.Message}"); }
            _ws = null;
            await Task.Delay(wait);
            wait = TimeSpan.FromSeconds(Math.Min(wait.TotalSeconds * 2, 30));
        }
    }

    static async Task ReadLoop(ClientWebSocket ws)
    {
        var buffer = new byte[16 * 1024];
        while (ws.State == WebSocketState.Open)
        {
            using var text = new MemoryStream();
            WebSocketReceiveResult r;
            do
            {
                r = await ws.ReceiveAsync(buffer, CancellationToken.None);
                if (r.MessageType == WebSocketMessageType.Close) return;
                text.Write(buffer, 0, r.Count);
            } while (!r.EndOfMessage);

            var msg = JsonNode.Parse(text.ToArray())?.AsObject();
            switch (msg?["type"]?.GetValue<string>())
            {
                case "action": Inbox.Enqueue(msg); break;
                case "error": GD.PrintErr($"[FCC] server: {msg["message"]}"); break;
            }
        }
    }

    static async Task Send(object msg)
    {
        var ws = _ws;
        if (ws is not { State: WebSocketState.Open }) return;   // server re-sends unanswered actions
        var bytes = JsonSerializer.SerializeToUtf8Bytes(msg);
        await SendLock.WaitAsync();
        try { await ws.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None); }
        finally { SendLock.Release(); }
    }
}
```

## Things to work out once we open sts2.dll

1. **The real command APIs** for heal, damage, gold, block, potion and adding a curse,
   and whether they must go through the game's action queue / async sequence. (The
   tutorials warn against blocking on tasks with `.Wait()` / `.Result` from gameplay code.)
2. **Game state checks**: in a run, in combat, whose turn it is. `gain_block` only makes
   sense in combat. Answer `rejected` otherwise.
3. **Co-op multiplayer**: StS2 has co-op. Decide whether actions affect only the
   streamer's character and whether they stay in sync for other players (or simply
   reject actions in multiplayer runs at first).
4. **On-screen feedback**: a small toast or label saying "Maria healed you (+5)". This
   matters most for viewers.
5. **Settings**: read the server URL and token from a small config file next to the
   DLL instead of a constant.

## Adding an action later

1. Implement it in the mod's `Apply` switch and add its name to `Supported`.
2. Add a `[[command]]` for it to `server-rs/games.toml`, then restart the server.
