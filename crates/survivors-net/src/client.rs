//! Client-side netcode with no engine in it: snapshot interpolation for remote entities and
//! prediction/reconciliation for the local player. The Bevy glue (`benilla-app`'s
//! `survivors/online.rs`) only feeds it clocks and applies what it returns, so the part most
//! likely to feel wrong is unit-tested natively.

use std::collections::VecDeque;

use crate::{EnemyId, PlayerId, PlayerState, Snapshot};

/// Remote entities render this far behind the newest snapshot: two ticks at 20 Hz, so one lost
/// or late snapshot still leaves a pair to interpolate between.
pub const INTERP_DELAY: f64 = 0.1;
const KEEP: usize = 32;

pub struct Interpolated<T> {
    pub id: T,
    pub pos: [f32; 2],
    /// Direction of travel over the interpolation span; zero when standing still.
    pub vel: [f32; 2],
}

pub struct View {
    pub players: Vec<Interpolated<PlayerId>>,
    pub enemies: Vec<Interpolated<EnemyId>>,
}

#[derive(Default)]
pub struct SnapshotBuffer {
    /// `(receive time in seconds, snapshot)`, oldest first.
    snaps: VecDeque<(f64, Snapshot)>,
}

fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

impl SnapshotBuffer {
    pub fn push(&mut self, now: f64, s: Snapshot) {
        // Drop reordered/duplicate snapshots (a stale tick would make entities jump backwards).
        if self.snaps.back().is_some_and(|(_, last)| s.tick <= last.tick) {
            return;
        }
        self.snaps.push_back((now, s));
        while self.snaps.len() > KEEP {
            self.snaps.pop_front();
        }
    }

    pub fn latest(&self) -> Option<&Snapshot> {
        self.snaps.back().map(|(_, s)| s)
    }

    /// The world as it looked `INTERP_DELAY` before `now`. Entities in only the newer snapshot
    /// appear at their new position; entities missing from it are gone.
    pub fn sample(&self, now: f64) -> Option<View> {
        let render = now - INTERP_DELAY;
        let newest = self.snaps.back()?;
        // The pair around `render`, or the newest alone (no extrapolation) when render is past it.
        let i = self.snaps.iter().position(|(t, _)| *t >= render);
        let (a, b, f) = match i {
            None => (newest, newest, 1.0),
            Some(0) => (&self.snaps[0], &self.snaps[0], 1.0),
            Some(i) => {
                let (a, b) = (&self.snaps[i - 1], &self.snaps[i]);
                let span = (b.0 - a.0).max(1e-6);
                (a, b, (((render - a.0) / span).clamp(0.0, 1.0)) as f32)
            }
        };
        let span = (b.0 - a.0).max(1e-6) as f32;
        let mk = |pa: Option<[f32; 2]>, pb: [f32; 2]| {
            let pa = pa.unwrap_or(pb);
            let vel = if a.0 == b.0 { [0.0; 2] } else { [(pb[0] - pa[0]) / span, (pb[1] - pa[1]) / span] };
            (lerp(pa, pb, f), vel)
        };
        Some(View {
            players: b
                .1
                .players
                .iter()
                .map(|p| {
                    let (pos, vel) = mk(a.1.players.iter().find(|q| q.id == p.id).map(|q| q.pos), p.pos);
                    Interpolated { id: p.id, pos, vel }
                })
                .collect(),
            enemies: b
                .1
                .enemies
                .iter()
                .map(|e| {
                    let (pos, vel) = mk(a.1.enemies.iter().find(|q| q.id == e.id).map(|q| q.pos), e.pos);
                    Interpolated { id: e.id, pos, vel }
                })
                .collect(),
        })
    }
}

/// Prediction for the local player: it moves immediately on input; each snapshot says where the
/// server had it at `ack_seq`, and the difference to where we predicted it then is the error.
#[derive(Default)]
pub struct Predictor {
    history: VecDeque<(u32, [f32; 2])>,
}

/// Errors past this many yards (a teleport, a dropped client) snap instead of blending.
pub const SNAP_DISTANCE: f32 = 3.0;

impl Predictor {
    /// Remember where the local player was when input `seq` was sent.
    pub fn record(&mut self, seq: u32, pos: [f32; 2]) {
        self.history.push_back((seq, pos));
        while self.history.len() > 128 {
            self.history.pop_front();
        }
    }

    /// The correction to add to the current local position for a server state, or `None` when
    /// that `ack_seq` is unknown (use the server position directly then, see `needs_snap`).
    pub fn error(&mut self, server: &PlayerState) -> Option<[f32; 2]> {
        while self.history.len() > 1 && self.history[1].0 <= server.ack_seq {
            self.history.pop_front();
        }
        let (seq, pos) = *self.history.front()?;
        (seq <= server.ack_seq).then(|| [server.pos[0] - pos[0], server.pos[1] - pos[1]])
    }
}

/// New local position given the current one and the prediction error: blend small errors over a
/// few frames, snap large ones.
pub fn reconcile(local: [f32; 2], err: [f32; 2], dt: f32) -> [f32; 2] {
    let len = (err[0] * err[0] + err[1] * err[1]).sqrt();
    if len > SNAP_DISTANCE {
        return [local[0] + err[0], local[1] + err[1]];
    }
    let k = 1.0 - (-10.0 * dt).exp();
    [local[0] + err[0] * k, local[1] + err[1] * k]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnemyState;

    fn snap(tick: u32, x: f32) -> Snapshot {
        Snapshot {
            tick,
            players: vec![],
            enemies: vec![EnemyState { id: 1, pos: [x, 0.0], hp: 1.0 }],
            events: vec![],
        }
    }

    #[test]
    fn interpolates_between_two_snapshots() {
        let mut b = SnapshotBuffer::default();
        b.push(1.00, snap(1, 0.0));
        b.push(1.05, snap(2, 1.0));
        b.push(1.10, snap(3, 2.0));
        // render time = 1.175 - 0.1 = 1.075: halfway between ticks 2 and 3.
        let v = b.sample(1.175).unwrap();
        assert!((v.enemies[0].pos[0] - 1.5).abs() < 1e-4);
        assert!((v.enemies[0].vel[0] - 20.0).abs() < 1e-2);
    }

    #[test]
    fn never_extrapolates_and_drops_stale_ticks() {
        let mut b = SnapshotBuffer::default();
        b.push(1.0, snap(5, 4.0));
        b.push(1.1, snap(4, 99.0)); // reordered: ignored
        let v = b.sample(5.0).unwrap();
        assert_eq!(v.enemies[0].pos, [4.0, 0.0]);
        assert_eq!(v.enemies[0].vel, [0.0, 0.0]);
    }

    #[test]
    fn entity_missing_from_newer_snapshot_is_gone() {
        let mut b = SnapshotBuffer::default();
        b.push(1.0, snap(1, 0.0));
        b.push(1.05, Snapshot { enemies: vec![], ..snap(2, 0.0) });
        assert!(b.sample(1.2).unwrap().enemies.is_empty());
    }

    fn me(ack: u32, x: f32) -> PlayerState {
        PlayerState { id: 0, name: String::new(), pos: [x, 0.0], hp: 1.0, max_hp: 1.0, ack_seq: ack, dead: false }
    }

    #[test]
    fn perfect_prediction_has_no_error_despite_latency() {
        let mut p = Predictor::default();
        // Moving at 7 yd/s, one input per 50 ms; the server's answer for seq 3 arrives late, when
        // the local player is already at seq 8. The error must be about seq 3, not "now".
        for s in 1..=8u32 {
            p.record(s, [s as f32 * 0.35, 0.0]);
        }
        let e = p.error(&me(3, 3.0 * 0.35)).unwrap();
        assert!(e[0].abs() < 1e-5, "{e:?}");
    }

    #[test]
    fn mispredicted_position_is_corrected_and_large_errors_snap() {
        let mut p = Predictor::default();
        p.record(1, [0.0, 0.0]);
        p.record(2, [0.35, 0.0]);
        let e = p.error(&me(2, 0.0)).unwrap(); // server says we were blocked
        assert!((e[0] + 0.35).abs() < 1e-5);
        let blended = reconcile([1.0, 0.0], e, 0.016);
        assert!(blended[0] < 1.0 && blended[0] > 0.65);
        assert_eq!(reconcile([10.0, 0.0], [-5.0, 0.0], 0.016), [5.0, 0.0]);
    }

    #[test]
    fn unknown_ack_gives_no_error() {
        let mut p = Predictor::default();
        p.record(10, [0.0, 0.0]);
        assert!(p.error(&me(3, 0.0)).is_none());
    }
}
