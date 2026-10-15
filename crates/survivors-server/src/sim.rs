//! The authoritative simulation: plain structs, no Bevy, no I/O, deterministic for a seed.
//!
//! PoC rules, deliberately a thin slice of the solo mode (`benilla-app/src/survivors`):
//! players move by input, enemies chase the nearest living player and hurt on contact, and every
//! player auto-casts one bolt at the nearest enemy. The numbers mirror the solo constants where
//! one exists (`BASE_HP`, `BASE_SPEED`); everything else is PoC placeholder until the real
//! ability/enemy logic is extracted into a shared crate.

use survivors_net::{EnemyId, EnemyState, Event, PlayerId, PlayerState, Snapshot};

pub const BASE_HP: f32 = 120.0;
pub const BASE_SPEED: f32 = 7.0;
pub const ENEMY_SPEED: f32 = 3.5;
pub const ENEMY_HP: f32 = 30.0;
pub const ENEMY_DPS: f32 = 8.0;
pub const ENEMY_REACH: f32 = 1.2;
pub const BOLT_RANGE: f32 = 20.0;
pub const BOLT_DAMAGE: f32 = 12.0;
pub const BOLT_COOLDOWN: f32 = 1.0;
pub const MAX_ENEMIES: usize = 220;

pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub pos: [f32; 2],
    pub hp: f32,
    pub dir: [f32; 2],
    pub seq: u32,
    pub cd: f32,
}

struct Enemy {
    id: EnemyId,
    pos: [f32; 2],
    hp: f32,
}

pub struct World {
    pub tick: u32,
    pub elapsed: f32,
    pub players: Vec<Player>,
    enemies: Vec<Enemy>,
    next_enemy: EnemyId,
    rng: u64,
    events: Vec<Event>,
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Enemy population target at `elapsed` seconds, scaled by how many players are alive: the solo
/// curve (`data::target_alive`, 14 up to 220) times `1 + 0.6 * (n - 1)`, capped.
pub fn target_alive(elapsed: f32, players: usize) -> usize {
    let solo = 14.0 + (elapsed / 600.0).min(1.0) * 225.0;
    let scale = 1.0 + 0.6 * players.saturating_sub(1) as f32;
    ((solo * scale) as usize).min(MAX_ENEMIES)
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            tick: 0,
            elapsed: 0.0,
            players: Vec::new(),
            enemies: Vec::new(),
            next_enemy: 1,
            rng: seed | 1,
            events: Vec::new(),
        }
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        ((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }

    pub fn add_player(&mut self, id: PlayerId, name: String) {
        // Spawn side by side so two players never start on top of each other.
        let x = f32::from(id) * 2.0;
        self.players.push(Player { id, name, pos: [x, 0.0], hp: BASE_HP, dir: [0.0; 2], seq: 0, cd: 0.0 });
    }

    pub fn remove_player(&mut self, id: PlayerId) {
        self.players.retain(|p| p.id != id);
    }

    pub fn set_input(&mut self, id: PlayerId, seq: u32, dir: [f32; 2]) {
        if let Some(p) = self.players.iter_mut().find(|p| p.id == id) {
            // Ignore stale/reordered inputs; clamp so a client cannot move faster than BASE_SPEED.
            if seq >= p.seq {
                let len = (dir[0] * dir[0] + dir[1] * dir[1]).sqrt();
                p.dir = if len > 1.0 { [dir[0] / len, dir[1] / len] } else { dir };
                if !p.dir.iter().all(|v| v.is_finite()) {
                    p.dir = [0.0; 2];
                }
                p.seq = seq;
            }
        }
    }

    fn alive(p: &Player) -> bool {
        p.hp > 0.0
    }

    pub fn step(&mut self, dt: f32) {
        self.tick += 1;
        self.elapsed += dt;

        for p in self.players.iter_mut().filter(|p| Self::alive(p)) {
            p.pos[0] += p.dir[0] * BASE_SPEED * dt;
            p.pos[1] += p.dir[1] * BASE_SPEED * dt;
        }

        self.spawn();
        self.move_enemies(dt);
        self.cast(dt);
    }

    fn spawn(&mut self) {
        let living: Vec<[f32; 2]> = self.players.iter().filter(|p| Self::alive(p)).map(|p| p.pos).collect();
        if living.is_empty() {
            return;
        }
        let want = target_alive(self.elapsed, living.len());
        for _ in 0..4 {
            if self.enemies.len() >= want {
                break;
            }
            let center = living[(self.rand() * living.len() as f32) as usize % living.len()];
            let a = self.rand() * std::f32::consts::TAU;
            let r = 34.0 + self.rand() * 8.0;
            let id = self.next_enemy;
            self.next_enemy += 1;
            self.enemies.push(Enemy { id, pos: [center[0] + a.cos() * r, center[1] + a.sin() * r], hp: ENEMY_HP });
        }
    }

    fn move_enemies(&mut self, dt: f32) {
        let mut hurt: Vec<(usize, f32)> = Vec::new();
        for e in &mut self.enemies {
            let target = self
                .players
                .iter()
                .enumerate()
                .filter(|(_, p)| Self::alive(p))
                .min_by(|a, b| dist(a.1.pos, e.pos).total_cmp(&dist(b.1.pos, e.pos)));
            let Some((i, p)) = target else { continue };
            let d = dist(p.pos, e.pos);
            if d > ENEMY_REACH {
                let step = (ENEMY_SPEED * dt).min(d - ENEMY_REACH * 0.5);
                e.pos[0] += (p.pos[0] - e.pos[0]) / d * step;
                e.pos[1] += (p.pos[1] - e.pos[1]) / d * step;
            } else {
                hurt.push((i, ENEMY_DPS * dt));
            }
        }
        for (i, dmg) in hurt {
            let p = &mut self.players[i];
            if Self::alive(p) {
                p.hp = (p.hp - dmg).max(0.0);
                self.events.push(Event::PlayerHurt { player: p.id, damage: dmg });
            }
        }
    }

    fn cast(&mut self, dt: f32) {
        for pi in 0..self.players.len() {
            let p = &mut self.players[pi];
            p.cd -= dt;
            if p.cd > 0.0 || !Self::alive(p) {
                continue;
            }
            let (pos, id) = (p.pos, p.id);
            let nearest = self
                .enemies
                .iter_mut()
                .filter(|e| dist(e.pos, pos) <= BOLT_RANGE)
                .min_by(|a, b| dist(a.pos, pos).total_cmp(&dist(b.pos, pos)));
            let Some(e) = nearest else { continue };
            e.hp -= BOLT_DAMAGE;
            self.events.push(Event::Bolt { caster: id, enemy: e.id, damage: BOLT_DAMAGE });
            if e.hp <= 0.0 {
                self.events.push(Event::EnemyDied { enemy: e.id });
            }
            self.players[pi].cd = BOLT_COOLDOWN;
        }
        self.enemies.retain(|e| e.hp > 0.0);
    }

    pub fn enemy_count(&self) -> usize {
        self.enemies.len()
    }

    /// Build the snapshot and drain the event list.
    pub fn snapshot(&mut self) -> Snapshot {
        Snapshot {
            tick: self.tick,
            players: self
                .players
                .iter()
                .map(|p| PlayerState {
                    id: p.id,
                    name: p.name.clone(),
                    pos: p.pos,
                    hp: p.hp,
                    max_hp: BASE_HP,
                    ack_seq: p.seq,
                    dead: !Self::alive(p),
                })
                .collect(),
            enemies: self.enemies.iter().map(|e| EnemyState { id: e.id, pos: e.pos, hp: e.hp }).collect(),
            events: std::mem::take(&mut self.events),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(w: &mut World, secs: f32) {
        for _ in 0..(secs * 20.0) as u32 {
            w.step(0.05);
        }
    }

    #[test]
    fn deterministic_for_a_seed() {
        let mk = || {
            let mut w = World::new(42);
            w.add_player(0, "a".into());
            w.add_player(1, "b".into());
            w.set_input(0, 1, [1.0, 0.0]);
            run(&mut w, 10.0);
            w.snapshot()
        };
        assert_eq!(mk(), mk());
    }

    #[test]
    fn input_is_clamped_to_base_speed() {
        let mut w = World::new(1);
        w.add_player(0, "a".into());
        w.set_input(0, 1, [100.0, 0.0]);
        w.step(1.0);
        assert!((w.players[0].pos[0] - BASE_SPEED).abs() < 1e-3);
        w.set_input(0, 2, [f32::NAN, 0.0]);
        let x = w.players[0].pos[0];
        w.step(1.0);
        assert_eq!(w.players[0].pos[0], x);
    }

    #[test]
    fn stale_inputs_are_ignored() {
        let mut w = World::new(1);
        w.add_player(0, "a".into());
        w.set_input(0, 5, [1.0, 0.0]);
        w.set_input(0, 4, [-1.0, 0.0]);
        assert_eq!(w.players[0].dir, [1.0, 0.0]);
    }

    #[test]
    fn two_players_scale_population_and_both_cast() {
        let mut w = World::new(7);
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        let mut events = Vec::new();
        for _ in 0..600 {
            w.step(0.05);
            events.extend(w.snapshot().events);
        }
        for id in [0, 1] {
            assert!(
                events.iter().any(|e| matches!(e, Event::Bolt { caster, .. } if *caster == id)),
                "player {id} never cast"
            );
        }
        assert!(target_alive(0.0, 2) > target_alive(0.0, 1));
    }

    #[test]
    fn enemies_chase_the_nearest_player_and_can_kill() {
        let mut w = World::new(3);
        w.add_player(0, "a".into());
        // No casting possible if we remove range: just stand still and let the horde come.
        run(&mut w, 120.0);
        assert!(w.players[0].hp < BASE_HP);
    }

    #[test]
    fn dead_player_stops_being_targeted() {
        let mut w = World::new(3);
        w.add_player(0, "a".into());
        w.add_player(1, "b".into());
        w.players[0].hp = 0.0;
        run(&mut w, 5.0);
        assert!(w.players[1].hp <= BASE_HP);
        assert_eq!(w.players[0].hp, 0.0);
    }
}
