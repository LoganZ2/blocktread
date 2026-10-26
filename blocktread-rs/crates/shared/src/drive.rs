//! Ground vehicles: a rigid body on raycast suspension, a spring under every wheel and hover pad,
//! over the terrain both sides generate. Wheels drive themselves; hover pads float, slide and turn
//! fast on energy; thrusters only push while boosting. Deterministic like the rest of shared, so
//! the client predicts its own vehicle with the very code the server runs.

use crate::blocks::DriveSpec;
use crate::flight::{BTN_BOOST, BTN_FLIP, InputCmd, Kin};
use crate::ship::ShipBody;
use crate::terrain::{self, Terrain};
use glam::{Quat, Vec3};

pub const GRAVITY: f32 = 14.0;
/// How far below its block a wheel's centre hangs with the spring fully out.
pub const DROP: f32 = 0.45;
/// Where it hangs under the vehicle's own weight on level ground.
pub const REST_DROP: f32 = 0.2;
const SUBSTEPS: u32 = 2;
/// Steering lock at a standstill (rad); it narrows with speed (see `steer_angle`).
const STEER: f32 = 0.6;
/// Blocks touch the ground as spheres of this radius.
const BLOCK_R: f32 = 0.5;
/// Inertia on top of the blocks' own: steadier over bumps.
const INERTIA_SCALE: f32 = 1.6;
/// Acceleration per unit of drive power over mass, and per unit of thrust over mass (boost).
const DRIVE: f32 = 11.0;
const THRUST: f32 = 11.5;
/// Energy a boost burns per second: a base, and more for every unit of thrust.
const BOOST_DRAIN: f32 = 25.0;
const BOOST_DRAIN_PER_THRUST: f32 = 0.2;
/// After the battery runs dry under boost, the boost stays off this long (and until Shift is
/// pressed again).
pub const BOOST_LOCK: f32 = 1.0;

/// Acceleration and top speed (m/s²; m/s) from the movers' power over the vehicle's mass. Top
/// speed is the movers' own, weighted by their power, and lower for a heavy vehicle.
pub fn performance_of<'a>(movers: impl Iterator<Item = &'a DriveSpec>, mass: f32) -> (f32, f32) {
    let (mut power, mut paced) = (0.0, 0.0);
    for d in movers {
        power += d.power;
        paced += d.power * d.top;
    }
    if power <= 0.0 {
        return (0.0, 0.0);
    }
    let pw = power / mass.max(1.0);
    ((DRIVE * pw).clamp(1.5, 12.0), paced / power * (0.55 + 0.7 * pw).clamp(0.6, 1.0))
}

/// `performance_of` for a vehicle's surviving movers.
pub fn performance(body: &ShipBody) -> (f32, f32) {
    performance_of(body.movers.iter().map(|&i| body.design.blocks[i].ty.drive().unwrap()), body.stats.mass)
}

/// How far the front wheels turn (rad, positive right) for a steering input at a forward speed.
pub fn steer_angle(steer: f32, speed: f32) -> f32 {
    steer * STEER / (1.0 + speed.abs() / 14.0)
}

/// Where a wheel hanging from `mount` (along `-up`) meets the ground: the distance along the
/// suspension, and the ground's normal there. None when the vehicle lies on its side or roof.
pub fn wheel_ray(ground: &Terrain, mount: Vec3, up: Vec3) -> Option<(f32, Vec3)> {
    if up.y < 0.25 {
        return None;
    }
    let mut t = (mount.y - ground.height(mount.x, mount.z)) / up.y;
    for _ in 0..2 {
        let p = mount - up * t;
        t += (p.y - ground.height(p.x, p.z)) / up.y;
    }
    let p = mount - up * t;
    Some((t, ground.normal(p.x, p.z)))
}

/// How far below its block a wheel of `radius` sits, for a suspension `dist` from the ground.
pub fn wheel_drop(dist: Option<f32>, radius: f32) -> f32 {
    dist.map_or(DROP, |d| (d - radius).clamp(-0.1, DROP))
}

/// Height to set a vehicle down at (x, z), facing `rot` (a heading only), so that nothing is in
/// the ground and its wheels just touch.
pub fn spawn_height(body: &ShipBody, ground: &Terrain, x: f32, z: f32, rot: Quat) -> f32 {
    let mut y = f32::MIN;
    for i in 0..body.alive.len() {
        if body.alive[i] {
            let l = rot * body.local(i);
            let reach = body.design.blocks[i].ty.drive().map_or(BLOCK_R, |d| d.radius + REST_DROP);
            y = y.max(ground.height(x + l.x, z + l.z) - l.y + reach);
        }
    }
    y + 0.3
}

/// Advance one vehicle by one tick.
pub fn step_drive(k: &mut Kin, body: &ShipBody, inp: &InputCmd, ground: &Terrain, dt: f32) {
    let st = &body.stats;
    let blocks = &body.design.blocks;
    let m = st.mass.max(1.0);
    let inertia = (st.inertia * INERTIA_SCALE).max(Vec3::splat(m * 0.5));
    let (accel, top) = performance(body);
    let power = body.power.max(1e-3);

    // the boost: thrusters push while Shift is held and there is energy; running dry locks it
    // for a second, and until Shift is let go
    let held = inp.has(BTN_BOOST);
    if !held {
        k.boost_hold = false;
    }
    k.boost_lock = (k.boost_lock - dt).max(0.0);
    k.boosting = held && !k.boost_hold && k.boost_lock <= 0.0 && st.thrust > 0.0 && k.energy > 0.0;
    if k.boosting {
        k.energy -= (BOOST_DRAIN + BOOST_DRAIN_PER_THRUST * st.thrust) * dt;
        if k.energy <= 0.0 {
            k.energy = 0.0;
            k.boost_lock = BOOST_LOCK;
            k.boost_hold = true;
        }
    }
    // hover pads lift on energy, and drop when it's gone
    let hover_drain: f32 = body.movers.iter().filter_map(|&i| blocks[i].ty.drive().filter(|d| d.hover)).map(|d| d.drain).sum();
    let lifting = k.energy > 0.0;
    if hover_drain > 0.0 && lifting {
        k.energy = (k.energy - hover_drain * dt).max(0.0);
    }

    // the springs hold the vehicle up at REST_DROP when every mover carries its share
    let share = m / body.movers.len().max(1) as f32;
    let spring = share * GRAVITY / (DROP - REST_DROP);
    let damp = 0.9 * (spring * share).sqrt();
    let drive = inp.drive_f();
    let steer = steer_angle(inp.steer_f(), k.vel.dot(k.fwd()));
    let h = dt / SUBSTEPS as f32;
    for _ in 0..SUBSTEPS {
        let (up, fwd) = (k.up(), k.fwd());
        let mut force = Vec3::new(0.0, -GRAVITY * m, 0.0);
        let mut torque = Vec3::ZERO;
        let mut spin = 0.0;
        let mut hovering = 0;
        for &i in &body.movers {
            let spec = blocks[i].ty.drive().unwrap();
            if spec.hover && !lifting {
                continue;
            }
            let local = body.local(i);
            let r = k.rot * local;
            let Some((dist, n)) = wheel_ray(ground, k.pos + r, up) else { continue };
            let c = DROP + spec.radius - dist;
            if c <= 0.0 {
                continue;
            }
            let vp = k.vel + k.ang.cross(r);
            let mut load = spring * c - damp * vp.dot(up);
            // bump stop: the wheel pushed up into its block
            if c > DROP + 0.1 {
                load += spring * 6.0 * (c - DROP - 0.1);
            }
            let load = load.clamp(0.0, share * GRAVITY * 8.0);
            // wheels ahead of the centre of mass steer; hover pads turn the whole vehicle instead
            let wf = if !spec.hover && local.z < -0.5 { Quat::from_axis_angle(up, -steer) * fwd } else { fwd };
            let f = (wf - n * wf.dot(n)).normalize_or_zero();
            let side = n.cross(f);
            let (vf, vs) = (vp.dot(f), vp.dot(side));
            let grip = spec.grip.min(1.0);
            // tyres stop a sideways slide (half of it each substep, so it doesn't ring); hover pads
            // barely do
            let lat = -vs * share / h * 0.5 * grip;
            // this mover's part of the push: its share of the power
            let pull = m * accel * spec.power / power;
            let long = if drive.abs() > 0.01 && drive * vf >= -0.5 {
                // pulling the way it rolls, or from a standstill: drive, fading at top speed
                drive * pull * (1.0 - (vf * drive.signum() / top.max(1.0)).clamp(0.0, 1.0))
            } else if drive.abs() > 0.01 {
                // pushing against the roll: brakes, never past a stop
                -vf.signum() * (pull * 1.6).min(vf.abs() * share / h)
            } else if vf.abs() < 2.0 {
                // hands off and nearly stopped: the brakes hold it, even on a slope
                -vf * share / h * 0.5 * grip
            } else {
                // coasting (hover pads glide on)
                -vf * share * 0.7 * grip
            };
            let mut t = f * long + side * lat;
            if spec.hover {
                hovering += 1;
                spin += spec.turn;
            } else {
                let cap = spec.grip * load;
                if t.length() > cap {
                    t = t.normalize() * cap;
                }
            }
            let fw = up * load + t;
            force += fw;
            torque += r.cross(fw);
        }
        if k.boosting {
            // thrust along the hull, fading well past the wheels' top speed
            let fade = 1.0 - (k.vel.dot(fwd) / (top.max(10.0) * 1.6)).clamp(0.0, 1.0);
            force += fwd * st.thrust * THRUST * fade;
        }
        k.vel += force / m * h;
        k.ang += k.rot * ((k.rot.inverse() * torque) / inertia) * h;
        if hovering > 0 {
            // hover pads swing the vehicle round to the commanded turn rate, fast
            let want = -inp.steer_f() * spin / hovering as f32;
            let now = k.ang.dot(up);
            let rate = (h * 14.0 * hovering as f32 / body.movers.len() as f32).min(1.0);
            k.ang += up * (want - now) * rate;
        }
        if inp.has(BTN_FLIP) && up.y < 0.9 {
            // roll back onto the wheels; flat on the roof there's no "uphill" side, so roll sideways
            let axis = if up.y < -0.9 { fwd } else { up.cross(Vec3::Y).normalize_or(fwd) };
            k.ang += axis * 7.0 * h;
            k.vel.y += GRAVITY * 0.6 * h;
        }
        k.ang *= (-0.4 * h).exp();
        k.pos += k.vel * h;
        k.rot = (Quat::from_scaled_axis(k.ang * h) * k.rot).normalize();
        ground_contact(k, body, ground, m, inertia);
    }
    // the arena's edge: pushed back in, never past it
    let lim = terrain::HALF - 20.0;
    for a in [0, 2] {
        let p = k.pos[a];
        if p.abs() > lim {
            k.vel[a] -= p.signum() * (p.abs() - lim) * 8.0 * dt;
            k.pos[a] = p.clamp(-terrain::HALF + 2.0, terrain::HALF - 2.0);
        }
    }
    k.energy = (k.energy + st.energy_regen * dt).min(st.energy_cap);
    if k.shield_delay > 0.0 {
        k.shield_delay -= dt;
    } else {
        k.shield = (k.shield + st.shield_regen * dt).min(st.shield_cap);
    }
}

/// The hull where it reaches into the ground (landing on the roof, scraping over a crest, hover
/// pads out of energy): lift the vehicle out at its deepest point and stop that point sinking,
/// with friction along the ground.
fn ground_contact(k: &mut Kin, body: &ShipBody, ground: &Terrain, m: f32, inertia: Vec3) {
    let mut deepest: Option<(f32, Vec3)> = None;
    for i in 0..body.alive.len() {
        if !body.alive[i] {
            continue;
        }
        let w = body.world(k.pos, k.rot, i);
        let pen = ground.height(w.x, w.z) + BLOCK_R - w.y;
        if pen > 0.0 && deepest.is_none_or(|d| pen > d.0) {
            deepest = Some((pen, w));
        }
    }
    let Some((pen, p)) = deepest else { return };
    k.pos.y += pen;
    let n = ground.normal(p.x, p.z);
    let r = p - k.pos;
    let vp = k.vel + k.ang.cross(r);
    let vn = vp.dot(n);
    if vn >= 0.0 {
        return;
    }
    let rot = k.rot;
    let inv = |v: Vec3| rot * ((rot.inverse() * v) / inertia);
    // how hard it is to change the point's velocity along `d`
    let resist = |d: Vec3| 1.0 / m + r.cross(d).dot(inv(r.cross(d)));
    let j = -1.1 * vn / resist(n);
    let vt = vp - n * vn;
    let mut imp = n * j;
    if vt.length() > 1e-4 {
        let t = vt.normalize();
        imp -= t * (vt.length() / resist(t)).min(0.6 * j);
    }
    k.vel += imp / m;
    k.ang += inv(r.cross(imp));
}
