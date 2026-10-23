//! Remote ships are drawn slightly in the past, between two received snapshots, so their motion
//! stays smooth despite 30 Hz updates and network jitter.

use glam::{Quat, Vec3};
use std::collections::VecDeque;
use voxwing_shared::TICK_RATE;

/// How far behind the newest snapshot remote ships are drawn (ticks; 6 = 100 ms).
pub const INTERP_DELAY: f64 = 6.0;

/// The client's running estimate of the server tick it is currently observing.
#[derive(Default)]
pub struct ServerClock {
    pub tick: f64,
    pub latest: u32,
    pub synced: bool,
}

impl ServerClock {
    pub fn sync(&mut self, tick: u32) {
        self.tick = tick as f64;
        self.latest = tick;
        self.synced = true;
    }

    pub fn on_snapshot(&mut self, tick: u32) {
        if !self.synced {
            self.sync(tick);
        }
        self.latest = self.latest.max(tick);
    }

    pub fn advance(&mut self, dt: f32) {
        if !self.synced {
            return;
        }
        self.tick += dt as f64 * TICK_RATE as f64;
        let err = self.latest as f64 - self.tick;
        if err.abs() > 20.0 {
            self.tick = self.latest as f64;
        } else {
            self.tick += err * (dt as f64 * 2.0).min(1.0);
        }
    }

    pub fn render_tick(&self) -> f64 {
        self.tick - INTERP_DELAY
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub tick: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub flags: u8,
    pub throttle: u8,
}

#[derive(Default)]
pub struct Interp {
    buf: VecDeque<Sample>,
}

impl Interp {
    pub fn push(&mut self, s: Sample) {
        if self.buf.back().is_some_and(|b| b.tick >= s.tick) {
            if let Some(i) = self.buf.iter().position(|b| b.tick >= s.tick) {
                if self.buf[i].tick == s.tick {
                    self.buf[i] = s;
                } else {
                    self.buf.insert(i, s);
                }
            }
        } else {
            self.buf.push_back(s);
        }
        while self.buf.len() > 40 {
            self.buf.pop_front();
        }
    }

    pub fn newest(&self) -> Option<&Sample> {
        self.buf.back()
    }

    /// Pose at fractional tick `t`. Hermite interpolation between neighbours (using their
    /// velocities), short extrapolation past the newest sample.
    pub fn sample(&self, t: f64) -> Option<(Vec3, Quat, Vec3, u8)> {
        let first = self.buf.front()?;
        if t <= first.tick as f64 {
            return Some((first.pos, first.rot, first.vel, first.flags));
        }
        for w in self.buf.iter().zip(self.buf.iter().skip(1)) {
            let (a, b) = w;
            if t >= a.tick as f64 && t <= b.tick as f64 {
                let span = (b.tick - a.tick) as f32;
                let u = ((t - a.tick as f64) as f32 / span).clamp(0.0, 1.0);
                let h = span / TICK_RATE as f32;
                let pos = hermite(a.pos, a.vel * h, b.pos, b.vel * h, u);
                return Some((pos, a.rot.slerp(b.rot, u), a.vel.lerp(b.vel, u), if u < 0.5 { a.flags } else { b.flags }));
            }
        }
        let last = self.buf.back()?;
        let ahead = ((t - last.tick as f64) as f32 / TICK_RATE as f32).min(0.3);
        Some((last.pos + last.vel * ahead, last.rot, last.vel, last.flags))
    }
}

fn hermite(p0: Vec3, m0: Vec3, p1: Vec3, m1: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    p0 * (2.0 * t3 - 3.0 * t2 + 1.0) + m0 * (t3 - 2.0 * t2 + t) + p1 * (-2.0 * t3 + 3.0 * t2) + m1 * (t3 - t2)
}
