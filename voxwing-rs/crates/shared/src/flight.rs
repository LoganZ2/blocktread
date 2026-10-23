use crate::ARENA_R;
use crate::analysis::Stats;
use crate::ship::ShipBody;
use crate::world::Asteroid;
use glam::{EulerRot, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Everything the flight model integrates. This is what client prediction rewinds and replays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kin {
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    /// Current angular rates: x = pitch, y = yaw, z = roll (rad/s).
    pub ang: Vec3,
    pub energy: f32,
    pub shield: f32,
    pub shield_delay: f32,
    pub boosting: bool,
}

impl Kin {
    pub fn new(pos: Vec3, rot: Quat, stats: &Stats) -> Self {
        Kin {
            pos,
            rot,
            vel: rot * Vec3::NEG_Z * 30.0,
            ang: Vec3::ZERO,
            energy: stats.energy_cap,
            shield: stats.shield_cap,
            shield_delay: 0.0,
            boosting: false,
        }
    }
    pub fn fwd(&self) -> Vec3 {
        self.rot * Vec3::NEG_Z
    }
    pub fn up(&self) -> Vec3 {
        self.rot * Vec3::Y
    }
}

pub const BTN_FIRE: u8 = 1;
pub const BTN_MISSILE: u8 = 2;
pub const BTN_BOOST: u8 = 4;
pub const NO_LOCK: u16 = u16::MAX;

/// One tick of player intent. Quantised so that client prediction and the server feed the
/// flight model bit-identical inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCmd {
    pub tick: u32,
    pub pitch: i8,
    pub yaw: i8,
    pub roll: i8,
    pub throttle: u8,
    pub buttons: u8,
    /// Gun convergence distance in metres.
    pub aim_dist: u16,
    /// How many ticks behind the server the shooter was seeing other ships (lag compensation).
    pub view_lag: u8,
    /// Ship id of the missile lock target, or NO_LOCK.
    pub lock: u16,
}

impl Default for InputCmd {
    fn default() -> Self {
        InputCmd { tick: 0, pitch: 0, yaw: 0, roll: 0, throttle: 180, buttons: 0, aim_dist: 320, view_lag: 0, lock: NO_LOCK }
    }
}

pub fn q_axis(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

impl InputCmd {
    pub fn pitch_f(&self) -> f32 {
        self.pitch as f32 / 127.0
    }
    pub fn yaw_f(&self) -> f32 {
        self.yaw as f32 / 127.0
    }
    pub fn roll_f(&self) -> f32 {
        self.roll as f32 / 127.0
    }
    pub fn throttle_f(&self) -> f32 {
        self.throttle as f32 / 255.0
    }
    pub fn has(&self, b: u8) -> bool {
        self.buttons & b != 0
    }
}

/// Engine output while boosting, relative to full throttle.
pub const BOOST_PUSH: f32 = 1.8;

/// Advance one ship by one tick. `turn_bonus` gives human pilots a small edge over bots.
pub fn step_flight(k: &mut Kin, st: &Stats, inp: &InputCmd, dt: f32, turn_bonus: f32) {
    k.boosting = inp.has(BTN_BOOST) && k.energy > 2.0 && st.thrust > 0.0;
    let lift_pen = if st.lift_ratio < 1.0 { st.lift_ratio.max(0.3) } else { 1.0 };
    let steer = lift_pen * turn_bonus;
    // engines off the centre-of-mass line pull the nose, harder the more they push
    let push = if st.thrust > 0.0 { inp.throttle_f() * if k.boosting { BOOST_PUSH } else { 1.0 } } else { 0.0 };
    let drift = st.thrust_drift(push);
    let resp = (dt * 5.0).min(1.0);
    k.ang.x += (inp.pitch_f() * st.pitch_rate * steer + drift.x - k.ang.x) * resp;
    k.ang.y += (inp.yaw_f() * st.yaw_rate * steer + drift.y - k.ang.y) * resp;
    k.ang.z += (inp.roll_f() * st.roll_rate * lift_pen.max(0.4) + st.roll_drift - k.ang.z) * resp;
    let dq = Quat::from_euler(EulerRot::XYZ, k.ang.x * dt, -k.ang.y * dt, -k.ang.z * dt);
    k.rot = (k.rot * dq).normalize();
    let fwd = k.fwd();
    if k.boosting {
        k.energy -= 16.0 * dt;
    }
    let vf = k.vel.dot(fwd);
    let mut lat = k.vel - fwd * vf;
    let mut nvf = vf;
    if st.thrust > 0.0 {
        let acc = st.accel * if k.boosting { BOOST_PUSH } else { 1.0 };
        let tgt = inp.throttle_f() * st.max_speed * if k.boosting { 1.6 } else { 1.0 };
        nvf += (tgt - vf).clamp(-acc * 1.2 * dt, acc * dt);
        lat *= (-2.4 * dt).exp();
    } else {
        nvf *= (-0.15 * dt).exp();
        lat *= (-0.2 * dt).exp();
    }
    k.vel = fwd * nvf + lat;
    let d = k.pos.length();
    if d > ARENA_R {
        k.vel += k.pos / d * (-60.0 * dt * (1.0 + (d - ARENA_R) / 100.0));
    }
    k.pos += k.vel * dt;
    k.energy = (k.energy + st.energy_regen * dt).min(st.energy_cap);
    if k.shield_delay > 0.0 {
        k.shield_delay -= dt;
    } else {
        k.shield = (k.shield + st.shield_regen * dt).min(st.shield_cap);
    }
}

/// Rotation whose forward (-Z) points from `from` towards `to`.
pub fn look_rotation(from: Vec3, to: Vec3, up: Vec3) -> Quat {
    let f = (to - from).normalize_or(Vec3::NEG_Z);
    let r = f.cross(up).normalize_or(Vec3::X);
    let u = r.cross(f);
    Quat::from_mat3(&glam::Mat3::from_cols(r, u, -f)).normalize()
}

pub struct Impact {
    pub point: Vec3,
    pub speed: f32,
}

/// Push the ship out of any asteroid it overlaps (per block, not just its bounding sphere).
/// Returns hard impacts so the server can apply damage; prediction ignores them.
/// Push the ship out of any asteroid it overlaps at `t` seconds of server time (the rocks turn),
/// testing every live block against the rock's actual surface.
pub fn collide_asteroids(k: &mut Kin, body: &ShipBody, asteroids: &[Asteroid], t: f64) -> Option<Impact> {
    /// Blocks collide as spheres of this radius (a cube's faces are 0.5 from its centre).
    const BLOCK_R: f32 = 0.55;
    let mut out = None;
    for a in asteroids {
        if k.pos.distance(a.pos) > body.radius + a.r {
            continue;
        }
        let rot = a.rotation(t);
        let mut pen = 0.0;
        let mut contact = Vec3::ZERO;
        let mut n = Vec3::Y;
        for i in 0..body.alive.len() {
            if !body.alive[i] {
                continue;
            }
            let w = body.world(k.pos, k.rot, i);
            let (dist, normal) = a.surface(w, rot);
            let p = BLOCK_R - dist;
            if p > pen {
                pen = p;
                contact = w;
                n = normal;
            }
        }
        if pen <= 0.0 {
            continue;
        }
        k.pos += n * pen;
        let vn = k.vel.dot(n);
        if vn < 0.0 {
            k.vel -= n * vn * 1.5;
            if -vn > 6.0 {
                out = Some(Impact { point: contact, speed: -vn });
            }
        }
    }
    out
}
