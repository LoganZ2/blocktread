//! 铁笼, the arena free-for-all and 2v2 are fought in: a walled square of concrete, the hill in the
//! middle with a ramp up each side, a pit north and south of it with a kicker at either end to
//! jump it, cover round the middle and crates by the spawns, and pick-up pads between them.
//! Symmetric both ways, so every spawn faces the same fight.

use crate::solid::{Solid, Style};
use glam::Vec3;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

/// The inside faces of the outer walls, from the centre.
pub const CAGE_HALF: f32 = 120.0;
/// A vehicle whose centre drops below this has fallen into a pit.
pub const KILL_Y: f32 = -3.0;
/// How deep the pits go.
pub const PIT_DEPTH: f32 = 14.0;
/// The pits: how far their near rim is from the middle, and how far it is across.
pub const PIT_NEAR: f32 = 48.0;
pub const PIT_LONG: f32 = 16.0;
/// Half a pit's width (across the way you jump it).
pub const PIT_HALF_W: f32 = 12.0;
/// The height of a kicker's deck.
pub const KICK_TOP: f32 = 1.8;
/// How far the walls round a pit reach out under the floor beside it. That strip of ground is
/// these walls, not the height field, so that the rim is a true edge, rounded and slick.
pub const RIM: f32 = 3.0;
/// The hill: half its top's side, and its height.
pub const HILL_HALF: f32 = 10.0;
pub const HILL_TOP: f32 = 2.5;
/// A saw blade's radius, how high its hub stands over the floor, and half its thickness.
pub const SAW_R: f32 = 1.5;
pub const SAW_HUB: f32 = 0.3;
pub const SAW_HALF_T: f32 = 0.1;
/// A flame vent's cycle: it burns for the first `VENT_ON` seconds of every `VENT_CYCLE`, and its
/// grate glows for `VENT_WARN` seconds before. The flames reach `VENT_HIGH` up.
pub const VENT_CYCLE: f32 = 6.0;
pub const VENT_ON: f32 = 2.0;
pub const VENT_WARN: f32 = 0.8;
pub const VENT_HIGH: f32 = 3.0;
/// What a saw's bite deals (shared over the blocks round where it bit), and what a vent's flames
/// deal a second to a vehicle in them (shared over its blocks there).
pub const SAW_DMG: f32 = 24.0;
pub const VENT_DPS: f32 = 80.0;
/// How near a pad's middle a vehicle has to be to take it.
pub const PAD_R: f32 = 3.0;
/// Overdrive: how much harder its weapons hit, and for how long.
pub const OVERDRIVE_MUL: f32 = 1.5;
pub const OVERDRIVE_SECS: f32 = 12.0;
/// 2v2 on the hill: the points that win, and what a kill is worth on top of holding it.
pub const HILL_WIN: u32 = 100;
pub const HILL_KILL: u32 = 5;

/// An area of the ground, along the axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub x1: f32,
    pub z0: f32,
    pub z1: f32,
}

impl Rect {
    pub fn around(x: f32, z: f32, hx: f32, hz: f32) -> Rect {
        Rect { x0: x - hx, x1: x + hx, z0: z - hz, z1: z + hz }
    }

    pub fn contains(&self, x: f32, z: f32) -> bool {
        self.x0 <= x && x <= self.x1 && self.z0 <= z && z <= self.z1
    }

    pub fn centre(&self) -> Vec3 {
        Vec3::new((self.x0 + self.x1) * 0.5, 0.0, (self.z0 + self.z1) * 0.5)
    }
}

/// A circular saw standing up out of a slot in the floor, running back and forth along it. Where
/// it is depends only on the time, so every client sees it where the server has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Saw {
    /// The slot's middle, its way (flat, unit length) and how far the blade runs either side.
    pub mid: Vec3,
    pub dir: Vec3,
    pub reach: f32,
    /// Seconds for a run there and back, and how far into one it is at time 0 (0..1).
    pub period: f32,
    pub phase: f32,
}

impl Saw {
    /// The hub at `t` seconds of server time.
    pub fn at(&self, t: f64) -> Vec3 {
        let turn = (t / self.period as f64 + self.phase as f64).fract() as f32;
        self.mid + self.dir * self.reach * (turn * TAU).sin() + Vec3::Y * SAW_HUB
    }

    /// The nearest point of the blade, with its hub at `hub`, to `p`. The blade is a thin disc
    /// standing along the slot: its faces look across it.
    pub fn nearest(&self, hub: Vec3, p: Vec3) -> Vec3 {
        let across = self.dir.cross(Vec3::Y);
        let d = p - hub;
        let flat = d - across * d.dot(across);
        let flat = if flat.length() > SAW_R { flat.normalize() * SAW_R } else { flat };
        hub + flat + across * d.dot(across).clamp(-SAW_HALF_T, SAW_HALF_T)
    }

    /// The ground the blade sweeps, and a metre round it: where bots don't drive. (They look
    /// across their whole width, so this needn't allow for it.)
    pub fn zone(&self) -> Rect {
        let e = self.dir * (self.reach + SAW_R + 1.0) + self.dir.cross(Vec3::Y) * 1.0;
        Rect::around(self.mid.x, self.mid.z, e.x.abs(), e.z.abs())
    }
}

/// A grate in the floor that breathes fire on a timer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vent {
    pub area: Rect,
    /// Seconds into the cycle at time 0.
    pub phase: f32,
}

impl Vent {
    /// Seconds into its cycle at `t`: burning while under `VENT_ON`.
    pub fn into(&self, t: f64) -> f32 {
        ((t + self.phase as f64) / VENT_CYCLE as f64).fract() as f32 * VENT_CYCLE
    }

    pub fn burning(&self, t: f64) -> bool {
        self.into(t) < VENT_ON
    }

    /// Burning, or about to: no time to cross it.
    pub fn hot(&self, t: f64) -> bool {
        let x = self.into(t);
        x < VENT_ON || x > VENT_CYCLE - VENT_WARN - 0.5
    }
}

/// What a pick-up pad gives whoever drives onto it while it is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadKind {
    /// Every block back, as good as new.
    Repair,
    /// A full battery and shield.
    Energy,
    /// Its weapons hit harder for a while.
    Overdrive,
}

impl PadKind {
    /// Seconds it takes to come back once taken.
    pub fn respawn(self) -> f32 {
        match self {
            PadKind::Repair => 40.0,
            PadKind::Energy => 25.0,
            PadKind::Overdrive => 60.0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PadKind::Repair => "修理",
            PadKind::Energy => "充能",
            PadKind::Overdrive => "超载",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pad {
    pub at: Vec3,
    pub kind: PadKind,
}

impl Pad {
    /// A vehicle with its centre at `p` is on it.
    pub fn under(&self, p: Vec3) -> bool {
        p.with_y(0.0).distance(self.at.with_y(0.0)) < PAD_R && (p.y - self.at.y).abs() < 3.0
    }
}

/// What a map has besides its ground and pieces.
#[derive(Clone, Debug)]
pub struct Arena {
    /// The inside faces of the outer walls, from the centre.
    pub half: f32,
    /// Open pits: anything that drops into one is lost.
    pub pits: Vec<Rect>,
    /// Each pit with its kickers and a margin round them: where bots don't drive.
    pub pit_zones: Vec<Rect>,
    /// The hill's top, which 2v2 is fought over.
    pub hill: Rect,
    /// Where free-for-all players enter, facing the middle.
    pub spawns: Vec<Vec3>,
    /// Where each 2v2 side enters: even teams in the west, odd ones in the east.
    pub team_spawns: [Vec<Vec3>; 2],
    /// The feet of the hill's ramps: the way up.
    pub hill_ways: Vec<Vec3>,
    pub saws: Vec<Saw>,
    pub vents: Vec<Vent>,
    pub pads: Vec<Pad>,
}

impl Arena {
    pub fn in_pit(&self, x: f32, z: f32) -> bool {
        self.pits.iter().any(|p| p.contains(x, z))
    }

    pub fn near_pit(&self, x: f32, z: f32) -> bool {
        self.pit_zones.iter().any(|p| p.contains(x, z))
    }

    /// The nearest point over a pit to (x, z), if it is on the strip round that pit.
    pub fn pit_by(&self, x: f32, z: f32) -> Option<Vec3> {
        self.pits.iter().find(|p| Rect { x0: p.x0 - RIM, x1: p.x1 + RIM, z0: p.z0 - RIM, z1: p.z1 + RIM }.contains(x, z)).map(|p| Vec3::new(x.clamp(p.x0, p.x1), 0.0, z.clamp(p.z0, p.z1)))
    }

    /// On the strip of ground round a pit that its walls make (not over the pit itself).
    pub fn by_rim(&self, x: f32, z: f32) -> bool {
        self.pits.iter().any(|p| Rect { x0: p.x0 - RIM, x1: p.x1 + RIM, z0: p.z0 - RIM, z1: p.z1 + RIM }.contains(x, z) && !(p.x0 < x && x < p.x1 && p.z0 < z && z < p.z1))
    }

    pub fn on_hill(&self, p: Vec3) -> bool {
        self.hill.contains(p.x, p.z) && p.y > HILL_TOP - 1.0
    }

    /// In a saw's slot, or by it.
    pub fn by_saw(&self, x: f32, z: f32) -> bool {
        self.saws.iter().any(|s| s.zone().contains(x, z))
    }

    /// In a saw's slot, or on a vent that is hot at `t` (None: any time): where bots keep off.
    pub fn in_hazard(&self, x: f32, z: f32, t: Option<f64>) -> bool {
        self.saws.iter().any(|s| s.zone().contains(x, z)) || self.vents.iter().any(|v| t.is_none_or(|t| v.hot(t)) && v.area.contains(x, z))
    }
}

/// Every (±x, ±z) corner: the four quarters of a symmetric layout.
const QUARTERS: [(f32, f32); 4] = [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)];

/// The cage's layout and its pieces.
pub fn cage() -> (Arena, Vec<Solid>) {
    let mut s = Vec::new();
    let block = |style, x: f32, y: f32, z: f32, half: Vec3, yaw: f32| Solid::block(style, Vec3::new(x, y, z), half, yaw);
    let ramp = |x: f32, z: f32, half: Vec3, yaw: f32| Solid::ramp(Style::Ramp, Vec3::new(x, 0.0, z), half, yaw);

    // the outer wall, 8 m high, reaching a metre into the ground
    let (h, t) = (CAGE_HALF, 1.0);
    for k in [-1.0, 1.0] {
        s.push(block(Style::Wall, 0.0, 3.5, k * (h + t), Vec3::new(h + 2.0 * t, 4.5, t), 0.0));
        s.push(block(Style::Wall, k * (h + t), 3.5, 0.0, Vec3::new(t, 4.5, h), 0.0));
    }

    // the hill: a platform with a ramp up each side and a low block at each corner of its top
    s.push(block(Style::Platform, 0.0, (HILL_TOP - 1.0) * 0.5, 0.0, Vec3::new(HILL_HALF, (HILL_TOP + 1.0) * 0.5, HILL_HALF), 0.0));
    // as wide as the top: nothing coming up one at an angle runs off its side
    let up = Vec3::new(HILL_HALF, HILL_TOP * 0.5, 5.0);
    let foot = HILL_HALF + 2.0 * up.z;
    s.push(ramp(0.0, -(HILL_HALF + up.z), up, 0.0));
    s.push(ramp(0.0, HILL_HALF + up.z, up, PI));
    s.push(ramp(HILL_HALF + up.z, 0.0, up, -FRAC_PI_2));
    s.push(ramp(-(HILL_HALF + up.z), 0.0, up, FRAC_PI_2));
    for (sx, sz) in QUARTERS {
        s.push(block(Style::Cover, sx * 8.6, HILL_TOP + 0.6, sz * 8.6, Vec3::new(1.4, 0.6, 1.4), 0.0));
    }

    // a pit north and south of the hill, a kicker on either side of each: a ramp up to a deck that
    // ends at the rim. Either one launches you across onto the other; one that comes up short
    // drops into the pit rather than into the far deck's face. They stop short of the pit's sides,
    // so that nothing hanging over a side can catch on a deck's corner.
    let mut pits = Vec::new();
    let mut pit_zones = Vec::new();
    let (near, far) = (PIT_NEAR, PIT_NEAR + PIT_LONG);
    let kick = Vec3::new(PIT_HALF_W - 3.0, KICK_TOP * 0.5, 3.0);
    let deck = 2.0;
    for k in [-1.0f32, 1.0] {
        let (x0, x1) = (-PIT_HALF_W, PIT_HALF_W);
        pits.push(if k > 0.0 { Rect { x0, x1, z0: near, z1: far } } else { Rect { x0, x1, z0: -far, z1: -near } });
        let (z0, z1) = (near - deck - 2.0 * kick.z - 1.0, far + deck + 2.0 * kick.z + 1.0);
        pit_zones.push(if k > 0.0 { Rect { x0: -14.0, x1: 14.0, z0, z1 } } else { Rect { x0: -14.0, x1: 14.0, z0: -z1, z1: -z0 } });
        // its walls, from the floor up to the ground round it
        let p = *pits.last().unwrap();
        let low = (PIT_DEPTH + 1.0) * 0.5;
        let mut wall = |x0: f32, x1: f32, z0: f32, z1: f32| {
            s.push(block(Style::PitWall, (x0 + x1) * 0.5, -low, (z0 + z1) * 0.5, Vec3::new((x1 - x0) * 0.5, low, (z1 - z0) * 0.5), 0.0));
        };
        wall(p.x0 - RIM, p.x0, p.z0 - RIM, p.z1 + RIM);
        wall(p.x1, p.x1 + RIM, p.z0 - RIM, p.z1 + RIM);
        wall(p.x0, p.x1, p.z0 - RIM, p.z0);
        wall(p.x0, p.x1, p.z1, p.z1 + RIM);
        let (towards, away) = if k > 0.0 { (0.0, PI) } else { (PI, 0.0) };
        for (edge, dir, yaw) in [(near, -1.0, towards), (far, 1.0, away)] {
            let z = k * (edge + dir * deck * 0.5);
            s.push(block(Style::Ramp, 0.0, (KICK_TOP - 1.0) * 0.5, z, Vec3::new(kick.x, (KICK_TOP + 1.0) * 0.5, deck * 0.5), 0.0));
            s.push(ramp(0.0, k * (edge + dir * (deck + kick.z)), kick, yaw));
        }
    }

    // cover: an L round each diagonal, open at its corner wide enough that nothing is trapped in
    // it or wedged in the opening, and a wall out from the outer wall between every two spawns
    // along it, so that none looks down the wall at the next
    let tall = 2.25;
    for (sx, sz) in QUARTERS {
        s.push(block(Style::Cover, sx * 53.0, tall - 1.0, sz * 36.0, Vec3::new(5.0, tall, 0.75), 0.0));
        s.push(block(Style::Cover, sx * 36.0, tall - 1.0, sz * 53.0, Vec3::new(0.75, tall, 5.0), 0.0));
        s.push(block(Style::Cover, sx * 110.0, tall - 1.0, sz * 52.0, Vec3::new(10.0, tall, 0.75), 0.0));
        s.push(block(Style::Cover, sx * 52.0, tall - 1.0, sz * 110.0, Vec3::new(0.75, tall, 10.0), 0.0));
    }
    for sx in [-1.0, 1.0] {
        s.push(block(Style::Cover, sx * 72.0, tall - 1.0, 0.0, Vec3::new(0.75, tall, 9.0), 0.0));
        // beyond each pit, a wall either side of the way through
        s.push(block(Style::Cover, sx * 22.0, tall - 1.0, 88.0, Vec3::new(6.0, tall, 0.75), 0.0));
        s.push(block(Style::Cover, sx * 22.0, tall - 1.0, -88.0, Vec3::new(6.0, tall, 0.75), 0.0));
    }
    // a squat bunker out in each quarter, turned across the diagonal, and a low wall between it
    // and the hill, far enough from the L that the biggest vehicle can turn between them
    for (sx, sz) in QUARTERS {
        s.push(block(Style::Cover, sx * 66.0, 0.75, sz * 70.0, Vec3::new(4.0, 1.75, 4.0), FRAC_PI_4));
        s.push(block(Style::Cover, sx * 52.0, 0.25, sz * 12.0, Vec3::new(0.75, 1.25, 5.0), 0.0));
    }

    // crates: at the inner end of each of those walls, a few stacked, and one more pile out in
    // every corner
    let c = Vec3::splat(1.5);
    for (sx, sz) in QUARTERS {
        for (x, y, z, yaw) in [(94.0, 1.5, 50.0, 0.2), (97.5, 1.5, 55.5, 0.0), (94.0, 4.5, 50.0, 0.55), (50.0, 1.5, 94.0, -0.3), (55.5, 1.5, 97.5, 0.0), (80.0, 1.5, 86.0, 0.4)] {
            s.push(block(Style::Crate, sx * x, y, sz * z, c, yaw * sx * sz));
        }
    }

    let spawns = vec![
        Vec3::new(104.0, 0.0, 104.0),
        Vec3::new(-104.0, 0.0, -104.0),
        Vec3::new(104.0, 0.0, -104.0),
        Vec3::new(-104.0, 0.0, 104.0),
        Vec3::new(104.0, 0.0, 0.0),
        Vec3::new(-104.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 104.0),
        Vec3::new(0.0, 0.0, -104.0),
    ];
    // saws: one in a slot down either side of each pit, a lane's width off its edge, so that one
    // that catches you there may throw you in; and one across the gap between each pair of low
    // walls
    let mut saws = Vec::new();
    for (sx, sz) in QUARTERS {
        let phase = if sx * sz > 0.0 { 0.0 } else { 0.5 };
        saws.push(Saw { mid: Vec3::new(sx * (PIT_HALF_W + RIM + 3.5), 0.0, sz * (PIT_NEAR + PIT_LONG * 0.5)), dir: Vec3::Z, reach: 6.0, period: 7.0, phase });
    }
    for sx in [-1.0, 1.0] {
        saws.push(Saw { mid: Vec3::new(sx * 52.0, 0.0, 0.0), dir: Vec3::Z, reach: 5.0, period: 5.0, phase: if sx > 0.0 { 0.25 } else { 0.75 } });
    }
    // flame vents before the foot of every ramp up the hill: north and south burn together, then
    // east and west, so there is always a way up
    let mut vents = Vec::new();
    let vent_at = foot + 2.5;
    for (x, z, phase) in [(0.0, vent_at, 0.0), (0.0, -vent_at, 0.0), (vent_at, 0.0, 3.0), (-vent_at, 0.0, 3.0)] {
        let (hx, hz) = if x == 0.0 { (up.x, 2.0) } else { (2.0, up.x) };
        vents.push(Vent { area: Rect::around(x, z, hx, hz), phase });
    }

    // pick-ups: a repair inside every L, out of sight of the middle; energy in the way through
    // beyond each pit; overdrive between the hill and each pit, in sight of everyone
    let mut pads = Vec::new();
    for (sx, sz) in QUARTERS {
        pads.push(Pad { at: Vec3::new(sx * 46.0, 0.0, sz * 46.0), kind: PadKind::Repair });
    }
    for k in [-1.0, 1.0] {
        pads.push(Pad { at: Vec3::new(0.0, 0.0, k * 88.0), kind: PadKind::Energy });
        pads.push(Pad { at: Vec3::new(0.0, 0.0, k * 32.0), kind: PadKind::Overdrive });
    }

    let side = |x: f32| vec![Vec3::new(x, 0.0, -16.0), Vec3::new(x, 0.0, 0.0), Vec3::new(x, 0.0, 16.0)];
    let way = foot + 3.0;
    let arena = Arena {
        half: CAGE_HALF,
        pits,
        pit_zones,
        hill: Rect::around(0.0, 0.0, HILL_HALF, HILL_HALF),
        spawns,
        team_spawns: [side(-104.0), side(104.0)],
        hill_ways: vec![Vec3::new(0.0, 0.0, -way), Vec3::new(0.0, 0.0, way), Vec3::new(way, 0.0, 0.0), Vec3::new(-way, 0.0, 0.0)],
        saws,
        vents,
        pads,
    };
    (arena, s)
}

/// The cage's ground at (x, z): level concrete inside the walls with the pits sunk into it, and
/// beyond the walls the dunes `outer` would have there, rising away.
pub fn cage_height(x: f32, z: f32, pits: &[Rect], outer: f32) -> f32 {
    let d = x.abs().max(z.abs()) - CAGE_HALF;
    if d <= 2.0 {
        // a sample on a pit's rim stays up, so the pit opens exactly over its rectangle
        let inside = pits.iter().any(|p| p.x0 < x && x < p.x1 && p.z0 < z && z < p.z1);
        return if inside { -PIT_DEPTH } else { 0.0 };
    }
    let k = ((d - 2.0) / 120.0).clamp(0.0, 1.0);
    (outer + 10.0 + d * 0.05) * k * k * (3.0 - 2.0 * k)
}
