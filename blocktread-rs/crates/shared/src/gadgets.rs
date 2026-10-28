//! What the gadgets leave behind: mines lying in wait, and clouds of smoke. The server keeps both
//! and tells the clients; both sides ask the clouds whether they hide one thing from another.

use glam::Vec3;

/// Seconds after it is laid that a mine goes live.
pub const MINE_ARM: f32 = 1.0;

/// A cloud of smoke: where, how far it reaches, and the server tick it is gone at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cloud {
    pub pos: Vec3,
    pub radius: f32,
    pub until: u32,
}

impl Cloud {
    /// Whether it hangs between `a` and `b` at server tick `tick` (one of them in it counts).
    pub fn hides(&self, a: Vec3, b: Vec3, tick: u32) -> bool {
        tick < self.until && seg_dist(a, b, self.pos) < self.radius
    }
}

/// Whether any of `clouds` hides `b` from `a` at `tick`.
pub fn hidden(clouds: &[Cloud], a: Vec3, b: Vec3, tick: u32) -> bool {
    clouds.iter().any(|c| c.hides(a, b, tick))
}

/// How far `p` is from the segment `a`→`b`.
fn seg_dist(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-9 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cloud_hides_what_is_behind_it_and_in_it_until_it_clears() {
        let c = Cloud { pos: Vec3::ZERO, radius: 7.0, until: 100 };
        let (a, b) = (Vec3::new(-50.0, 1.0, 0.0), Vec3::new(50.0, 1.0, 0.0));
        assert!(c.hides(a, b, 10), "straight through it");
        assert!(c.hides(a, Vec3::new(3.0, 0.0, 0.0), 10), "into it");
        assert!(!c.hides(a, Vec3::new(50.0, 1.0, 20.0), 10), "past it");
        assert!(!c.hides(a, b, 100), "gone");
    }
}
