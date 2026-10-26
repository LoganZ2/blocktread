//! The ground: a height field both sides generate from the match seed. Physics reads the very
//! triangles the client draws, so wheels sit exactly on the rendered surface.

use crate::world::fbm3;
use glam::Vec3;

/// Half the arena's side: the ground runs from -HALF to +HALF on x and z.
pub const HALF: f32 = 600.0;
/// Grid spacing in metres.
pub const CELL: f32 = 4.0;
/// Samples along each side.
pub const N: usize = (2.0 * HALF / CELL) as usize + 1;

pub struct Terrain {
    h: Vec<f32>,
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
    pub fn generate(seed: u64) -> Self {
        // a different slice of the noise for every match
        let off = (seed % 10_007) as f32 * 0.37;
        let mut h = Vec::with_capacity(N * N);
        for j in 0..N {
            for i in 0..N {
                h.push(shape(-HALF + i as f32 * CELL, -HALF + j as f32 * CELL, off));
            }
        }
        Terrain { h }
    }

    /// Level ground at height 0, until the match's terrain is known.
    pub fn flat() -> Self {
        Terrain { h: vec![0.0; N * N] }
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
    /// from its (0,0) corner to its (1,1) corner, the same as the mesh.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let (u, v, [h00, h10, h01, h11]) = self.cell(x, z);
        if u >= v { h00 + (h10 - h00) * u + (h11 - h10) * v } else { h00 + (h11 - h01) * u + (h01 - h00) * v }
    }

    /// Upward normal of the triangle under (x, z).
    pub fn normal(&self, x: f32, z: f32) -> Vec3 {
        let (u, v, [h00, h10, h01, h11]) = self.cell(x, z);
        let (dx, dz) = if u >= v { (h10 - h00, h11 - h10) } else { (h11 - h01, h01 - h00) };
        Vec3::new(-dx / CELL, 1.0, -dz / CELL).normalize()
    }
}
