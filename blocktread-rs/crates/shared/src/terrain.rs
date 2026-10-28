//! The ground: a height field both sides generate from the match's map and seed, and on an arena
//! the pieces built on it. Physics reads the very triangles and shapes the client draws, so wheels
//! sit exactly on the rendered surface.

use crate::arena::{self, Arena};
use crate::nav::Nav;
use crate::solid::Solid;
use crate::world::fbm3;
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// What a match is played on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Map {
    /// Open hills and boulders, 1.2 km across (the test range).
    #[default]
    Desert,
    /// 铁笼: the walled arena (see `arena`).
    Cage,
}

/// Half the arena's side: the ground runs from -HALF to +HALF on x and z.
pub const HALF: f32 = 600.0;
/// Grid spacing in metres.
pub const CELL: f32 = 4.0;
/// Samples along each side.
pub const N: usize = (2.0 * HALF / CELL) as usize + 1;

pub struct Terrain {
    h: Vec<f32>,
    pub map: Map,
    /// Walls, crates, ramps: what stands on the ground.
    pub solids: Vec<Solid>,
    /// The arena's layout, on a map that is one.
    pub arena: Option<Arena>,
    /// Where vehicles can drive on it, for bots to plan with.
    pub nav: Option<Nav>,
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The ground's height at (x, z) before it is cut into triangles.
fn shape(x: f32, z: f32, off: f32) -> f32 {
    // rolling hills, a few metres of bumps on them, and a rim of ridges where the arena ends
    let hills = (fbm3(Vec3::new(x / 230.0, off, z / 230.0), 3) - 0.44) * 55.0;
    let bumps = (fbm3(Vec3::new(x / 42.0, off + 19.3, z / 42.0), 2) - 0.38) * 3.5;
    let edge = x.abs().max(z.abs()) / HALF;
    hills + bumps + smoothstep(0.8, 1.0, edge) * 60.0
}

impl Terrain {
    pub fn for_map(map: Map, seed: u64) -> Self {
        match map {
            Map::Desert => Terrain::generate(seed),
            Map::Cage => Terrain::cage(seed),
        }
    }

    /// The open hills.
    pub fn generate(seed: u64) -> Self {
        // a different slice of the noise for every match
        let off = (seed % 10_007) as f32 * 0.37;
        Terrain { h: Terrain::heights(|x, z| shape(x, z, off)), map: Map::Desert, solids: Vec::new(), arena: None, nav: None }
    }

    /// The cage, with the seed's dunes beyond its walls.
    pub fn cage(seed: u64) -> Self {
        let off = (seed % 10_007) as f32 * 0.37;
        let (layout, solids) = arena::cage();
        let h = Terrain::heights(|x, z| arena::cage_height(x, z, &layout.pits, shape(x, z, off)));
        let mut t = Terrain { h, map: Map::Cage, solids, arena: Some(layout), nav: None };
        t.nav = Nav::build(&t);
        t
    }

    fn heights(f: impl Fn(f32, f32) -> f32) -> Vec<f32> {
        let mut h = Vec::with_capacity(N * N);
        for j in 0..N {
            for i in 0..N {
                h.push(f(-HALF + i as f32 * CELL, -HALF + j as f32 * CELL));
            }
        }
        h
    }

    /// Level ground at height 0, until the match's terrain is known.
    pub fn flat() -> Self {
        Terrain { h: vec![0.0; N * N], map: Map::Desert, solids: Vec::new(), arena: None, nav: None }
    }

    /// Level the ground to height `y` within `r` of (x, z), blending back into the hills over the
    /// next `r` (the menus park vehicles on such a patch).
    pub fn level(&mut self, x: f32, z: f32, r: f32, y: f32) {
        for j in 0..N {
            for i in 0..N {
                let d = ((-HALF + i as f32 * CELL - x).powi(2) + (-HALF + j as f32 * CELL - z).powi(2)).sqrt();
                let k = 1.0 - smoothstep(r, 2.0 * r, d);
                let h = &mut self.h[j * N + i];
                *h += (y - *h) * k;
            }
        }
    }

    /// Height of sample (i, j), at x = -HALF + i·CELL, z = -HALF + j·CELL.
    pub fn sample(&self, i: usize, j: usize) -> f32 {
        self.h[j * N + i]
    }

    /// The cell under (x, z), where in it the point is (0..1 on each axis), and its corners'
    /// heights: (0,0), (1,0), (0,1), (1,1).
    fn cell(&self, x: f32, z: f32) -> (f32, f32, [f32; 4]) {
        let top = (N - 1) as f32 - 1e-3;
        let fx = ((x + HALF) / CELL).clamp(0.0, top);
        let fz = ((z + HALF) / CELL).clamp(0.0, top);
        let (i, j) = (fx as usize, fz as usize);
        (fx - i as f32, fz - j as f32, [self.sample(i, j), self.sample(i + 1, j), self.sample(i, j + 1), self.sample(i + 1, j + 1)])
    }

    /// Height of the drawn surface at (x, z). Each cell is two triangles split along the diagonal
    /// from its (0,0) corner to its (1,1) corner, the same as the mesh. A pit drops straight
    /// down from its rim, so nothing can rest on its edge.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        if self.in_pit(x, z) {
            return -arena::PIT_DEPTH;
        }
        let (u, v, [h00, h10, h01, h11]) = self.cell(x, z);
        if u >= v { h00 + (h10 - h00) * u + (h11 - h10) * v } else { h00 + (h11 - h01) * u + (h01 - h00) * v }
    }

    /// Strictly inside one of the arena's pits.
    fn in_pit(&self, x: f32, z: f32) -> bool {
        self.arena.as_ref().is_some_and(|a| a.pits.iter().any(|p| p.x0 < x && x < p.x1 && p.z0 < z && z < p.z1))
    }

    /// Upward normal of the triangle under (x, z).
    pub fn normal(&self, x: f32, z: f32) -> Vec3 {
        if self.in_pit(x, z) {
            return Vec3::Y;
        }
        let (u, v, [h00, h10, h01, h11]) = self.cell(x, z);
        let (dx, dz) = if u >= v { (h10 - h00, h11 - h10) } else { (h11 - h01, h01 - h00) };
        Vec3::new(-dx / CELL, 1.0, -dz / CELL).normalize()
    }

    /// The pieces that may reach within `r` of `p`.
    pub fn solids_near(&self, p: Vec3, r: f32) -> impl Iterator<Item = &Solid> {
        self.solids.iter().filter(move |s| s.centre.distance_squared(p) < (s.radius + r) * (s.radius + r))
    }

    /// What there is to stand on at (x, z): the ground, or the top of a piece standing there.
    pub fn surface(&self, x: f32, z: f32) -> f32 {
        let p = Vec3::new(x, 0.0, z);
        self.solids.iter().filter(|s| s.centre.with_y(0.0).distance_squared(p) < s.radius * s.radius).filter_map(|s| s.top(x, z)).fold(self.height(x, z), f32::max)
    }

    /// Where the segment `a`→`b` first meets the ground or a piece, as a fraction along it.
    pub fn segment_hit(&self, a: Vec3, b: Vec3) -> Option<f32> {
        let under = |p: Vec3| p.y < self.height(p.x, p.z);
        let mut best = if under(a) {
            Some(0.0)
        } else if under(b) {
            // narrow down where it went under
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..8 {
                let m = (lo + hi) * 0.5;
                if under(a.lerp(b, m)) { hi = m } else { lo = m }
            }
            Some(hi)
        } else {
            None
        };
        for s in self.solids_near((a + b) * 0.5, a.distance(b) * 0.5) {
            if let Some((t, _)) = s.clip(a, b)
                && best.is_none_or(|b| t < b)
            {
                best = Some(t);
            }
        }
        best
    }
}
