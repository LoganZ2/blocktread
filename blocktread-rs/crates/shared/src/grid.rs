use glam::{IVec3, Vec3};

/// Build volume, inclusive. Block centres sit on integer coordinates.
pub const GMIN: IVec3 = IVec3::new(-7, -4, -10);
pub const GMAX: IVec3 = IVec3::new(7, 4, 10);
pub const GDIM: IVec3 = IVec3::new(15, 9, 21);
pub const CELLS: usize = (GDIM.x * GDIM.y * GDIM.z) as usize;

pub const DIRS6: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

pub fn in_grid(p: IVec3) -> bool {
    p.x >= GMIN.x && p.x <= GMAX.x && p.y >= GMIN.y && p.y <= GMAX.y && p.z >= GMIN.z && p.z <= GMAX.z
}

pub fn cell_index(p: IVec3) -> Option<usize> {
    if !in_grid(p) {
        return None;
    }
    let q = p - GMIN;
    Some((q.x + GDIM.x * (q.y + GDIM.y * q.z)) as usize)
}

/// Dense occupancy map: cell -> block index.
#[derive(Clone, Debug)]
pub struct Occupancy(pub Vec<i16>);

impl Default for Occupancy {
    fn default() -> Self {
        Occupancy(vec![-1; CELLS])
    }
}

impl Occupancy {
    pub fn get(&self, p: IVec3) -> Option<usize> {
        cell_index(p).and_then(|c| {
            let v = self.0[c];
            (v >= 0).then_some(v as usize)
        })
    }
    pub fn set(&mut self, p: IVec3, idx: Option<usize>) {
        if let Some(c) = cell_index(p) {
            self.0[c] = idx.map_or(-1, |i| i as i16);
        }
    }
    pub fn has(&self, p: IVec3) -> bool {
        self.get(p).is_some()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GridHit<T> {
    pub cell: IVec3,
    pub t: f32,
    /// Face normal of the entered face (zero if the ray started inside a solid cell).
    pub normal: IVec3,
    pub val: T,
}

/// Amanatides-Woo traversal in grid space (cells centred on integers). `dir` must be normalised.
pub fn raycast<T>(o: Vec3, dir: Vec3, max_t: f32, mut solid: impl FnMut(IVec3) -> Option<T>) -> Option<GridHit<T>> {
    let mut c = IVec3::new((o.x + 0.5).floor() as i32, (o.y + 0.5).floor() as i32, (o.z + 0.5).floor() as i32);
    let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
    let td = Vec3::new(inv_abs(dir.x), inv_abs(dir.y), inv_abs(dir.z));
    let mut tm = Vec3::new(
        first_t(c.x, step.x, o.x, dir.x),
        first_t(c.y, step.y, o.y, dir.y),
        first_t(c.z, step.z, o.z, dir.z),
    );
    let mut t = 0.0;
    let mut n = IVec3::ZERO;
    for _ in 0..600 {
        if let Some(val) = solid(c) {
            return Some(GridHit { cell: c, t, normal: n, val });
        }
        if tm.x < tm.y && tm.x < tm.z {
            t = tm.x;
            c.x += step.x;
            tm.x += td.x;
            n = IVec3::new(-step.x, 0, 0);
        } else if tm.y < tm.z {
            t = tm.y;
            c.y += step.y;
            tm.y += td.y;
            n = IVec3::new(0, -step.y, 0);
        } else {
            t = tm.z;
            c.z += step.z;
            tm.z += td.z;
            n = IVec3::new(0, 0, -step.z);
        }
        if t > max_t {
            break;
        }
    }
    None
}

fn sign(v: f32) -> i32 {
    if v > 0.0 { 1 } else if v < 0.0 { -1 } else { 0 }
}
fn inv_abs(v: f32) -> f32 {
    if v != 0.0 { (1.0 / v).abs() } else { f32::INFINITY }
}
fn first_t(c: i32, s: i32, o: f32, d: f32) -> f32 {
    if s == 0 { f32::INFINITY } else { ((c as f32 + 0.5 * s as f32) - o) / d }
}
