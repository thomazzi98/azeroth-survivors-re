//! `survivors-server`: authoritative room for online Survivors (PoC: one room, fixed capacity).
//!
//! `GET /ws` upgrades to a WebSocket speaking `survivors-net`. The first message must be `Join`.
//! A fixed-rate task steps the [`sim::World`] and broadcasts one encoded snapshot per tick to
//! every connection. Rooms, codes and lobbies are the next phase.

pub mod sim;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use survivors_net::{decode, encode, ClientMsg, ServerMsg, PROTOCOL_VERSION, TICK_HZ};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Server {
    world: Arc<Mutex<sim::World>>,
    snapshots: broadcast::Sender<Arc<Vec<u8>>>,
    max_players: usize,
}

impl Server {
    /// Starts the tick task on the current tokio runtime.
    pub fn start(max_players: usize, seed: u64) -> Self {
        let (snapshots, _) = broadcast::channel(8);
        let server = Self { world: Arc::new(Mutex::new(sim::World::new(seed))), snapshots, max_players };
        let tick = server.clone();
        tokio::spawn(async move {
            let dt = 1.0 / TICK_HZ as f32;
            let mut interval = tokio::time::interval(Duration::from_secs_f32(dt));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let bytes = {
                    let mut w = tick.world.lock().unwrap();
                    if w.players.is_empty() {
                        continue; // an empty room does not simulate
                    }
                    w.step(dt);
                    encode(&ServerMsg::Snapshot(w.snapshot()))
                };
                let _ = tick.snapshots.send(Arc::new(bytes));
            }
        });
        server
    }

    pub fn router(self) -> Router {
        Router::new().route("/ws", get(upgrade)).with_state(self)
    }

    fn join(&self, name: String) -> Result<u8, String> {
        let mut w = self.world.lock().unwrap();
        if w.players.len() >= self.max_players {
            return Err("room full".into());
        }
        let id = (0..=u8::MAX).find(|i| w.players.iter().all(|p| p.id != *i)).ok_or("room full")?;
        let name: String = name.chars().take(24).collect();
        w.add_player(id, name);
        Ok(id)
    }
}

async fn upgrade(State(s): State<Server>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(4096).on_upgrade(move |socket| session(s, socket))
}

async fn session(s: Server, socket: WebSocket) {
    let (mut tx, mut rx) = socket.split();
    // The first frame must be a valid Join.
    let name = match rx.next().await {
        Some(Ok(Message::Binary(b))) => match decode::<ClientMsg>(&b) {
            Ok(ClientMsg::Join { version, name }) if version == PROTOCOL_VERSION => name,
            _ => return reject(&mut tx, "bad join or protocol version").await,
        },
        _ => return,
    };
    let id = match s.join(name) {
        Ok(id) => id,
        Err(reason) => return reject(&mut tx, &reason).await,
    };
    let mut snaps = s.snapshots.subscribe();
    let welcome = encode(&ServerMsg::Welcome { you: id, tick_hz: TICK_HZ });
    if tx.send(Message::binary(welcome)).await.is_ok() {
        loop {
            tokio::select! {
                snap = snaps.recv() => match snap {
                    Ok(b) => { if tx.send(Message::binary((*b).clone())).await.is_err() { break; } }
                    Err(broadcast::error::RecvError::Lagged(_)) => {} // slow client: skip ahead
                    Err(_) => break,
                },
                msg = rx.next() => match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if let Ok(ClientMsg::Input { seq, dir, .. }) = decode::<ClientMsg>(&b) {
                            s.world.lock().unwrap().set_input(id, seq, dir);
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                },
            }
        }
    }
    s.world.lock().unwrap().remove_player(id);
}

async fn reject<S: SinkExt<Message> + Unpin>(tx: &mut S, reason: &str) {
    let _ = tx.send(Message::binary(encode(&ServerMsg::Rejected { reason: reason.into() }))).await;
}
