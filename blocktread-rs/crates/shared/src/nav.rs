//! Where a vehicle can drive in an arena, on a grid of 2 m cells, and the way across it. Bots
//! plan on it, so that they go round the pits and the cover rather than nosing into them, and
//! find the ramps up the hill.

use crate::terrain::Terrain;
use glam::Vec3;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};

/// The grid's spacing.
pub const STEP: f32 = 2.0;
/// The most the ground may rise from one cell to the next for a vehicle to drive across, and in
/// any one stretch of `CREEP` along the way (no sheer step, such as a ramp's side, however low the
/// ramp is on the far side of it).
const CLIMB: f32 = 0.9;
const CREEP: f32 = 0.5;
const STEP_UP: f32 = 0.4;
/// The eight ways out of a cell: the four sides first, then the corners.
const WAYS: [(i32, i32); 8] = [(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (-1, -1), (1, -1)];
/// A plan gives up after looking at this many cells, and heads for the nearest it reached. Enough
/// for the whole grid: the way up the hill is up a narrow ramp, dear enough that the search has
/// to rule out most of the floor before it takes it.
const SEARCH: usize = 16000;

pub struct Nav {
    n: usize,
    half: f32,
    /// The ground's height at each cell's middle (a piece's top where one stands).
    h: Vec<f32>,
    /// Which of `WAYS` a vehicle can drive from each cell, a bit each.
    links: Vec<u8>,
    /// How many cells it is to the nearest edge (a wall, a step, a drop): 1 on an edge, 0 by a
    /// pit or in a saw's slot, where no vehicle belongs (one there can still find its way out).
    /// The vents are open ground: the way up the hill is across one, and a bot waits for it to go
    /// out.
    room: Vec<u8>,
}

impl Nav {
    /// The grid over the arena's floor; None on a map without walls.
    pub fn build(ground: &Terrain) -> Option<Nav> {
        let arena = ground.arena.as_ref()?;
        let half = arena.half;
        let n = (2.0 * half / STEP) as usize;
        let mid = |i: usize| -half + (i as f32 + 0.5) * STEP;
        let mut h = Vec::with_capacity(n * n);
        let mut open = Vec::with_capacity(n * n);
        let mut keep_off = Vec::with_capacity(n * n);
        for j in 0..n {
            for i in 0..n {
                let (x, z) = (mid(i), mid(j));
                h.push(ground.surface(x, z));
                open.push(!arena.in_pit(x, z));
                keep_off.push(arena.near_pit(x, z) || arena.by_saw(x, z));
            }
        }
        // a way is open when the ground steps no more than a vehicle climbs and nothing stands
        // across it at knee height (cover thinner than a cell falls between the samples)
        let mut links = vec![0u8; n * n];
        for j in 0..n {
            for i in 0..n {
                let c = j * n + i;
                if !open[c] {
                    continue;
                }
                for (k, &(di, dj)) in WAYS.iter().enumerate() {
                    let (i2, j2) = (i as i32 + di, j as i32 + dj);
                    if i2 < 0 || j2 < 0 || i2 >= n as i32 || j2 >= n as i32 {
                        continue;
                    }
                    let c2 = j2 as usize * n + i2 as usize;
                    if !open[c2] || (h[c2] - h[c]).abs() > CLIMB * (di.abs() + dj.abs()) as f32 {
                        continue;
                    }
                    let (x0, z0, x1, z1) = (mid(i), mid(j), mid(i2 as usize), mid(j2 as usize));
                    let len = STEP * ((di * di + dj * dj) as f32).sqrt();
                    let parts = (len / CREEP).ceil() as usize;
                    let along: Vec<f32> = (0..=parts).map(|k| k as f32 / parts as f32).map(|f| ground.surface(x0 + (x1 - x0) * f, z0 + (z1 - z0) * f)).collect();
                    if along.windows(2).any(|w| (w[1] - w[0]).abs() > STEP_UP) {
                        continue;
                    }
                    let (a, b) = (Vec3::new(mid(i), h[c] + CLIMB, mid(j)), Vec3::new(mid(i2 as usize), h[c2] + CLIMB, mid(j2 as usize)));
                    if ground.solids_near((a + b) * 0.5, STEP).any(|s| s.clip(a, b).is_some()) {
                        continue;
                    }
                    links[c] |= 1 << k;
                }
            }
        }
        // corners only where both sides round them are open, so a path never cuts one
        for l in &mut links {
            for k in 4..8 {
                let (a, b) = (k - 4, (k - 3) % 4);
                if *l & (1 << a) == 0 || *l & (1 << b) == 0 {
                    *l &= !(1 << k);
                }
            }
        }
        // how far from an edge: outward from every cell that can't drive all four ways
        let mut room = vec![u8::MAX; n * n];
        let mut queue = VecDeque::new();
        for c in 0..n * n {
            if !open[c] || keep_off[c] {
                room[c] = 0;
                queue.push_back(c);
            } else if links[c] & 0x0f != 0x0f {
                room[c] = 1;
                queue.push_back(c);
            }
        }
        while let Some(c) = queue.pop_front() {
            for k in 0..8 {
                if links[c] & (1 << k) != 0 {
                    let c2 = step(c, n, k);
                    if room[c2] == u8::MAX {
                        room[c2] = room[c].max(1) + 1;
                        queue.push_back(c2);
                    }
                }
            }
        }
        // whatever no edge reached (an island) has room enough
        Some(Nav { n, half, h, links, room })
    }

    fn cell(&self, p: Vec3) -> usize {
        let at = |v: f32| (((v + self.half) / STEP) as i32).clamp(0, self.n as i32 - 1) as usize;
        at(p.z) * self.n + at(p.x)
    }

    fn centre(&self, c: usize) -> Vec3 {
        let (i, j) = (c % self.n, c / self.n);
        Vec3::new(-self.half + (i as f32 + 0.5) * STEP, self.h[c], -self.half + (j as f32 + 0.5) * STEP)
    }

    /// How many cells a vehicle reaching `half_w` out to either side needs between its middle
    /// and an edge.
    pub fn room_for(half_w: f32) -> u8 {
        ((half_w + 0.5) / STEP + 1.0).ceil() as u8
    }

    /// The way from `from` to `to` for a vehicle that needs `need` cells of room (see
    /// `room_for`), as the middles of the cells along it; through tighter places only when there
    /// is no other way. Towards the nearest place it can reach when it can't reach `to` (on a
    /// crate, say).
    pub fn path(&self, from: Vec3, to: Vec3, need: u8) -> Vec<Vec3> {
        let n = self.n;
        let (start, goal) = (self.cell(from), self.cell(to));
        let gp = self.centre(goal);
        let guess = |c: usize| self.centre(c).with_y(0.0).distance(gp.with_y(0.0));
        let mut cost = vec![f32::MAX; n * n];
        let mut came = vec![usize::MAX; n * n];
        let mut open = BinaryHeap::new();
        cost[start] = 0.0;
        open.push((Reverse(guess(start).to_bits()), start));
        let mut best = (guess(start), start);
        let mut seen = 0;
        while let Some((_, c)) = open.pop() {
            if c == goal {
                best = (0.0, c);
                break;
            }
            seen += 1;
            if seen > SEARCH {
                break;
            }
            let g = guess(c);
            if g < best.0 {
                best = (g, c);
            }
            for k in 0..8 {
                if self.links[c] & (1 << k) == 0 {
                    continue;
                }
                let c2 = step(c, n, k);
                // tight spots cost more, much more where the vehicle doesn't fit
                let r = self.room[c2];
                let tight = if r < need { 6.0 } else { 1.0 + 2.0 / r as f32 };
                let c2cost = cost[c] + STEP * if k < 4 { 1.0 } else { std::f32::consts::SQRT_2 } * tight;
                if c2cost < cost[c2] {
                    cost[c2] = c2cost;
                    came[c2] = c;
                    open.push((Reverse((c2cost + guess(c2)).to_bits()), c2));
                }
            }
        }
        let mut way = Vec::new();
        let mut c = best.1;
        while c != usize::MAX {
            way.push(self.centre(c));
            c = came[c];
        }
        way.reverse();
        way
    }
}

/// The cell one step from `c` along `WAYS[k]` (the caller knows it is on the grid).
fn step(c: usize, n: usize, k: usize) -> usize {
    let (di, dj) = WAYS[k];
    ((c / n) as i32 + dj) as usize * n + ((c % n) as i32 + di) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{HILL_TOP, PIT_NEAR};

    #[test]
    fn the_way_goes_round_a_pit_and_up_a_ramp() {
        let ground = Terrain::cage(1);
        let nav = ground.nav.as_ref().unwrap();
        // from one side of the north pit to the other: round it, never across
        let way = nav.path(Vec3::new(-30.0, 0.0, 56.0), Vec3::new(30.0, 0.0, 56.0), 3);
        let arena = ground.arena.as_ref().unwrap();
        assert!((way.last().unwrap().x - 30.0).abs() < 2.0, "should get there");
        assert!(way.iter().all(|p| !arena.near_pit(p.x, p.z)), "should keep off the pit");
        assert!(way.iter().any(|p| p.z < PIT_NEAR - 8.0 || p.z > PIT_NEAR + 24.0), "should go round an end");
        // onto the hill: up one of its ramps
        let way = nav.path(Vec3::new(40.0, 0.0, 40.0), Vec3::ZERO, 3);
        assert!((way.last().unwrap().y - HILL_TOP).abs() < 0.1, "should get up onto the hill");
        assert!(way.windows(2).all(|w| (w[1].y - w[0].y).abs() < 1.3), "should climb a ramp, not a wall");
    }
}

