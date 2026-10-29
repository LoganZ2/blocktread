//! Client-side prediction for the local ship.
//!
//! Every tick the client simulates its own ship immediately with the shared vehicle code and keeps
//! (input, resulting state) per tick. When the server's state for tick S arrives, the client
//! compares it with what it predicted for S. If they differ it adopts the server's state at S and
//! replays every newer input on top. The visible jump is hidden by an offset that fades out.

use glam::{Quat, Vec3};
use std::collections::VecDeque;
use blocktread_shared::DT;
use blocktread_shared::drive::step_drive;
use blocktread_shared::flight::{InputCmd, Kin, collide_rocks};
use blocktread_shared::ship::ShipBody;
use blocktread_shared::terrain::Terrain;
use blocktread_shared::weapons::{Shot, WeaponState, step_weapons};
use blocktread_shared::world::Rock;

/// Inputs should reach the server this many ticks before it needs them (jitter buffer).
pub const TARGET_LEAD: f32 = 3.0;
const HISTORY: usize = 240;

#[derive(Clone)]
struct Frame {
    tick: u32,
    cmd: InputCmd,
    kin: Kin,
    ws: WeaponState,
}

pub struct Predictor {
    pub ship: u16,
    /// Next tick to simulate.
    pub tick: u32,
    accum: f32,
    /// Time dilation (about 0.9 to 1.1) to keep inputs arriving just ahead of the server.
    pub speed: f32,
    pub kin: Kin,
    pub prev: Kin,
    pub ws: WeaponState,
    frames: VecDeque<Frame>,
    lead_ema: f32,
    pub off_pos: Vec3,
    pub off_rot: Quat,
    /// Extra ticks to simulate immediately (after (re)initialising behind the target tick).
    pub catch_up: u32,
    pub corrections: u32,
    pub checks: u32,
    pub sum_err: f32,
    pub max_err: f32,
}

impl Predictor {
    pub fn new(ship: u16, kin: Kin, body: &ShipBody, next_tick: u32) -> Self {
        Predictor {
            ship,
            tick: next_tick,
            accum: 0.0,
            speed: 1.0,
            kin,
            prev: kin,
            ws: WeaponState::new(body),
            frames: VecDeque::new(),
            lead_ema: TARGET_LEAD,
            off_pos: Vec3::ZERO,
            off_rot: Quat::IDENTITY,
            catch_up: 0,
            corrections: 0,
            checks: 0,
            sum_err: 0.0,
            max_err: 0.0,
        }
    }

    /// How many fixed ticks to run this frame.
    pub fn steps_due(&mut self, real_dt: f32) -> u32 {
        self.accum += real_dt * self.speed;
        let mut n = 0;
        while self.accum >= DT && n < 8 {
            self.accum -= DT;
            n += 1;
        }
        if self.accum > DT * 4.0 {
            self.accum = 0.0;
        }
        n
    }

    pub fn step(&mut self, cmd: InputCmd, body: &ShipBody, ground: &Terrain, rocks: &[Rock], shots: &mut Vec<Shot>) {
        self.prev = self.kin;
        step_drive(&mut self.kin, body, &cmd, ground, DT);
        collide_rocks(&mut self.kin, body, rocks);
        step_weapons(&mut self.kin, body, &mut self.ws, &cmd, DT, cmd.tick, false, shots);
        self.frames.push_back(Frame { tick: cmd.tick, cmd, kin: self.kin, ws: self.ws.clone() });
        while self.frames.len() > HISTORY {
            self.frames.pop_front();
        }
        self.tick = cmd.tick + 1;
    }

    pub fn recent_cmds(&self, n: usize) -> Vec<InputCmd> {
        self.frames.iter().rev().take(n).map(|f| f.cmd).collect::<Vec<_>>().into_iter().rev().collect()
    }

    pub fn last_cmd(&self) -> Option<InputCmd> {
        self.frames.back().map(|f| f.cmd)
    }

    /// Feed the server's "how early do your inputs arrive" report. Returns true when the client
    /// is so far off that it should re-initialise its tick.
    pub fn on_lead(&mut self, lead: i16) -> bool {
        let lead = lead as f32;
        self.lead_ema += (lead - self.lead_ema) * 0.1;
        self.speed = 1.0 + ((TARGET_LEAD - self.lead_ema) * 0.015).clamp(-0.1, 0.1);
        (lead - TARGET_LEAD).abs() > 40.0
    }

    pub fn lead(&self) -> f32 {
        self.lead_ema
    }

    pub fn reconcile(&mut self, server_tick: u32, server: &Kin, body: &ShipBody, ground: &Terrain, rocks: &[Rock]) {
        let Some(idx) = self.frames.iter().position(|f| f.tick == server_tick) else { return };
        let f = &self.frames[idx];
        let err = f.kin.pos.distance(server.pos);
        let rerr = f.kin.rot.angle_between(server.rot);
        self.checks += 1;
        self.sum_err += err;
        self.max_err = self.max_err.max(err);
        let same = err < 0.05 && rerr < 0.004 && (f.kin.energy - server.energy).abs() < 2.0 && (f.kin.shield - server.shield).abs() < 2.0;
        if !same {
            self.corrections += 1;
            let (old_pos, old_rot) = (self.kin.pos, self.kin.rot);
            let mut kin = *server;
            let mut ws = f.ws.clone();
            self.frames[idx].kin = kin;
            let mut scratch = Vec::new();
            for j in idx + 1..self.frames.len() {
                let cmd = self.frames[j].cmd;
                step_drive(&mut kin, body, &cmd, ground, DT);
                collide_rocks(&mut kin, body, rocks);
                step_weapons(&mut kin, body, &mut ws, &cmd, DT, cmd.tick, false, &mut scratch);
                self.frames[j].kin = kin;
                self.frames[j].ws = ws.clone();
            }
            let n = self.frames.len();
            self.prev = if n >= 2 { self.frames[n - 2].kin } else { kin };
            self.kin = kin;
            self.ws = ws;
            self.off_pos += old_pos - kin.pos;
            self.off_rot = (self.off_rot * old_rot * kin.rot.inverse()).normalize();
            // a large error is not worth smoothing over
            if self.off_pos.length() > 25.0 {
                self.off_pos = Vec3::ZERO;
                self.off_rot = Quat::IDENTITY;
            }
        }
        self.frames.drain(..idx);
    }

    pub fn decay(&mut self, dt: f32) {
        let k = (-dt * 10.0).exp();
        self.off_pos *= k;
        self.off_rot = Quat::IDENTITY.slerp(self.off_rot, k);
    }

    /// Smoothed pose for rendering: interpolated between the last two ticks plus the fading
    /// correction offset.
    pub fn render(&self) -> (Vec3, Quat) {
        let a = (self.accum / DT).clamp(0.0, 1.0);
        let pos = self.prev.pos.lerp(self.kin.pos, a) + self.off_pos;
        let rot = self.off_rot * self.prev.rot.slerp(self.kin.rot, a);
        (pos, rot)
    }

    /// The tick `render` shows, as the server's snapshots count them (`kin` is the state after
    /// tick `tick - 1`, `prev` the one before).
    pub fn render_tick(&self) -> f64 {
        self.tick as f64 - 2.0 + (self.accum / DT).clamp(0.0, 1.0) as f64
    }
}
