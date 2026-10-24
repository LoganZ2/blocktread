use crate::blocks::BlockType;
use crate::flight::{BTN_FIRE, BTN_MISSILE, InputCmd, Kin};
use crate::rng::hash01;
use crate::ship::ShipBody;
use glam::Vec3;

/// A projectile leaving a muzzle this tick.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub block: usize,
    pub weapon: BlockType,
    pub pos: Vec3,
    pub vel: Vec3,
}

/// Per-weapon cooldowns, indexed by block index.
#[derive(Clone, Debug)]
pub struct WeaponState {
    pub cds: Vec<f32>,
    pub missile_cd: f32,
}

impl WeaponState {
    pub fn new(body: &ShipBody) -> Self {
        WeaponState { cds: (0..body.alive.len()).map(|i| hash01(i as u32, 7) * 0.3).collect(), missile_cd: 0.0 }
    }
}

/// Fire whatever is ready. Cooldown jitter and spread come from `hash01(tick, block)` so the client
/// predicts exactly the shots the server will spawn. `free_energy` is for cosmetic remote ships.
pub fn step_weapons(k: &mut Kin, body: &ShipBody, ws: &mut WeaponState, inp: &InputCmd, dt: f32, tick: u32, free_energy: bool, out: &mut Vec<Shot>) {
    let blocks = &body.design.blocks;
    for &i in &body.stats.weapons {
        ws.cds[i] -= dt;
    }
    ws.missile_cd -= dt;
    let fwd = k.fwd();
    let aim = k.pos + fwd * inp.aim_dist.max(30) as f32;
    if inp.has(BTN_FIRE) {
        for &i in &body.stats.weapons {
            let spec = blocks[i].ty.weapon().unwrap();
            if spec.homing || ws.cds[i] > 0.0 {
                continue;
            }
            if !free_energy {
                if k.energy < spec.energy {
                    continue;
                }
                k.energy -= spec.energy;
            }
            ws.cds[i] = 1.0 / spec.rate * (0.92 + 0.16 * hash01(tick, i as u32));
            let muzzle = body.world(k.pos, k.rot, i) + fwd * 1.4;
            let mut aim_dir = (aim - muzzle).normalize_or(fwd);
            if aim_dir.dot(fwd) <= 0.985 {
                aim_dir = fwd;
            }
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
        if let Some(&i) = body.stats.weapons.iter().find(|&&i| blocks[i].ty.weapon().unwrap().homing && ws.cds[i] <= 0.0) {
            let spec = blocks[i].ty.weapon().unwrap();
            if !free_energy {
                if k.energy < spec.energy {
                    return;
                }
                k.energy -= spec.energy;
            }
            ws.cds[i] = 1.0 / spec.rate;
            ws.missile_cd = 0.22;
            let speed = k.vel.length() + 40.0;
            out.push(Shot { block: i, weapon: blocks[i].ty, pos: body.world(k.pos, k.rot, i) + fwd * 0.8, vel: fwd * speed });
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
