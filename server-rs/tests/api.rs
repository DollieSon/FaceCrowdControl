//! End-to-end: a real server on a random port, talked to over HTTP (like the extension)
//! and WebSocket (like a game mod).

use std::time::Duration;

use fcc_server::{AppState, Settings, games, games::catalog::Catalog, routes, store};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CATALOG: &str = r#"
[[command]]
name = "heal"
game = "sts2"
action = "heal"
cost = 30
params = { amount = 5 }

[[command]]
name = "slow"
game = "sts2"
action = "heal"
cost = 1
cooldown_secs = 600

[[command]]
name = "missing"
game = "sts2"
action = "not_in_mod"
cost = 1
"#;

struct Server {
    base: String,
    ws: String,
    state: AppState,
    http: reqwest::Client,
}

async fn start(settings: Settings) -> Server {
    let store = store::connect("sqlite::memory:").await.unwrap();
    let state = AppState::new(store, Catalog::from_toml(CATALOG).unwrap(), settings);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = routes::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        base: format!("http://{addr}"),
        ws: format!("ws://{addr}/ws/game"),
        state,
        http: reqwest::Client::new(),
    }
}

impl Server {
    async fn chat(&self, events: Value) -> Value {
        let res = self
            .http
            .post(format!("{}/chat", self.base))
            .json(&json!({ "events": events }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        res.json().await.unwrap()
    }

    /// Sends one message as `user` and returns its result.
    async fn say(&self, user: &str, message: &str) -> Value {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = format!(
            "msg-{}",
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let body = self.chat(json!([event(user, message, &id)])).await;
        body["results"][0].clone()
    }

    async fn points_of(&self, user_id: i64) -> i64 {
        self.state
            .store
            .user(user_id)
            .await
            .unwrap()
            .unwrap()
            .points
    }

    async fn fund(&self, amount: i64) {
        self.state
            .store
            .award_active_users(store::now() - chrono::Duration::hours(1), amount)
            .await
            .unwrap();
    }

    /// Connects as the Slay the Spire 2 mod.
    async fn game(&self, actions: &[&str]) -> Ws {
        let (mut ws, _) = connect_async(self.ws.as_str()).await.unwrap();
        send(
            &mut ws,
            json!({ "type": "hello", "game": "sts2", "version": "0.1.0", "actions": actions }),
        )
        .await;
        assert_eq!(recv(&mut ws).await["type"], "welcome");
        ws
    }
}

fn event(user: &str, message: &str, message_id: &str) -> Value {
    json!({ "platform": "facebook", "platformUserId": user, "displayName": format!("Viewer {user}"),
            "message": message, "messageId": message_id, "timestamp": "2026-11-16T15:00:00.000Z" })
}

async fn send(ws: &mut Ws, msg: Value) {
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
}

/// Next JSON message from the server (skipping pings), failing after 5 s.
async fn recv(ws: &mut Ws) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match ws.next().await.expect("socket closed").unwrap() {
                Message::Text(t) => return serde_json::from_str(&t).unwrap(),
                Message::Close(_) => panic!("socket closed"),
                _ => continue,
            }
        }
    })
    .await
    .expect("no message from server")
}

/// Waits until `check` passes (the server handles game messages asynchronously).
async fn eventually<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition never became true");
}

// ---- extension contract --------------------------------------------------

#[tokio::test]
async fn health_reports_db() {
    let s = start(Settings::defaults()).await;
    let body: Value = s
        .http
        .get(format!("{}/health", s.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (
            body["ok"].clone(),
            body["db"].clone(),
            body["backend"].clone()
        ),
        (json!(true), json!(true), json!("sqlite"))
    );
}

#[tokio::test]
async fn chat_records_dedupes_and_validates() {
    let s = start(Settings::defaults()).await;
    let ev = event("100", "hello", "fb:1");
    let body = s
        .chat(json!([ev.clone(), ev, { "platform": "facebook" }]))
        .await;
    let r = &body["results"];
    assert_eq!(r[0]["status"], "ok");
    assert!(r[0]["userId"].is_i64());
    assert_eq!(r[1]["status"], "duplicate");
    assert_eq!(r[2]["status"], "invalid");
    assert_eq!(r[2]["error"], "platformUserId is required");

    // A single event without the { events } wrapper works too.
    let res = s
        .http
        .post(format!("{}/chat", s.base))
        .json(&event("100", "hi", "fb:2"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.json::<Value>().await.unwrap()["results"][0]["status"],
        "ok"
    );
}

#[tokio::test]
async fn chat_rejects_bad_requests_with_json_errors() {
    let s = start(Settings::defaults()).await;
    // Not JSON content type: what a cross-site form post would look like.
    let res = s
        .http
        .post(format!("{}/chat", s.base))
        .body("{}")
        .header("content-type", "text/plain")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 415);
    assert!(res.json::<Value>().await.unwrap()["error"].is_string());

    let too_many: Vec<Value> = (0..101).map(|i| event("1", "x", &i.to_string())).collect();
    let res = s
        .http
        .post(format!("{}/chat", s.base))
        .json(&too_many)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    let res = s
        .http
        .post(format!("{}/chat", s.base))
        .json(&json!([]))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

// ---- built-in commands -----------------------------------------------------

#[tokio::test]
async fn give_points_and_balance() {
    let s = start(Settings::defaults()).await;
    let a = s.say("a", "hi").await["userId"].as_i64().unwrap();
    let b = s.say("b", "hi").await["userId"].as_i64().unwrap();
    s.fund(100).await;

    let r = s.say("a", &format!("/give #{b} 40")).await;
    assert_eq!(r["command"]["ok"], true, "{r}");
    assert_eq!(r["command"]["result"]["balance"], 60);
    assert_eq!(s.points_of(b).await, 140);

    let r = s.say("a", "/points").await;
    assert_eq!(r["command"]["result"], json!({ "userId": a, "points": 60 }));

    for (msg, err) in [
        (
            format!("/give #{b} 61"),
            "Not enough points (you have 60)".to_string(),
        ),
        (
            format!("/give #{a} 1"),
            "You can't give points to yourself".to_string(),
        ),
        (
            "/give #9999 1".to_string(),
            "User #9999 doesn't exist".to_string(),
        ),
        (
            format!("/give #{b} -5"),
            "Amount must be a positive whole number".to_string(),
        ),
        (
            "/give bob 5".to_string(),
            "Target must be a user id number, e.g. /give #42 10".to_string(),
        ),
        ("/give".to_string(), "Usage: /give #id amount".to_string()),
        ("/dance".to_string(), "Unknown command: /dance".to_string()),
    ] {
        let r = s.say("a", &msg).await;
        assert_eq!(r["command"]["ok"], false, "{msg}");
        assert_eq!(r["command"]["error"], err, "{msg}");
    }
    assert_eq!(s.points_of(a).await, 60, "failed commands change nothing");
}

// ---- games -----------------------------------------------------------------

#[tokio::test]
async fn game_command_round_trip_with_refund() {
    let s = start(Settings::defaults()).await;
    let v = s.say("v", "hi").await["userId"].as_i64().unwrap();

    // Game not connected yet.
    let r = s.say("v", "/heal").await;
    assert_eq!(r["command"]["error"], "sts2 isn't connected");

    let mut game = s.game(&["heal"]).await;

    let r = s.say("v", "/heal").await;
    assert_eq!(
        r["command"]["error"],
        "not enough points (you have 0, /heal costs 30)"
    );

    s.fund(100).await;
    let r = s.say("v", "/heal").await;
    assert_eq!(r["command"]["ok"], true, "{r}");
    assert_eq!(r["command"]["result"]["status"], "delivered");
    assert_eq!(s.points_of(v).await, 70);

    let action = recv(&mut game).await;
    assert_eq!(action["type"], "action");
    assert_eq!(action["action"], "heal");
    assert_eq!(action["params"], json!({ "amount": 5 }));
    assert_eq!(action["user"], json!({ "id": v, "name": "Viewer v" }));

    // The game can't do it right now: refunded.
    send(&mut game, json!({ "type": "result", "id": action["id"], "status": "rejected", "message": "not in combat" })).await;
    eventually(|| async { s.points_of(v).await == 100 }).await;

    // This time it works: stays charged.
    s.say("v", "/heal").await;
    let action = recv(&mut game).await;
    send(
        &mut game,
        json!({ "type": "result", "id": action["id"], "status": "done" }),
    )
    .await;
    let id = action["id"].as_i64().unwrap();
    eventually(|| async {
        s.state.store.game_action(id).await.unwrap().unwrap().status == store::ActionStatus::Done
    })
    .await;
    assert_eq!(s.points_of(v).await, 70);

    // A duplicate result can't refund a finished action.
    send(
        &mut game,
        json!({ "type": "result", "id": id, "status": "failed" }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(s.points_of(v).await, 70);
}

#[tokio::test]
async fn cooldown_and_unsupported_actions() {
    let s = start(Settings::defaults()).await;
    let v = s.say("v", "hi").await["userId"].as_i64().unwrap();
    s.fund(100).await;
    let mut game = s.game(&["heal"]).await;

    assert_eq!(s.say("v", "/slow").await["command"]["ok"], true);
    recv(&mut game).await;
    let r = s.say("v", "/slow").await;
    assert!(
        r["command"]["error"]
            .as_str()
            .unwrap()
            .starts_with("on cooldown"),
        "{r}"
    );

    let r = s.say("v", "/missing").await;
    assert_eq!(
        r["command"]["error"],
        "sts2 doesn't support \"not_in_mod\" (update the mod?)"
    );
    assert_eq!(s.points_of(v).await, 99, "only /slow was charged");
}

#[tokio::test]
async fn unanswered_actions_are_redelivered_then_expire() {
    let s = start(Settings::defaults()).await;
    let v = s.say("v", "hi").await["userId"].as_i64().unwrap();
    s.fund(100).await;

    let mut game = s.game(&["heal"]).await;
    s.say("v", "/heal").await;
    let first = recv(&mut game).await;
    drop(game); // crashed before answering

    // On reconnect the same action comes again; the mod uses the id to avoid doing it twice.
    let mut game = s.game(&["heal"]).await;
    let again = recv(&mut game).await;
    assert_eq!(again["id"], first["id"]);
    assert_eq!(s.points_of(v).await, 70);

    // Nobody answers: the expiry timer refunds it.
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        games::expire_stale(&s.state, Duration::ZERO).await.unwrap(),
        1
    );
    assert_eq!(s.points_of(v).await, 100);
    let a = s
        .state
        .store
        .game_action(first["id"].as_i64().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.status, store::ActionStatus::Expired);
}

#[tokio::test]
async fn game_socket_guards() {
    // Browsers send an Origin header; they must not be able to pose as a game.
    let s = start(Settings::defaults()).await;
    let mut req = s.ws.as_str().into_client_request().unwrap();
    req.headers_mut()
        .insert("Origin", "https://evil.example".parse().unwrap());
    assert!(connect_async(req).await.is_err());

    // First message must be hello.
    let (mut ws, _) = connect_async(s.ws.as_str()).await.unwrap();
    send(
        &mut ws,
        json!({ "type": "result", "id": 1, "status": "done" }),
    )
    .await;
    assert_eq!(recv(&mut ws).await["type"], "error");

    // With GAME_TOKEN set, the token is required.
    let mut settings = Settings::defaults();
    settings.game_token = Some("s3cret".into());
    let s = start(settings).await;
    assert!(connect_async(s.ws.as_str()).await.is_err());
    let (mut ws, _) = connect_async(format!("{}?token=s3cret", s.ws))
        .await
        .unwrap();
    send(
        &mut ws,
        json!({ "type": "hello", "game": "sts2", "actions": [] }),
    )
    .await;
    assert_eq!(recv(&mut ws).await["type"], "welcome");
}

#[tokio::test]
async fn games_endpoint_lists_connections_and_commands() {
    let s = start(Settings::defaults()).await;
    let _game = s.game(&["heal"]).await;
    let body: Value = s
        .http
        .get(format!("{}/games", s.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["connected"][0]["game"], "sts2");
    assert_eq!(body["connected"][0]["actions"], json!(["heal"]));
    assert_eq!(body["commands"].as_array().unwrap().len(), 3);
}
