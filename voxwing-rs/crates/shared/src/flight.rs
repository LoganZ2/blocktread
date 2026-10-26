use crate::analysis::Stats;
use crate::ship::ShipBody;
use crate::world::Asteroid;
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Everything the vehicle model integrates. This is what client prediction rewinds and replays.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Kin {
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    /// Angular velocity in world axes (rad/s).
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
            vel: Vec3::ZERO,
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
/// Held: turn the vehicle back onto its wheels.
pub const BTN_FLIP: u8 = 8;
pub const NO_LOCK: u16 = u16::MAX;

/// One tick of player intent. Quantised so that client prediction and the server feed the
/// vehicle model bit-identical inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCmd {
    pub tick: u32,
    /// Forward (+) or reverse (−).
    pub drive: i8,
    /// Steer right (+) or left (−).
    pub steer: i8,
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
        InputCmd { tick: 0, drive: 0, steer: 0, buttons: 0, aim_dist: 320, view_lag: 0, lock: NO_LOCK }
    }
}

pub fn q_axis(v: f32) -> i8 {
    (v.clamp(-1.0, 1.0) * 127.0).round() as i8
}

impl InputCmd {
    pub fn drive_f(&self) -> f32 {
        self.drive as f32 / 127.0
    }
    pub fn steer_f(&self) -> f32 {
        self.steer as f32 / 127.0
    }
    /// How hard the engine works, 0..1 (sound, snapshots).
    pub fn throttle_f(&self) -> f32 {
        self.drive_f().abs()
    }
    pub fn has(&self, b: u8) -> bool {
        self.buttons & b != 0
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
