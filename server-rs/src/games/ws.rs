//! `GET /ws/game`: the WebSocket a game mod connects to.

use std::time::Duration;

use axum::{
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use serde::Deserialize;

use super::{
    catalog::is_simple_name,
    protocol::{GameMessage, PROTOCOL_VERSION, ServerMessage},
};
use crate::AppState;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
pub struct WsQuery {
    token: Option<String>,
}

pub async fn ws_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    // Browsers always send Origin on WebSocket requests; game mods don't. WebSockets
    // aren't covered by CORS, so without this any web page could connect to
    // localhost and pose as a game.
    if headers.contains_key(header::ORIGIN) {
        return (
            StatusCode::FORBIDDEN,
            "browser pages can't connect as a game",
        )
            .into_response();
    }
    if let Some(expected) = &state.settings.game_token
        && query.token.as_deref() != Some(expected.as_str())
    {
        return (StatusCode::UNAUTHORIZED, "wrong or missing ?token=").into_response();
    }
    ws.on_upgrade(move |socket| run_session(state, socket))
}

async fn run_session(state: AppState, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();

    // 1. The first message must be `hello`.
    let (game, version, actions) = match tokio::time::timeout(
        HELLO_TIMEOUT,
        next_message(&mut stream),
    )
    .await
    {
        Ok(Some(GameMessage::Hello {
            game,
            protocol,
            version,
            actions,
        })) if protocol == PROTOCOL_VERSION && is_simple_name(&game) => (game, version, actions),
        Ok(Some(GameMessage::Hello { protocol, .. })) if protocol != PROTOCOL_VERSION => {
            let _ = send(
                &mut sink,
                &ServerMessage::Error {
                    message: format!(
                        "unsupported protocol {protocol}; this server speaks {PROTOCOL_VERSION}"
                    ),
                },
            )
            .await;
            return;
        }
        _ => {
            let _ = send(&mut sink, &ServerMessage::Error {
                message: "first message must be {\"type\":\"hello\",\"game\":\"<lowercase id>\",\"actions\":[...]}".into(),
            })
            .await;
            return;
        }
    };

    // 2. Register. From here on everything to the game goes through the hub's channel,
    //    written by this task, so request handlers never touch the socket directly.
    let (conn_id, mut rx) = state
        .games
        .register(&game, version.clone(), actions.clone());
    tracing::info!(game, conn_id, ?version, ?actions, "game connected");
    state.games.send(
        &game,
        ServerMessage::Welcome {
            protocol: PROTOCOL_VERSION,
            server: format!("fcc-server {}", env!("CARGO_PKG_VERSION")),
        },
    );

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => if send(&mut sink, &msg).await.is_err() { break },
                    // The hub dropped our sender: a newer connection replaced this one.
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Default::default())).await.is_err() { break },
            }
        }
        let _ = sink.close().await;
    });

    // 3. Send whatever was queued while the game was away.
    if let Err(e) = super::deliver_open(&state, &game).await {
        tracing::error!(game, "couldn't load queued actions: {e}");
    }

    // 4. Read results until the game disconnects.
    loop {
        let msg = match stream.next().await {
            Some(Ok(Message::Text(text))) => text,
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
            Some(Ok(_)) => continue, // ping/pong/binary
        };
        match serde_json::from_str::<GameMessage>(&msg) {
            Ok(GameMessage::Result {
                id,
                status,
                message,
            }) => {
                if let Err(e) =
                    super::handle_result(&state, &game, id, status, message.as_deref()).await
                {
                    tracing::error!(game, action_id = id, "couldn't save result: {e}");
                }
            }
            Ok(GameMessage::Hello { .. }) => {
                state.games.send(
                    &game,
                    ServerMessage::Error {
                        message: "already said hello".into(),
                    },
                );
            }
            Err(e) => {
                tracing::warn!(game, "bad message from game: {e}");
                state.games.send(
                    &game,
                    ServerMessage::Error {
                        message: format!("couldn't read message: {e}"),
                    },
                );
            }
        }
        if writer.is_finished() {
            break; // replaced by a newer connection
        }
    }

    state.games.unregister(&game, conn_id);
    writer.abort();
    tracing::info!(game, conn_id, "game disconnected");
}

/// Next text message parsed as a [`GameMessage`], skipping pings. None on close/error/garbage.
async fn next_message(stream: &mut SplitStream<WebSocket>) -> Option<GameMessage> {
    loop {
        match stream.next().await? {
            Ok(Message::Text(text)) => return serde_json::from_str(&text).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

async fn send<S>(sink: &mut S, msg: &ServerMessage) -> Result<(), axum::Error>
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    let text = serde_json::to_string(msg).expect("ServerMessage always serializes");
    sink.send(Message::Text(text.into())).await
}
