//! The arena's built pieces: walls, crates, the hill and its ramps. Each is a convex prism standing
//! on the ground, bounded by a few planes, so wheels, hulls, rounds and the camera all test the
//! very shape the client draws.

use glam::{Quat, Vec3};

/// How a piece is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// The arena's outer wall.
    Wall,
    /// Concrete cover in the field.
    Cover,
    Crate,
    /// The hill in the middle.
    Platform,
    /// Steel ramps: the hill's, and the kickers by the pits.
    Ramp,
    /// The ground round a pit, down to its floor: never drawn (the floor and the pit are), but it
    /// gives the pit a real edge.
    PitWall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A box round `pos`, `half` out each way, turned `yaw` about the vertical.
    Block,
    /// A wedge on the ground: level with it at its low end (local −z), `2·half.y` high at the
    /// other. `pos` is the middle of its footprint, on the ground.
    Ramp,
}

#[derive(Clone, Debug)]
pub struct Solid {
    pub shape: Shape,
    pub style: Style,
    pub pos: Vec3,
    pub half: Vec3,
    pub yaw: f32,
    /// Outward normal and offset of each face: inside is where `n·p <= d` for all of them.
    planes: Vec<(Vec3, f32)>,
    /// A sphere round it all.
    pub centre: Vec3,
    pub radius: f32,
}

/// Ramps reach this far under the ground, so no gap opens beneath their low end.
const SINK: f32 = 1.0;
/// How far in from the edge of a top a hull resting on it starts to slide off.
const EDGE: f32 = 1.0;

impl Solid {
    pub fn block(style: Style, pos: Vec3, half: Vec3, yaw: f32) -> Solid {
        let local = vec![
            (Vec3::X, half.x),
            (Vec3::NEG_X, half.x),
            (Vec3::Y, half.y),
            (Vec3::NEG_Y, half.y),
            (Vec3::Z, half.z),
            (Vec3::NEG_Z, half.z),
        ];
        Solid::new(Shape::Block, style, pos, half, yaw, local, Vec3::ZERO, half.length())
    }

    /// A ramp `2·half.z` long rising to `2·half.y`, its high end towards local +z.
    pub fn ramp(style: Style, pos: Vec3, half: Vec3, yaw: f32) -> Solid {
        let h = 2.0 * half.y;
        let slope = Vec3::new(0.0, 2.0 * half.z, -h).normalize();
        let local = vec![
            (Vec3::X, half.x),
            (Vec3::NEG_X, half.x),
            (Vec3::NEG_Y, SINK),
            (Vec3::Z, half.z),
            (Vec3::NEG_Z, half.z),
            (slope, slope.dot(Vec3::new(0.0, 0.0, -half.z))),
        ];
        let mid = Vec3::new(0.0, (h - SINK) * 0.5, 0.0);
        Solid::new(Shape::Ramp, style, pos, half, yaw, local, mid, Vec3::new(half.x, (h + SINK) * 0.5, half.z).length())
    }

    #[allow(clippy::too_many_arguments)]
    fn new(shape: Shape, style: Style, pos: Vec3, half: Vec3, yaw: f32, local: Vec<(Vec3, f32)>, mid: Vec3, radius: f32) -> Solid {
        let rot = Quat::from_rotation_y(yaw);
        let planes = local
            .into_iter()
            .map(|(n, d)| {
                let n = rot * n;
                (n, d + n.dot(pos))
            })
            .collect();
        Solid { shape, style, pos, half, yaw, planes, centre: pos + rot * mid, radius: radius + 0.05 }
    }

    /// How far `p` is outside (negative inside) and which way is out. Outside, the gaps to every
    /// face it is beyond add up, which is exact off a box's edges and corners and close off a
    /// ramp's (just taking the widest gap would let a corner reach out and catch what passes
    /// over the face next to it, where a ramp meets the deck it leads onto). Inside, it is the
    /// nearest face.
    pub fn distance(&self, p: Vec3) -> (f32, Vec3) {
        let mut nearest = (f32::MIN, Vec3::Y);
        let (mut gap2, mut out) = (0.0f32, Vec3::ZERO);
        for &(n, d) in &self.planes {
            let s = n.dot(p) - d;
            if s > 0.0 {
                gap2 += s * s;
                out += n * s;
            }
            if s > nearest.0 {
                nearest = (s, n);
            }
        }
        if gap2 > 0.0 { (gap2.sqrt(), out.normalize_or(nearest.1)) } else { nearest }
    }

    /// Where the segment `a`→`b` enters it: the fraction along it and the face it crosses. A start
    /// already inside enters at 0, through the top.
    pub fn clip(&self, a: Vec3, b: Vec3) -> Option<(f32, Vec3)> {
        let ab = b - a;
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        let mut face = None;
        for &(n, d) in &self.planes {
            let num = d - n.dot(a);
            let den = n.dot(ab);
            if den.abs() < 1e-9 {
                if num < 0.0 {
                    return None;
                }
                continue;
            }
            let t = num / den;
            if den < 0.0 {
                if t > t0 {
                    t0 = t;
                    face = Some(n);
                }
            } else if t < t1 {
                t1 = t;
            }
            if t0 > t1 {
                return None;
            }
        }
        Some((t0, face.unwrap_or(Vec3::Y)))
    }

    /// The height of its top over (x, z), if it stands there.
    pub fn top(&self, x: f32, z: f32) -> Option<f32> {
        let mut top = f32::MAX;
        for &(n, d) in &self.planes {
            if n.y > 0.1 {
                top = top.min((d - n.x * x - n.z * z) / n.y);
            } else if n.y.abs() <= 0.1 && n.x * x + n.z * z > d {
                return None;
            }
        }
        (top < f32::MAX).then_some(top)
    }

    /// Which way a hull resting on its top at `p` slides off: outward near the top's edges, as
    /// though they were rounded, the more so the nearer (a ramp's sides only: its ends meet the
    /// ground and what it leads up to).
    pub fn edge_off(&self, p: Vec3) -> Vec3 {
        let l = self.rotation().inverse() * (p - self.pos);
        let (dx, dz) = (self.half.x - l.x.abs(), self.half.z - l.z.abs());
        let (gap, out) = if self.shape == Shape::Block && dz < dx { (dz, Vec3::Z * l.z.signum()) } else { (dx, Vec3::X * l.x.signum()) };
        self.rotation() * out * (1.0 - gap / EDGE).clamp(0.0, 1.0)
    }

    /// How much a hull sliding on it is held back, as a fraction of how hard it presses.
    pub fn friction(&self) -> f32 {
        // a pit's rim is slick, so whatever hangs over it slides off one way or the other
        if self.style == Style::PitWall { 0.0 } else { 0.6 }
    }

    /// Its turn about the vertical, as a rotation.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_measures_and_clips() {
        let b = Solid::block(Style::Crate, Vec3::new(0.0, 1.0, 0.0), Vec3::splat(1.0), 0.0);
        assert!((b.distance(Vec3::new(3.0, 1.0, 0.0)).0 - 2.0).abs() < 1e-5);
        assert!(b.distance(Vec3::new(0.0, 1.0, 0.0)).0 < 0.0);
        // off an edge: the way to the edge, not to the nearer face
        let (d, n) = b.distance(Vec3::new(1.3, 2.4, 0.0));
        assert!((d - 0.5).abs() < 1e-5 && (n - Vec3::new(0.6, 0.8, 0.0)).length() < 1e-5);
        let (t, n) = b.clip(Vec3::new(-5.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0)).unwrap();
        assert!((t - 0.4).abs() < 1e-5 && n == Vec3::NEG_X);
        assert!(b.clip(Vec3::new(-5.0, 3.0, 0.0), Vec3::new(5.0, 3.0, 0.0)).is_none());
        assert_eq!(b.top(0.5, -0.5), Some(2.0));
        assert_eq!(b.top(1.5, 0.0), None);
    }

    #[test]
    fn a_ramp_rises_to_its_high_end() {
        // 10 long, 2.5 high, turned to rise towards −x
        let r = Solid::ramp(Style::Ramp, Vec3::new(15.0, 0.0, 0.0), Vec3::new(4.0, 1.25, 5.0), -std::f32::consts::FRAC_PI_2);
        assert!(r.top(19.9, 0.0).unwrap().abs() < 0.05);
        assert!((r.top(10.1, 0.0).unwrap() - 2.5).abs() < 0.05);
        assert!((r.top(15.0, 3.0).unwrap() - 1.25).abs() < 0.05);
        assert_eq!(r.top(15.0, 4.5), None);
        // straight down onto its middle
        let (t, n) = r.clip(Vec3::new(15.0, 5.0, 0.0), Vec3::new(15.0, -5.0, 0.0)).unwrap();
        assert!((5.0 - t * 10.0 - 1.25).abs() < 0.05 && n.y > 0.9);
    }
}
