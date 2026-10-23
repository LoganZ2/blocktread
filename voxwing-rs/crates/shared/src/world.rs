use crate::TICK_RATE;
use crate::rng::Rng;
use glam::{EulerRot, Quat, Vec3};

pub const PLAYER_SPAWN: Vec3 = Vec3::new(0.0, 0.0, 600.0);

/// Asteroid. Both sides generate the same field from the seed the server announces. Its shape is
/// the rendered mesh itself (`rock_radius` stretched by `scale`, times `size`) and it turns with
/// server time (`rotation`), so collisions happen exactly where the rock is drawn.
#[derive(Clone, Debug)]
pub struct Asteroid {
    pub pos: Vec3,
    /// Bounding radius: no part of the rock reaches further from its centre.
    pub r: f32,
    /// Visual radius before per-axis scale.
    pub size: f32,
    pub scale: Vec3,
    pub shape_seed: f32,
    pub spin: Vec3,
    pub tint: u8,
}

pub fn generate_asteroids(seed: u64) -> Vec<Asteroid> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    for i in 0..46 {
        let pos = loop {
            let mut p = rng.unit() * rng.f32().cbrt() * 1250.0;
            p.y *= 0.55;
            if p.distance(PLAYER_SPAWN) >= 170.0 && p.length() > 60.0 {
                break p;
            }
        };
        let size = if i < 5 { rng.range(60.0, 95.0) } else { rng.range(8.0, 40.0) };
        let scale = Vec3::new(rng.range(0.75, 1.25), rng.range(0.65, 1.05), rng.range(0.75, 1.25));
        out.push(Asteroid {
            pos,
            r: size * scale.max_element() * ROCK_RADIUS_MAX,
            size,
            scale,
            shape_seed: i as f32 * 7.31,
            spin: rng.unit() * rng.range(0.02, 0.12),
            tint: (i % 3) as u8,
        });
    }
    out
}

pub fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x.wrapping_mul(374_761_393)) ^ (y.wrapping_mul(668_265_263)) ^ (z.wrapping_mul(1_440_662_683));
    h = (h ^ ((h as u32) >> 13) as i32).wrapping_mul(1_274_126_177);
    h ^= ((h as u32) >> 16) as i32;
    (h as u32) as f32 / 4_294_967_296.0
}

pub fn vnoise3(p: Vec3) -> f32 {
    let i = p.floor();
    let f = p - i;
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
    let (xi, yi, zi) = (i.x as i32, i.y as i32, i.z as i32);
    let h = |a: i32, b: i32, c: i32| hash3(xi + a, yi + b, zi + c);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    lerp(
        lerp(lerp(h(0, 0, 0), h(1, 0, 0), u.x), lerp(h(0, 1, 0), h(1, 1, 0), u.x), u.y),
        lerp(lerp(h(0, 0, 1), h(1, 0, 1), u.x), lerp(h(0, 1, 1), h(1, 1, 1), u.x), u.y),
        u.z,
    )
}

pub fn fbm3(mut p: Vec3, octaves: u32) -> f32 {
    let (mut v, mut a) = (0.0, 0.5);
    for _ in 0..octaves {
        v += a * vnoise3(p);
        p *= 2.03;
        a *= 0.5;
    }
    v
}

/// Radial displacement for asteroid vertex direction `d` (unit vector).
pub fn rock_radius(d: Vec3, seed: f32) -> f32 {
    0.68 + fbm3(d * 1.8 + Vec3::new(seed, 0.0, 0.0), 4) * 0.6 + fbm3(d * 5.0 + Vec3::new(seed, 0.0, 0.0), 4) * 0.18
}

/// Upper bound of `rock_radius` (the fbm terms stay below 15/16 of their weight).
pub const ROCK_RADIUS_MAX: f32 = 1.42;

/// Seconds of server time at `tick`, the clock asteroids turn by.
pub fn tick_time(tick: u32) -> f64 {
    tick as f64 / TICK_RATE as f64
}

impl Asteroid {
    /// Orientation at `t` seconds of server time.
    pub fn rotation(&self, t: f64) -> Quat {
        // wrap in f64 so the angle stays exact however long the server runs
        let angle = |w: f32| ((w as f64 * t) % std::f64::consts::TAU) as f32;
        Quat::from_euler(EulerRot::XYZ, angle(self.spin.x), angle(self.spin.y), 0.0)
    }

    /// Signed distance from `p` to the surface (negative inside, roughly in metres) and the
    /// outward normal there, for the rock turned to `rot`.
    pub fn surface(&self, p: Vec3, rot: Quat) -> (f32, Vec3) {
        // into the unit mesh's space, then undo the per-axis stretch: there the surface is
        // simply `rock_radius` along each direction
        let local = rot.inverse() * (p - self.pos) / self.size;
        let q = local / self.scale;
        let len = q.length();
        if len < 1e-4 {
            return (-self.size, Vec3::Y);
        }
        let d = q / len;
        let gap = len - rock_radius(d, self.shape_seed);
        // metres per unit of `q` along this direction
        let stretch = (d * self.scale).length() * self.size;
        // the stretched ellipsoid's normal; the fine bumps only tilt it slightly
        let n = rot * (q / self.scale).normalize_or(d);
        (gap * stretch, n)
    }

    /// Where the segment `a`→`b` first enters the rock, as a fraction along it.
    pub fn hit_segment(&self, a: Vec3, b: Vec3, rot: Quat) -> Option<f32> {
        let ab = b - a;
        let l2 = ab.length_squared();
        let closest = if l2 > 0.0 { ((self.pos - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        if (a + ab * closest).distance_squared(self.pos) > self.r * self.r {
            return None;
        }
        // march the true surface in steps of under a metre
        let steps = (l2.sqrt() / 0.75).ceil().clamp(1.0, 48.0) as u32;
        (0..=steps).map(|i| i as f32 / steps as f32).find(|&f| self.surface(a + ab * f, rot).0 < 0.0)
    }
}
