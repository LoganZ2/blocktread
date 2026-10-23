//! Flight: drives the client core each frame and presents its state.

pub mod env;
pub mod exhaust;
pub mod fx;
pub mod hud;
pub mod ships;

use crate::audio::{Sfx, Sounds};
use crate::settings::Settings;
use crate::screens::Overlays;
use crate::ui::UiAction;
use crate::ui::kit::ModalReq;
use bevy::input_focus::InputFocus;
use crate::{DevOpts, LastError, MainCamera, Screen};
use bevy::audio::Volume;
use bevy::light::Skybox;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use voxwing_client_core::{ClientCore, ConnState};
use voxwing_shared::ai::{AiPilot, Contact, Skill};
use voxwing_shared::flight::{BTN_BOOST, BTN_FIRE, BTN_MISSILE, InputCmd, NO_LOCK, q_axis};
use voxwing_shared::weapons::intercept;

#[derive(Resource)]
pub struct Session {
    pub core: ClientCore,
    pub addr: String,
    pub killer: Option<String>,
    pub target: Option<u16>,
    pub lock: f32,
    pub locked: bool,
    pub throttle: f32,
    pub cockpit_cam: bool,
    cam_rot: Quat,
    pub cam_init: bool,
    pub paused: bool,
    pub death: Option<(Vec3, f32)>,
    pub shake: f32,
    ai: Option<AiPilot>,
    pitch: f32,
    yaw: f32,
    roll: f32,
    buttons: u8,
    /// The ship the camera rides with while spectating (or waiting to respawn).
    pub spectate: Option<u16>,
    /// Spectating: the watched ship just died. The camera circles its wreck for a moment, then
    /// moves on to the killer: (wreck, seconds left, killer).
    pub wreck: Option<(Vec3, f32, Option<u16>)>,
}

impl Session {
    pub fn new(core: ClientCore, addr: String) -> Self {
        Session {
            core,
            addr,
            killer: None,
            target: None,
            lock: 0.0,
            locked: false,
            throttle: 0.7,
            cockpit_cam: false,
            cam_rot: Quat::IDENTITY,
            cam_init: false,
            paused: false,
            death: None,
            shake: 0.0,
            ai: None,
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
            buttons: 0,
            spectate: None,
            wreck: None,
        }
    }

    /// The ship being watched, while spectating and not between ships.
    pub fn watched(&self) -> Option<&voxwing_client_core::CoreShip> {
        if !self.core.watching || self.wreck.is_some() {
            return None;
        }
        self.spectate.and_then(|id| self.core.ships.get(&id))
    }

    /// The ship whose instruments the HUD shows: your own, or the one you are watching.
    pub fn view(&self) -> Option<&voxwing_client_core::CoreShip> {
        self.core.my().or_else(|| self.watched())
    }
}

/// Ships a spectator can ride with, AI first (that is what spectating is for), then by id.
fn watch_order(core: &ClientCore) -> Vec<u16> {
    let mut ids: Vec<(bool, u16)> = core.ships.values().map(|s| (s.owner.is_some(), s.id)).collect();
    ids.sort();
    ids.into_iter().map(|(_, id)| id).collect()
}

/// Seconds the camera lingers on a watched ship's wreck.
pub const WRECK_LINGER: f32 = 2.2;

#[derive(Component)]
struct Hum;

pub struct FlightPlugin;

impl Plugin for FlightPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(exhaust::ExhaustPlugin)
            .init_resource::<ships::ShipVisuals>()
            .init_resource::<hud::Hud>()
            .add_systems(Startup, setup_assets)
            .add_systems(OnEnter(Screen::Flight), (enter_flight, hud::spawn_hud))
            .add_systems(OnExit(Screen::Flight), exit_flight)
            .add_systems(
                Update,
                (read_pilot, step_core, ships::apply_events, ships::sync_ships, targeting, spectate, camera_follow, exhaust::update_exhaust, fx::update_fx, env::orient_rocks, hud::update_hud, hud::update_vignettes, hud::flight_overlays, flight_actions, engine_hum)
                    .chain()
                    .run_if(in_state(Screen::Flight))
                    .run_if(resource_exists::<Session>),
            );
    }
}

fn setup_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(fx::Fx::new(&mut meshes, &mut mats));
    commands.insert_resource(ships::flame_mats(&mut mats));
    commands.insert_resource(env::SkyImage(env::build_sky(&mut images)));
    commands.insert_resource(env::build_env_assets(&mut meshes, &mut mats, &mut images));
}

#[allow(clippy::too_many_arguments)]
fn enter_flight(
    mut commands: Commands,
    mut mats: ResMut<Assets<StandardMaterial>>,
    env_assets: Res<env::EnvAssets>,
    sky: Res<env::SkyImage>,
    mut fx: ResMut<fx::Fx>,
    cam: Single<(Entity, &mut Projection), With<MainCamera>>,
    dev: Res<DevOpts>,
    mut session: Option<ResMut<Session>>,
    sounds: Res<Sounds>,
    mut focus: ResMut<InputFocus>,
) {
    focus.clear();
    let (ce, mut proj) = cam.into_inner();
    commands.entity(ce).insert(Skybox { image: Some(sky.0.clone()), brightness: 1200.0, ..default() });
    if let Projection::Perspective(p) = &mut *proj {
        p.fov = 68f32.to_radians();
    }
    env::spawn_env(&mut commands, &env_assets, Screen::Flight, Vec3::ZERO);
    fx::spawn_dust(&mut commands, &mut fx, &mut mats);
    if let Some(s) = session.as_mut()
        && dev.autopilot
    {
        s.ai = Some(AiPilot::new(Skill { aim_err: 0.012, cone: 0.05, react: 7.0, dmg_mul: 1.0 }, 1));
    }
    if sounds.enabled {
        commands.spawn((AudioPlayer(sounds.hum.clone()), PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0)), Hum, DespawnOnExit(Screen::Flight)));
    }
}

fn exit_flight(mut commands: Commands, session: Option<Res<Session>>, mut vis: ResMut<ships::ShipVisuals>) {
    if let Some(s) = session {
        s.core.leave();
    }
    commands.remove_resource::<Session>();
    vis.0.clear();
}

fn read_pilot(keys: Res<ButtonInput<KeyCode>>, mouse: Res<ButtonInput<MouseButton>>, window: Single<&Window, With<PrimaryWindow>>, mut session: ResMut<Session>, time: Res<Time>, settings: Res<Settings>) {
    let s = &mut *session;
    if s.paused || s.core.watching {
        s.buttons = 0;
        return;
    }
    let dt = time.delta_secs();
    let (w, h) = (window.width(), window.height());
    let r = w.min(h) * 0.32 / settings.sensitivity.clamp(0.3, 3.0);
    let mut stick = window.cursor_position().map_or(Vec2::ZERO, |c| (c - Vec2::new(w, h) / 2.0) / r);
    let m = stick.length();
    if m > 1.0 {
        stick /= m;
    }
    let dz = 0.06;
    let m = stick.length();
    stick *= if m < dz { 0.0 } else { (m - dz) / (1.0 - dz) / m };
    s.yaw = stick.x;
    s.pitch = -stick.y * if settings.invert_y { -1.0 } else { 1.0 };
    if keys.pressed(KeyCode::ArrowLeft) {
        s.yaw = -1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        s.yaw = 1.0;
    }
    if keys.pressed(KeyCode::ArrowUp) {
        s.pitch = 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        s.pitch = -1.0;
    }
    s.roll = (keys.any_pressed([KeyCode::KeyD, KeyCode::KeyE]) as i32 - keys.any_pressed([KeyCode::KeyA, KeyCode::KeyQ]) as i32) as f32;
    if keys.pressed(KeyCode::KeyW) {
        s.throttle = (s.throttle + dt * 0.8).min(1.0);
    }
    if keys.pressed(KeyCode::KeyS) {
        s.throttle = (s.throttle - dt * 0.8).max(0.0);
    }
    let mut b = 0;
    if mouse.pressed(MouseButton::Left) || keys.pressed(KeyCode::Space) {
        b |= BTN_FIRE;
    }
    if mouse.pressed(MouseButton::Right) || keys.pressed(KeyCode::KeyF) {
        b |= BTN_MISSILE;
    }
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        b |= BTN_BOOST;
    }
    s.buttons = b;
}

fn step_core(mut session: ResMut<Session>, time: Res<Time>, mut next: ResMut<NextState<Screen>>, mut last_err: ResMut<LastError>) {
    let dt = time.delta_secs().min(0.1);
    let s = &mut *session;
    let (pitch, yaw, roll, throttle, buttons, target, locked) = (s.pitch, s.yaw, s.roll, s.throttle, s.buttons, s.target, s.locked);
    let mut ai = s.ai.take();
    s.core.update(dt, &mut |c: &ClientCore, _tick| {
        if let Some(ai) = ai.as_mut()
            && let (Some(p), Some(me)) = (c.pred.as_ref(), c.my())
        {
            let others: Vec<Contact> = c.ships.values().filter(|o| Some(o.id) != c.my_ship).map(|o| Contact { id: o.id, team: o.team, pos: o.pos, vel: o.vel, radius: o.body.radius }).collect();
            return ai.think(&p.kin, &me.body, 0.0, me.team, &others, &c.rocks, voxwing_shared::DT);
        }
        let mut cmd = InputCmd { pitch: q_axis(pitch), yaw: q_axis(yaw), roll: q_axis(roll), throttle: (throttle * 255.0) as u8, buttons, ..Default::default() };
        if let (Some(t), Some(p), Some(me)) = (target.and_then(|id| c.ships.get(&id)), c.pred.as_ref(), c.my()) {
            let lead = intercept(p.kin.pos, p.kin.vel, t.pos, t.vel, me.body.bolt_speed);
            if p.kin.fwd().angle_between(lead - p.kin.pos) < 0.12 {
                cmd.aim_dist = (lead - p.kin.pos).length().clamp(40.0, 900.0) as u16;
            }
        }
        cmd.lock = if locked { target.unwrap_or(NO_LOCK) } else { NO_LOCK };
        cmd
    });
    s.ai = ai;
    let joined = s.core.pid.is_some();
    let err = match &s.core.state {
        ConnState::Rejected(r) => Some(("无法加入".to_string(), format!("服务器拒绝了加入：{r}"))),
        ConnState::Closed(r) if joined => Some(("连接中断".to_string(), format!("与 {} 的连接断开了。{r}", s.addr))),
        ConnState::Closed(r) => Some(("无法连接".to_string(), format!("连不上 {}。{r}", s.addr))),
        _ => None,
    };
    if let Some(e) = err {
        warn!("session ended: {}", e.1);
        last_err.0 = Some(e);
        next.set(Screen::Lobby);
    }
}

fn targeting(keys: Res<ButtonInput<KeyCode>>, mut session: ResMut<Session>, time: Res<Time>, mut sounds: ResMut<Sounds>, mut commands: Commands) {
    let dt = time.delta_secs();
    let s = &mut *session;
    let Some(me) = s.core.my() else {
        s.target = None;
        s.lock = 0.0;
        s.locked = false;
        return;
    };
    let fwd = me.rot * Vec3::NEG_Z;
    let score = |o: &voxwing_client_core::CoreShip| fwd.angle_between(o.pos - me.pos) + me.pos.distance(o.pos) / 3000.0;
    let mut foes: Vec<_> = s.core.ships.values().filter(|o| o.team != me.team).collect();
    foes.sort_by(|a, b| score(a).total_cmp(&score(b)));
    let valid = s.target.is_some_and(|t| foes.iter().any(|o| o.id == t));
    if !valid {
        s.target = foes.first().map(|o| o.id);
        s.lock = 0.0;
    }
    if keys.just_pressed(KeyCode::KeyT) && !foes.is_empty() {
        let i = foes.iter().position(|o| Some(o.id) == s.target).map_or(0, |i| (i + 1) % foes.len());
        s.target = Some(foes[i].id);
        s.lock = 0.0;
    }
    let was = s.locked;
    match (me.body.has_missile, s.target.and_then(|t| s.core.ships.get(&t))) {
        (true, Some(t)) => {
            let to = t.pos - me.pos;
            if fwd.angle_between(to) < 0.3 && to.length() < 950.0 {
                s.lock = (s.lock + dt).min(hud::LOCK_T);
            } else {
                s.lock = (s.lock - dt * 2.0).max(0.0);
            }
        }
        _ => s.lock = 0.0,
    }
    s.locked = s.lock >= hud::LOCK_T;
    if s.locked && !was {
        sounds.play(&mut commands, time.elapsed_secs_f64(), Sfx::Lock, 0.5, 0.0);
    }
}

/// Spectating: pick a ship to ride with, switch with ← / → (or A / D, or a click), and after the
/// watched ship dies, move on to whoever shot it down.
fn spectate(keys: Res<ButtonInput<KeyCode>>, mouse: Res<ButtonInput<MouseButton>>, mut session: ResMut<Session>, time: Res<Time>, mut hud: ResMut<hud::Hud>) {
    let s = &mut *session;
    if !s.core.watching {
        return;
    }
    let mut pick = |s: &mut Session, id: Option<u16>| {
        if s.spectate != id {
            s.spectate = id;
            s.cam_init = false;
            hud.silo_dirty = true;
        }
    };
    if let Some((_, left, killer)) = s.wreck.as_mut() {
        *left -= time.delta_secs();
        if *left > 0.0 {
            return;
        }
        let next = killer.filter(|k| s.core.ships.contains_key(k));
        s.wreck = None;
        pick(s, next);
    }
    let order = watch_order(&s.core);
    if !s.spectate.is_some_and(|id| s.core.ships.contains_key(&id)) {
        pick(s, order.first().copied());
    }
    if s.paused || order.is_empty() {
        return;
    }
    let step = if keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::KeyD]) || mouse.just_pressed(MouseButton::Left) {
        1
    } else if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::KeyA]) {
        order.len() - 1
    } else {
        return;
    };
    let at = s.spectate.and_then(|id| order.iter().position(|&o| o == id)).unwrap_or(0);
    pick(s, Some(order[(at + step) % order.len()]));
}

/// What the camera needs from the ship it rides with, copied out so the session stays mutable.
#[derive(Clone, Copy)]
struct Rider {
    pos: Vec3,
    rot: Quat,
    radius: f32,
    cockpit: Option<Vec3>,
    speed: f32,
    boosting: bool,
}

impl Rider {
    fn of(o: &voxwing_client_core::CoreShip) -> Self {
        Rider { pos: o.pos, rot: o.rot, radius: o.body.radius, cockpit: o.body.cockpit.map(|c| o.body.world(o.pos, o.rot, c)), speed: o.vel.length(), boosting: o.boosting }
    }
}

/// The chase (or cockpit) camera on ship `o`; returns how much to widen the field of view.
fn ride(s: &mut Session, o: Rider, t: &mut Transform, dt: f32, side: bool) -> f32 {
    if !s.cam_init {
        s.cam_rot = o.rot;
        s.cam_init = true;
    }
    s.cam_rot = s.cam_rot.slerp(o.rot, 1.0 - (-dt * 7.0).exp());
    if side {
        // dev view: beside the ship, looking back along its exhaust
        let r = o.radius;
        t.translation = o.pos + o.rot * Vec3::new(r * 2.0 + 16.0, 3.0, 6.0);
        t.look_at(o.pos + o.rot * Vec3::Z * 14.0, o.rot * Vec3::Y);
    } else if s.cockpit_cam && let Some(eye) = o.cockpit {
        t.translation = eye + o.rot * Vec3::Y * 0.75;
        t.rotation = o.rot;
    } else {
        let r = o.radius;
        t.translation = o.pos + s.cam_rot * Vec3::new(0.0, r * 0.5 + 2.4, r * 1.9 + 7.5);
        t.rotation = s.cam_rot * Quat::from_rotation_x(-0.06);
    }
    (o.speed / 120.0).clamp(0.0, 1.0) * 8.0 + if o.boosting { 8.0 } else { 0.0 }
}

fn camera_follow(mut session: ResMut<Session>, cam: Single<(&mut Transform, &mut Projection), With<MainCamera>>, time: Res<Time>, keys: Res<ButtonInput<KeyCode>>, dev: Res<DevOpts>) {
    let dt = time.delta_secs();
    let s = &mut *session;
    let (mut t, mut proj) = cam.into_inner();
    if keys.just_pressed(KeyCode::KeyC) {
        s.cockpit_cam = !s.cockpit_cam;
    }
    let mut fov = 66.0;
    let side = dev.forced("side", time.elapsed_secs());
    if let Some(v) = s.view().map(Rider::of) {
        // your own ship, or the one you are spectating: the same camera either way
        fov += ride(s, v, &mut t, dt, side);
    } else if let Some((p, left, _)) = s.wreck {
        // the watched ship went down: circle the wreck until the cut to its killer
        let a = (WRECK_LINGER - left) * 0.5;
        let r = 34.0;
        t.translation = p + Vec3::new(a.sin() * r, r * 0.4, a.cos() * r);
        t.look_at(p, Vec3::Y);
    } else if let Some((p, a)) = s.death.as_mut() {
        *a += dt * 0.25;
        let r = 40.0;
        t.translation = *p + Vec3::new(a.sin() * r, r * 0.35, a.cos() * r);
        t.look_at(*p, Vec3::Y);
    } else {
        // waiting to spawn: watch a teammate or the action
        let pick = s.spectate.filter(|id| s.core.ships.contains_key(id)).or_else(|| s.core.ships.keys().next().copied());
        s.spectate = pick;
        if let Some(o) = pick.and_then(|id| s.core.ships.get(&id)) {
            let want = o.pos + o.rot * Vec3::new(0.0, o.body.radius + 6.0, o.body.radius * 2.5 + 14.0);
            t.translation = t.translation.lerp(want, (dt * 3.0).min(1.0));
            t.look_at(o.pos, Vec3::Y);
        } else {
            t.translation = Vec3::new(0.0, 60.0, 700.0);
            t.look_at(Vec3::ZERO, Vec3::Y);
        }
    }
    if s.shake > 0.01 {
        let k = s.shake * 0.4;
        t.translation += Vec3::new(rand_f() - 0.5, rand_f() - 0.5, rand_f() - 0.5) * k;
        s.shake *= (-dt * 5.0).exp();
    }
    if let Projection::Perspective(p) = &mut *proj {
        let cur = p.fov.to_degrees();
        p.fov = (cur + (fov - cur) * (dt * 3.0).min(1.0)).to_radians();
    }
}

fn rand_f() -> f32 {
    use std::cell::Cell;
    thread_local!(static S: Cell<u32> = const { Cell::new(0x1234_5678) });
    S.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        s.set(x);
        (x >> 8) as f32 / (1u32 << 24) as f32
    })
}

fn flight_actions(
    keys: Res<ButtonInput<KeyCode>>,
    mut msgs: MessageReader<UiAction>,
    mut session: ResMut<Session>,
    mut next: ResMut<NextState<Screen>>,
    overlays: Res<Overlays>,
    modal: Res<ModalReq>,
    mut focus: ResMut<InputFocus>,
    dev: Res<crate::DevOpts>,
    time: Res<Time>,
    mut forced: Local<bool>,
) {
    if !*forced && session.core.state == ConnState::InGame && dev.forced("pause", time.elapsed_secs()) {
        *forced = true;
        session.paused = true;
    }
    if keys.just_pressed(KeyCode::Escape) && !overlays.any(&modal) {
        if session.core.state == ConnState::Connecting {
            next.set(Screen::Lobby);
        } else {
            session.paused = !session.paused;
            if !session.paused {
                focus.clear();
            }
        }
    }
    for m in msgs.read() {
        match m {
            UiAction::Resume => {
                session.paused = false;
                focus.clear();
            }
            UiAction::LeaveFlight | UiAction::CancelConnect => next.set(Screen::Lobby),
            _ => {}
        }
    }
}

fn engine_hum(session: Res<Session>, sounds: Res<Sounds>, mut sinks: Query<&mut AudioSink, With<Hum>>) {
    for mut sink in &mut sinks {
        let (v, sp) = match session.view() {
            Some(m) if sounds.enabled && !session.paused => (0.08 + m.throttle * 0.14 + if m.boosting { 0.08 } else { 0.0 }, 0.8 + m.throttle * 0.5 + if m.boosting { 0.3 } else { 0.0 }),
            _ => (0.0, 1.0),
        };
        sink.set_volume(Volume::Linear(v));
        sink.set_speed(sp);
    }
}
