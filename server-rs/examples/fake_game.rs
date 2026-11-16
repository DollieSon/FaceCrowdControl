//! A stand-in for the Slay the Spire 2 mod: connects to the server, prints every action it
//! receives, and answers `done` (or `rejected` for every third one, to show refunds).
//!
//!   cargo run --example fake_game                       # ws://127.0.0.1:3000/ws/game
//!   cargo run --example fake_game -- ws://host:port/ws/game?token=...
//!
//! Then send chat through the extension (or curl, see README) like `/heal`.

use std::collections::HashSet;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ws://127.0.0.1:3000/ws/game".into());
    let (mut ws, _) = connect_async(url.as_str()).await?;
    println!("connected to {url}");

    let hello = json!({
        "type": "hello",
        "game": "sts2",
        "version": "fake-0.1.0",
        "actions": ["heal", "damage", "gain_gold", "gain_block", "random_potion", "add_curse"],
    });
    ws.send(Message::Text(hello.to_string().into())).await?;

    // Actions can be redelivered after a reconnect: remember what was already handled.
    let mut handled = HashSet::new();
    let mut count = 0u32;
    while let Some(msg) = ws.next().await {
        let Message::Text(text) = msg? else { continue };
        let msg: Value = serde_json::from_str(&text)?;
        match msg["type"].as_str() {
            Some("welcome") => println!("server says welcome: {msg}"),
            Some("action") => {
                let id = msg["id"].as_i64().unwrap_or_default();
                if !handled.insert(id) {
                    println!("action #{id} again; already handled, ignoring");
                    continue;
                }
                count += 1;
                println!(
                    "action #{id}: {} {} (from {}, cost {})",
                    msg["action"], msg["params"], msg["user"]["name"], msg["cost"]
                );
                let reply = if count.is_multiple_of(3) {
                    json!({ "type": "result", "id": id, "status": "rejected", "message": "fake game says no" })
                } else {
                    json!({ "type": "result", "id": id, "status": "done" })
                };
                ws.send(Message::Text(reply.to_string().into())).await?;
            }
            _ => println!("server: {msg}"),
        }
    }
    println!("disconnected");
    Ok(())
}
