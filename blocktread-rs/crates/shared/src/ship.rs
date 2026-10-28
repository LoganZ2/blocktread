use crate::analysis::{Stats, compute_stats, occupancy};
use crate::balance::balance;
use crate::blocks::{BlockType, Design, Gadget};
use crate::grid::{DIRS6, Occupancy, raycast};
use crate::weapons::{Mount, mounts};
use glam::{IVec3, Quat, Vec3};
use std::sync::Arc;

/// A ship's blocks at runtime: which are still attached, their hit points, and the derived stats.
///
/// Every block change goes through `destroy` + `settle`. `settle` is deterministic, so a client that
/// applies the same destroyed blocks as the server ends up with exactly the same ship.
#[derive(Clone, Debug)]
pub struct ShipBody {
    pub design: Arc<Design>,
    pub alive: Vec<bool>,
    pub hp: Vec<f32>,
    pub occ: Occupancy,
    /// Centre of mass of the full design. The ship's local frame stays centred here even as
    /// blocks fall off, so transforms never jump.
    pub com0: Vec3,
    pub stats: Stats,
    /// Each block's full hit points (a cockpit's depend on what the whole ship cost).
    pub block_max: Vec<f32>,
    pub max_hp: f32,
    pub cockpit: Option<usize>,
    pub engines0: usize,
    /// Bounding radius around `com0`.
    pub radius: f32,
    pub bolt_speed: f32,
    pub gun_range: f32,
    pub has_missile: bool,
    /// The wheels and hover pads still on, and the drive power the wheels add up to.
    pub movers: Vec<usize>,
    pub power: f32,
    /// Every weapon's mount: how it turns and where it can point.
    pub mounts: Vec<Mount>,
    /// How many working gadgets of each kind it has (by `Gadget::index`).
    pub gadgets: [u8; 3],
}

impl ShipBody {
    pub fn new(design: Arc<Design>) -> Self {
        let n = design.blocks.len();
        let alive = vec![true; n];
        let full = compute_stats(&design.blocks, &alive);
        let block_max: Vec<f32> = design.blocks.iter().map(|b| balance().block_hp(b.ty, full.cost)).collect();
        let mut body = ShipBody {
            hp: block_max.clone(),
            max_hp: block_max.iter().sum(),
            block_max,
            occ: Occupancy::default(),
            com0: full.com,
            engines0: full.engines.len(),
            stats: full,
            alive,
            design,
            cockpit: None,
            radius: 1.0,
            bolt_speed: 400.0,
            gun_range: 0.0,
            has_missile: false,
            movers: Vec::new(),
            power: 0.0,
            mounts: Vec::new(),
            gadgets: [0; 3],
        };
        body.rebuild();
        body
    }

    pub fn rebuild(&mut self) {
        let blocks = &self.design.blocks;
        self.stats = compute_stats(blocks, &self.alive);
        self.occ = occupancy(blocks, &self.alive);
        self.cockpit = (0..blocks.len()).find(|&i| self.alive[i] && blocks[i].ty == BlockType::Cockpit);
        let mut r: f32 = 0.0;
        for i in 0..blocks.len() {
            if self.alive[i] {
                r = r.max(self.local(i).length());
            }
        }
        self.radius = r + 0.9;
        let guns: Vec<_> = self.stats.weapons.iter().map(|&i| blocks[i].ty.weapon().unwrap()).filter(|w| !w.homing).collect();
        self.bolt_speed = if guns.is_empty() { 400.0 } else { guns.iter().map(|w| w.speed).sum::<f32>() / guns.len() as f32 };
        self.gun_range = guns.iter().map(|w| w.speed * w.life).fold(0.0, f32::max) * 0.9;
        self.has_missile = self.stats.weapons.iter().any(|&i| blocks[i].ty.weapon().unwrap().homing);
        self.movers = self.stats.wheels.clone();
        self.power = self.stats.power;
        self.mounts = mounts(blocks, &self.alive, &self.occ);
        self.gadgets = [0; 3];
        for &i in &self.stats.gadgets {
            self.gadgets[blocks[i].ty.gadget().unwrap().kind.index()] += 1;
        }
    }

    /// How many working gadgets of this kind it has.
    pub fn gadget(&self, g: Gadget) -> u8 {
        self.gadgets[g.index()]
    }

    pub fn repair(&mut self) {
        self.alive.iter_mut().for_each(|a| *a = true);
        self.hp.clone_from(&self.block_max);
        self.rebuild();
    }

    /// Whether anything still drives it: a wheel, or hover pads with thrusters to push them.
    pub fn can_drive(&self) -> bool {
        self.power > 0.0 || (self.stats.thrust > 0.0 && self.movers.iter().any(|&i| self.design.blocks[i].ty.drive().is_some_and(|d| d.hover)))
    }

    pub fn cockpit_alive(&self) -> bool {
        self.cockpit.is_some()
    }

    /// Block centre in the ship's local frame (origin at `com0`).
    pub fn local(&self, i: usize) -> Vec3 {
        self.design.blocks[i].pos().as_vec3() - self.com0
    }

    pub fn world(&self, pos: Vec3, rot: Quat, i: usize) -> Vec3 {
        pos + rot * self.local(i)
    }

    /// World point -> fractional grid coordinates.
    pub fn to_grid(&self, pos: Vec3, rot: Quat, p: Vec3) -> Vec3 {
        rot.inverse() * (p - pos) + self.com0
    }

    pub fn block_at(&self, c: IVec3) -> Option<usize> {
        self.occ.get(c)
    }

    pub fn hp_total(&self) -> f32 {
        (0..self.hp.len()).filter(|&i| self.alive[i]).map(|i| self.hp[i].max(0.0)).sum()
    }

    pub fn hp_frac(&self, i: usize) -> f32 {
        (self.hp[i] / self.block_max[i]).clamp(0.0, 1.0)
    }

    /// First alive block hit by the segment a->b, with the world-space hit point.
    pub fn ray(&self, pos: Vec3, rot: Quat, a: Vec3, b: Vec3) -> Option<(usize, Vec3)> {
        let o = self.to_grid(pos, rot, a);
        let e = self.to_grid(pos, rot, b);
        let d = e - o;
        let len = d.length();
        if len < 1e-6 {
            return None;
        }
        let hit = raycast(o, d / len, len, |c| self.occ.get(c))?;
        Some((hit.val, a.lerp(b, (hit.t / len).clamp(0.0, 1.0))))
    }

    /// The first `n` alive blocks along the segment a->b, nearest first (a piercing round).
    pub fn ray_through(&self, pos: Vec3, rot: Quat, a: Vec3, b: Vec3, n: usize) -> Vec<usize> {
        let o = self.to_grid(pos, rot, a);
        let d = self.to_grid(pos, rot, b) - o;
        let len = d.length();
        let mut hits = Vec::new();
        if len < 1e-6 {
            return hits;
        }
        // the walk visits cells in order; stop it once enough blocks are collected
        raycast(o, d / len, len, |c| {
            if let Some(i) = self.occ.get(c) {
                hits.push(i);
            }
            (hits.len() >= n).then_some(())
        });
        hits
    }

    /// Alive blocks within `r` (grid units) of world point `p`, with their distance.
    pub fn within(&self, pos: Vec3, rot: Quat, p: Vec3, r: f32) -> Vec<(usize, f32)> {
        let g = self.to_grid(pos, rot, p);
        (0..self.alive.len())
            .filter(|&i| self.alive[i])
            .map(|i| (i, self.design.blocks[i].pos().as_vec3().distance(g)))
            .filter(|&(_, d)| d < r)
            .collect()
    }

    /// Damage one block. Returns true when it dropped to zero (caller then calls `destroy`).
    pub fn damage(&mut self, i: usize, dmg: f32) -> bool {
        if !self.alive[i] {
            return false;
        }
        self.hp[i] -= dmg;
        self.hp[i] <= 0.0
    }

    pub fn destroy(&mut self, i: usize) {
        if self.alive[i] {
            self.alive[i] = false;
            self.hp[i] = 0.0;
            let p = self.design.blocks[i].pos();
            self.occ.set(p, None);
        }
    }

    /// After a batch of `destroy` calls: drop every block no longer connected to the cockpit and
    /// recompute stats. Returns the detached pieces (each a connected group) for debris effects.
    /// If the cockpit is gone, nothing is detached and the caller should treat the ship as dead.
    pub fn settle(&mut self) -> Vec<Vec<usize>> {
        let blocks = &self.design.blocks;
        let Some(root) = (0..blocks.len()).find(|&i| self.alive[i] && blocks[i].ty == BlockType::Cockpit) else {
            self.rebuild();
            return Vec::new();
        };
        let mut seen = vec![false; blocks.len()];
        seen[root] = true;
        let mut stack = vec![root];
        while let Some(i) = stack.pop() {
            for d in DIRS6 {
                if let Some(j) = self.occ.get(blocks[i].pos() + d)
                    && !seen[j]
                {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let mut loose: Vec<usize> = (0..blocks.len()).filter(|&i| self.alive[i] && !seen[i]).collect();
        let mut pieces = Vec::new();
        while let Some(first) = loose.pop() {
            let mut piece = vec![first];
            let mut k = 0;
            while k < piece.len() {
                let p = blocks[piece[k]].pos();
                let mut j = 0;
                while j < loose.len() {
                    if DIRS6.iter().any(|&d| blocks[loose[j]].pos() == p + d) {
                        piece.push(loose.swap_remove(j));
                    } else {
                        j += 1;
                    }
                }
                k += 1;
            }
            pieces.push(piece);
        }
        for piece in &pieces {
            for &i in piece {
                self.alive[i] = false;
            }
        }
        self.rebuild();
        pieces
    }
}
