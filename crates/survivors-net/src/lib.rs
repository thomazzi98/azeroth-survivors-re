//! Wire protocol between the online Survivors client and `survivors-server`.
//!
//! One WebSocket binary frame = one postcard-encoded message. Positions are in yards on the
//! ground plane (x, z), the same axes the client's `survivors` mode uses after `wow_to_bevy`.
//! PoC scope: join, input, snapshots. Cards, rooms and bosses come in later phases
//! (docs/MULTIPLAYER_PLAN.md).

pub mod client;

use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change; the server refuses a mismatched `Join`.
pub const PROTOCOL_VERSION: u16 = 1;
/// Server simulation and snapshot rate.
pub const TICK_HZ: u32 = 20;

pub type PlayerId = u8;
pub type EnemyId = u32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    Join { version: u16, name: String },
    /// Latest input. `seq` is echoed back as `ack_seq` so the client can reconcile prediction.
    Input { seq: u32, dir: [f32; 2], aim: [f32; 2] },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome { you: PlayerId, tick_hz: u32 },
    Rejected { reason: String },
    Snapshot(Snapshot),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: u32,
    pub players: Vec<PlayerState>,
    pub enemies: Vec<EnemyState>,
    /// One-shot cosmetic events since the previous snapshot (the client plays VFX from these).
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: PlayerId,
    pub name: String,
    pub pos: [f32; 2],
    pub hp: f32,
    pub max_hp: f32,
    pub ack_seq: u32,
    pub dead: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnemyState {
    pub id: EnemyId,
    pub pos: [f32; 2],
    pub hp: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// `caster` hit `enemy` for `damage` (a bolt from caster to enemy on the client).
    Bolt { caster: PlayerId, enemy: EnemyId, damage: f32 },
    EnemyDied { enemy: EnemyId },
    PlayerHurt { player: PlayerId, damage: f32 },
}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    postcard::to_stdvec(msg).expect("protocol messages always serialise")
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let m = ClientMsg::Input { seq: 7, dir: [1.0, 0.0], aim: [0.0, 1.0] };
        assert_eq!(decode::<ClientMsg>(&encode(&m)).unwrap(), m);
    }

    #[test]
    fn snapshot_of_220_enemies_is_small() {
        let s = ServerMsg::Snapshot(Snapshot {
            tick: 1,
            players: vec![],
            enemies: (0..220)
                .map(|id| EnemyState { id, pos: [1.5, 2.5], hp: 10.0 })
                .collect(),
            events: vec![],
        });
        // Measured bandwidth input for the PoC's 20 Hz budget (no delta, no quantisation).
        assert!(encode(&s).len() < 3200, "{}", encode(&s).len());
    }
}
