//! The battle: drives the client core each frame and presents its state.

pub mod arena;
pub mod env;
pub mod exhaust;
pub mod fx;
pub mod gadgets;
pub mod ground;
pub mod hud;
pub mod radar;
pub mod ships;

use crate::audio::{Sfx, Sounds};
use crate::settings::Settings;
use crate::screens::Overlays;
use crate::ui::UiAction;
use crate::ui::kit::ModalReq;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input_focus::InputFocus;
use crate::{DevOpts, LastError, MainCamera, Screen};
use bevy::audio::Volume;
use bevy::light::Skybox;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use blocktread_client_core::{ClientCore, ConnState, Phase};
use blocktread_shared::protocol::Mode;
use blocktread_shared::ai::{AiPilot, Contact, Skill};
use blocktread_shared::flight::{BTN_BOOST, BTN_FIRE, BTN_FLIP, BTN_FLIPPER, BTN_JUMP, BTN_MINE, BTN_MISSILE, BTN_SMOKE, InputCmd, NO_LOCK, q_axis, quantize_aim};

/// A connection to a server, from the mode select until the player goes back to it: it lives
/// through matchmaking, any number of matches and their results.
#[derive(Resource)]
pub struct Session {
    pub core: ClientCore,
    pub addr: String,
    /// The queue to join once connected.
    pub want: Option<(Mode, Option<String>)>,
    pub killer: Option<String>,
    pub target: Option<u16>,
    pub lock: f32,
    pub locked: bool,
    /// Smoke hangs between us and the target: no lock through it.
    pub veiled: bool,
    pub throttle: f32,
    pub cockpit_cam: bool,
    /// What the crosshair is on (world point): the guns turn to it.
    pub aim: Vec3,
    /// Where the camera looks, in world angles: yaw round the vertical, pitch (rad).
    look: Vec2,
    pub cam_init: bool,
    /// How much of its full distance the camera keeps from the vehicle: less while a wall would
    /// come in between.
    cam_pull: f32,
    pub paused: bool,
    pub death: Option<(Vec3, f32)>,
    pub shake: f32,
    ai: Option<AiPilot>,
    drive: f32,
    steer: f32,
    buttons: u8,
    /// The ship the camera rides with while spectating (or waiting to respawn).
    pub spectate: Option<u16>,
    /// Spectating: the watched ship just died. The camera circles its wreck for a moment, then
    /// moves on to the killer: (wreck, seconds left, killer).
    pub wreck: Option<(Vec3, f32, Option<u16>)>,
}

impl Session {
    pub fn new(core: ClientCore, addr: String, want: Option<(Mode, Option<String>)>) -> Self {
        Session {
            core,
            addr,
            want,
            killer: None,
            target: None,
            lock: 0.0,
            locked: false,
            veiled: false,
            throttle: 0.7,
            cockpit_cam: false,
            aim: Vec3::NEG_Z * 300.0,
            look: Vec2::ZERO,
            cam_init: false,
            cam_pull: 1.0,
            paused: false,
            death: None,
            shake: 0.0,
            ai: None,
            drive: 0.0,
            steer: 0.0,
            buttons: 0,
            spectate: None,
            wreck: None,
        }
    }

    /// Forget the last flight (camera, target, death) before the next match.
    fn reset_flight(&mut self) {
        self.killer = None;
        self.target = None;
        self.lock = 0.0;
        self.locked = false;
        self.veiled = false;
        self.throttle = 0.7;
        self.cam_init = false;
        self.paused = false;
        self.death = None;
        self.shake = 0.0;
        self.ai = None;
        self.spectate = None;
        self.wreck = None;
    }

    /// The ship being watched, while spectating and not between ships.
    pub fn watched(&self) -> Option<&blocktread_client_core::CoreShip> {
        if !self.core.watching || self.wreck.is_some() {
            return None;
        }
        self.spectate.and_then(|id| self.core.ships.get(&id))
    }

    /// The ship whose instruments the HUD shows: your own, or the one you are watching.
    pub fn view(&self) -> Option<&blocktread_client_core::CoreShip> {
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

/// The jet engine loop, and the afterburner loop over it.
#[derive(Component)]
struct Hum;
#[derive(Component)]
struct Burner;

/// The gadgets' keys.
const GADGETS: u8 = BTN_JUMP | BTN_MINE | BTN_SMOKE;

pub struct FlightPlugin;

impl Plugin for FlightPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((exhaust::ExhaustPlugin, radar::RadarPlugin))
            .init_resource::<ships::ShipVisuals>()
            .init_resource::<hud::Hud>()
            .add_systems(Startup, setup_assets)
            .add_systems(OnEnter(Screen::Flight), (enter_flight, hud::spawn_hud))
            .add_systems(OnExit(Screen::Flight), exit_flight)
            // back in the lobby the session ends, unless it is queueing there
            .add_systems(OnEnter(Screen::Menu), end_session)
            .add_systems(OnEnter(Screen::Hangar), end_session)
            .add_systems(Update, step_core.run_if(|s: Res<State<Screen>>| matches!(s.get(), Screen::Menu | Screen::Match)).run_if(resource_exists::<Session>))
            .add_systems(
                Update,
                (read_pilot, step_core, ships::apply_events, ships::sync_ships, targeting, spectate, camera_follow, aim_at, exhaust::update_exhaust, fx::update_fx, env::orient_rocks, (arena::update_traps, arena::update_pads, arena::update_hill_light, gadgets::sync_mines, gadgets::sync_smoke), hud::update_hud, radar::update_radar_map, hud::update_vignettes, hud::flight_overlays, flight_actions, engine_hum, grab_cursor, hud::dev_hide_hud)
                    .chain()
                    .run_if(in_state(Screen::Flight))
                    .run_if(resource_exists::<Session>),
            );
    }
}

fn setup_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(fx::Fx::new(&mut meshes, &mut mats));
    commands.insert_resource(gadgets::GadgetArt::new(&mut meshes, &mut mats));
    commands.insert_resource(ships::flame_mats(&mut mats));
    commands.insert_resource(ground::DaySky(ground::build_day_sky(&mut images)));
    commands.insert_resource(ground::build_stage(&mut meshes, &mut mats));
}

#[allow(clippy::too_many_arguments)]
fn enter_flight(
    mut commands: Commands,
    sky: Res<ground::DaySky>,
    cam: Single<(Entity, &mut Projection), With<MainCamera>>,
    dev: Res<DevOpts>,
    mut session: Option<ResMut<Session>>,
    sounds: Res<Sounds>,
    mut focus: ResMut<InputFocus>,
) {
    focus.clear();
    let (ce, mut proj) = cam.into_inner();
    commands.entity(ce).insert((Skybox { image: Some(sky.0.clone()), brightness: ground::SKY_BRIGHTNESS, ..default() }, ground::fog()));
    if let Projection::Perspective(p) = &mut *proj {
        p.fov = 68f32.to_radians();
    }
    ground::spawn_daylight(&mut commands, Screen::Flight);
    if let Some(s) = session.as_mut() {
        s.reset_flight();
        if dev.autopilot {
            s.ai = Some(AiPilot::new(Skill { aim_err: 0.012, cone: 0.05, react: 7.0, dmg_mul: 1.0 }, 1));
        }
    }
    if sounds.enabled {
        commands.spawn((AudioPlayer(sounds.hum.clone()), PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0)), Hum, DespawnOnExit(Screen::Flight)));
        commands.spawn((AudioPlayer(sounds.burner.clone()), PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0)), Burner, DespawnOnExit(Screen::Flight)));
    }
}

fn exit_flight(mut commands: Commands, mut vis: ResMut<ships::ShipVisuals>, cam: Single<Entity, With<MainCamera>>, mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>) {
    vis.0.clear();
    commands.entity(*cam).remove::<DistanceFog>();
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
}

/// While driving, the mouse turns the camera: the cursor is hidden and held. Paused, spectating
/// with a menu open, or anywhere else, it is free.
fn grab_cursor(session: Res<Session>, overlays: Res<Overlays>, modal: Res<ModalReq>, mut cursor: Single<&mut CursorOptions, With<PrimaryWindow>>) {
    let held = !session.paused && session.core.state == ConnState::InGame && !overlays.any(&modal);
    let (grab, visible) = if held { (CursorGrabMode::Locked, false) } else { (CursorGrabMode::None, true) };
    if cursor.grab_mode != grab || cursor.visible != visible {
        cursor.grab_mode = grab;
        cursor.visible = visible;
    }
}

fn end_session(mut commands: Commands, session: Option<Res<Session>>, screen: Res<State<Screen>>, mut toasts: ResMut<crate::ui::kit::Toasts>) {
    let Some(s) = session else { return };
    let queueing = !s.core.watching && (matches!(s.core.phase, Phase::Queued { .. }) || s.want.is_some());
    if queueing && *screen.get() == Screen::Menu {
        return;
    }
    if queueing {
        toasts.push("已取消匹配", crate::ui::kit::ToastKind::Info);
    }
    s.core.leave();
    commands.remove_resource::<Session>();
}

fn read_pilot(keys: Res<ButtonInput<KeyCode>>, mouse: Res<ButtonInput<MouseButton>>, mut session: ResMut<Session>, dev: Res<DevOpts>, time: Res<Time>) {
    let s = &mut *session;
    if s.paused || s.core.watching {
        s.buttons = 0;
        s.drive = 0.0;
        s.steer = 0.0;
        return;
    }
    let axis = |pos: [KeyCode; 2], neg: [KeyCode; 2]| (keys.any_pressed(pos) as i32 - keys.any_pressed(neg) as i32) as f32;
    s.drive = axis([KeyCode::KeyW, KeyCode::ArrowUp], [KeyCode::KeyS, KeyCode::ArrowDown]);
    s.steer = axis([KeyCode::KeyD, KeyCode::ArrowRight], [KeyCode::KeyA, KeyCode::ArrowLeft]);
    s.throttle = s.drive.abs();
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
    if keys.pressed(KeyCode::KeyR) {
        b |= BTN_FLIP;
    }
    // the flipper and the gadgets, each on its own key
    for (k, btn) in [(KeyCode::KeyE, BTN_FLIPPER), (KeyCode::KeyQ, BTN_JUMP), (KeyCode::KeyG, BTN_MINE), (KeyCode::KeyX, BTN_SMOKE)] {
        if keys.pressed(k) {
            b |= btn;
        }
    }
    // for screenshots: every gadget, as often as it goes
    if dev.forced("gadgets", time.elapsed_secs()) {
        b |= GADGETS;
    }
    s.buttons = b;
}

/// Run the client core one frame (in flight and on the match screen); leave flight when the match
/// is over, and the session when the connection fails.
fn step_core(mut commands: Commands, mut session: ResMut<Session>, time: Res<Time>, screen: Res<State<Screen>>, mut next: ResMut<NextState<Screen>>, mut last_err: ResMut<LastError>) {
    let dt = time.delta_secs().min(0.1);
    let s = &mut *session;
    let (drive, steer, buttons, locked, target) = (s.drive, s.steer, s.buttons, s.locked, s.target);
    let aim = quantize_aim(s.aim);
    let mut ai = s.ai.take();
    s.core.update(dt, &mut |c: &ClientCore, tick| {
        if let Some(ai) = ai.as_mut()
            && let (Some(p), Some(me)) = (c.pred.as_ref(), c.my())
        {
            let others: Vec<Contact> = c.ships.values().filter(|o| Some(o.id) != c.my_ship).map(|o| Contact { id: o.id, team: o.team, pos: o.pos, vel: o.vel, radius: o.body.radius }).collect();
            // the autopilot drives; the gadget keys still work
            let mut cmd = ai.think(&p.kin, &me.body, 0.0, me.team, &others, &c.rocks, &c.terrain, tick);
            cmd.buttons |= buttons & GADGETS;
            return cmd;
        }
        let mut cmd = InputCmd { drive: q_axis(drive), steer: q_axis(steer), buttons, aim, ..Default::default() };
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
        if *screen.get() == Screen::Menu {
            commands.remove_resource::<Session>();
        } else {
            next.set(Screen::Menu);
        }
    } else if *screen.get() == Screen::Flight && matches!(s.core.phase, Phase::Over { .. }) {
        next.set(Screen::Match);
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
    // the target is the one nearest where you look (the crosshair), not where the hull points
    let look = (s.aim - me.pos).normalize_or(me.rot * Vec3::NEG_Z);
    let score = |o: &blocktread_client_core::CoreShip| look.angle_between(o.pos - me.pos) + me.pos.distance(o.pos) / 3000.0;
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
    s.veiled = false;
    match (me.body.has_missile, s.target.and_then(|t| s.core.ships.get(&t))) {
        // smoke in the way: the lock is lost at once, and can't be had through it
        (true, Some(t)) if s.core.smoke_between(me.pos + Vec3::Y * 1.5, t.pos) => {
            s.veiled = true;
            s.lock = 0.0;
        }
        (true, Some(t)) => {
            let to = t.pos - me.pos;
            if look.angle_between(to) < 0.3 && to.length() < 950.0 {
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
    fn of(o: &blocktread_client_core::CoreShip) -> Self {
        Rider { pos: o.pos, rot: o.rot, radius: o.body.radius, cockpit: o.body.cockpit.map(|c| o.body.world(o.pos, o.rot, c)), speed: o.vel.length(), boosting: o.boosting }
    }
}

/// The camera on vehicle `o`: orbiting it where the mouse turns it, or in its cockpit. Watching
/// someone else drive (spectating, the autopilot) it swings round behind the vehicle by itself.
/// Returns how much to widen the field of view.
fn ride(s: &mut Session, o: Rider, t: &mut Transform, dt: f32, side: bool) -> f32 {
    let f = o.rot * Vec3::NEG_Z;
    let behind = (-f.x).atan2(-f.z);
    if !s.cam_init {
        // start behind the vehicle, looking a little down on it
        s.look = Vec2::new(behind, -0.18);
        s.cam_init = true;
    }
    if s.core.watching || s.ai.is_some() {
        let turn = (behind - s.look.x + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        s.look.x += turn * (1.0 - (-dt * 2.5).exp());
    }
    let look = Quat::from_rotation_y(s.look.x) * Quat::from_rotation_x(s.look.y);
    if side {
        // dev view: beside the vehicle, from the right
        let r = o.radius;
        t.translation = o.pos + o.rot * Vec3::new(r * 2.0 + 8.0, 2.0, 0.0);
        t.look_at(o.pos, Vec3::Y);
    } else if s.cockpit_cam && let Some(eye) = o.cockpit {
        t.translation = eye + o.rot * Vec3::Y * 0.75;
        t.rotation = look;
    } else {
        // the pivot sits above the vehicle, so it rides in the lower part of the view
        let r = o.radius;
        let pivot = o.pos + Vec3::Y * (1.5 + r * 0.25);
        let dist = r * 1.7 + 6.0;
        let back = look * Vec3::Z;
        // a wall or a crate in between: come in in front of it at once, and ease back out
        let ground = &s.core.terrain;
        let end = pivot + back * dist;
        let clear = ground.solids_near((pivot + end) * 0.5, dist * 0.5).filter_map(|w| w.clip(pivot, end)).map(|(f, _)| ((f * dist - 0.8) / dist).max(1.5 / dist)).fold(1.0, f32::min);
        s.cam_pull = if clear < s.cam_pull { clear } else { s.cam_pull + (clear - s.cam_pull) * (1.0 - (-dt * 2.0).exp()) };
        t.translation = pivot + back * dist * s.cam_pull;
        t.rotation = look;
    }
    (o.speed / 30.0).clamp(0.0, 1.0) * 6.0 + if o.boosting { 6.0 } else { 0.0 }
}

#[allow(clippy::too_many_arguments)]
fn camera_follow(
    mut session: ResMut<Session>,
    cam: Single<(&mut Transform, &mut Projection), With<MainCamera>>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    settings: Res<Settings>,
    cursor: Single<&CursorOptions, With<PrimaryWindow>>,
    dev: Res<DevOpts>,
) {
    let dt = time.delta_secs();
    let s = &mut *session;
    let (mut t, mut proj) = cam.into_inner();
    if keys.just_pressed(KeyCode::KeyC) {
        s.cockpit_cam = !s.cockpit_cam;
    }
    if cursor.grab_mode == CursorGrabMode::Locked {
        let k = 0.0025 * settings.sensitivity.clamp(0.3, 3.0);
        let invert = if settings.invert_y { -1.0 } else { 1.0 };
        s.look.x -= motion.delta.x * k;
        s.look.y = (s.look.y - motion.delta.y * k * invert).clamp(-0.8, 0.35);
    }
    let mut fov = 66.0;
    let side = dev.forced("side", time.elapsed_secs());
    if dev.forced("overview", time.elapsed_secs()) {
        // dev view: the whole arena from above one end
        t.translation = Vec3::new(0.0, 125.0, 165.0);
        t.look_at(Vec3::new(0.0, 0.0, 12.0), Vec3::Y);
    } else if dev.forced("hillcam", time.elapsed_secs()) {
        // dev view: the hill from the south, past the overdrive pad before it
        t.translation = Vec3::new(16.0, 13.0, 50.0);
        t.look_at(Vec3::new(0.0, 2.0, 12.0), Vec3::Y);
    } else if let Some(v) = s.view().map(Rider::of) {
        // your own vehicle, or the one you are spectating: the same camera either way
        fov += ride(s, v, &mut t, dt, side);
    } else if let Some((p, left, _)) = s.wreck {
        // the watched vehicle went down: circle the wreck until the cut to its killer
        let a = (WRECK_LINGER - left) * 0.5;
        let r = 24.0;
        t.translation = p + Vec3::new(a.sin() * r, r * 0.5, a.cos() * r);
        t.look_at(p, Vec3::Y);
    } else if let Some((p, a)) = s.death.as_mut() {
        *a += dt * 0.25;
        let r = 28.0;
        t.translation = *p + Vec3::new(a.sin() * r, r * 0.45, a.cos() * r);
        t.look_at(*p, Vec3::Y);
    } else {
        // waiting to spawn: watch a teammate or the action
        let pick = s.spectate.filter(|id| s.core.ships.contains_key(id)).or_else(|| s.core.ships.keys().next().copied());
        s.spectate = pick;
        if let Some(o) = pick.and_then(|id| s.core.ships.get(&id)) {
            let want = o.pos + o.rot * Vec3::new(0.0, o.body.radius + 5.0, o.body.radius * 2.5 + 10.0);
            t.translation = t.translation.lerp(want, (dt * 3.0).min(1.0));
            t.look_at(o.pos, Vec3::Y);
        } else {
            t.translation = if s.core.terrain.arena.is_some() { Vec3::new(0.0, 70.0, 175.0) } else { Vec3::new(0.0, 120.0, 420.0) };
            t.look_at(Vec3::ZERO, Vec3::Y);
        }
    }
    // never under the ground
    let g = s.core.terrain.height(t.translation.x, t.translation.z) + 1.2;
    if t.translation.y < g {
        t.translation.y = g;
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

/// What the crosshair, in the middle of the screen, is on: the first enemy, rock, wall or patch of
/// ground along the camera's line of sight, else a point far down it. The guns turn to it.
fn aim_at(mut session: ResMut<Session>, cam: Single<&GlobalTransform, With<MainCamera>>) {
    let (o, d) = (cam.translation(), cam.forward().as_vec3());
    let s = &mut *session;
    let core = &s.core;
    let sphere = |c: Vec3, r: f32| {
        let b = (c - o).dot(d);
        let q = (c - o).length_squared() - b * b;
        (b > 0.0 && q < r * r).then(|| b - (r * r - q).sqrt())
    };
    let mut best: f32 = 700.0;
    for sh in core.ships.values().filter(|sh| Some(sh.id) != core.my_ship) {
        if let Some(t) = sphere(sh.pos, sh.body.radius * 0.75) {
            best = best.min(t);
        }
    }
    for r in &core.rocks {
        if let Some(t) = sphere(r.pos, r.r * 0.7) {
            best = best.min(t);
        }
    }
    // the arena's walls and crates
    let far = best;
    for w in &core.terrain.solids {
        if let Some((f, _)) = w.clip(o, o + d * far)
            && f > 0.0
        {
            best = best.min(f * far);
        }
    }
    // the ground: march along the line, then halve in on where it goes under
    let ground = &core.terrain;
    let below = |t: f32| {
        let p = o + d * t;
        p.y < ground.height(p.x, p.z)
    };
    let (mut a, mut t) = (0.0, 3.0);
    while t < best {
        if below(t) {
            let mut b = t;
            for _ in 0..8 {
                let mid = (a + b) / 2.0;
                if below(mid) { b = mid } else { a = mid }
            }
            best = b;
            break;
        }
        a = t;
        t += 3.0;
    }
    s.aim = o + d * best;
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
            next.set(Screen::Menu);
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
            UiAction::LeaveFlight => next.set(Screen::Menu),
            _ => {}
        }
    }
}

/// Engine sound for the ship in view: the jet spools up in level and a little in pitch with the
/// throttle, and under boost the afterburner fades in over it after its ignition. Everything is
/// eased, so a change of throttle or boost never jumps.
#[allow(clippy::type_complexity)]
fn engine_hum(
    mut commands: Commands,
    session: Res<Session>,
    mut sounds: ResMut<Sounds>,
    time: Res<Time>,
    mut hums: Query<&mut AudioSink, (With<Hum>, Without<Burner>)>,
    mut burners: Query<&mut AudioSink, (With<Burner>, Without<Hum>)>,
    // eased level, pitch and afterburner, and whether the ship was boosting last frame
    mut eased: Local<(f32, f32, f32, bool)>,
) {
    let dt = time.delta_secs();
    let (level, pitch, burn, boosting) = match session.view() {
        Some(m) if sounds.enabled && !session.paused => {
            let b = if m.boosting { 1.0 } else { 0.0 };
            (0.45 + 0.55 * m.throttle, 0.9 + 0.2 * m.throttle + 0.05 * b, b, m.boosting)
        }
        _ => (0.0, 1.0, 0.0, false),
    };
    let e = &mut *eased;
    let k = 1.0 - (-dt * 5.0).exp();
    e.0 += (level - e.0) * k;
    e.1 += (pitch - e.1) * k;
    // the afterburner lights fast and dies away slower
    let kb = 1.0 - (-dt * if burn > e.2 { 9.0 } else { 3.0 }).exp();
    e.2 += (burn - e.2) * kb;
    if boosting && !e.3 {
        sounds.play(&mut commands, time.elapsed_secs_f64(), crate::audio::Sfx::Ignite, 0.8, 0.4);
    }
    e.3 = boosting;
    for mut sink in &mut hums {
        sink.set_volume(Volume::Linear(e.0 * crate::audio::ENGINE_GAIN * sounds.volume));
        sink.set_speed(e.1);
    }
    for mut sink in &mut burners {
        sink.set_volume(Volume::Linear(e.2 * crate::audio::BURNER_GAIN * sounds.volume));
    }
}
