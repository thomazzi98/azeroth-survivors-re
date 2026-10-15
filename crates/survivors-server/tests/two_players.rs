//! End to end: two real WebSocket clients against a real server on an ephemeral port.

use futures_util::{SinkExt, StreamExt};
use survivors_net::*;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn start() -> String {
    let app = survivors_server::Server::start(2, 5).router();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

async fn send(ws: &mut Ws, m: &ClientMsg) {
    ws.send(Message::binary(encode(m))).await.unwrap();
}

async fn next(ws: &mut Ws) -> ServerMsg {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap() {
            Message::Binary(b) => return decode(&b).unwrap(),
            _ => continue,
        }
    }
}

async fn join(url: &str, name: &str) -> (Ws, PlayerId) {
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    send(&mut ws, &ClientMsg::Join { version: PROTOCOL_VERSION, name: name.into() }).await;
    match next(&mut ws).await {
        ServerMsg::Welcome { you, .. } => (ws, you),
        m => panic!("expected welcome, got {m:?}"),
    }
}

async fn snapshot(ws: &mut Ws) -> Snapshot {
    loop {
        if let ServerMsg::Snapshot(s) = next(ws).await {
            return s;
        }
    }
}

#[tokio::test]
async fn two_clients_see_each_other_move_and_a_third_is_refused() {
    let url = start().await;
    let (mut a, ida) = join(&url, "alice").await;
    let (mut b, idb) = join(&url, "bob").await;
    assert_ne!(ida, idb);

    // Alice walks +x; Bob must see her move and see his own name too.
    send(&mut a, &ClientMsg::Input { seq: 1, dir: [1.0, 0.0], aim: [0.0, 0.0] }).await;
    let mut moved = false;
    for _ in 0..60 {
        let s = snapshot(&mut b).await;
        assert_eq!(s.players.len(), 2);
        let alice = s.players.iter().find(|p| p.id == ida).unwrap();
        if alice.pos[0] > 2.0 + 1.0 && alice.ack_seq == 1 {
            moved = true;
            break;
        }
    }
    assert!(moved, "bob never saw alice move");

    // Third player: room is full.
    let (mut c, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    send(&mut c, &ClientMsg::Join { version: PROTOCOL_VERSION, name: "carol".into() }).await;
    assert!(matches!(next(&mut c).await, ServerMsg::Rejected { .. }));

    // Alice leaves: Bob keeps playing alone.
    drop(a);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let s = snapshot(&mut b).await;
    assert_eq!(s.players.len(), 1);
}

#[tokio::test]
async fn wrong_protocol_version_is_rejected() {
    let url = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    send(&mut ws, &ClientMsg::Join { version: 999, name: "x".into() }).await;
    assert!(matches!(next(&mut ws).await, ServerMsg::Rejected { .. }));
}
