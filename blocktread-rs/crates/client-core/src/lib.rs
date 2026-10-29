//! Everything the client does that isn't drawing: the network session, the local ship's
//! prediction, remote ships' interpolation, and applying the server's block-damage events.
//! It has no engine dependency, so the Bevy client and the headless bot harness share it.

pub mod interp;
pub mod net;
pub mod ping;
pub mod predict;

use glam::{Quat, Vec3};
use interp::{Interp, Sample, ServerClock};
use net::{NetClient, NetIn};
use predict::{Predictor, TARGET_LEAD};
pub use blocktread_net::Target;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use blocktread_shared::blocks::{BlockType, Design};
use blocktread_shared::flight::{BTN_FIRE, InputCmd};
use blocktread_shared::gadgets::{Cloud, hidden};
use blocktread_shared::protocol::*;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::weapons::{MISSILE_ACCEL, MISSILE_TOP, Shot, WeaponState, missile_ahead, step_weapons};
use blocktread_shared::terrain::{Map, Terrain};
use blocktread_shared::world::{Rock, generate_boulders, tick_time};
use blocktread_shared::{DT, PROTOCOL_VERSION, SNAPSHOT_INTERVAL, TICK_RATE};

pub struct CoreShip {
    pub id: u16,
    pub team: u16,
    pub name: String,
    pub body: ShipBody,
    interp: Interp,
    pub shield_frac: f32,
    pub firing: bool,
    pub boosting: bool,
    /// Its thrusters push: on the throttle, or boosting.
    pub thrusting: bool,
    /// Its saw bites into something; its flipper is up; its jump jets fire.
    pub cutting: bool,
    pub flipping: bool,
    pub jumping: bool,
    /// The server tick its overdrive (from a pad) runs out.
    pub overdrive_until: u32,
    pub throttle: f32,
    /// Its guns' cooldowns and where they point, as turned by its last known aim.
    pub ws: WeaponState,
    cos_tick: u32,
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
}

/// A mine lying in wait: where, whose side laid it, and the server tick it is live from.
#[derive(Clone, Copy, Debug)]
pub struct MineView {
    pub pos: Vec3,
    pub team: u16,
    pub armed_at: u32,
}

#[derive(Clone, Copy)]
pub struct MissileView {
    pub pos: Vec3,
    pub vel: Vec3,
    /// The team that fired it.
    pub team: u16,
    /// What it is drawn as: its own id, or the key of the launch it took over from.
    pub key: u32,
    /// Fired by this client's ship: drawn on the predicted ship's clock at first.
    own: bool,
    /// Seconds since it showed up here.
    age: f32,
    /// A fading correction that keeps its drawn path continuous across updates.
    off: Vec3,
    /// The snapshot that first carried it, and the latest one.
    born: u32,
    tick: u32,
}

/// A missile this client's ship just fired, drawn from the launcher until the server's copy of it
/// arrives. It is at `pos` after tick `tick`, as the server would have it (it leaves on the next).
struct Launch {
    key: u32,
    tick: u32,
    pos: Vec3,
    vel: Vec3,
}

/// An own missile moves over from the predicted ship's clock to the world's (the one everything
/// else is drawn at) over its first this many seconds.
const MISSILE_WARP: f32 = 0.7;
/// Launch keys sit above the server's missile ids.
const LAUNCH_KEYS: u32 = 1 << 16;
/// A launch waits this many ticks past its own for a snapshot carrying the server's copy (a few
/// snapshots can be lost); a launch the server never made (it disagreed about the energy, say) is
/// dropped after that.
const LAUNCH_WAIT: u32 = 10;

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
    /// One of our rounds landed on `target`: on its shield, or its hull.
    HitConfirm { target: u16, shield: bool },
    /// A cloud of smoke goes up.
    Smoke(Cloud),
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
    /// `waited`: seconds in the queue; `players`: how many wait for this mode.
    Queued { mode: Mode, tier: Option<String>, waited: f32, players: u32 },
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
    pub terrain: Arc<Terrain>,
    pub rocks: Vec<Rock>,
    pub clock: ServerClock,
    pub pred: Option<Predictor>,
    pub scores: Vec<ScoreRow>,
    /// Kills that win the match (0 = none) and seconds left on its clock (0 = no clock).
    pub limit: u32,
    pub left: f32,
    /// The free-for-all room's round (`left` counts down the round, or the break after it).
    pub round: Round,
    /// 2v2's hill: the points and who holds it.
    pub hill: Option<HillState>,
    /// The server tick each of the arena's pick-up pads is back up.
    pub pads: Vec<u32>,
    /// Mines down, by id, and the clouds of smoke hanging.
    pub mines: BTreeMap<u16, MineView>,
    pub clouds: Vec<Cloud>,
    pub events: Vec<CoreEvent>,
    pub stats: NetStats,
    shots: Vec<Shot>,
    launches: Vec<Launch>,
    next_launch: u32,
}

impl ClientCore {
    /// Connect to play: then `queue`, `pick` and `ready` take the player into a match.
    /// Join `server` (an address, or an address and the certificate it must show).
    pub fn connect(server: impl Into<Target>, name: &str, latency: Duration) -> Self {
        Self::start(server.into(), ClientMsg::Hello { version: PROTOCOL_VERSION, name: name.to_string(), balance: blocktread_shared::balance::hash() }, latency)
    }

    /// Join to watch instead of fly.
    pub fn watch(server: impl Into<Target>, name: &str, latency: Duration) -> Self {
        Self::start(server.into(), ClientMsg::Watch { version: PROTOCOL_VERSION, name: name.to_string(), balance: blocktread_shared::balance::hash() }, latency)
    }

    fn start(server: Target, hello: ClientMsg, latency: Duration) -> Self {
        ClientCore {
            watching: matches!(hello, ClientMsg::Watch { .. }),
            net: NetClient::connect(server, hello, latency),
            state: ConnState::Connecting,
            phase: Phase::Idle,
            queued_for: None,
            notice: None,
            pid: None,
            mode: None,
            my_ship: None,
            ships: BTreeMap::new(),
            missiles: BTreeMap::new(),
            terrain: Arc::new(Terrain::flat()),
            rocks: Vec::new(),
            clock: ServerClock::default(),
            pred: None,
            scores: Vec::new(),
            limit: 0,
            left: 0.0,
            round: Round::None,
            hill: None,
            pads: Vec::new(),
            mines: BTreeMap::new(),
            clouds: Vec::new(),
            events: Vec::new(),
            stats: NetStats::default(),
            shots: Vec::new(),
            launches: Vec::new(),
            next_launch: 0,
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
        self.phase = Phase::Queued { mode, tier, waited: 0.0, players: 1 };
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
            self.net.send(ClientMsg::Watch { version: PROTOCOL_VERSION, name: String::new(), balance: blocktread_shared::balance::hash() });
            self.phase = Phase::Idle;
        }
    }

    /// Forget the last match's world (ships, missiles, prediction, scores).
    fn reset_world(&mut self) {
        self.ships.clear();
        self.missiles.clear();
        self.launches.clear();
        self.my_ship = None;
        self.pred = None;
        self.scores.clear();
        self.clock = ServerClock::default();
        self.limit = 0;
        self.left = 0.0;
        self.round = Round::None;
        self.hill = None;
        self.pads.clear();
        self.mines.clear();
        self.clouds.clear();
    }

    pub fn my(&self) -> Option<&CoreShip> {
        self.my_ship.and_then(|id| self.ships.get(&id))
    }

    pub fn render_tick(&self) -> f64 {
        self.clock.render_tick()
    }

    /// Seconds until pick-up pad `i` is back up (0: it is up).
    pub fn pad_left(&self, i: usize) -> f32 {
        self.pads.get(i).map_or(0.0, |&b| b.saturating_sub(self.clock.latest) as f32 / TICK_RATE as f32)
    }

    /// Whether smoke hides `b` from `a` now.
    pub fn smoke_between(&self, a: Vec3, b: Vec3) -> bool {
        hidden(&self.clouds, a, b, self.clock.latest)
    }

    /// Seconds of overdrive `s` has left.
    pub fn overdrive_left(&self, s: &CoreShip) -> f32 {
        s.overdrive_until.saturating_sub(self.clock.latest) as f32 / TICK_RATE as f32
    }

    /// Server time (seconds) to draw the arena's saws and vents at: the predicted tick while
    /// driving, so they sit exactly where the predicted vehicle meets them; the interpolated time
    /// otherwise.
    pub fn arena_time(&self) -> f64 {
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
            Phase::Queued { waited, .. } => *waited += dt,
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
                p.step(cmd, &self.ships[&sid].body, &self.terrain, &self.rocks, &mut self.shots);
                for s in self.shots.drain(..) {
                    // a missile leaves the launcher at once; the server's copy takes over when it arrives
                    if s.weapon.weapon().is_some_and(|w| w.homing) {
                        self.launches.push(Launch { key: LAUNCH_KEYS + self.next_launch, tick: tick.saturating_sub(1), pos: s.pos, vel: s.vel });
                        self.next_launch = (self.next_launch + 1) % LAUNCH_KEYS;
                    }
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
                s.thrusting = s.body.stats.thrust > 0.0 && p.last_cmd().is_some_and(|c| blocktread_shared::drive::thrust_push(&c, p.kin.boosting) > 0.0);
                s.throttle = p.last_cmd().map_or(0.7, |c| c.throttle_f());
                s.shield_frac = if s.body.stats.shield_cap > 0.0 { p.kin.shield / s.body.stats.shield_cap } else { 0.0 };
                s.flipping = p.ws.flip_t < FLIP_SHOWN;
                s.jumping = p.kin.jump_cd > BlockType::JumpJet.gadget().map_or(0.0, |g| g.cooldown) - JUMP_SHOWN;
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
                s.thrusting = flags & FLAG_THRUST != 0;
                s.cutting = flags & FLAG_CUT != 0;
                s.flipping = flags & FLAG_FLIP != 0;
                s.jumping = flags & FLAG_JUMP != 0;
            }
            if s.cos_tick == 0 || target_tick.saturating_sub(s.cos_tick) > 6 {
                s.cos_tick = target_tick;
            }
            while s.cos_tick < target_tick {
                s.cos_tick += 1;
                let Some((pos, rot, vel, flags)) = s.interp.sample(s.cos_tick as f64) else { break };
                let mut kin = blocktread_shared::flight::Kin::new(pos, rot, &s.body.stats);
                kin.vel = vel;
                // the same aim the server turns its guns to, so they swing (and fire) alike here
                let cmd = InputCmd { tick: s.cos_tick, buttons: if flags & FLAG_FIRING != 0 { BTN_FIRE } else { 0 }, aim: s.interp.aim(s.cos_tick as f64), ..Default::default() };
                self.shots.clear();
                step_weapons(&mut kin, &s.body, &mut s.ws, &cmd, DT, s.cos_tick, true, &mut self.shots);
                for sh in self.shots.drain(..) {
                    self.events.push(CoreEvent::Shot { ship: s.id, weapon: sh.weapon, pos: sh.pos, vel: sh.vel, own: false });
                }
            }
        }
        let latest = self.clock.latest;
        self.clouds.retain(|c| c.until > latest);
        self.missiles.retain(|_, m| latest.saturating_sub(m.tick) < 20);
        let fade = (-dt * 12.0).exp();
        for m in self.missiles.values_mut() {
            m.age += dt;
            m.off *= fade;
        }
        self.launches.retain(|l| latest <= l.tick + LAUNCH_WAIT);
    }

    /// The tick the own ship is drawn at, while flying one.
    fn own_tick(&self) -> Option<f64> {
        self.pred.as_ref().map(|p| p.render_tick())
    }

    /// The tick to draw a missile at: the world's, as everything else, except that an own missile
    /// leaves on the predicted ship's clock (drawn on the world's, the ship would already be past
    /// it) and eases over to the world's as it flies out to whatever it is after.
    fn missile_tick(&self, m: &MissileView) -> f64 {
        let world = self.clock.render_tick();
        match self.own_tick() {
            Some(own) if m.own => {
                let x = (m.age / MISSILE_WARP).min(1.0);
                world + (own - world) * (1.0 - x * x * (3.0 - 2.0 * x)) as f64
            }
            _ => world,
        }
    }

    /// Where to draw a missile this frame, if at all: not before it is launched on the clock it
    /// is drawn at, nor further than the snapshots carried it (it hit something or burnt out).
    pub fn missile_pos(&self, m: &MissileView) -> Option<Vec3> {
        let t = self.missile_tick(m);
        let gone = self.clock.latest >= m.tick + 2 * SNAPSHOT_INTERVAL;
        if t < m.born as f64 - 1.0 || (gone && t > (m.tick + SNAPSHOT_INTERVAL) as f64) {
            return None;
        }
        Some(missile_ahead(m.pos, m.vel, (t - m.tick as f64) as f32 / TICK_RATE as f32) + m.off)
    }

    /// Where a launch is now, and how fast it flies.
    fn launch_draw(&self, l: &Launch) -> Option<(Vec3, Vec3)> {
        let secs = (self.own_tick()? - l.tick as f64) as f32 / TICK_RATE as f32;
        let speed = (l.vel.length() + MISSILE_ACCEL * secs.max(0.0)).min(MISSILE_TOP);
        Some((missile_ahead(l.pos, l.vel, secs), l.vel.normalize_or_zero() * speed))
    }

    /// Every missile to draw this frame: (key, position, velocity). A missile keeps its key when
    /// the server's copy takes over from the launch drawn until it arrived.
    pub fn missile_draws(&self) -> impl Iterator<Item = (u32, Vec3, Vec3)> + '_ {
        let server = self.missiles.values().filter_map(|m| Some((m.key, self.missile_pos(m)?, m.vel)));
        server.chain(self.launches.iter().filter_map(|l| {
            let (pos, vel) = self.launch_draw(l)?;
            Some((l.key, pos, vel))
        }))
    }

    fn on_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome { pid, tick, mode, seed, map } => {
                self.reset_world();
                self.pid = Some(pid);
                self.mode = Some(mode);
                self.terrain = Arc::new(Terrain::for_map(map, seed));
                self.pads = vec![0; self.terrain.arena.as_ref().map_or(0, |a| a.pads.len())];
                self.rocks = if map == Map::Desert { generate_boulders(seed, &self.terrain) } else { Vec::new() };
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
            ServerMsg::Queued { players } => {
                if let Phase::Queued { players: p, .. } = &mut self.phase {
                    *p = players;
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
            ServerMsg::ShipSpawn { id, team, name, design, kin } => {
                let body = ShipBody::new(Arc::new(design));
                let mut interp = Interp::default();
                interp.push(Sample { tick: self.clock.latest, pos: kin.pos, rot: kin.rot, vel: kin.vel, flags: 0, throttle: 0, aim: blocktread_shared::flight::quantize_aim(kin.pos + kin.fwd() * 100.0) });
                let ws = WeaponState::new(&body);
                self.ships.insert(
                    id,
                    CoreShip { id, team, name, body, interp, shield_frac: 1.0, firing: false, boosting: false, thrusting: false, cutting: false, flipping: false, jumping: false, overdrive_until: 0, throttle: 0.7, ws, cos_tick: 0, pos: kin.pos, rot: kin.rot, vel: kin.vel },
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
            ServerMsg::HitConfirm { target, shield } => self.events.push(CoreEvent::HitConfirm { target, shield }),
            ServerMsg::PadState { pad, back_at } => {
                if let Some(p) = self.pads.get_mut(pad as usize) {
                    *p = back_at;
                }
            }
            ServerMsg::ShipBuff { id, until } => {
                if let Some(s) = self.ships.get_mut(&id) {
                    s.overdrive_until = until;
                }
            }
            ServerMsg::MineDropped { id, pos, team, armed_at } => {
                self.mines.insert(id, MineView { pos, team, armed_at });
            }
            ServerMsg::MineGone { id, .. } => {
                self.mines.remove(&id);
            }
            ServerMsg::Smoke { pos, radius, until } => {
                let c = Cloud { pos, radius, until };
                self.clouds.push(c);
                self.events.push(CoreEvent::Smoke(c));
            }
            ServerMsg::Scores { rows, limit, left, round, hill } => {
                self.hill = hill;
                self.scores = rows;
                self.limit = limit;
                self.left = left;
                self.round = round;
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
                ship.interp.push(Sample { tick: s.tick, pos: sn.pos, rot: dequantize_quat(sn.rot), vel: sn.vel, flags: sn.flags, throttle: sn.throttle, aim: sn.aim });
                ship.shield_frac = sn.shield as f32 / 255.0;
                ship.throttle = sn.throttle as f32 / 255.0;
            }
        }
        for sm in &s.missiles {
            let (mut m, before) = match self.missiles.get(&sm.id) {
                // a late snapshot, older than the one it has
                Some(m) if m.tick >= s.tick => continue,
                Some(m) => (*m, self.missile_pos(m)),
                None => {
                    let own = Some(sm.owner) == self.my_ship;
                    let mut m = MissileView { pos: sm.pos, vel: sm.vel, team: sm.team, key: sm.id as u32, own, age: 0.0, off: Vec3::ZERO, born: s.tick, tick: s.tick };
                    // our own missile takes over from its launch: the first one made by this
                    // snapshot's tick (the server counts ticks as the prediction does)
                    let mut before = None;
                    if own && let Some(i) = self.launches.iter().position(|l| l.tick < s.tick) {
                        let l = self.launches.remove(i);
                        m.key = l.key;
                        before = self.launch_draw(&l).map(|(p, _)| p);
                    }
                    (m, before)
                }
            };
            m.pos = sm.pos;
            m.vel = sm.vel;
            m.tick = s.tick;
            // fade out the step between where it was drawn and where it is now
            if let (Some(b), Some(a)) = (before, self.missile_pos(&m)) {
                m.off += b - a;
                if m.off.length() > 20.0 {
                    m.off = Vec3::ZERO;
                }
            }
            self.missiles.insert(sm.id, m);
        }
        let Some(own) = s.own else { return };
        if Some(own.ship) != self.my_ship {
            return;
        }
        let Some(ship) = self.ships.get_mut(&own.ship) else { return };
        ship.cutting = own.flags & FLAG_CUT != 0;
        let ship = &*ship;
        let rtt_ticks = (self.net.rtt() * TICK_RATE as f32).ceil() as u32;
        let target = s.tick + rtt_ticks + TARGET_LEAD as u32 + 1;
        let reinit = match self.pred.as_mut() {
            None => true,
            Some(p) => {
                let off = p.on_lead(s.lead);
                if !off {
                    p.reconcile(s.tick, &own.kin, &ship.body, &self.terrain, &self.rocks);
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
