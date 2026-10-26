use crate::blocks::{Block, BlockType};
use crate::flight::{BTN_FIRE, BTN_MISSILE, InputCmd, Kin};
use crate::grid::{Occupancy, raycast};
use crate::rng::hash01;
use crate::ship::ShipBody;
use glam::{IVec3, Vec3};

/// A projectile leaving a muzzle this tick.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub block: usize,
    pub weapon: BlockType,
    pub pos: Vec3,
    pub vel: Vec3,
}

/// How far a weapon turns: all the way round, and from a little below level to well above it
/// (radians). Its mount decides only where its plate sits; what it can point at is what its own
/// vehicle leaves clear.
const YAW: f32 = std::f32::consts::PI;
const PITCH_DOWN: f32 = -25.0 * std::f32::consts::PI / 180.0;
const PITCH_UP: f32 = 60.0 * std::f32::consts::PI / 180.0;
/// Directions are tested on this grid (radians between samples).
const STEP: f32 = 5.0 * std::f32::consts::PI / 180.0;
/// A barrel is this long; rounds leave its tip.
pub const BARREL: f32 = 1.4;
/// How far out a line of fire must stay clear of its own vehicle.
const CLEAR: f32 = 14.0;
/// A gun fires only while it points this close to its aim point (radians): one that is still
/// turning, or can't turn that far, holds its fire.
const ON_TARGET: f32 = 10.0 * std::f32::consts::PI / 180.0;

/// One weapon's mount: the face of its cell where its plate sits (towards the block holding it),
/// and which directions keep its barrel and its line of fire out of its own vehicle (sampled every
/// `STEP`).
#[derive(Clone, Debug)]
pub struct Mount {
    pub block: usize,
    /// Towards the block it is fixed to: its plate sits on that face.
    pub face: IVec3,
    yaws: usize,
    pitches: usize,
    /// Row-major over (pitch, yaw) samples.
    clear: Vec<bool>,
}

impl Mount {
    /// Whether the weapon can point at (yaw, pitch) without hitting its own vehicle.
    pub fn is_clear(&self, yaw: f32, pitch: f32) -> bool {
        let iy = (((wrap(yaw) + YAW) / STEP).round() as isize).clamp(0, self.yaws as isize - 1) as usize;
        let ip = (((pitch - PITCH_DOWN) / STEP).round() as isize).clamp(0, self.pitches as isize - 1) as usize;
        self.clear[ip * self.yaws + iy]
    }

    /// True when no direction is clear: the weapon is boxed in.
    pub fn boxed_in(&self) -> bool {
        !self.clear.contains(&true)
    }

    /// Where the weapon points when the vehicle enters: straight ahead, or the clear direction
    /// nearest it.
    pub fn rest(&self) -> (f32, f32) {
        let mut best = (0.0, 0.0);
        let mut gap = f32::INFINITY;
        for ip in 0..self.pitches {
            for iy in 0..self.yaws {
                let (yaw, pitch) = (-YAW + iy as f32 * STEP, PITCH_DOWN + ip as f32 * STEP);
                let g = yaw * yaw + pitch * pitch;
                if self.clear[ip * self.yaws + iy] && g < gap {
                    (best, gap) = ((yaw, pitch), g);
                }
            }
        }
        best
    }

    /// Whether turning from `from` by `dy` in yaw and to pitch `to`, both at the same rate, stays
    /// clear all the way.
    fn path_clear(&self, from: (f32, f32), dy: f32, to: f32) -> bool {
        let dp = to - from.1;
        let s = STEP * 0.5;
        let n = (dy.abs().max(dp.abs()) / s).ceil() as usize;
        (1..=n).all(|k| {
            let d = k as f32 * s;
            self.is_clear(from.0 + dy.clamp(-d, d), from.1 + dp.clamp(-d, d))
        })
    }

    /// One tick of turning from `now` toward `want`, at most `turn` on each axis. It goes the short
    /// way round unless only the long way gets there clear, and it never turns into its own
    /// vehicle: toward a direction it can't get to, it turns as far as is clear and stops there
    /// (no detours over or round the vehicle).
    fn turn(&self, now: (f32, f32), want: (f32, f32), turn: f32) -> (f32, f32) {
        let to = want.1.clamp(PITCH_DOWN, PITCH_UP);
        let short = wrap(want.0 - now.0);
        let long = short - std::f32::consts::TAU.copysign(short);
        let dy = if !self.path_clear(now, short, to) && self.path_clear(now, long, to) { long } else { short };
        let (y, p) = (dy.clamp(-turn, turn), (to - now.1).clamp(-turn, turn));
        // pointing into the vehicle already (it can't, short of a bug): any way out will do
        let stuck = !self.is_clear(now.0, now.1);
        // both axes if it can; else whichever one alone still gets closer
        [(y, p), (y, 0.0), (0.0, p)].into_iter().map(|(y, p)| (wrap(now.0 + y), now.1 + p)).find(|&(y, p)| stuck || self.is_clear(y, p)).unwrap_or(now)
    }
}

/// An angle folded onto -π..π.
fn wrap(a: f32) -> f32 {
    (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// The barrel's direction in the vehicle's frame at (yaw, pitch): yaw turns it right from straight
/// ahead (-z), pitch raises it.
pub fn barrel_dir(yaw: f32, pitch: f32) -> Vec3 {
    Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos())
}

/// The face a weapon at `at` is fixed by: standing on a block if it can, else backed onto one,
/// else held at a side, hung under one, or fixed by its front.
pub fn attach_face(occ: &Occupancy, at: IVec3) -> IVec3 {
    [IVec3::NEG_Y, IVec3::Z, IVec3::NEG_X, IVec3::X, IVec3::Y, IVec3::NEG_Z].into_iter().find(|&d| occ.get(at + d).is_some()).unwrap_or(IVec3::NEG_Y)
}

/// The mount of the weapon at `i`.
pub fn mount_of(blocks: &[Block], occ: &Occupancy, i: usize) -> Mount {
    let at = blocks[i].pos();
    let yaws = (2.0 * YAW / STEP).round() as usize;
    let pitches = ((PITCH_UP - PITCH_DOWN) / STEP).round() as usize + 1;
    let centre = at.as_vec3();
    let mut clear = vec![false; yaws * pitches];
    for ip in 0..pitches {
        for iy in 0..yaws {
            let (yaw, pitch) = (-YAW + iy as f32 * STEP, PITCH_DOWN + ip as f32 * STEP);
            let d = barrel_dir(yaw, pitch);
            // from the edge of its own cell out past the barrel and along the line of fire
            clear[ip * yaws + iy] = raycast(centre + d * 0.5, d, CLEAR, |c| occ.get(c).filter(|&j| j != i)).is_none();
        }
    }
    Mount { block: i, face: attach_face(occ, at), yaws, pitches, clear }
}

/// Every alive weapon's mount.
pub fn mounts(blocks: &[Block], alive: &[bool], occ: &Occupancy) -> Vec<Mount> {
    (0..blocks.len()).filter(|&i| alive[i] && blocks[i].ty.weapon().is_some()).map(|i| mount_of(blocks, occ, i)).collect()
}

/// Per-weapon cooldowns and where each weapon points (yaw, pitch), indexed by block index.
#[derive(Clone, Debug)]
pub struct WeaponState {
    pub cds: Vec<f32>,
    pub aim: Vec<(f32, f32)>,
    pub missile_cd: f32,
}

impl WeaponState {
    pub fn new(body: &ShipBody) -> Self {
        let n = body.alive.len();
        let mut aim = vec![(0.0, 0.0); n];
        for m in &body.mounts {
            aim[m.block] = m.rest();
        }
        WeaponState { cds: (0..n).map(|i| hash01(i as u32, 7) * 0.3).collect(), aim, missile_cd: 0.0 }
    }
}

/// Turn every weapon toward the aim point, then fire whatever is ready and points at it.
/// Cooldown jitter and spread come from `hash01(tick, block)` so the client predicts exactly the
/// shots the server will spawn. `free_energy` is for cosmetic remote vehicles.
#[allow(clippy::too_many_arguments)]
pub fn step_weapons(k: &mut Kin, body: &ShipBody, ws: &mut WeaponState, inp: &InputCmd, dt: f32, tick: u32, free_energy: bool, out: &mut Vec<Shot>) {
    let blocks = &body.design.blocks;
    let target = inp.aim_point();
    let to_local = k.rot.inverse();
    // per mount: pointing at the aim point (and clear of the vehicle), so it may fire
    let mut on = Vec::with_capacity(body.mounts.len());
    for m in &body.mounts {
        let i = m.block;
        ws.cds[i] -= dt;
        let spec = blocks[i].ty.weapon().unwrap();
        // the aim point seen from the weapon, in the vehicle's frame
        let d = (to_local * (target - body.world(k.pos, k.rot, i))).normalize_or(Vec3::NEG_Z);
        let want = (d.x.atan2(-d.z), d.y.clamp(-1.0, 1.0).asin());
        let (yaw, pitch) = m.turn(ws.aim[i], want, spec.turn.to_radians() * dt);
        ws.aim[i] = (yaw, pitch);
        on.push(m.is_clear(yaw, pitch) && barrel_dir(yaw, pitch).angle_between(d) < ON_TARGET);
    }
    ws.missile_cd -= dt;
    let rot = k.rot;
    let fire = |ws: &WeaponState, i: usize| {
        let (yaw, pitch) = ws.aim[i];
        rot * barrel_dir(yaw, pitch)
    };
    if inp.has(BTN_FIRE) {
        for (m, &on) in body.mounts.iter().zip(&on) {
            let i = m.block;
            let spec = blocks[i].ty.weapon().unwrap();
            if spec.homing || ws.cds[i] > 0.0 || !on {
                continue;
            }
            if !free_energy {
                if k.energy < spec.energy {
                    continue;
                }
                k.energy -= spec.energy;
            }
            ws.cds[i] = 1.0 / spec.rate * (0.92 + 0.16 * hash01(tick, i as u32));
            let aim_dir = fire(ws, i);
            let muzzle = body.world(k.pos, k.rot, i) + aim_dir * BARREL;
            // each pellet its own spread (pellet 0 keeps the single-shot seeds)
            for p in 0..spec.pellets as u32 {
                let mut dir = aim_dir;
                if spec.spread > 0.0 {
                    let seed = i as u32 * 3 + p * 4099;
                    let j = Vec3::new(hash01(tick, seed + 1), hash01(tick, seed + 2), hash01(tick, seed + 3)) - 0.5;
                    dir = (dir + j * spec.spread * 2.0).normalize();
                }
                out.push(Shot { block: i, weapon: blocks[i].ty, pos: muzzle, vel: dir * spec.speed + k.vel });
            }
        }
    }
    if inp.has(BTN_MISSILE) && ws.missile_cd <= 0.0 {
        let ready = body.mounts.iter().find(|m| blocks[m.block].ty.weapon().unwrap().homing && ws.cds[m.block] <= 0.0 && m.is_clear(ws.aim[m.block].0, ws.aim[m.block].1));
        if let Some(m) = ready {
            let i = m.block;
            let spec = blocks[i].ty.weapon().unwrap();
            if !free_energy {
                if k.energy < spec.energy {
                    return;
                }
                k.energy -= spec.energy;
            }
            ws.cds[i] = 1.0 / spec.rate;
            ws.missile_cd = 0.22;
            let dir = fire(ws, i);
            let speed = k.vel.length() + 40.0;
            out.push(Shot { block: i, weapon: blocks[i].ty, pos: body.world(k.pos, k.rot, i) + dir * 0.8, vel: dir * speed });
        }
    }
}

/// Where to aim so a projectile at `speed` (inheriting the shooter's velocity) meets the target.
pub fn intercept(sp: Vec3, sv: Vec3, tp: Vec3, tv: Vec3, speed: f32) -> Vec3 {
    let r = tp - sp;
    let v = tv - sv;
    let a = v.dot(v) - speed * speed;
    let b = 2.0 * r.dot(v);
    let c = r.dot(r);
    let fallback = c.sqrt() / speed;
    let t = if a.abs() < 1e-6 {
        if b < 0.0 { -c / b } else { fallback }
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 {
            fallback
        } else {
            let s = disc.sqrt();
            let (t1, t2) = ((-b - s) / (2.0 * a), (-b + s) / (2.0 * a));
            let mut t = t1.min(t2);
            if t < 0.0 {
                t = t1.max(t2);
            }
            if t < 0.0 { fallback } else { t }
        }
    };
    tp + v * t.min(4.0)
}
