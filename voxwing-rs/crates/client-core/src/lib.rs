//! Everything the client does that isn't drawing: the network session, the local ship's
//! prediction, remote ships' interpolation, and applying the server's block-damage events.
//! It has no engine dependency, so the Bevy client and the headless bot harness share it.

pub mod interp;
pub mod net;
pub mod predict;

use glam::{Quat, Vec3};
use interp::{Interp, Sample, ServerClock};
use net::{NetClient, NetIn};
use predict::{Predictor, TARGET_LEAD};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use voxwing_shared::blocks::{BlockType, Design};
use voxwing_shared::flight::{BTN_FIRE, InputCmd};
use voxwing_shared::protocol::*;
use voxwing_shared::ship::ShipBody;
use voxwing_shared::weapons::{Shot, WeaponState, step_weapons};
use voxwing_shared::world::{Asteroid, generate_asteroids, tick_time};
use voxwing_shared::{DT, PROTOCOL_VERSION, TICK_RATE};

pub struct CoreShip {
    pub id: u16,
    pub team: u16,
    pub name: String,
    pub owner: Option<u16>,
    pub body: ShipBody,
    interp: Interp,
    pub shield_frac: f32,
    pub firing: bool,
    pub boosting: bool,
    pub throttle: f32,
    ws: WeaponState,
    cos_tick: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
}

pub struct MissileView {
    pub pos: Vec3,
    pub vel: Vec3,
    tick: u32,
}

/// Things the presentation layer should react to (spawn effects, update meshes, show text).
#[derive(Clone, Debug)]
pub enum CoreEvent {
    Welcome,
    ShipSpawned(u16),
    ShipRemoved(u16),
    /// The ship is already gone from `ships`; its last pose and remaining blocks travel along.
    ShipKilled { id: u16, killer: Option<u16>, pos: Vec3, vel: Vec3, blocks: Vec<(Vec3, u32)> },
    BlocksDamaged { ship: u16, blocks: Vec<usize> },
    BlocksDestroyed { ship: u16, blocks: Vec<usize>, pieces: Vec<Vec<usize>> },
    ShipRepaired(u16),
    ShieldHit { ship: u16, pos: Vec3 },
    Explosion { pos: Vec3, size: f32 },
    Crash { pos: Vec3 },
    Shot { ship: u16, weapon: BlockType, pos: Vec3, vel: Vec3, own: bool },
    Announce { title: String, sub: String },
    Feed { text: String, bad: bool },
    YourShip(Option<u16>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Connecting,
    /// Connected, between matches (queueing, picking, reading results).
    Lobby,
    InGame,
    Rejected(String),
    Closed(String),
}

/// Where the player is in the match cycle: queue, ship pick, match, results.
#[derive(Clone, Debug)]
pub enum Phase {
    Idle,
    /// `waited`: seconds in the queue; `players`: how many wait for this mode; `fill_in`: seconds
    /// until AI fills the match.
    Queued { mode: Mode, tier: Option<String>, waited: f32, players: u32, fill_in: f32 },
    Picking(Pick),
    Playing,
    /// The match ended: who won, and the final scoreboard.
    Over { winner: String, rows: Vec<ScoreRow> },
}

/// The ship pick: every seat, the ship each has chosen so far, and who has locked in.
#[derive(Clone, Debug)]
pub struct Pick {
    pub mode: Mode,
    pub tier: Option<String>,
    /// This player's seat.
    pub you: usize,
    pub slots: Vec<SlotInfo>,
    pub ships: Vec<Option<Design>>,
    pub ready: Vec<bool>,
    /// Seconds until the match starts anyway.
    pub left: f32,
}

#[derive(Default, Clone, Debug)]
pub struct NetStats {
    pub snapshots: u32,
    pub bytes_in: u64,
    pub inputs_sent: u32,
}

pub struct ClientCore {
    net: NetClient,
    /// Joined as a spectator: never gets a ship.
    pub watching: bool,
    pub state: ConnState,
    pub phase: Phase,
    /// The last queue joined (mode, tier), for "play again".
    pub queued_for: Option<(Mode, Option<String>)>,
    /// A message from the server to show (a pick refused, say); the screen takes it.
    pub notice: Option<String>,
    pub pid: Option<u16>,
    pub mode: Option<Mode>,
    pub my_ship: Option<u16>,
    pub ships: BTreeMap<u16, CoreShip>,
    pub missiles: BTreeMap<u16, MissileView>,
    pub rocks: Vec<Asteroid>,
    pub clock: ServerClock,
    pub pred: Option<Predictor>,
    pub scores: Vec<ScoreRow>,
    pub wave: u32,
    pub enemies: u32,
    /// Kills that win the match (0 = none) and seconds left on its clock (0 = no clock).
    pub limit: u32,
    pub left: f32,
    pub events: Vec<CoreEvent>,
    pub stats: NetStats,
    shots: Vec<Shot>,
}

impl ClientCore {
    /// Connect to play: then `queue`, `pick` and `ready` take the player into a match.
    pub fn connect(addr: &str, name: &str, latency: Duration) -> Self {
        Self::start(addr, ClientMsg::Hello { version: PROTOCOL_VERSION, name: name.to_string(), balance: voxwing_shared::balance::hash() }, latency)
    }

    /// Join to watch instead of fly.
    pub fn watch(addr: &str, name: &str, latency: Duration) -> Self {
        Self::start(addr, ClientMsg::Watch { version: PROTOCOL_VERSION, name: name.to_string(), balance: voxwing_shared::balance::hash() }, latency)
    }

    fn start(addr: &str, hello: ClientMsg, latency: Duration) -> Self {
        ClientCore {
            watching: matches!(hello, ClientMsg::Watch { .. }),
            net: NetClient::connect(addr.to_string(), hello, latency),
            state: ConnState::Connecting,
            phase: Phase::Idle,
            queued_for: None,
            notice: None,
            pid: None,
            mode: None,
            my_ship: None,
            ships: BTreeMap::new(),
            missiles: BTreeMap::new(),
            rocks: Vec::new(),
            clock: ServerClock::default(),
            pred: None,
            scores: Vec::new(),
            wave: 0,
            enemies: 0,
            limit: 0,
            left: 0.0,
            events: Vec::new(),
            stats: NetStats::default(),
            shots: Vec::new(),
        }
    }

    pub fn rtt(&self) -> f32 {
        self.net.rtt()
    }

    pub fn leave(&self) {
        self.net.leave();
    }

    /// Look for a match. Ignored unless idle (or reading the last match's results).
    pub fn queue(&mut self, mode: Mode, tier: Option<String>) {
        if !matches!(self.phase, Phase::Idle | Phase::Over { .. }) || self.watching {
            return;
        }
        self.net.send(ClientMsg::Queue { mode, tier: tier.clone() });
        self.queued_for = Some((mode, tier.clone()));
        self.phase = Phase::Queued { mode, tier, waited: 0.0, players: 1, fill_in: 0.0 };
    }

    pub fn unqueue(&mut self) {
        if matches!(self.phase, Phase::Queued { .. }) {
            self.net.send(ClientMsg::Unqueue);
            self.phase = Phase::Idle;
        }
    }

    /// During the ship pick: the ship to fly (everyone sees it; it can change until `ready`).
    pub fn pick(&mut self, design: Design) {
        if let Phase::Picking(p) = &self.phase
            && !p.ready[p.you]
        {
            self.net.send(ClientMsg::Pick { design });
        }
    }

    /// Lock in the picked ship (sent after a `pick`, the server takes them in order).
    pub fn ready(&mut self) {
        if let Phase::Picking(p) = &self.phase
            && !p.ready[p.you]
        {
            self.net.send(ClientMsg::Ready);
        }
    }

    /// A spectator whose match ended: watch the next one.
    pub fn watch_again(&mut self) {
        if self.watching && matches!(self.phase, Phase::Over { .. }) {
            self.net.send(ClientMsg::Watch { version: PROTOCOL_VERSION, name: String::new(), balance: voxwing_shared::balance::hash() });
            self.phase = Phase::Idle;
        }
    }

    /// Forget the last match's world (ships, missiles, prediction, scores).
    fn reset_world(&mut self) {
        self.ships.clear();
        self.missiles.clear();
        self.my_ship = None;
        self.pred = None;
        self.scores.clear();
        self.clock = ServerClock::default();
        self.limit = 0;
        self.left = 0.0;
    }

    pub fn my(&self) -> Option<&CoreShip> {
        self.my_ship.and_then(|id| self.ships.get(&id))
    }

    pub fn render_tick(&self) -> f64 {
        self.clock.render_tick()
    }

    /// Server time (seconds) to turn the asteroids to when drawing them: the predicted tick while
    /// flying, so the rocks sit exactly where the predicted ship collides with them; the
    /// interpolated time otherwise.
    pub fn rock_time(&self) -> f64 {
        match &self.pred {
            Some(p) => tick_time(p.tick),
            None => self.clock.render_tick() / TICK_RATE as f64,
        }
    }

    /// Advance one rendered frame. `input` is asked for a command each simulated tick.
    pub fn update(&mut self, dt: f32, input: &mut dyn FnMut(&ClientCore, u32) -> InputCmd) {
        while let Ok(m) = self.net.rx.try_recv() {
            match m {
                NetIn::Msg(msg) => self.on_msg(msg),
                NetIn::Snap(s, n) => {
                    self.stats.snapshots += 1;
                    self.stats.bytes_in += n as u64;
                    self.on_snapshot(s);
                }
                NetIn::Closed(reason) => {
                    if !matches!(self.state, ConnState::Rejected(_)) {
                        self.state = ConnState::Closed(reason);
                    }
                }
            }
        }
        match &mut self.phase {
            Phase::Queued { waited, fill_in, .. } => {
                *waited += dt;
                *fill_in = (*fill_in - dt).max(0.0);
            }
            Phase::Picking(p) => p.left = (p.left - dt).max(0.0),
            _ => {}
        }
        self.left = (self.left - dt).max(0.0);
        self.clock.advance(dt);
        let render_tick = self.clock.render_tick();

        // local prediction (the input callback may read `self`, including the predictor)
        let pred_ship = self.pred.as_ref().map(|p| p.ship).filter(|id| self.ships.contains_key(id));
        if let Some(sid) = pred_ship {
            let n = {
                let p = self.pred.as_mut().unwrap();
                p.steps_due(dt) + std::mem::take(&mut p.catch_up).min(90)
            };
            for _ in 0..n {
                let tick = self.pred.as_ref().unwrap().tick;
                let mut cmd = input(self, tick);
                cmd.tick = tick;
                cmd.view_lag = (tick as f64 - render_tick).clamp(0.0, 30.0) as u8;
                self.shots.clear();
                let p = self.pred.as_mut().unwrap();
                p.step(cmd, &self.ships[&sid].body, &self.rocks, &mut self.shots);
                for s in self.shots.drain(..) {
                    self.events.push(CoreEvent::Shot { ship: sid, weapon: s.weapon, pos: s.pos, vel: s.vel, own: true });
                }
            }
            let p = self.pred.as_mut().unwrap();
            if n > 0 {
                self.net.send_input(InputPacket { cmds: p.recent_cmds(5) });
                self.stats.inputs_sent += 1;
            }
            p.decay(dt);
            let (pos, rot) = p.render();
            if let Some(s) = self.ships.get_mut(&sid) {
                s.pos = pos;
                s.rot = rot;
                s.vel = p.kin.vel;
                s.boosting = p.kin.boosting;
                s.throttle = p.last_cmd().map_or(0.7, |c| c.throttle_f());
                s.shield_frac = if s.body.stats.shield_cap > 0.0 { p.kin.shield / s.body.stats.shield_cap } else { 0.0 };
            }
        }

        // remote ships: interpolate, and fire cosmetic bolts at the interpolated muzzles
        let me = self.my_ship;
        let target_tick = render_tick.floor().max(0.0) as u32;
        for s in self.ships.values_mut() {
            if Some(s.id) == me {
                continue;
            }
            if let Some((pos, rot, vel, flags)) = s.interp.sample(render_tick) {
                s.pos = pos;
                s.rot = rot;
                s.vel = vel;
                s.firing = flags & FLAG_FIRING != 0;
                s.boosting = flags & FLAG_BOOST != 0;
            }
            if s.cos_tick == 0 || target_tick.saturating_sub(s.cos_tick) > 6 {
                s.cos_tick = target_tick;
            }
            while s.cos_tick < target_tick {
                s.cos_tick += 1;
                let Some((pos, rot, vel, flags)) = s.interp.sample(s.cos_tick as f64) else { break };
                let mut kin = voxwing_shared::flight::Kin::new(pos, rot, &s.body.stats);
                kin.vel = vel;
                let cmd = InputCmd { tick: s.cos_tick, buttons: if flags & FLAG_FIRING != 0 { BTN_FIRE } else { 0 }, ..Default::default() };
                self.shots.clear();
                step_weapons(&mut kin, &s.body, &mut s.ws, &cmd, DT, s.cos_tick, true, &mut self.shots);
                for sh in self.shots.drain(..) {
                    self.events.push(CoreEvent::Shot { ship: s.id, weapon: sh.weapon, pos: sh.pos, vel: sh.vel, own: false });
                }
            }
        }
        let latest = self.clock.latest;
        self.missiles.retain(|_, m| latest.saturating_sub(m.tick) < 20);
        for m in self.missiles.values_mut() {
            let _ = m;
        }
    }

    /// Where to draw a missile this frame.
    pub fn missile_pos(&self, m: &MissileView) -> Vec3 {
        m.pos + m.vel * ((self.clock.render_tick() - m.tick as f64) as f32 / TICK_RATE as f32)
    }

    fn on_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome { pid, tick, mode, asteroid_seed } => {
                self.reset_world();
                self.pid = Some(pid);
                self.mode = Some(mode);
                self.rocks = generate_asteroids(asteroid_seed);
                self.clock.sync(tick);
                self.state = ConnState::InGame;
                self.phase = Phase::Playing;
                self.events.push(CoreEvent::Welcome);
            }
            ServerMsg::Reject { reason } => self.state = ConnState::Rejected(reason),
            ServerMsg::Connected { pid } => {
                self.pid = Some(pid);
                self.state = ConnState::Lobby;
            }
            ServerMsg::Queued { players, fill_in: f } => {
                if let Phase::Queued { players: p, fill_in, .. } = &mut self.phase {
                    *p = players;
                    *fill_in = f;
                }
            }
            ServerMsg::MatchFound { mode, tier, you, slots, pick_secs } => {
                let n = slots.len();
                self.phase = Phase::Picking(Pick { mode, tier, you: you as usize, slots, ships: vec![None; n], ready: vec![false; n], left: pick_secs });
            }
            ServerMsg::SlotPick { slot, design, ready } => {
                if let Phase::Picking(p) = &mut self.phase
                    && (slot as usize) < p.slots.len()
                {
                    p.ships[slot as usize] = design;
                    p.ready[slot as usize] = ready;
                }
            }
            ServerMsg::MatchOver { winner, rows } => {
                self.reset_world();
                self.events.clear();
                self.state = ConnState::Lobby;
                self.phase = Phase::Over { winner, rows };
            }
            ServerMsg::Notice { text } => self.notice = Some(text),
            ServerMsg::ShipSpawn { id, team, name, owner, design, kin } => {
                let body = ShipBody::new(Arc::new(design));
                let mut interp = Interp::default();
                interp.push(Sample { tick: self.clock.latest, pos: kin.pos, rot: kin.rot, vel: kin.vel, flags: 0, throttle: 180 });
                let ws = WeaponState::new(&body);
                self.ships.insert(
                    id,
                    CoreShip { id, team, name, owner, body, interp, shield_frac: 1.0, firing: false, boosting: false, throttle: 0.7, ws, cos_tick: 0, pos: kin.pos, rot: kin.rot, vel: kin.vel },
                );
                self.events.push(CoreEvent::ShipSpawned(id));
            }
            ServerMsg::YourShip { id } => {
                self.my_ship = id;
                self.pred = None;
                self.events.push(CoreEvent::YourShip(id));
            }
            ServerMsg::ShipKilled { id, killer } => {
                if let Some(s) = self.ships.get(&id) {
                    let blocks = (0..s.body.alive.len()).filter(|&i| s.body.alive[i]).map(|i| (s.body.world(s.pos, s.rot, i), s.body.design.blocks[i].color)).collect();
                    self.events.push(CoreEvent::ShipKilled { id, killer, pos: s.pos, vel: s.vel, blocks });
                }
                self.drop_ship(id);
            }
            ServerMsg::ShipRemoved { id } => {
                self.events.push(CoreEvent::ShipRemoved(id));
                self.drop_ship(id);
            }
            ServerMsg::ShipRepaired { id } => {
                if let Some(s) = self.ships.get_mut(&id) {
                    s.body.repair();
                    s.ws = WeaponState::new(&s.body);
                    self.events.push(CoreEvent::ShipRepaired(id));
                }
            }
            ServerMsg::BlocksDamaged { ship, hits } => {
                if let Some(s) = self.ships.get_mut(&ship) {
                    let mut blocks = Vec::new();
                    for (i, f) in hits {
                        let i = i as usize;
                        if i < s.body.hp.len() && s.body.alive[i] {
                            s.body.hp[i] = s.body.block_max[i] * f as f32 / 255.0;
                            blocks.push(i);
                        }
                    }
                    self.events.push(CoreEvent::BlocksDamaged { ship, blocks });
                }
            }
            ServerMsg::BlocksDestroyed { ship, blocks } => {
                if let Some(s) = self.ships.get_mut(&ship) {
                    let mut gone = Vec::new();
                    for b in blocks {
                        let b = b as usize;
                        if b < s.body.alive.len() && s.body.alive[b] {
                            s.body.destroy(b);
                            gone.push(b);
                        }
                    }
                    let pieces = s.body.settle();
                    self.events.push(CoreEvent::BlocksDestroyed { ship, blocks: gone, pieces });
                }
            }
            ServerMsg::ShieldHit { ship, pos } => self.events.push(CoreEvent::ShieldHit { ship, pos }),
            ServerMsg::Explosion { pos, size } => self.events.push(CoreEvent::Explosion { pos, size }),
            ServerMsg::Crash { pos } => self.events.push(CoreEvent::Crash { pos }),
            ServerMsg::Announce { title, sub } => self.events.push(CoreEvent::Announce { title, sub }),
            ServerMsg::Feed { text, bad } => self.events.push(CoreEvent::Feed { text, bad }),
            ServerMsg::Scores { rows, wave, enemies, limit, left } => {
                self.scores = rows;
                self.wave = wave;
                self.enemies = enemies;
                self.limit = limit;
                self.left = left;
            }
        }
    }

    fn drop_ship(&mut self, id: u16) {
        self.ships.remove(&id);
        if self.my_ship == Some(id) {
            self.my_ship = None;
            self.pred = None;
        }
    }

    fn on_snapshot(&mut self, s: Snapshot) {
        self.clock.on_snapshot(s.tick);
        for sn in &s.ships {
            if let Some(ship) = self.ships.get_mut(&sn.id) {
                ship.interp.push(Sample { tick: s.tick, pos: sn.pos, rot: dequantize_quat(sn.rot), vel: sn.vel, flags: sn.flags, throttle: sn.throttle });
                ship.shield_frac = sn.shield as f32 / 255.0;
                ship.throttle = sn.throttle as f32 / 255.0;
            }
        }
        for m in &s.missiles {
            self.missiles.insert(m.id, MissileView { pos: m.pos, vel: m.vel, tick: s.tick });
        }
        let Some(own) = s.own else { return };
        if Some(own.ship) != self.my_ship {
            return;
        }
        let Some(ship) = self.ships.get(&own.ship) else { return };
        let rtt_ticks = (self.net.rtt() * TICK_RATE as f32).ceil() as u32;
        let target = s.tick + rtt_ticks + TARGET_LEAD as u32 + 1;
        let reinit = match self.pred.as_mut() {
            None => true,
            Some(p) => {
                let off = p.on_lead(s.lead);
                if !off {
                    p.reconcile(s.tick, &own.kin, &ship.body, &self.rocks);
                }
                off
            }
        };
        if reinit {
            let mut p = Predictor::new(own.ship, own.kin, &ship.body, s.tick + 1);
            p.catch_up = target.saturating_sub(s.tick + 1);
            self.pred = Some(p);
        }
    }
}
