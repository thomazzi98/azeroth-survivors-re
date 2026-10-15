//! Online mode (PoC): the client stops simulating and renders what `survivors-server` says.
//!
//! Turned on by `?survivors_server=ws://host:port/ws` (or `WOW_SURVIVORS_SERVER`). The local
//! spawn/chase/cast/damage systems are switched off ([`offline`]); the hero keeps its local WASD
//! movement as *prediction* and is corrected from the server's acknowledged position. Other
//! players and every enemy are display-only units moved by snapshot interpolation. The netcode
//! maths lives in `survivors_net::client` (unit-tested natively); this file is the Bevy glue.
//!
//! PoC limits, on purpose: every player is drawn with the local player's class and look, every
//! enemy with the roster's first creature, bolts use the local class's starting spell, and there
//! are no cards, XP or bosses online yet. Browser only: the native build logs and stays solo.

use std::collections::HashMap;

use benilla_world::world_point::WorldPoint;
use bevy::prelude::*;
use survivors_net::client::{reconcile, Predictor, SnapshotBuffer};
use survivors_net::{
    decode, encode, ClientMsg, EnemyId, Event, PlayerId, ServerMsg, PROTOCOL_VERSION,
};

use super::enemies;
use super::hero::{self, CamMode, Hero};
use super::units::{self, yaw_toward, Fx, Marks, UnitSpec, COLOR_SPELL};
use super::{data, Phase, Run, Selection};
use crate::creature_anim::{move_flags, MovementState};
use crate::net::{FieldChanged, Guid, ObjectStore};

/// Input is sent at the server's tick rate.
const SEND_EVERY: f32 = 1.0 / survivors_net::TICK_HZ as f32;

#[derive(Resource)]
pub(super) struct OnlineCfg {
    pub url: String,
}

/// The server URL if this page asked for online play.
pub(super) fn server_url() -> Option<String> {
    crate::webenv::var("WOW_SURVIVORS_SERVER").filter(|s| !s.is_empty())
}

/// Run condition for the local simulation: true when there is no server to defer to.
pub(super) fn offline(cfg: Option<Res<OnlineCfg>>) -> bool {
    cfg.is_none()
}

#[derive(Component)]
struct RemotePlayer(PlayerId);

#[derive(Component)]
struct RemoteEnemy(EnemyId);

#[derive(Resource, Default)]
struct OnlineState {
    me: Option<PlayerId>,
    buf: SnapshotBuffer,
    pred: Predictor,
    seq: u32,
    send_t: f32,
    /// The last snapshot tick the local hero was reconciled against.
    reconciled: u32,
    synced: bool,
    players: HashMap<PlayerId, Entity>,
    enemies: HashMap<EnemyId, Entity>,
}

pub(super) fn plugin(app: &mut App, url: String) {
    info!("survivors online: server {url}");
    app.insert_resource(OnlineCfg { url })
        .init_resource::<OnlineState>()
        .insert_non_send_resource(LinkSlot(None))
        .add_systems(OnEnter(Phase::Playing), connect)
        .add_systems(OnEnter(Phase::Menu), disconnect)
        .add_systems(
            Update,
            net_frame
                .after(hero::drive_hero)
                .after(hero::place_camera)
                .run_if(in_state(Phase::Playing)),
        );
}

struct LinkSlot(Option<link::Link>);

fn connect(
    cfg: Res<OnlineCfg>,
    mut slot: NonSendMut<LinkSlot>,
    mut run: ResMut<Run>,
    mut state: ResMut<OnlineState>,
) {
    // Playing is re-entered after a pause or card pick; only a missing/dead link reconnects.
    if slot.0.as_ref().is_some_and(|l| !l.closed()) {
        return;
    }
    *state = OnlineState::default();
    match link::Link::open(&cfg.url) {
        Ok(l) => {
            l.send(&encode(&ClientMsg::Join {
                version: PROTOCOL_VERSION,
                name: run.class.name.chars().take(24).collect(),
            }));
            slot.0 = Some(l);
        }
        Err(e) => {
            warn!("survivors online: {e}");
            run.banner = Some((format!("Online unavailable: {e}"), 5.0));
        }
    }
}

fn disconnect(mut slot: NonSendMut<LinkSlot>, mut state: ResMut<OnlineState>) {
    slot.0 = None; // dropping closes the socket
    *state = OnlineState::default(); // the entities themselves go with `reset_run`
}

/// WASD as the server wants it: world-relative ground axes (x, z), camera-relative when orbiting,
/// mirroring `hero::drive_hero`.
fn wanted_dir(keys: &ButtonInput<KeyCode>, rig: &hero::CamRig) -> Vec2 {
    let mut d = Vec3::ZERO;
    if keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]) {
        d.z -= 1.0;
    }
    if keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]) {
        d.z += 1.0;
    }
    if keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]) {
        d.x -= 1.0;
    }
    if keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]) {
        d.x += 1.0;
    }
    if rig.mode == CamMode::Orbit {
        let (sy, cy) = rig.yaw.sin_cos();
        d = Vec3::new(d.x * cy + d.z * sy, 0.0, d.z * cy - d.x * sy);
    }
    Vec2::new(d.x, d.z).normalize_or_zero()
}

/// One frame of online play, in the order that keeps events pointing at live entities:
/// read the socket → spawn what appeared → play events → despawn what left → place everything →
/// reconcile the local hero → send input.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn net_frame(
    mut commands: Commands,
    time: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    rig: Res<hero::CamRig>,
    selection: Res<Selection>,
    mut run: ResMut<Run>,
    mut state: ResMut<OnlineState>,
    slot: NonSend<LinkSlot>,
    world: WorldPoint,
    mut hero_q: Query<(Entity, &mut Transform), With<Hero>>,
    mut remotes: Query<
        (&mut Transform, &mut MovementState),
        (Or<(With<RemotePlayer>, With<RemoteEnemy>)>, Without<Hero>),
    >,
    mut stores: Query<(&Guid, &mut ObjectStore, &mut Marks), With<RemoteEnemy>>,
    mut fields: MessageWriter<FieldChanged>,
    mut fx: Fx,
) {
    let Some(link) = slot.0.as_ref() else { return };
    let Ok((hero_e, mut htf)) = hero_q.single_mut() else { return };
    let now = time.elapsed_secs_f64();
    let center = hero::arena(selection.stage_def());

    while let Some(bytes) = link.recv() {
        match decode::<ServerMsg>(&bytes) {
            Ok(ServerMsg::Welcome { you, .. }) => state.me = Some(you),
            Ok(ServerMsg::Rejected { reason }) => {
                run.banner = Some((format!("Server: {reason}"), 5.0));
            }
            Ok(ServerMsg::Snapshot(s)) => state.buf.push(now, s),
            Err(e) => warn!("survivors online: bad frame: {e}"),
        }
    }
    let Some(me) = state.me else { return };
    let Some(latest) = state.buf.latest().cloned() else {
        send_input(&mut state, link, &keys, &rig, &htf, center, time.delta_secs(), false);
        return;
    };

    // Spawn what appeared.
    let look = selection.look.and_then(|i| selection.looks.get(i)).copied();
    for p in &latest.players {
        if p.id == me || state.players.contains_key(&p.id) {
            continue;
        }
        let (Some(class), Some((display, race, sex))) = (selection.class, look) else { continue };
        let pos = center + Vec3::new(p.pos[0], 0.0, p.pos[1]);
        let e = hero::spawn_hero(
            &mut commands,
            &data::CLASSES[class],
            display,
            race,
            sex,
            run.guid(),
            pos,
        );
        commands.entity(e).remove::<Hero>().insert(RemotePlayer(p.id));
        state.players.insert(p.id, e);
    }
    let roster = data::roster(selection.map);
    for en in &latest.enemies {
        if state.enemies.contains_key(&en.id) {
            continue;
        }
        let def = &roster.enemies[0];
        let pos = center + Vec3::new(en.pos[0], 0.0, en.pos[1]);
        let e = units::spawn_unit(
            &mut commands,
            &UnitSpec {
                serial: 1_000_000 + u64::from(en.id),
                display: def.display,
                scale: def.scale,
                level: def.level,
                faction: units::FACTION_MONSTER,
                bytes0: 1 << 8,
                weapons: &[],
                sheath: 0,
            },
            pos,
            0.0,
        );
        commands.entity(e).insert((RemoteEnemy(en.id), Marks::debuffs()));
        state.enemies.insert(en.id, e);
    }

    // Events run once per snapshot, the frame it first becomes the latest.
    let new_tick = latest.tick != state.reconciled;
    if new_tick {
        let spell = data::ability(run.class.start).spell;
        let who = |state: &OnlineState, id: PlayerId| {
            if id == me {
                Some(hero_e)
            } else {
                state.players.get(&id).copied()
            }
        };
        for ev in &latest.events {
            match *ev {
                Event::Bolt { caster, enemy, damage } => {
                    if let (Some(c), Some(&t)) = (who(&state, caster), state.enemies.get(&enemy)) {
                        fx.cast(c, spell, &[t], None);
                        fx.number(t, damage, false, COLOR_SPELL);
                    }
                }
                Event::EnemyDied { enemy } => {
                    if let Some(e) = state.enemies.remove(&enemy) {
                        if let Ok((guid, mut store, mut marks)) = stores.get_mut(e) {
                            units::kill_unit(&mut store);
                            marks.clear();
                            marks.flush(e, guid.0, &mut store, &mut fields);
                        }
                        enemies::mark_dying(&mut commands, e);
                    }
                }
                Event::PlayerHurt { .. } => {}
            }
        }
    }

    // Despawn what left (corpses already left the map above and fade on their own).
    let gone: Vec<EnemyId> = state
        .enemies
        .keys()
        .copied()
        .filter(|id| !latest.enemies.iter().any(|e| e.id == *id))
        .collect();
    for id in gone {
        if let Some(e) = state.enemies.remove(&id) {
            commands.entity(e).despawn();
        }
    }
    let left: Vec<PlayerId> = state
        .players
        .keys()
        .copied()
        .filter(|id| !latest.players.iter().any(|p| p.id == *id))
        .collect();
    for id in left {
        if let Some(e) = state.players.remove(&id) {
            commands.entity(e).despawn();
        }
    }

    // Place everything interpolated.
    if let Some(view) = state.buf.sample(now) {
        let mut place = |e: Entity, pos: [f32; 2], vel: [f32; 2]| {
            let Ok((mut tf, mut mv)) = remotes.get_mut(e) else { return };
            let mut p = center + Vec3::new(pos[0], 0.0, pos[1]);
            p.y = world.terrain_height_under(p).unwrap_or(tf.translation.y);
            tf.translation = p;
            let v = Vec3::new(vel[0], 0.0, vel[1]);
            if v.length() > 0.2 {
                tf.rotation = Quat::from_rotation_y(yaw_toward(v));
                mv.speed = v.length();
                mv.flags = move_flags::FORWARD;
            } else {
                mv.speed = 0.0;
                mv.flags = 0;
            }
        };
        for p in &view.players {
            if let Some(&e) = state.players.get(&p.id) {
                place(e, p.pos, p.vel);
            }
        }
        for en in &view.enemies {
            if let Some(&e) = state.enemies.get(&en.id) {
                place(e, en.pos, en.vel);
            }
        }
    }

    // The local hero: authoritative health, and position corrected from the acked input.
    let mut dead = false;
    if let Some(mine) = latest.players.iter().find(|p| p.id == me) {
        run.hp = mine.hp;
        dead = mine.dead;
        if new_tick {
            let local = [htf.translation.x - center.x, htf.translation.z - center.z];
            if !state.synced {
                // First snapshot: stand where the server put us.
                htf.translation.x = center.x + mine.pos[0];
                htf.translation.z = center.z + mine.pos[1];
                state.synced = true;
            } else if let Some(err) = state.pred.error(mine) {
                let fixed = reconcile(local, err, SEND_EVERY);
                htf.translation.x = center.x + fixed[0];
                htf.translation.z = center.z + fixed[1];
            }
            state.reconciled = latest.tick;
        }
    }
    send_input(&mut state, link, &keys, &rig, &htf, center, time.delta_secs(), dead);
}

#[allow(clippy::too_many_arguments)]
fn send_input(
    state: &mut OnlineState,
    link: &link::Link,
    keys: &ButtonInput<KeyCode>,
    rig: &hero::CamRig,
    htf: &Transform,
    center: Vec3,
    dt: f32,
    dead: bool,
) {
    state.send_t += dt;
    if state.send_t < SEND_EVERY {
        return;
    }
    state.send_t = 0.0;
    state.seq += 1;
    let dir = if dead { Vec2::ZERO } else { wanted_dir(keys, rig) };
    state
        .pred
        .record(state.seq, [htf.translation.x - center.x, htf.translation.z - center.z]);
    link.send(&encode(&ClientMsg::Input {
        seq: state.seq,
        dir: dir.into(),
        aim: [0.0; 2],
    }));
}

#[cfg(target_arch = "wasm32")]
mod link {
    //! The browser WebSocket, one binary frame per message. Not `Send`, so it lives in a
    //! `NonSend` slot; Bevy's `Update` is single-threaded here anyway.

    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    use web_sys::{BinaryType, CloseEvent, MessageEvent, WebSocket};

    pub struct Link {
        ws: WebSocket,
        inbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
        /// Frames sent while still connecting (the Join); flushed on open.
        outbox: Rc<RefCell<Vec<Vec<u8>>>>,
        closed: Rc<RefCell<bool>>,
        _keep: (
            Closure<dyn FnMut(MessageEvent)>,
            Closure<dyn FnMut()>,
            Closure<dyn FnMut(CloseEvent)>,
        ),
    }

    impl Link {
        pub fn open(url: &str) -> Result<Self, String> {
            let ws = WebSocket::new(url).map_err(|e| format!("{e:?}"))?;
            ws.set_binary_type(BinaryType::Arraybuffer);
            let inbox = Rc::new(RefCell::new(VecDeque::new()));
            let outbox: Rc<RefCell<Vec<Vec<u8>>>> = Rc::default();
            let closed = Rc::new(RefCell::new(false));

            let q = inbox.clone();
            let on_msg = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
                if let Ok(buf) = ev.data().dyn_into::<js_sys::ArrayBuffer>() {
                    q.borrow_mut().push_back(js_sys::Uint8Array::new(&buf).to_vec());
                }
            });
            ws.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));

            let (w, o) = (ws.clone(), outbox.clone());
            let on_open = Closure::<dyn FnMut()>::new(move || {
                for f in o.borrow_mut().drain(..) {
                    let _ = w.send_with_u8_array(&f);
                }
            });
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));

            let c = closed.clone();
            let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |_| *c.borrow_mut() = true);
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));

            Ok(Self { ws, inbox, outbox, closed, _keep: (on_msg, on_open, on_close) })
        }

        pub fn send(&self, bytes: &[u8]) {
            match self.ws.ready_state() {
                WebSocket::OPEN => {
                    let _ = self.ws.send_with_u8_array(bytes);
                }
                WebSocket::CONNECTING => self.outbox.borrow_mut().push(bytes.to_vec()),
                _ => {}
            }
        }

        pub fn recv(&self) -> Option<Vec<u8>> {
            self.inbox.borrow_mut().pop_front()
        }

        pub fn closed(&self) -> bool {
            *self.closed.borrow()
        }
    }

    impl Drop for Link {
        fn drop(&mut self) {
            self.ws.set_onmessage(None);
            self.ws.set_onopen(None);
            self.ws.set_onclose(None);
            let _ = self.ws.close();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod link {
    //! Native stub: online play is the browser build's feature for now.

    pub struct Link;

    impl Link {
        pub fn open(_url: &str) -> Result<Self, String> {
            Err("online play needs the browser build".into())
        }
        pub fn send(&self, _bytes: &[u8]) {}
        pub fn recv(&self) -> Option<Vec<u8>> {
            None
        }
        pub fn closed(&self) -> bool {
            true
        }
    }
}
