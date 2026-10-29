use crate::Outgoing;
use bytes::Bytes;
use glam::{Quat, Vec3};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use blocktread_shared::ai::{AiPilot, Contact, Skill};
use blocktread_shared::analysis::analyze;
use blocktread_shared::balance::balance;
use blocktread_shared::blocks::{BlockType, DamageKind, Design, Melee, MeleeSpec};
use blocktread_shared::economy::Showing;
use blocktread_shared::drive::{saw_bites, spawn_height, step_drive};
use blocktread_shared::flight::{InputCmd, Kin, NO_LOCK, collide_rocks, look_rotation};
use blocktread_shared::gadgets::{Cloud, MINE_ARM, hidden};
use blocktread_shared::presets::{PRESETS, preset, repaint};
use blocktread_shared::protocol::*;
use blocktread_shared::rng::Rng;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::weapons::{MISSILE_ACCEL, MISSILE_TOP, MISSILE_TURN, Shot, WeaponState, step_weapons};
use blocktread_shared::arena::{HILL_HALF, HILL_KILL, HILL_TOP, KILL_Y, OVERDRIVE_MUL, OVERDRIVE_SECS, PadKind, SAW_DMG, VENT_DPS, VENT_HIGH};
use blocktread_shared::terrain::{Map, Terrain};
use blocktread_shared::world::{Rock, SPAWN, generate_boulders, tick_time};
use blocktread_shared::{DT, HISTORY_TICKS, SNAPSHOT_INTERVAL, TICK_RATE};

pub struct GameConfig {
    pub mode: Mode,
    pub bots: u32,
    /// The room's build budget, if it has one: free-for-all bots only fly ships that fit.
    pub budget: Option<f32>,
    pub verbose: bool,
}

/// The test range's map: the same hills and rocks every time.
pub const SOLO_SEED: u64 = 2604;
/// The break between two rounds of the free-for-all room, with the results up.
pub const ROUND_BREAK_SECS: f32 = 10.0;
const MAX_DATAGRAM: usize = 1100;
const MAX_REWIND_TICKS: u32 = 30;

struct Player {
    name: String,
    /// None for a spectator, who never gets a ship.
    design: Option<Arc<Design>>,
    /// Set by a match (2v2 teams, a fixed seat in free-for-all); otherwise from the mode.
    team: Option<u16>,
    out: tokio::sync::mpsc::UnboundedSender<Outgoing>,
    ship: Option<u16>,
    inputs: BTreeMap<u32, InputCmd>,
    last: InputCmd,
    newest: u32,
    ack: u32,
    kills: u32,
    deaths: u32,
    score: u32,
    respawn_at: Option<u32>,
    /// The tick they came in.
    since: u32,
}

impl Player {
    fn watching(&self) -> bool {
        self.design.is_none()
    }

    fn send(&self, msg: &ServerMsg) {
        let _ = self.out.send(Outgoing::Reliable(Bytes::from(encode(msg))));
    }
}

struct Ship {
    id: u16,
    team: u16,
    name: String,
    owner: Option<u16>,
    body: ShipBody,
    kin: Kin,
    ws: WeaponState,
    ai: Option<AiPilot>,
    input: InputCmd,
    hist: Vec<(Vec3, Quat)>,
    spawn_tick: u32,
    last_attacker: Option<u16>,
    /// Seconds since someone last hit it: whoever drove it into a pit only gets the kill while
    /// this is short.
    since_hit: f32,
    /// Seconds its centre has been over a pit: one that stays hung on the edge goes in.
    over_pit: f32,
    collide_cd: f32,
    /// Seconds until its saws cut again; how much longer it shows them cutting; until the
    /// arena's saws can bite it again, and its vents burn it again.
    cut_cd: f32,
    cutting: f32,
    bite_cd: f32,
    burn_cd: f32,
    recent_dmg: f32,
    disabled_t: Option<f32>,
    dmg_mul: f32,
    /// Seconds of overdrive left (from a pad): its weapons hit harder.
    overdrive: f32,
    destroyed: Vec<u16>,
    damaged: Vec<(u16, u8)>,
    killed: bool,
    is_bot: bool,
    /// Free-for-all bot seat this ship flies for.
    seat: Option<usize>,
}

/// A bot identity. Each respawn flies the same ship for the same seat, so its score carries over.
/// To everyone else it is a player like any other: an id of the same kind, a name like theirs.
struct BotSeat {
    pid: u16,
    name: String,
    design: Arc<Design>,
    /// A match's team for it; otherwise each life gets a team of its own.
    team: Option<u16>,
    ship: Option<u16>,
    kills: u32,
    deaths: u32,
    score: u32,
}

impl Ship {
    /// What its weapons' damage is multiplied by.
    fn hits_mul(&self) -> f32 {
        self.dmg_mul * if self.overdrive > 0.0 { OVERDRIVE_MUL } else { 1.0 }
    }

    /// Whether it has any use for what a pad gives.
    fn wants(&self, kind: PadKind) -> bool {
        let st = &self.body.stats;
        match kind {
            PadKind::Repair => self.body.hp_total() < self.body.max_hp - 1.0,
            PadKind::Energy => self.kin.energy < st.energy_cap * 0.9 || self.kin.shield < st.shield_cap * 0.9,
            PadKind::Overdrive => true,
        }
    }

    fn transform_at(&self, now: u32, rewind: u32) -> (Vec3, Quat) {
        if rewind == 0 || now < rewind || now - rewind <= self.spawn_tick {
            return (self.kin.pos, self.kin.rot);
        }
        self.hist[((now - rewind) as usize) % HISTORY_TICKS]
    }

    /// Apply damage to one block, through its armour. Shields soak hits first; the caller handles
    /// shield checks.
    fn hit_block(&mut self, i: usize, dmg: f32, kind: DamageKind, src: Option<u16>) {
        if !self.body.alive[i] {
            return;
        }
        let dmg = balance().through_armor(dmg, self.body.design.blocks[i].ty.info().armor, kind);
        self.recent_dmg += dmg;
        if src.is_some() {
            self.last_attacker = src;
            self.since_hit = 0.0;
        }
        if self.body.damage(i, dmg) {
            self.body.destroy(i);
            self.destroyed.push(i as u16);
        } else {
            let frac = (self.body.hp_frac(i) * 255.0).round() as u8;
            self.damaged.push((i as u16, frac));
        }
    }

    /// An explosion: each block in reach takes up to `dmg`, less the farther out it is, but the
    /// blast as a whole deals at most the balance's `splash_total` times `dmg`, shared out. So a
    /// solid hull isn't hit many times harder than a thin one.
    fn splash(&mut self, pos: Vec3, rot: Quat, p: Vec3, r: f32, dmg: f32, kind: DamageKind, src: Option<u16>) {
        let hit: Vec<(usize, f32)> = self.body.within(pos, rot, p, r).into_iter().map(|(i, d)| (i, 1.0 - 0.65 * d / r)).collect();
        let weight: f32 = hit.iter().map(|h| h.1).sum();
        let share = (balance().splash_total / weight.max(1e-3)).min(1.0);
        for (i, w) in hit {
            self.hit_block(i, dmg * w * share, kind, src);
        }
    }

    fn contact(&self) -> Contact {
        Contact { id: self.id, team: self.team, pos: self.kin.pos, vel: self.kin.vel, radius: self.body.radius }
    }

    /// A ram on it close enough to `p` to take a crash there.
    fn ram_at(&self, p: Vec3) -> Option<&'static MeleeSpec> {
        let blocks = &self.body.design.blocks;
        self.body.stats.melee.iter().map(|&i| (i, blocks[i].ty.melee().unwrap())).find(|(i, m)| m.kind == Melee::Ram && self.body.world(self.kin.pos, self.kin.rot, *i).distance(p) < m.reach).map(|(_, m)| m)
    }

    /// What its saws and flippers reach: each one's kind and the middle and radius of the space
    /// in front of it.
    fn melee_reach(&self) -> impl Iterator<Item = (&'static MeleeSpec, Vec3, f32)> + '_ {
        let fwd = self.kin.fwd();
        let blocks = &self.body.design.blocks;
        self.body.stats.melee.iter().map(move |&i| (i, blocks[i].ty.melee().unwrap())).filter(|(_, m)| m.kind != Melee::Ram).map(move |(i, m)| {
            let face = self.body.world(self.kin.pos, self.kin.rot, i) + fwd * 0.5;
            (m, face + fwd * m.reach * 0.5, m.reach * 0.5 + 0.5)
        })
    }

    fn flags(&self) -> u8 {
        (if self.input.has(blocktread_shared::flight::BTN_FIRE) { FLAG_FIRING } else { 0 })
            | (if self.kin.boosting { FLAG_BOOST } else { 0 })
            | (if self.body.stats.thrust > 0.0 && blocktread_shared::drive::thrust_push(&self.input, self.kin.boosting) > 0.0 { FLAG_THRUST } else { 0 })
            | (if self.cutting > 0.0 { FLAG_CUT } else { 0 })
            | (if self.ws.flip_t < FLIP_SHOWN { FLAG_FLIP } else { 0 })
            | (if self.kin.jump_cd > BlockType::JumpJet.gadget().map_or(0.0, |g| g.cooldown) - JUMP_SHOWN { FLAG_JUMP } else { 0 })
    }
}

/// A mine lying in wait: who laid it (their ship, for the kill) and for which side, and the
/// ticks it goes live and is cleared away at.
struct Mine {
    id: u16,
    pos: Vec3,
    team: u16,
    owner: u16,
    armed_at: u32,
    gone_at: u32,
}

struct Bolt {
    id: u16,
    pos: Vec3,
    prev: Vec3,
    vel: Vec3,
    team: u16,
    owner: u16,
    weapon: BlockType,
    dmg: f32,
    life: f32,
    rewind: u32,
    target: Option<u16>,
    speed: f32,
}

enum Timer {
    SpawnBot,
}

pub struct Game {
    cfg: GameConfig,
    tick: u32,
    rng: Rng,
    players: BTreeMap<u16, Player>,
    ships: BTreeMap<u16, Ship>,
    next_ship: u16,
    bolts: Vec<Bolt>,
    next_bolt: u16,
    terrain: Arc<Terrain>,
    rocks: Vec<Rock>,
    seed: u64,
    timers: Vec<(u32, Timer)>,
    pending_bots: u32,
    seats: Vec<BotSeat>,
    events: Vec<ServerMsg>,
    scores_dirty: bool,
    shots: Vec<Shot>,
    /// Kills scored by each team (a free-for-all seat is a team of one).
    team_kills: BTreeMap<u16, u32>,
    /// Kills that win a match (0 = none) and the tick it ends at: shown on the scoreboard.
    pub limit: u32,
    pub ends_at: Option<u32>,
    /// The free-for-all room's rounds: their length in seconds (None: no rounds), where the
    /// current one stands, and the tick its clock (or the break after it) runs out.
    pub rounds: Option<f32>,
    round: Round,
    round_end: u32,
    /// The free-for-all room's rounds so far, and the tick the current one started.
    round_no: u32,
    round_start: u32,
    /// How each player did in rounds that have ended, for the hub to pay out (`take_results`).
    results: Vec<(u16, Showing)>,
    /// The tick each of the arena's pick-up pads is back up (up now once it has passed).
    pads: Vec<u32>,
    /// 2v2 in the arena: the fight over the hill (None in the other modes).
    hill: Option<Hill>,
    /// Mines down, oldest first, and the clouds of smoke hanging.
    mines: Vec<Mine>,
    next_mine: u16,
    clouds: Vec<Cloud>,
}

/// 2v2's hill: each team's points (from holding it, and kills), the team alone on top if one is,
/// whether both are, and how long the holder has held it towards its next point.
#[derive(Default)]
struct Hill {
    points: BTreeMap<u16, u32>,
    holder: Option<u16>,
    contested: bool,
    held: f32,
}

/// How a ship was lost, for the feed.
#[derive(Clone, Copy)]
enum Fate {
    Wrecked,
    Fell,
}

/// Seconds a hit counts towards a fall into a pit.
const PIT_CREDIT: f32 = 8.0;
/// Seconds a vehicle may stay over a pit (a jump takes well under one) before it is lost to it.
const PIT_HANG: f32 = 2.0;
/// Saws cut in chunks this many seconds apart (so armour meets each chunk, not each tick), and
/// vents burn in chunks too.
const CUT_EVERY: f32 = 0.1;
const BURN_EVERY: f32 = 0.25;
/// Seconds before anyone wrecked is back, player or AI alike.
const RESPAWN_SECS: f32 = 4.0;
/// A crash bounces vehicles apart this hard (1 + restitution).
const CRASH_BOUNCE: f32 = 1.8;

fn secs(s: f32) -> u32 {
    (s * TICK_RATE as f32) as u32
}

/// "8 分钟", "2:30", "45 秒".
fn span(s: f32) -> String {
    let s = s.ceil() as u32;
    match (s / 60, s % 60) {
        (0, x) => format!("{x} 秒"),
        (m, 0) => format!("{m} 分钟"),
        (m, x) => format!("{m}:{x:02}"),
    }
}

fn seg_sphere(a: Vec3, b: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 0.0 { ((c - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((a + ab * t).distance_squared(c) <= r * r).then_some(t)
}

impl Game {
    pub fn new(cfg: GameConfig) -> Self {
        Self::with_seed(cfg, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7))
    }

    /// A game whose ground and randomness come from `seed` (for reproducible simulations), on its
    /// mode's map: free-for-all and 2v2 in the cage, the rest in the desert.
    pub fn with_seed(cfg: GameConfig, seed: u64) -> Self {
        let map = match cfg.mode {
            Mode::Ffa | Mode::Teams => Map::Cage,
            Mode::Solo => Map::Desert,
        };
        Self::with_map(cfg, seed, map)
    }

    pub fn with_map(cfg: GameConfig, seed: u64, map: Map) -> Self {
        let terrain = Arc::new(Terrain::for_map(map, seed));
        let arena = terrain.arena.as_ref();
        let pads = arena.map_or(Vec::new(), |a| vec![0; a.pads.len()]);
        let hill = (cfg.mode == Mode::Teams && arena.is_some()).then(Hill::default);
        Game {
            cfg,
            tick: 0,
            rng: Rng::new(seed ^ 0xA5A5),
            players: BTreeMap::new(),
            ships: BTreeMap::new(),
            next_ship: 1,
            bolts: Vec::new(),
            next_bolt: 1,
            // the arena's pieces take the boulders' place
            rocks: if map == Map::Desert { generate_boulders(seed, &terrain) } else { Vec::new() },
            terrain,
            seed,
            timers: Vec::new(),
            pending_bots: 0,
            seats: Vec::new(),
            events: Vec::new(),
            scores_dirty: true,
            shots: Vec::new(),
            team_kills: BTreeMap::new(),
            limit: 0,
            ends_at: None,
            rounds: None,
            round: Round::None,
            round_end: 0,
            round_no: 0,
            round_start: 0,
            results: Vec::new(),
            pads,
            hill,
            mines: Vec::new(),
            next_mine: 0,
            clouds: Vec::new(),
        }
    }

    pub fn now(&self) -> u32 {
        self.tick
    }

    /// What a match is won on, by team: its kills, or on the hill its points.
    pub fn points(&self) -> &BTreeMap<u16, u32> {
        self.hill.as_ref().map_or(&self.team_kills, |h| &h.points)
    }

    /// Players still connected (pilots and spectators).
    pub fn has_players(&self) -> bool {
        !self.players.is_empty()
    }

    /// Send a message to everyone in this game.
    pub fn broadcast(&self, msg: &ServerMsg) {
        for p in self.players.values() {
            p.send(msg);
        }
    }

    fn after(&mut self, s: f32, t: Timer) {
        self.timers.push((self.tick + secs(s), t));
    }

    fn feed(&mut self, text: String, bad: bool) {
        self.events.push(ServerMsg::Feed { text, bad });
    }

    fn announce(&mut self, title: &str, sub: String) {
        self.events.push(ServerMsg::Announce { title: title.into(), sub });
    }

    pub fn input(&mut self, pid: u16, pkt: InputPacket) {
        let now = self.tick;
        if let Some(p) = self.players.get_mut(&pid) {
            for mut c in pkt.cmds {
                if c.tick > now && c.tick <= now + 240 {
                    c.view_lag = c.view_lag.min(MAX_REWIND_TICKS as u8);
                    p.newest = p.newest.max(c.tick);
                    p.inputs.insert(c.tick, c);
                }
            }
            while p.inputs.len() > 300 {
                p.inputs.pop_first();
            }
        }
    }

    pub fn leave(&mut self, pid: u16) {
        if let Some(p) = self.players.remove(&pid) {
            if let Some(sid) = p.ship {
                self.ships.remove(&sid);
                self.events.push(ServerMsg::ShipRemoved { id: sid });
            }
            self.feed(format!("{} {}", p.name, if p.watching() { "结束了观战" } else { "离开了战场" }), false);
            self.scores_dirty = true;
        }
    }

    /// Players who fly (not spectators).
    fn pilots(&self) -> usize {
        self.players.values().filter(|p| !p.watching()).count()
    }

    /// A player (or with no design, a spectator) enters the game. `team` fixes their side.
    pub fn join(&mut self, pid: u16, name: String, design: Option<Design>, team: Option<u16>, out: tokio::sync::mpsc::UnboundedSender<Outgoing>) {
        let mut name: String = name.trim().chars().filter(|c| !c.is_control()).take(12).collect();
        if name.is_empty() {
            name = format!("车长{pid}");
        }
        // what happened before this join goes to those already here: the newcomer gets the world
        // as it is now (several join between two ticks when a match starts)
        self.send_events();
        let p = Player {
            name: name.clone(),
            design: design.map(|d| Arc::new(Design { name: d.sanitized_name(), blocks: d.blocks })),
            team,
            out,
            ship: None,
            inputs: BTreeMap::new(),
            last: InputCmd::default(),
            newest: 0,
            ack: 0,
            kills: 0,
            deaths: 0,
            score: 0,
            respawn_at: None,
            since: self.tick,
        };
        p.send(&ServerMsg::Welcome { pid, tick: self.tick, mode: self.cfg.mode, seed: self.seed, map: self.terrain.map });
        // bring the newcomer up to date with every ship, including blocks already shot off
        for s in self.ships.values() {
            p.send(&spawn_msg(s));
            let dead: Vec<u16> = (0..s.body.alive.len()).filter(|&i| !s.body.alive[i]).map(|i| i as u16).collect();
            if !dead.is_empty() {
                p.send(&ServerMsg::BlocksDestroyed { ship: s.id, blocks: dead });
            }
            let hurt: Vec<(u16, u8)> = (0..s.body.alive.len())
                .filter(|&i| s.body.alive[i] && s.body.hp_frac(i) < 1.0)
                .map(|i| (i as u16, (s.body.hp_frac(i) * 255.0) as u8))
                .collect();
            if !hurt.is_empty() {
                p.send(&ServerMsg::BlocksDamaged { ship: s.id, hits: hurt });
            }
            if s.overdrive > 0.0 {
                p.send(&ServerMsg::ShipBuff { id: s.id, until: self.tick + secs(s.overdrive) });
            }
        }
        for (i, &back_at) in self.pads.iter().enumerate().filter(|&(_, &b)| b > self.tick) {
            p.send(&ServerMsg::PadState { pad: i as u8, back_at });
        }
        for m in &self.mines {
            p.send(&ServerMsg::MineDropped { id: m.id, pos: m.pos, team: m.team, armed_at: m.armed_at });
        }
        for c in &self.clouds {
            p.send(&ServerMsg::Smoke { pos: c.pos, radius: c.radius, until: c.until });
        }
        let watching = p.watching();
        self.players.insert(pid, p);
        if watching {
            self.feed(format!("{name} 开始观战"), false);
            if let Some(p) = self.players.get(&pid) {
                p.send(&ServerMsg::Announce { title: "观战".into(), sub: format!("{} · 附身到任意一辆战车上看", self.cfg.mode.label()) });
            }
            return;
        }
        self.feed(format!("{name} 加入了战场"), false);
        self.scores_dirty = true;
        match self.cfg.mode {
            Mode::Ffa | Mode::Teams => {
                self.spawn_player(pid);
                let sub = match (self.cfg.mode, self.limit) {
                    _ if self.rounds.is_some() => self.room_status(),
                    (Mode::Teams, n) if self.hill.is_some() => {
                        let win = if n > 0 { format!("先到 {n} 分的队伍获胜") } else { "分高的队伍获胜".into() };
                        Some(format!("占住中央高台每秒得 1 分，击毁得 {HILL_KILL} 分 · {win}"))
                    }
                    (Mode::Teams, 0) => Some("击毁对方队伍的战车".to_string()),
                    (Mode::Teams, n) => Some(format!("先拿到 {n} 次击毁的队伍获胜")),
                    (_, 0) => Some("击毁任何其他战车".to_string()),
                    (_, n) => Some(format!("先拿到 {n} 次击毁的人获胜")),
                };
                if let Some(sub) = sub
                    && let Some(p) = self.players.get(&pid)
                {
                    p.send(&ServerMsg::Announce { title: self.cfg.mode.label().into(), sub });
                }
            }
            Mode::Solo => {
                self.spawn_player(pid);
                if let Some(p) = self.players.get(&pid) {
                    p.send(&ServerMsg::Announce { title: "试车场".into(), sub: "只有你自己 · 没有对手，也不限时间".into() });
                }
            }
        }
    }

    fn alloc_ship(&mut self) -> u16 {
        loop {
            let id = self.next_ship;
            self.next_ship = self.next_ship.wrapping_add(1).max(1);
            if !self.ships.contains_key(&id) {
                return id;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_ship(&mut self, design: Arc<Design>, name: String, team: u16, owner: Option<u16>, pos: Vec3, rot: Quat, ai: Option<Skill>) -> u16 {
        let id = self.alloc_ship();
        let body = ShipBody::new(design);
        // set down on the ground, level, facing the way it was told
        let f = (rot * Vec3::NEG_Z).with_y(0.0).normalize_or(Vec3::NEG_Z);
        let rot = look_rotation(Vec3::ZERO, f, Vec3::Y);
        let pos = pos.with_y(spawn_height(&body, &self.terrain, pos.x, pos.z, rot));
        let kin = Kin::new(pos, rot, &body.stats);
        let ws = WeaponState::new(&body);
        let dmg_mul = ai.map_or(1.0, |s| s.dmg_mul);
        let ship = Ship {
            id,
            team,
            name,
            owner,
            kin,
            ws,
            ai: ai.map(|s| AiPilot::new(s, self.rng.next_u64())),
            input: InputCmd::default(),
            hist: vec![(pos, rot); HISTORY_TICKS],
            spawn_tick: self.tick,
            last_attacker: None,
            since_hit: f32::MAX,
            over_pit: 0.0,
            collide_cd: 0.0,
            cut_cd: 0.0,
            cutting: 0.0,
            bite_cd: 0.0,
            burn_cd: 0.0,
            recent_dmg: 0.0,
            disabled_t: None,
            dmg_mul,
            overdrive: 0.0,
            destroyed: Vec::new(),
            damaged: Vec::new(),
            killed: false,
            is_bot: owner.is_none(),
            seat: None,
            body,
        };
        self.events.push(spawn_msg(&ship));
        self.ships.insert(id, ship);
        id
    }

    /// A bot that drives `design` for a match, on `team`; it rolls out at once and respawns. It
    /// comes in the way a player does.
    pub fn add_seat(&mut self, pid: u16, name: String, design: Design, team: Option<u16>) {
        let design = Arc::new(repaint(&design, &mut self.rng));
        self.seats.push(BotSeat { pid, name: name.clone(), design, team, ship: None, kills: 0, deaths: 0, score: 0 });
        self.cfg.bots += 1;
        self.feed(format!("{name} 加入了战场"), false);
        self.spawn_seat(self.seats.len() - 1);
    }

    /// A player leaves a match that goes on without them: AI takes over their vehicle where it
    /// is, under their name and with their score, and nobody is told.
    pub fn hand_over(&mut self, pid: u16) {
        let Some(p) = self.players.remove(&pid) else { return };
        let Some(design) = p.design else { return };
        let seat = self.seats.len();
        self.seats.push(BotSeat { pid, name: p.name, design, team: p.team, ship: p.ship, kills: p.kills, deaths: p.deaths, score: p.score });
        self.cfg.bots += 1;
        let skill = Skill::ffa_bot();
        let ai = AiPilot::new(skill, self.rng.next_u64());
        if let Some(s) = p.ship.and_then(|id| self.ships.get_mut(&id)) {
            s.owner = None;
            s.is_bot = true;
            s.seat = Some(seat);
            s.ai = Some(ai);
            s.dmg_mul = skill.dmg_mul;
        }
        self.scores_dirty = true;
    }

    fn spawn_seat(&mut self, seat: usize) {
        let s = &self.seats[seat];
        let team = s.team.unwrap_or(1000 + s.pid);
        let (design, name) = (s.design.clone(), s.name.clone());
        let (pos, rot) = self.spawn_point(team);
        let id = self.add_ship(design, name, team, None, pos, rot, Some(Skill::ffa_bot()));
        if let Some(s) = self.ships.get_mut(&id) {
            s.seat = Some(seat);
        }
        self.seats[seat].ship = Some(id);
        self.scores_dirty = true;
    }

    /// A bot seat with no ship in the air, creating one (with an unused preset if any) if needed.
    fn free_seat(&mut self) -> usize {
        let flying = |s: &BotSeat, i: usize| s.ship.and_then(|id| self.ships.get(&id)).is_some_and(|sh| sh.seat == Some(i));
        if let Some(i) = self.seats.iter().enumerate().position(|(i, s)| !flying(s, i)) {
            return i;
        }
        // presets within the room's budget (all of them if none fits), unused ones first
        let fits: Vec<usize> = (0..PRESETS.len()).filter(|&p| self.cfg.budget.is_none_or(|b| analyze(&preset(PRESETS[p].key)).stats.cost <= b + 1e-3)).collect();
        let pool = if fits.is_empty() { (0..PRESETS.len()).collect() } else { fits };
        let free: Vec<usize> = pool.iter().copied().filter(|&p| !self.seats.iter().any(|s| s.design.name == PRESETS[p].name)).collect();
        let k = if free.is_empty() { pool[self.rng.below(pool.len())] } else { free[self.rng.below(free.len())] };
        let design = Arc::new(repaint(&preset(PRESETS[k].key), &mut self.rng));
        let pid = crate::new_pid();
        let taken: Vec<&str> = self.players.values().map(|p| p.name.as_str()).chain(self.seats.iter().map(|s| s.name.as_str())).collect();
        let name = crate::names::pick(&mut self.rng, pid, &|n| taken.contains(&n));
        self.seats.push(BotSeat { pid, name, design, team: None, ship: None, kills: 0, deaths: 0, score: 0 });
        self.seats.len() - 1
    }

    fn spawn_player(&mut self, pid: u16) {
        let Some(p) = self.players.get(&pid) else { return };
        let Some(design) = p.design.clone() else { return };
        if p.ship.is_some() {
            return;
        }
        let name = p.name.clone();
        let team = p.team.unwrap_or(1000 + pid);
        let (pos, rot) = match self.cfg.mode {
            Mode::Ffa | Mode::Teams => self.spawn_point(team),
            // the test range: always the same spot, facing the middle of the map
            Mode::Solo => (SPAWN, Quat::IDENTITY),
        };
        let id = self.add_ship(design, name, team, Some(pid), pos, rot, None);
        if let Some(p) = self.players.get_mut(&pid) {
            p.ship = Some(id);
            p.respawn_at = None;
            p.inputs.clear();
            p.send(&ServerMsg::YourShip { id: Some(id) });
        }
        self.scores_dirty = true;
    }

    /// Where a ship of `team` enters: in 2v2 each team on its own side of the arena, facing the
    /// middle; otherwise anywhere clear.
    fn spawn_point(&mut self, team: u16) -> (Vec3, Quat) {
        let terrain = self.terrain.clone();
        if let Some(a) = &terrain.arena {
            // the map's own spots: a free one, then the one the fewest enemies can see, then the
            // one farthest from every ship
            let spots = if self.cfg.mode == Mode::Teams { &a.team_spawns[(team % 2) as usize] } else { &a.spawns };
            let seen = |p: Vec3| self.ships.values().filter(|s| s.team != team && terrain.segment_hit(p + Vec3::Y * 2.0, s.kin.pos + Vec3::Y).is_none()).count() as f32;
            let room = |p: Vec3| {
                let gap = self.ships.values().map(|s| s.kin.pos.with_y(0.0).distance(p)).fold(f32::MAX, f32::min);
                gap.min(200.0) - seen(p) * 300.0 - if gap < 20.0 { 1e5 } else { 0.0 }
            };
            let first = self.rng.below(spots.len());
            let pos = (0..spots.len()).map(|k| spots[(first + k) % spots.len()]).fold(spots[first], |best, p| if room(p) > room(best) + 1.0 { p } else { best });
            return (pos, look_rotation(pos, Vec3::ZERO, Vec3::Y));
        }
        if self.cfg.mode != Mode::Teams {
            return self.ffa_spawn_point();
        }
        let side = if team % 2 == 1 { 1.0 } else { -1.0 };
        for _ in 0..30 {
            let pos = Vec3::new(side * self.rng.range(330.0, 420.0), 0.0, self.rng.range(-150.0, 150.0));
            let clear = self.clear_ground(pos, 15.0) && self.ships.values().all(|s| s.kin.pos.with_y(0.0).distance(pos) > 25.0);
            if clear {
                return (pos, look_rotation(pos, Vec3::ZERO, Vec3::Y));
            }
        }
        self.ffa_spawn_point()
    }

    fn ffa_spawn_point(&mut self) -> (Vec3, Quat) {
        for _ in 0..30 {
            let a = self.rng.range(0.0, std::f32::consts::TAU);
            let pos = Vec3::new(a.cos(), 0.0, a.sin()) * self.rng.range(150.0, 420.0);
            let clear = self.clear_ground(pos, 15.0) && self.ships.values().all(|s| s.kin.pos.with_y(0.0).distance(pos) > 80.0);
            if clear {
                let rot = look_rotation(pos, Vec3::ZERO, Vec3::Y);
                return (pos, rot);
            }
        }
        (SPAWN, Quat::IDENTITY)
    }

    /// No rock within `gap` of (x, z).
    fn clear_ground(&self, pos: Vec3, gap: f32) -> bool {
        self.rocks.iter().all(|a| a.pos.with_y(0.0).distance(pos.with_y(0.0)) > a.r + gap)
    }

    /// Every player's vehicle as good as new, and those without one back in.
    fn repair_players(&mut self) {
        let ids: Vec<u16> = self.players.values().filter_map(|p| p.ship).collect();
        for id in ids {
            if let Some(s) = self.ships.get_mut(&id) {
                s.body.repair();
                s.ws = WeaponState::new(&s.body);
                s.kin.energy = s.body.stats.energy_cap;
                s.kin.shield = s.body.stats.shield_cap;
                s.disabled_t = None;
                self.events.push(ServerMsg::ShipRepaired { id });
            }
        }
        let dead: Vec<u16> = self.players.iter().filter(|(_, p)| p.ship.is_none()).map(|(&k, _)| k).collect();
        for pid in dead {
            self.spawn_player(pid);
        }
    }

    fn run_timers(&mut self) {
        let now = self.tick;
        let due: Vec<Timer> = {
            let mut due = Vec::new();
            let mut i = 0;
            while i < self.timers.len() {
                if self.timers[i].0 <= now {
                    due.push(self.timers.swap_remove(i).1);
                } else {
                    i += 1;
                }
            }
            due
        };
        for t in due {
            match t {
                Timer::SpawnBot => {
                    self.pending_bots = self.pending_bots.saturating_sub(1);
                    let seat = self.free_seat();
                    self.spawn_seat(seat);
                }
            }
        }
    }

    pub fn tick(&mut self) {
        self.tick += 1;
        let t = self.tick;
        self.run_timers();

        // inputs: the command stamped with this tick, or a repeat of the last one if it was late
        for p in self.players.values_mut() {
            let cmd = match p.inputs.remove(&t) {
                Some(c) => {
                    p.ack = t;
                    c
                }
                None => InputCmd { tick: t, ..p.last },
            };
            while let Some((&k, _)) = p.inputs.first_key_value() {
                if k <= t { p.inputs.pop_first(); } else { break; }
            }
            p.last = cmd;
            if let Some(sid) = p.ship
                && let Some(s) = self.ships.get_mut(&sid)
            {
                s.input = cmd;
            }
        }

        // bots: they don't see enemies through smoke, and know when a missile is after them
        let contacts: Vec<Contact> = self.ships.values().filter(|s| !s.killed).map(|s| s.contact()).collect();
        let goals: BTreeMap<u16, Option<(Vec3, f32)>> = self.ships.values().filter(|s| s.ai.is_some()).map(|s| (s.id, self.bot_goal(s))).collect();
        let hunted: Vec<u16> = self.bolts.iter().filter_map(|b| b.target.filter(|id| self.ships.get(id).is_some_and(|s| s.kin.pos.distance(b.pos) < 160.0))).collect();
        for s in self.ships.values_mut() {
            if let Some(ai) = s.ai.as_mut() {
                ai.goal = goals.get(&s.id).copied().flatten();
                ai.hunted = hunted.contains(&s.id);
                let eye = s.kin.pos + Vec3::Y * 1.5;
                let others: Vec<Contact> = contacts.iter().filter(|c| c.id != s.id && (c.team == s.team || !hidden(&self.clouds, eye, c.pos, t))).copied().collect();
                s.input = ai.think(&s.kin, &s.body, s.recent_dmg, s.team, &others, &self.rocks, &self.terrain, t);
            }
        }

        // flight + weapons (and the gadgets: where each mine is laid, where smoke goes up)
        let mut crashes = Vec::new();
        let (mut laid, mut smoke) = (Vec::new(), Vec::new());
        for s in self.ships.values_mut() {
            let knock = step_drive(&mut s.kin, &s.body, &s.input, &self.terrain, DT);
            let rock = collide_rocks(&mut s.kin, &s.body, &self.rocks);
            // the harder of a wall's knock and a rock's
            if let Some(imp) = knock.into_iter().chain(rock).max_by(|a, b| a.speed.total_cmp(&b.speed))
                && s.collide_cd <= 0.0
            {
                s.collide_cd = 0.3;
                let (pos, rot) = (s.kin.pos, s.kin.rot);
                s.splash(pos, rot, imp.point, 1.7, imp.speed * 0.9, DamageKind::Kinetic, None);
                crashes.push(imp.point);
            }
            s.collide_cd -= DT;
            s.overdrive = (s.overdrive - DT).max(0.0);
            s.since_hit += DT;
            s.recent_dmg *= (-1.5 * DT).exp();
            self.shots.clear();
            step_weapons(&mut s.kin, &s.body, &mut s.ws, &s.input, DT, t, false, &mut self.shots);
            if s.ws.mined {
                // on the ground just behind it (short of a wall there)
                let (from, back) = (s.kin.pos, s.kin.pos - s.kin.fwd().with_y(0.0).normalize_or(Vec3::Z) * (s.body.radius - 0.4));
                let at = from.lerp(back, self.terrain.segment_hit(from, back).map_or(1.0, |f| (f - 0.1).max(0.0)));
                laid.push((s.id, s.team, at.with_y(self.terrain.surface(at.x, at.z) + 0.1)));
            }
            if s.ws.smoked {
                smoke.push(s.kin.pos);
            }
            for shot in &self.shots {
                let spec = shot.weapon.weapon().unwrap();
                let id = self.next_bolt;
                self.next_bolt = self.next_bolt.wrapping_add(1);
                self.bolts.push(Bolt {
                    id,
                    pos: shot.pos,
                    prev: shot.pos,
                    vel: shot.vel,
                    team: s.team,
                    owner: s.id,
                    weapon: shot.weapon,
                    dmg: spec.dmg * s.hits_mul(),
                    life: spec.life,
                    rewind: if s.owner.is_some() && !spec.homing { s.input.view_lag as u32 } else { 0 },
                    target: (spec.homing && s.input.lock != NO_LOCK).then_some(s.input.lock),
                    speed: shot.vel.length(),
                });
            }
            s.hist[(t as usize) % HISTORY_TICKS] = (s.kin.pos, s.kin.rot);
        }
        for p in crashes {
            self.events.push(ServerMsg::Crash { pos: p });
        }
        for (owner, team, pos) in laid {
            self.lay_mine(owner, team, pos);
        }
        if let Some(spec) = BlockType::Smoke.gadget() {
            for pos in smoke {
                let c = Cloud { pos, radius: spec.radius, until: t + secs(spec.secs) };
                self.events.push(ServerMsg::Smoke { pos, radius: c.radius, until: c.until });
                self.clouds.push(c);
            }
        }

        self.ship_collisions();
        self.melee();
        self.hazards(t);
        self.mines(t);
        self.update_bolts(t);
        self.settle_ships();
        self.pick_ups();
        self.hold_hill();

        // into a pit (or hung on its edge): down to whoever last hit it, if that was lately
        if let Some(a) = &self.terrain.arena {
            for s in self.ships.values_mut() {
                s.over_pit = if a.in_pit(s.kin.pos.x, s.kin.pos.z) { s.over_pit + DT } else { 0.0 };
            }
            let fell: Vec<(u16, Option<u16>)> = self.ships.values().filter(|s| s.kin.pos.y < KILL_Y || s.over_pit > PIT_HANG).map(|s| (s.id, s.last_attacker.filter(|_| s.since_hit < PIT_CREDIT))).collect();
            for (id, by) in fell {
                self.kill(id, by, Fate::Fell);
            }
        }

        // vehicles that can neither drive nor shoot give up after a few seconds
        let mut give_up = Vec::new();
        for s in self.ships.values_mut() {
            if (!s.body.can_drive() || s.body.movers.len() < 2) && s.body.stats.weapons.is_empty() {
                let left = s.disabled_t.get_or_insert(3.0);
                *left -= DT;
                if *left <= 0.0 {
                    give_up.push((s.id, s.last_attacker));
                }
            }
        }
        for (id, by) in give_up {
            self.kill(id, by, Fate::Wrecked);
        }

        self.ffa_logic();
        self.flush(t);
    }

    /// Vehicles that run into each other: pushed apart and bounced off, and both hurt round where
    /// they touched. The heavier one takes less, and so does the one that drove into the other;
    /// a ram there hits much harder and takes little.
    fn ship_collisions(&mut self) {
        let ids: Vec<u16> = self.ships.keys().copied().collect();
        for a in 0..ids.len() {
            for b in a + 1..ids.len() {
                let (ia, ib) = (ids[a], ids[b]);
                let (sa, sb) = (&self.ships[&ia], &self.ships[&ib]);
                if sa.kin.pos.distance(sb.kin.pos) > sa.body.radius + sb.body.radius {
                    continue;
                }
                // where they touch: the middle of A's blocks that are inside B's
                let (mut sum, mut n_in) = (Vec3::ZERO, 0);
                for i in (0..sa.body.alive.len()).filter(|&i| sa.body.alive[i]) {
                    let w = sa.body.world(sa.kin.pos, sa.kin.rot, i);
                    if sb.body.block_at(sb.body.to_grid(sb.kin.pos, sb.kin.rot, w).round().as_ivec3()).is_some() {
                        sum += w;
                        n_in += 1;
                    }
                }
                if n_in == 0 {
                    continue;
                }
                let cp = sum / n_in as f32;
                let n = (sa.kin.pos - sb.kin.pos).normalize_or(Vec3::Y);
                let (ma, mb) = (sa.body.stats.mass.max(1.0), sb.body.stats.mass.max(1.0));
                let rel = (sa.kin.vel - sb.kin.vel).dot(n);
                let cd_ready = sa.collide_cd <= 0.0 || sb.collide_cd <= 0.0;
                let friendly = sa.team == sb.team;
                // which of them drove into the other, and the rams at the point
                let (into_a, into_b) = (sa.kin.vel.dot(-n), sb.kin.vel.dot(n));
                let (ram_a, ram_b) = (sa.ram_at(cp), sb.ram_at(cp));
                self.ships.get_mut(&ia).unwrap().kin.pos += n * 0.8 * mb / (ma + mb);
                self.ships.get_mut(&ib).unwrap().kin.pos -= n * 0.8 * ma / (ma + mb);
                if rel >= 0.0 {
                    continue;
                }
                let j = -CRASH_BOUNCE * rel / (1.0 / ma + 1.0 / mb);
                self.ships.get_mut(&ia).unwrap().kin.vel += n * (j / ma);
                self.ships.get_mut(&ib).unwrap().kin.vel -= n * (j / mb);
                if !cd_ready {
                    continue;
                }
                let dmg = (-rel * 1.2).min(90.0) * if friendly { 0.3 } else { 1.0 };
                let rammed = |by: Option<&MeleeSpec>| by.map_or(1.0, |m| m.dmg);
                let guarded = |own: Option<&MeleeSpec>| own.map_or(1.0, |m| m.guard);
                let (drove_a, drove_b) = if into_a > into_b + 1.0 { (0.7, 1.3) } else if into_b > into_a + 1.0 { (1.3, 0.7) } else { (1.0, 1.0) };
                let to_a = dmg * 2.0 * mb / (ma + mb) * drove_a * rammed(ram_b) * guarded(ram_a);
                let to_b = dmg * 2.0 * ma / (ma + mb) * drove_b * rammed(ram_a) * guarded(ram_b);
                for (id, other, d) in [(ia, ib, to_a), (ib, ia, to_b)] {
                    let s = self.ships.get_mut(&id).unwrap();
                    s.collide_cd = 0.35;
                    let (pos, rot) = (s.kin.pos, s.kin.rot);
                    s.splash(pos, rot, cp, 1.9, d, DamageKind::Kinetic, Some(other));
                }
                self.events.push(ServerMsg::Crash { pos: cp });
            }
        }
    }

    /// Saws cut what is just ahead of them, a chunk at a time; flippers that threw this tick throw
    /// whatever enemy is in front of them up and over.
    fn melee(&mut self) {
        // (attacker, target, block, damage, where, by a saw); (target, its change of speed and of
        // spin)
        let mut cuts: Vec<(u16, u16, usize, f32, Vec3, bool)> = Vec::new();
        let mut throws: Vec<(u16, Vec3, Vec3)> = Vec::new();
        for s in self.ships.values_mut() {
            s.cut_cd -= DT;
            s.cutting -= DT;
        }
        for s in self.ships.values() {
            let flipping = s.ws.flipped;
            if s.body.stats.melee.is_empty() || (s.cut_cd > 0.0 && !flipping) {
                continue;
            }
            let fwd = s.kin.fwd().with_y(0.0).normalize_or(Vec3::NEG_Z);
            for o in self.ships.values() {
                if o.team == s.team || o.kin.pos.distance(s.kin.pos) > s.body.radius + o.body.radius + 3.0 {
                    continue;
                }
                let mut thrown = false;
                for (m, mid, r) in s.melee_reach() {
                    let nearest = o.body.within(o.kin.pos, o.kin.rot, mid, r).into_iter().min_by(|a, b| a.1.total_cmp(&b.1));
                    let Some((i, _)) = nearest else { continue };
                    let at = o.body.world(o.kin.pos, o.kin.rot, i);
                    match m.kind {
                        Melee::Saw if s.cut_cd <= 0.0 => cuts.push((s.id, o.id, i, m.dmg * CUT_EVERY * s.hits_mul(), at, true)),
                        Melee::Flipper if flipping && !thrown => {
                            thrown = true;
                            // light vehicles fly, heavy ones only rock
                            let k = (70.0 / o.body.stats.mass.max(1.0)).clamp(0.35, 1.2);
                            throws.push((o.id, (Vec3::Y * m.lift + fwd * m.push) * k, Vec3::Y.cross(fwd) * 3.0 * k));
                            cuts.push((s.id, o.id, i, m.dmg * s.hits_mul(), at, false));
                        }
                        _ => {}
                    }
                }
            }
        }
        for (by, target, i, dmg, at, saw) in cuts {
            self.ships.get_mut(&target).unwrap().hit_block(i, dmg, DamageKind::Kinetic, Some(by));
            if saw {
                let s = self.ships.get_mut(&by).unwrap();
                (s.cut_cd, s.cutting) = (CUT_EVERY, 0.25);
            } else {
                self.events.push(ServerMsg::Crash { pos: at });
            }
        }
        for (target, dv, spin) in throws {
            let o = self.ships.get_mut(&target).unwrap();
            o.kin.vel += dv;
            o.kin.ang += spin;
        }
    }

    /// The arena's saws bite what touches them, and its vents burn what stands in their flames.
    /// No one gets the kill for either, unless they hit the victim lately.
    fn hazards(&mut self, t: u32) {
        let ground = self.terrain.clone();
        let Some(a) = ground.arena.as_ref() else { return };
        let now = tick_time(t);
        for s in self.ships.values_mut() {
            s.bite_cd -= DT;
            s.burn_cd -= DT;
            if s.bite_cd <= 0.0
                && let Some((_, q, _)) = a.saws.iter().find_map(|saw| saw_bites(&s.kin, &s.body, saw, now).first().copied())
            {
                let (pos, rot) = (s.kin.pos, s.kin.rot);
                s.splash(pos, rot, q, 1.3, SAW_DMG, DamageKind::Kinetic, None);
                s.bite_cd = 0.3;
                self.events.push(ServerMsg::Crash { pos: q });
            }
            if s.burn_cd <= 0.0 {
                for v in a.vents.iter().filter(|v| v.burning(now)) {
                    let hot: Vec<usize> = (0..s.body.alive.len()).filter(|&i| s.body.alive[i]).filter(|&i| {
                        let w = s.body.world(s.kin.pos, s.kin.rot, i);
                        v.area.contains(w.x, w.z) && w.y < VENT_HIGH
                    }).collect();
                    if !hot.is_empty() {
                        let each = VENT_DPS * BURN_EVERY / hot.len() as f32;
                        for i in hot {
                            s.hit_block(i, each, DamageKind::Energy, None);
                        }
                        s.burn_cd = BURN_EVERY;
                    }
                }
            }
        }
    }

    /// A mine laid: one vehicle's oldest goes when it has as many down as it may.
    fn lay_mine(&mut self, owner: u16, team: u16, pos: Vec3) {
        let Some(spec) = BlockType::MineLayer.gadget() else { return };
        let mine = self.mines.iter().filter(|m| m.owner == owner).count();
        if mine >= spec.count as usize
            && let Some(i) = self.mines.iter().position(|m| m.owner == owner)
        {
            let old = self.mines.remove(i);
            self.events.push(ServerMsg::MineGone { id: old.id, exploded: false });
        }
        let id = self.next_mine;
        self.next_mine = self.next_mine.wrapping_add(1);
        let armed_at = self.tick + secs(MINE_ARM);
        self.mines.push(Mine { id, pos, team, owner, armed_at, gone_at: self.tick + secs(spec.secs) });
        self.events.push(ServerMsg::MineDropped { id, pos, team, armed_at });
    }

    /// Live mines go off under the first enemy to come near, blasting every enemy in reach (and
    /// kicking them up a little); old ones are cleared away. Clouds that have cleared go too.
    fn mines(&mut self, t: u32) {
        self.clouds.retain(|c| c.until > t);
        let Some(spec) = BlockType::MineLayer.gadget() else { return };
        let mut i = 0;
        while i < self.mines.len() {
            let m = &self.mines[i];
            if t >= m.gone_at {
                self.events.push(ServerMsg::MineGone { id: m.id, exploded: false });
                self.mines.remove(i);
                continue;
            }
            let near = |s: &Ship, r: f32| !s.killed && s.team != m.team && s.kin.pos.distance(m.pos) < s.body.radius + r && !s.body.within(s.kin.pos, s.kin.rot, m.pos, r).is_empty();
            if t < m.armed_at || !self.ships.values().any(|s| near(s, spec.reach)) {
                i += 1;
                continue;
            }
            let caught: Vec<u16> = self.ships.values().filter(|s| near(s, spec.radius.max(spec.reach))).map(|s| s.id).collect();
            let m = self.mines.remove(i);
            for id in caught {
                let s = self.ships.get_mut(&id).unwrap();
                let (pos, rot) = (s.kin.pos, s.kin.rot);
                s.splash(pos, rot, m.pos, spec.radius, spec.dmg, DamageKind::Explosive, Some(m.owner));
                s.kin.vel += Vec3::Y * 5.0 * (50.0 / s.body.stats.mass.max(1.0)).clamp(0.3, 1.0);
            }
            self.events.push(ServerMsg::Explosion { pos: m.pos + Vec3::Y * 0.5, size: 1.6 });
            self.events.push(ServerMsg::MineGone { id: m.id, exploded: true });
        }
    }

    fn update_bolts(&mut self, t: u32) {
        // (shooter ship, ship hit, on its shield) for the hit markers
        let mut landed: Vec<(u16, u16, bool)> = Vec::new();
        let mut i = 0;
        while i < self.bolts.len() {
            let b = &mut self.bolts[i];
            b.prev = b.pos;
            let spec = b.weapon.weapon().unwrap();
            if spec.homing {
                let cur = b.vel.normalize_or(Vec3::NEG_Z);
                let mut dir = cur;
                // smoke between it and its target: it loses it, and flies on straight
                if b.target.and_then(|tid| self.ships.get(&tid)).is_some_and(|tg| hidden(&self.clouds, b.pos, tg.kin.pos, t)) {
                    b.target = None;
                }
                if let Some(tid) = b.target
                    && let Some(tg) = self.ships.get(&tid)
                {
                    let des = (tg.kin.pos - b.pos).normalize_or(cur);
                    let ang = cur.angle_between(des);
                    if ang > 1e-4 {
                        dir = cur.lerp(des, (MISSILE_TURN * DT / ang).min(1.0)).normalize_or(cur);
                    }
                }
                b.speed = (b.speed + MISSILE_ACCEL * DT).min(MISSILE_TOP);
                b.vel = dir * b.speed;
            }
            b.pos += b.vel * DT;
            b.life -= DT;
            let mut dead = b.life <= 0.0;
            if !dead {
                for s in self.ships.values_mut() {
                    if s.killed || s.team == b.team {
                        continue;
                    }
                    let (sp, sr) = s.transform_at(t, b.rewind);
                    let shielded = s.kin.shield > 0.0;
                    let r = s.body.radius * if shielded { 1.08 } else { 1.0 } + 0.6;
                    let Some(tt) = seg_sphere(b.prev, b.pos, sp, r) else { continue };
                    if shielded {
                        let p = b.prev.lerp(b.pos, tt);
                        let pt = sp + (p - sp).normalize_or(Vec3::Y) * s.body.radius * 1.08;
                        s.kin.shield = (s.kin.shield - balance().on_shield(b.dmg, spec.kind)).max(0.0);
                        s.kin.shield_delay = 3.0;
                        s.recent_dmg += b.dmg * 0.5;
                        s.last_attacker = Some(b.owner);
                        s.since_hit = 0.0;
                        landed.push((b.owner, s.id, true));
                        self.events.push(ServerMsg::ShieldHit { ship: s.id, pos: pt });
                        if spec.splash > 0.0 {
                            self.events.push(ServerMsg::Explosion { pos: pt, size: 0.6 });
                        }
                        dead = true;
                        break;
                    }
                    if let Some((blk, pt)) = s.body.ray(sp, sr, b.prev, b.pos) {
                        landed.push((b.owner, s.id, false));
                        if spec.splash > 0.0 {
                            self.events.push(ServerMsg::Explosion { pos: pt, size: spec.splash * 0.55 });
                            s.splash(sp, sr, pt, spec.splash, b.dmg, spec.kind, Some(b.owner));
                        } else if spec.pierce > 1 {
                            // on through the ship: the next blocks along the line, each hit softer
                            let dir = (b.pos - b.prev).normalize_or(Vec3::NEG_Z);
                            let mut dmg = b.dmg;
                            for i in s.body.ray_through(sp, sr, b.prev, b.pos + dir * s.body.radius * 2.0, spec.pierce as usize) {
                                s.hit_block(i, dmg, spec.kind, Some(b.owner));
                                dmg *= blocktread_shared::balance::balance().pierce_falloff;
                            }
                        } else {
                            s.hit_block(blk, b.dmg, spec.kind, Some(b.owner));
                        }
                        dead = true;
                        break;
                    }
                }
            }
            if !dead && let Some(tt) = self.terrain.segment_hit(b.prev, b.pos) {
                // into the ground, a wall, a crate
                if spec.splash > 0.0 {
                    self.events.push(ServerMsg::Explosion { pos: b.prev.lerp(b.pos, tt), size: 0.7 });
                }
                dead = true;
            }
            if !dead {
                for a in &self.rocks {
                    if let Some(tt) = a.hit_segment(b.prev, b.pos) {
                        if spec.splash > 0.0 {
                            self.events.push(ServerMsg::Explosion { pos: b.prev.lerp(b.pos, tt), size: 0.7 });
                        }
                        dead = true;
                        break;
                    }
                }
            } else if b.life <= 0.0 && spec.homing {
                self.events.push(ServerMsg::Explosion { pos: b.pos, size: 0.6 });
            }
            if dead {
                self.bolts.swap_remove(i);
            } else {
                i += 1;
            }
        }
        // tell each shooter that has a player behind it
        for (shooter, target, shield) in landed {
            if let Some(p) = self.ships.get(&shooter).and_then(|s| s.owner).and_then(|pid| self.players.get(&pid)) {
                p.send(&ServerMsg::HitConfirm { target, shield });
            }
        }
    }

    fn settle_ships(&mut self) {
        let mut kills = Vec::new();
        for s in self.ships.values_mut() {
            let hurt = !s.damaged.is_empty() || !s.destroyed.is_empty();
            if !s.damaged.is_empty() {
                let mut hits = std::mem::take(&mut s.damaged);
                hits.retain(|(i, _)| s.body.alive[*i as usize]);
                let mut seen = HashMap::new();
                for (i, f) in hits {
                    seen.insert(i, f);
                }
                if !seen.is_empty() {
                    self.events.push(ServerMsg::BlocksDamaged { ship: s.id, hits: seen.into_iter().collect() });
                }
            }
            if !s.destroyed.is_empty() {
                let blocks = std::mem::take(&mut s.destroyed);
                self.events.push(ServerMsg::BlocksDestroyed { ship: s.id, blocks });
                s.body.settle();
            }
            // the cockpit is gone, or so little of the ship is left that it falls apart
            if hurt && (!s.body.cockpit_alive() || s.body.hp_total() < balance().collapse_below * s.body.max_hp) {
                kills.push((s.id, s.last_attacker));
            }
        }
        for (id, by) in kills {
            self.kill(id, by, Fate::Wrecked);
        }
    }

    fn kill(&mut self, id: u16, by: Option<u16>, fate: Fate) {
        let Some(mut victim) = self.ships.remove(&id) else { return };
        victim.killed = true;
        self.events.push(ServerMsg::ShipKilled { id, killer: by });
        for b in &mut self.bolts {
            if b.target == Some(id) {
                b.target = None;
            }
        }
        let killer = by.and_then(|k| self.ships.get(&k));
        let killer_name = killer.map(|k| k.name.clone());
        let killer_owner = killer.and_then(|k| k.owner);
        let killer_seat = killer.and_then(|k| k.seat);
        // between two rounds the results stand: nothing counts until the next one
        let counts = self.round != Round::Over;
        if let Some(k) = killer.filter(|k| k.team != victim.team && counts) {
            *self.team_kills.entry(k.team).or_insert(0) += 1;
            if let Some(h) = self.hill.as_mut() {
                *h.points.entry(k.team).or_insert(0) += HILL_KILL;
            }
        }
        let pts = 100 + (victim.body.max_hp / 10.0) as u32;
        if let Some(pid) = killer_owner.filter(|_| counts)
            && let Some(p) = self.players.get_mut(&pid)
        {
            p.kills += 1;
            p.score += pts;
        }
        if let Some(s) = killer_seat.and_then(|i| self.seats.get_mut(i)) {
            s.kills += 1;
            s.score += pts;
        }
        if let Some(s) = victim.seat.and_then(|i| self.seats.get_mut(i)) {
            s.deaths += 1;
            s.ship = None;
        }
        if let Some(pid) = victim.owner
            && let Some(p) = self.players.get_mut(&pid)
        {
            p.deaths += counts as u32;
            p.ship = None;
            p.send(&ServerMsg::YourShip { id: None });
            p.respawn_at = Some(self.tick + secs(RESPAWN_SECS));
        }
        let text = match (killer_name, fate) {
            (Some(k), Fate::Wrecked) => format!("{k} 击毁了 {}", victim.name),
            (None, Fate::Wrecked) => format!("{} 报废了", victim.name),
            (Some(k), Fate::Fell) => format!("{k} 把 {} 逼进了深坑", victim.name),
            (None, Fate::Fell) => format!("{} 掉进了深坑", victim.name),
        };
        self.feed(text, victim.owner.is_some());
        self.scores_dirty = true;
    }

    /// Where a bot would rather be than wherever its fight takes it: a repair when it is badly
    /// hurt, an overdrive close by, and in 2v2 the top of the hill. With the radius it counts as
    /// there within.
    fn bot_goal(&self, s: &Ship) -> Option<(Vec3, f32)> {
        let a = self.terrain.arena.as_ref()?;
        let nearest = |kind: PadKind| {
            a.pads.iter().zip(&self.pads).filter(|(p, b)| p.kind == kind && **b <= self.tick).map(|(p, _)| p.at).min_by(|x, y| x.distance_squared(s.kin.pos).total_cmp(&y.distance_squared(s.kin.pos)))
        };
        if s.body.hp_total() < s.body.max_hp * 0.45
            && let Some(p) = nearest(PadKind::Repair)
        {
            return Some((p, 1.0));
        }
        if s.overdrive <= 0.0
            && let Some(p) = nearest(PadKind::Overdrive).filter(|p| p.distance(s.kin.pos) < 50.0)
        {
            return Some((p, 1.0));
        }
        self.hill.as_ref().map(|_| (a.hill.centre().with_y(HILL_TOP), HILL_HALF - 3.0))
    }

    /// The arena's pads: the nearest vehicle on one that is up, and that has a use for it, takes
    /// it, and it comes back a while later.
    fn pick_ups(&mut self) {
        let ground = self.terrain.clone();
        let Some(a) = ground.arena.as_ref() else { return };
        for (i, pad) in a.pads.iter().enumerate() {
            if self.pads[i] > self.tick {
                continue;
            }
            let on = self.ships.values_mut().filter(|s| pad.under(s.kin.pos) && s.wants(pad.kind));
            let Some(s) = on.min_by(|x, y| x.kin.pos.distance_squared(pad.at).total_cmp(&y.kin.pos.distance_squared(pad.at))) else { continue };
            match pad.kind {
                PadKind::Repair => {
                    s.body.repair();
                    s.disabled_t = None;
                    self.events.push(ServerMsg::ShipRepaired { id: s.id });
                }
                PadKind::Energy => {
                    s.kin.energy = s.body.stats.energy_cap;
                    s.kin.shield = s.body.stats.shield_cap;
                }
                PadKind::Overdrive => {
                    s.overdrive = OVERDRIVE_SECS;
                    self.events.push(ServerMsg::ShipBuff { id: s.id, until: self.tick + secs(OVERDRIVE_SECS) });
                    self.events.push(ServerMsg::Feed { text: format!("{} 拿到了超载", s.name), bad: false });
                }
            }
            self.pads[i] = self.tick + secs(pad.kind.respawn());
            self.events.push(ServerMsg::PadState { pad: i as u8, back_at: self.pads[i] });
        }
    }

    /// 2v2 on the hill: a team alone on top scores a point a second; with both on it, neither does.
    fn hold_hill(&mut self) {
        let (Some(a), Some(h)) = (self.terrain.arena.as_ref(), self.hill.as_mut()) else { return };
        let mut on: Vec<u16> = self.ships.values().filter(|s| a.on_hill(s.kin.pos)).map(|s| s.team).collect();
        on.sort_unstable();
        on.dedup();
        let (holder, contested) = match on[..] {
            [] => (None, false),
            [t] => (Some(t), false),
            _ => (None, true),
        };
        if (holder, contested) != (h.holder, h.contested) {
            (h.holder, h.contested, h.held) = (holder, contested, 0.0);
            self.scores_dirty = true;
        }
        if let Some(t) = holder {
            h.held += DT;
            if h.held >= 1.0 {
                h.held -= 1.0;
                *h.points.entry(t).or_insert(0) += 1;
                self.scores_dirty = true;
            }
        }
    }

    fn ffa_logic(&mut self) {
        let now = self.tick;
        let due: Vec<u16> = self.players.iter().filter(|(_, p)| p.respawn_at.is_some_and(|t| t <= now)).map(|(&k, _)| k).collect();
        for pid in due {
            self.spawn_player(pid);
        }
        self.round_logic();
        if self.players.is_empty() {
            return;
        }
        let bots = self.ships.values().filter(|s| s.is_bot).count() as u32;
        if bots + self.pending_bots < self.cfg.bots {
            self.pending_bots += 1;
            let delay = if self.tick < secs(5.0) { 1.5 } else { RESPAWN_SECS };
            self.after(delay, Timer::SpawnBot);
        }
    }

    /// The free-for-all room's clock: a round starts once two players are in and is ranked by
    /// kills when its time is up; then, after a short break, the next one starts from nothing.
    /// With fewer than two in, the room waits (and a round under way is called off).
    fn round_logic(&mut self) {
        let Some(len) = self.rounds else { return };
        let enough = self.pilots() >= 2;
        let due = self.tick >= self.round_end;
        match self.round {
            Round::None | Round::Waiting if enough => self.start_round(len),
            Round::None => {
                self.round = Round::Waiting;
                self.scores_dirty = true;
            }
            Round::Running if !enough => {
                self.round = Round::Waiting;
                self.scores_dirty = true;
                self.announce("等待中", "场上不到 2 人，本局取消 · 再来一名玩家就重新计时".into());
            }
            Round::Running if due => self.end_round(),
            Round::Over if due && enough => self.start_round(len),
            Round::Over if due => {
                self.round = Round::Waiting;
                self.scores_dirty = true;
                self.announce("等待中", "再来一名玩家就开始下一局".into());
            }
            _ => {}
        }
    }

    fn start_round(&mut self, len: f32) {
        for p in self.players.values_mut() {
            p.kills = 0;
            p.deaths = 0;
            p.score = 0;
        }
        self.team_kills.clear();
        self.repair_players();
        self.round = Round::Running;
        self.round_no += 1;
        self.round_start = self.tick;
        self.round_end = self.tick + secs(len);
        self.scores_dirty = true;
        self.announce("开战", format!("{} 人混战 · {} 内击毁最多的人获胜", self.pilots(), span(len)));
    }

    fn end_round(&mut self) {
        self.round = Round::Over;
        self.round_end = self.tick + secs(ROUND_BREAK_SECS);
        self.scores_dirty = true;
        let rows = self.scores();
        // each player's showing: the minutes of the round they were in, their kills, their place
        for (place, r) in rows.iter().enumerate() {
            if let Some(p) = self.players.get(&r.pid) {
                let secs = self.tick.saturating_sub(p.since.max(self.round_start)) as f32 / TICK_RATE as f32;
                self.results.push((r.pid, Showing { secs, kills: r.kills, won: false, place: Some(place) }));
            }
        }
        let next = format!("{:.0} 秒后开始下一局", ROUND_BREAK_SECS);
        let Some(top) = rows.first().filter(|r| r.kills > 0) else {
            self.announce("本局结束", format!("没有人击毁对手 · {next}"));
            return;
        };
        // ranked by kills, then fewer deaths
        let tied: Vec<&str> = rows.iter().filter(|r| (r.kills, r.deaths) == (top.kills, top.deaths)).map(|r| r.name.as_str()).collect();
        let (title, sub) = if tied.len() > 1 {
            ("平局".to_string(), format!("{} 并列第一 · {next}", tied.join("、")))
        } else {
            (format!("{} 获胜", top.name), format!("击毁 {} · 阵亡 {} · {next}", top.kills, top.deaths))
        };
        self.announce(&title, sub);
    }

    /// Showings from the rounds that ended since the last call, with the round each was in.
    pub fn take_results(&mut self) -> (u32, Vec<(u16, Showing)>) {
        (self.round_no, std::mem::take(&mut self.results))
    }

    /// How each player still in it did over the whole game (a 2v2 at its end): `winner` is the
    /// winning team, if one won.
    pub fn showings(&self, winner: Option<u16>) -> Vec<(u16, Showing)> {
        self.players
            .iter()
            .filter(|(_, p)| !p.watching())
            .map(|(&pid, p)| {
                let secs = self.tick.saturating_sub(p.since) as f32 / TICK_RATE as f32;
                (pid, Showing { secs, kills: p.kills, won: winner.is_some_and(|w| p.team == Some(w)), place: None })
            })
            .collect()
    }

    /// For someone driving into the room: where its round stands (None when one starts right away).
    fn room_status(&self) -> Option<String> {
        let left = self.round_end.saturating_sub(self.tick) as f32 / TICK_RATE as f32;
        match self.round {
            _ if self.pilots() >= 2 && matches!(self.round, Round::None | Round::Waiting) => None,
            Round::None | Round::Waiting => Some("等待中 · 再来一名玩家就开始计时".into()),
            Round::Running => Some(format!("本局还剩 {} · 击毁最多的人获胜", span(left))),
            Round::Over => Some(format!("{:.0} 秒后开始下一局", left.ceil())),
        }
    }

    /// The scoreboard: players and bot seats, best first.
    pub fn scores(&self) -> Vec<ScoreRow> {
        let mut rows: Vec<ScoreRow> = self
            .players
            .iter()
            .filter(|(_, p)| !p.watching())
            .map(|(&pid, p)| {
                let team = p.team.unwrap_or(1000 + pid);
                ScoreRow { pid, name: p.name.clone(), kills: p.kills, deaths: p.deaths, score: p.score, alive: p.ship.is_some(), team }
            })
            .chain(self.seats.iter().map(|s| ScoreRow {
                pid: s.pid,
                name: s.name.clone(),
                kills: s.kills,
                deaths: s.deaths,
                score: s.score,
                alive: s.ship.is_some(),
                team: s.team.unwrap_or(1000 + s.pid),
            }))
            .collect();
        // by kills, then fewer deaths
        rows.sort_by(|a, b| b.kills.cmp(&a.kills).then(a.deaths.cmp(&b.deaths)).then(b.score.cmp(&a.score)));
        rows
    }

    /// Send this tick's events to everyone in the game.
    fn send_events(&mut self) {
        if !self.events.is_empty() {
            let encoded: Vec<Bytes> = self.events.drain(..).map(|m| Bytes::from(encode(&m))).collect();
            for p in self.players.values() {
                for b in &encoded {
                    let _ = p.out.send(Outgoing::Reliable(b.clone()));
                }
            }
        }
    }

    fn flush(&mut self, t: u32) {
        if self.scores_dirty && t % 15 == 0 {
            self.scores_dirty = false;
            let rows = self.scores();
            let end = match self.round {
                Round::Running | Round::Over => Some(self.round_end),
                Round::Waiting => None,
                Round::None => self.ends_at,
            };
            let left = end.map_or(0.0, |e| e.saturating_sub(self.tick) as f32 / TICK_RATE as f32);
            let hill = self.hill.as_ref().map(|h| HillState { points: [1, 2].map(|t| h.points.get(&t).copied().unwrap_or(0)), holder: h.holder, contested: h.contested });
            self.events.push(ServerMsg::Scores { rows, limit: self.limit, left, round: self.round, hill });
        }
        self.send_events();
        if t % SNAPSHOT_INTERVAL != 0 {
            return;
        }
        let all: Vec<ShipSnap> = self
            .ships
            .values()
            .map(|s| ShipSnap {
                id: s.id,
                pos: s.kin.pos,
                rot: quantize_quat(s.kin.rot),
                vel: s.kin.vel,
                flags: s.flags(),
                throttle: (s.input.throttle_f() * 255.0) as u8,
                shield: if s.body.stats.shield_cap > 0.0 { (s.kin.shield / s.body.stats.shield_cap * 255.0) as u8 } else { 0 },
                aim: s.input.aim,
            })
            .collect();
        let missiles: Vec<MissileSnap> = self.bolts.iter().filter(|b| b.weapon == BlockType::Missile).map(|b| MissileSnap { id: b.id, pos: b.pos, vel: b.vel, team: b.team, owner: b.owner }).collect();
        for p in self.players.values() {
            let own = p.ship.and_then(|id| self.ships.get(&id).map(|s| OwnState { ship: id, kin: s.kin, flags: s.flags() }));
            let ships: Vec<ShipSnap> = all.iter().filter(|s| Some(s.id) != p.ship).copied().collect();
            let lead = (p.newest as i64 - t as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
            let mut snap = Snapshot { tick: t, ack: p.ack, lead, own, ships, missiles: missiles.clone() };
            let mut bytes = encode(&snap);
            if bytes.len() <= MAX_DATAGRAM {
                let _ = p.out.send(Outgoing::Datagram(Bytes::from(bytes)));
                continue;
            }
            // too big for one datagram: first part carries own state + missiles, the rest carry ships
            let ships = std::mem::take(&mut snap.ships);
            snap.missiles.truncate(20);
            bytes = encode(&snap);
            let _ = p.out.send(Outgoing::Datagram(Bytes::from(bytes)));
            for chunk in ships.chunks(20) {
                let part = Snapshot { tick: t, ack: p.ack, lead, own: None, ships: chunk.to_vec(), missiles: Vec::new() };
                let _ = p.out.send(Outgoing::Datagram(Bytes::from(encode(&part))));
            }
        }
    }
}

/// The outcome of one `duel`.
#[derive(Clone, Copy, Debug)]
pub struct Duel {
    /// The side (0 or 1) still flying, or with more of its hull left when time ran out; None for
    /// a draw.
    pub winner: Option<usize>,
    pub secs: f32,
    /// Share of each side's hit points left.
    pub hull: [f32; 2],
}

/// A headless 1v1 for balance testing: two AI pilots of the same skill fly `a` and `b` at each
/// other in the normal arena, rocks and all, until one goes down or `max_secs` pass. Run it
/// many times with different seeds; one fight says little.
pub fn duel(a: &Design, b: &Design, seed: u64, max_secs: f32) -> Duel {
    let mut g = Game::with_map(GameConfig { mode: Mode::Ffa, bots: 0, budget: None, verbose: false }, seed, Map::Desert);
    // full damage on both sides: free-for-all bots normally hit a little softer than players
    let skill = Skill { dmg_mul: 1.0, ..Skill::ffa_bot() };
    // two spots 300 m apart, clear of rocks, facing each other
    let clear = |g: &Game, p: Vec3| g.clear_ground(p, 20.0);
    let mut spots = (Vec3::new(-150.0, 0.0, 0.0), Vec3::new(150.0, 0.0, 0.0));
    for _ in 0..60 {
        let c = g.rng.unit().with_y(0.0) * g.rng.range(0.0, 250.0);
        let d = g.rng.unit().with_y(0.0).normalize_or(Vec3::X) * 150.0;
        if clear(&g, c - d) && clear(&g, c + d) {
            spots = (c - d, c + d);
            break;
        }
    }
    let (pa, pb) = spots;
    let ia = g.add_ship(Arc::new(a.clone()), "A".into(), 1, None, pa, look_rotation(pa, pb, Vec3::Y), Some(skill));
    let ib = g.add_ship(Arc::new(b.clone()), "B".into(), 2, None, pb, look_rotation(pb, pa, Vec3::Y), Some(skill));
    let hull = |g: &Game, id: u16| g.ships.get(&id).map_or(0.0, |s| s.body.hp_total() / s.body.max_hp);
    let ticks = (max_secs * TICK_RATE as f32) as u32;
    for n in 0..ticks {
        g.tick();
        let (a_up, b_up) = (g.ships.contains_key(&ia), g.ships.contains_key(&ib));
        if !(a_up && b_up) {
            let winner = match (a_up, b_up) {
                (true, false) => Some(0),
                (false, true) => Some(1),
                _ => None,
            };
            return Duel { winner, secs: n as f32 / TICK_RATE as f32, hull: [hull(&g, ia), hull(&g, ib)] };
        }
    }
    let h = [hull(&g, ia), hull(&g, ib)];
    let winner = if (h[0] - h[1]).abs() < 0.02 { None } else if h[0] > h[1] { Some(0) } else { Some(1) };
    Duel { winner, secs: max_secs, hull: h }
}

fn spawn_msg(s: &Ship) -> ServerMsg {
    ServerMsg::ShipSpawn { id: s.id, team: s.team, name: s.name.clone(), design: (*s.body.design).clone(), kin: s.kin }
}


#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    /// The free-for-all room with these players in, its round under way.
    fn room(names: &[&str]) -> (Game, Vec<UnboundedReceiver<Outgoing>>) {
        let mut g = Game::with_seed(GameConfig { mode: Mode::Ffa, bots: 0, budget: None, verbose: false }, 1);
        g.rounds = Some(60.0);
        let mut rxs = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let (out, rx) = unbounded_channel();
            g.join(i as u16 + 1, name.to_string(), Some(preset("bison")), None, out);
            rxs.push(rx);
        }
        g.tick();
        assert_eq!(g.round, Round::Running);
        (g, rxs)
    }

    /// `killer` takes out `victim` (by pid), who is straight back in.
    fn frag(g: &mut Game, killer: u16, victim: u16) {
        let ship = |g: &Game, pid: u16| g.players[&pid].ship.unwrap();
        let (k, v) = (ship(g, killer), ship(g, victim));
        g.kill(v, Some(k), Fate::Wrecked);
        g.spawn_player(victim);
    }

    fn end_round(g: &mut Game) {
        g.round_end = g.tick;
        g.tick();
        assert_eq!(g.round, Round::Over);
    }

    fn announces(rx: &mut UnboundedReceiver<Outgoing>) -> Vec<(String, String)> {
        let mut v = Vec::new();
        while let Ok(o) = rx.try_recv() {
            if let Outgoing::Reliable(b) = o
                && let Some(ServerMsg::Announce { title, sub }) = decode::<ServerMsg>(&b)
            {
                v.push((title, sub));
            }
        }
        v
    }

    #[test]
    fn a_round_is_ranked_by_kills_and_the_break_counts_for_nothing() {
        let (mut g, mut rxs) = room(&["甲", "乙", "丙"]);
        // 乙 takes out 甲 twice and is taken out once; 丙 has one kill and no deaths
        frag(&mut g, 2, 1);
        frag(&mut g, 2, 1);
        frag(&mut g, 3, 2);
        let order: Vec<(String, u32, u32)> = g.scores().into_iter().map(|r| (r.name, r.kills, r.deaths)).collect();
        assert_eq!(order, [("乙".to_string(), 2, 1), ("丙".to_string(), 1, 0), ("甲".to_string(), 0, 2)], "kills first, not points");
        end_round(&mut g);
        let said = announces(&mut rxs[0]);
        assert!(said.iter().any(|(t, s)| t == "乙 获胜" && s.contains("击毁 2")), "{said:?}");
        // between rounds nothing counts
        frag(&mut g, 1, 2);
        assert_eq!(g.scores()[0].kills, 2);
        assert_eq!(g.players[&1].kills, 0);
        // the next round starts from nothing
        g.round_end = g.tick;
        g.tick();
        assert_eq!(g.round, Round::Running);
        assert!(g.scores().iter().all(|r| r.kills == 0 && r.deaths == 0));
    }

    #[test]
    fn bots_find_their_way_round_the_cage() {
        for seed in [1, 5, 8] {
            // a room of six bots, someone watching so that they keep coming
            let mut g = Game::with_seed(GameConfig { mode: Mode::Ffa, bots: 6, budget: None, verbose: false }, seed);
            let (out, _rx) = unbounded_channel();
            g.join(1, "观众".into(), None, None, out);
            let (mut alive, mut stuck) = (BTreeMap::<u16, f32>::new(), BTreeMap::<u16, f32>::new());
            let mut movers = BTreeMap::<u16, usize>::new();
            let mut fell = std::collections::BTreeSet::new();
            for _ in 0..secs(120.0) {
                g.tick();
                for s in g.ships.values() {
                    // only while it has all its wheels: one shot to pieces is stuck for a reason
                    if *movers.entry(s.id).or_insert(s.body.movers.len()) > s.body.movers.len() {
                        continue;
                    }
                    *alive.entry(s.id).or_default() += DT;
                    if s.kin.vel.length() < 1.0 {
                        *stuck.entry(s.id).or_default() += DT;
                    }
                    if s.kin.pos.y < -1.0 {
                        fell.insert(s.id);
                    }
                }
            }
            let kills: u32 = g.seats.iter().map(|s| s.kills).sum();
            let deaths: u32 = g.seats.iter().map(|s| s.deaths).sum();
            let worst = alive.iter().filter(|(_, t)| **t > 15.0).map(|(id, t)| stuck.get(id).copied().unwrap_or(0.0) / t).fold(0.0f32, f32::max);
            println!("seed {seed}: {} vehicles, {kills} kills, {deaths} losses, {} into a pit; the most stuck one sat still {:.0}% of its life", alive.len(), fell.len(), worst * 100.0);
            assert!(kills >= 4, "they should find each other");
            assert!(worst < 0.3, "no one should sit stuck");
            assert!(fell.len() * 3 <= deaths as usize + 1, "pits shouldn't take most of them");
        }
    }

    /// The cage with no one in it, for setting vehicles down where a test wants them.
    fn bare_cage() -> Game {
        Game::with_map(GameConfig { mode: Mode::Ffa, bots: 0, budget: None, verbose: false }, 1, Map::Cage)
    }

    fn put(g: &mut Game, d: Design, team: u16, at: Vec3, facing: Vec3) -> u16 {
        g.add_ship(Arc::new(d), format!("{team}"), team, None, at, look_rotation(at, at + facing, Vec3::Y), None)
    }

    /// The rover with a row of `t` across its nose.
    fn nosed(t: BlockType) -> Design {
        let mut d = preset("rover");
        for x in -1..=1 {
            d.blocks.push(blocktread_shared::blocks::Block::new(x, 0, -6, t, None));
        }
        d
    }

    fn lost(g: &Game, id: u16) -> f32 {
        g.ships.get(&id).map_or(f32::INFINITY, |s| s.body.max_hp - s.body.hp_total())
    }

    /// Two vehicles driven head on into each other: what each lost.
    fn head_on(a: Design, b: Design) -> (f32, f32) {
        let mut g = bare_cage();
        let (pa, pb) = (Vec3::new(-96.0, 0.0, -25.0), Vec3::new(-74.0, 0.0, -25.0));
        let ia = put(&mut g, a, 1, pa, Vec3::X);
        let ib = put(&mut g, b, 2, pb, Vec3::NEG_X);
        for _ in 0..secs(2.5) {
            for id in [ia, ib] {
                if let Some(s) = g.ships.get_mut(&id) {
                    s.input.drive = 127;
                }
            }
            g.tick();
        }
        (lost(&g, ia), lost(&g, ib))
    }

    #[test]
    fn a_ram_hits_hard_and_takes_little() {
        // against the same weight of armour on the nose
        let (armour_took, armour_dealt) = head_on(nosed(BlockType::Armor), preset("rover"));
        let (ram_took, ram_dealt) = head_on(nosed(BlockType::Ram), preset("rover"));
        println!("armoured nose: took {armour_took:.0}, dealt {armour_dealt:.0}; ram: took {ram_took:.0}, dealt {ram_dealt:.0}");
        assert!(armour_dealt > 10.0, "a crash should hurt");
        assert!(ram_dealt > armour_dealt * 2.0, "the ram should deal far more");
        assert!(ram_took < armour_took * 0.6, "and take less");
    }

    #[test]
    fn a_flipper_throws_what_is_in_front_of_it() {
        let mut g = bare_cage();
        let at = Vec3::new(-90.0, 0.0, -25.0);
        let ia = put(&mut g, nosed(BlockType::Flipper), 1, at, Vec3::X);
        // the other's tail a metre off the flipper's lip
        let ib = put(&mut g, preset("rover"), 2, at + Vec3::X * 12.5, Vec3::X);
        for _ in 0..secs(0.5) {
            g.tick();
        }
        let gap = g.ships[&ib].kin.pos.x - g.ships[&ia].kin.pos.x;
        g.ships.get_mut(&ia).unwrap().input.buttons = blocktread_shared::flight::BTN_FLIPPER;
        g.tick();
        let b = &g.ships[&ib];
        println!("{gap:.1} m apart: thrown at {:.1?}", b.kin.vel);
        assert!(b.kin.vel.y > 3.0 && b.kin.vel.x > 2.0, "it should go up and away");
        assert!(lost(&g, ib) > 1.0 && b.last_attacker == Some(ia));
        // and the next press does nothing for a while
        let vy = g.ships[&ib].kin.vel.y;
        g.tick();
        assert!(g.ships[&ib].kin.vel.y < vy, "only gravity now");
    }

    #[test]
    fn a_saw_cuts_what_is_just_ahead_of_it() {
        let mut g = bare_cage();
        let at = Vec3::new(-90.0, 0.0, -25.0);
        let ia = put(&mut g, nosed(BlockType::Saw), 1, at, Vec3::X);
        // the other's tail a metre off the blade
        let ib = put(&mut g, preset("rover"), 2, at + Vec3::X * 11.5, Vec3::X);
        let mut shown = false;
        for _ in 0..secs(1.0) {
            g.tick();
            shown |= g.ships[&ia].flags() & FLAG_CUT != 0;
        }
        println!("cut {:.0} in a second", lost(&g, ib));
        assert!(lost(&g, ib) > 40.0 && g.ships[&ib].last_attacker == Some(ia));
        assert!(shown, "others should see it cutting");
        // nothing for one out of reach
        let ic = put(&mut g, preset("rover"), 3, at + Vec3::new(0.0, 0.0, 20.0), Vec3::NEG_X);
        let id = put(&mut g, preset("rover"), 4, at + Vec3::new(11.8, 0.0, 30.0), Vec3::NEG_X);
        for _ in 0..secs(1.0) {
            g.tick();
        }
        assert_eq!((lost(&g, ic), lost(&g, id)), (0.0, 0.0));
    }

    #[test]
    fn a_vent_burns_only_while_it_breathes_fire() {
        let mut g = bare_cage();
        let v = g.terrain.arena.as_ref().unwrap().vents[0];
        let id = put(&mut g, preset("rover"), 1, v.area.centre(), Vec3::X);
        let mut by = Vec::new();
        for _ in 0..secs(6.0) {
            g.tick();
            by.push((tick_time(g.tick), lost(&g, id)));
        }
        let at = |t: f64| by.iter().find(|b| b.0 >= t).unwrap().1;
        println!("burnt {:.0} by 2 s, {:.0} by 5.9 s", at(2.0), at(5.9));
        assert!(at(2.0) > 60.0, "it should burn while the vent is on");
        assert!(at(5.9) - at(2.3) < 1.0, "and not while it is off");
    }

    /// The index and place of the cage's first pad of this kind.
    fn pad_of(g: &Game, kind: PadKind) -> (usize, Vec3) {
        let pads = &g.terrain.arena.as_ref().unwrap().pads;
        pads.iter().enumerate().find(|(_, p)| p.kind == kind).map(|(i, p)| (i, p.at)).unwrap()
    }

    #[test]
    fn a_pad_goes_to_who_needs_it_and_comes_back() {
        let mut g = bare_cage();
        let (i, at) = pad_of(&g, PadKind::Repair);
        let id = put(&mut g, preset("rover"), 1, at, Vec3::X);
        for _ in 0..10 {
            g.tick();
        }
        assert!(g.pads[i] <= g.tick, "nothing to mend: it stays up");
        g.ships.get_mut(&id).unwrap().hit_block(3, 25.0, DamageKind::Kinetic, None);
        g.tick();
        assert_eq!(lost(&g, id), 0.0, "mended");
        let back = g.pads[i];
        assert!(back > g.tick);
        // hurt again, nothing until it is back up
        g.ships.get_mut(&id).unwrap().hit_block(3, 25.0, DamageKind::Kinetic, None);
        while g.tick + 1 < back {
            g.tick();
            assert!(lost(&g, id) > 0.0);
        }
        g.tick();
        assert_eq!(lost(&g, id), 0.0, "and again once it is back");
    }

    #[test]
    fn overdrive_hits_harder_for_a_while() {
        let mut g = bare_cage();
        let (_, at) = pad_of(&g, PadKind::Overdrive);
        let id = put(&mut g, preset("rover"), 1, at, Vec3::X);
        g.tick();
        assert_eq!(g.ships[&id].hits_mul(), OVERDRIVE_MUL);
        for _ in 0..=secs(OVERDRIVE_SECS) {
            g.tick();
        }
        assert_eq!(g.ships[&id].hits_mul(), 1.0);
    }

    /// The cage set up for 2v2, no one in it yet.
    fn hill_cage() -> Game {
        Game::with_map(GameConfig { mode: Mode::Teams, bots: 0, budget: None, verbose: false }, 1, Map::Cage)
    }

    #[test]
    fn the_hill_scores_for_a_side_alone_on_it() {
        let mut g = hill_cage();
        let top = Vec3::Y * HILL_TOP;
        let a = put(&mut g, preset("rover"), 1, top + Vec3::X * 4.0, Vec3::X);
        for _ in 0..secs(3.2) {
            g.tick();
        }
        assert_eq!(g.points().get(&1), Some(&3), "a point a second");
        // the other side comes up: neither scores
        let b = put(&mut g, preset("rover"), 2, top - Vec3::X * 4.0, Vec3::NEG_X);
        for _ in 0..secs(2.0) {
            g.tick();
        }
        assert_eq!((g.points().get(&1), g.points().get(&2)), (Some(&3), None));
        assert!(g.hill.as_ref().unwrap().contested);
        // a kill is worth five, and the hill is theirs again
        g.kill(b, Some(a), Fate::Wrecked);
        g.tick();
        assert_eq!(g.points()[&1], 3 + HILL_KILL);
        assert_eq!(g.hill.as_ref().unwrap().holder, Some(1));
    }

    #[test]
    fn bots_fight_over_the_hill() {
        // three matches of a minute and a half: one alone can go either way
        let (mut held, mut contested) = (0.0, 0.0);
        // seconds of vehicles up there, by side
        let mut up: BTreeMap<u16, f32> = BTreeMap::new();
        for seed in 1..=3 {
            let mut g = Game::with_map(GameConfig { mode: Mode::Teams, bots: 0, budget: None, verbose: false }, seed, Map::Cage);
            let (out, _rx) = unbounded_channel();
            g.join(1, "观众".into(), None, None, out);
            for (i, ship) in ["bison", "scorpion", "rover", "bison"].iter().enumerate() {
                g.add_seat(crate::new_pid(), format!("AI{i}"), preset(ship), Some(1 + i as u16 % 2));
            }
            for _ in 0..secs(90.0) {
                g.tick();
                for s in g.ships.values().filter(|s| g.terrain.arena.as_ref().is_some_and(|a| a.on_hill(s.kin.pos))) {
                    *up.entry(s.team).or_insert(0.0) += DT;
                }
                let h = g.hill.as_ref().unwrap();
                if h.holder.is_some() {
                    held += DT;
                }
                if h.contested {
                    contested += DT;
                }
            }
            println!("seed {seed}: points {:?}", g.points());
        }
        println!("held {held:.0} s, contested {contested:.0} s; time up there by team {up:?}");
        assert!(held + contested > 90.0, "they should get up there");
        assert!(up.len() == 2 && up.values().all(|&t| t > 30.0), "both sides should");
        assert!(contested > 15.0, "and fight over it");
    }

    #[test]
    fn a_mine_waits_for_an_enemy_and_goes_off_under_it() {
        let mut g = bare_cage();
        let ground = g.terrain.clone();
        let on_floor = |x: f32, z: f32| Vec3::new(x, ground.surface(x, z) + 0.1, z);
        let owner = put(&mut g, preset("rhino"), 1, Vec3::new(-100.0, 0.0, -45.0), Vec3::X);
        // a friend drives over a live one
        g.lay_mine(owner, 1, on_floor(-85.0, -25.0));
        let friend = put(&mut g, preset("rover"), 1, Vec3::new(-96.0, 0.0, -25.0), Vec3::X);
        for _ in 0..secs(2.5) {
            g.ships.get_mut(&friend).unwrap().input.drive = 127;
            g.tick();
        }
        assert!(g.ships[&friend].kin.pos.x > -80.0, "it should have driven over it");
        assert_eq!((g.mines.len(), lost(&g, friend)), (1, 0.0), "a mine is no danger to its own side");
        g.ships.remove(&friend);
        // one laid under an enemy waits until it is live, then goes off
        let enemy = put(&mut g, preset("rover"), 2, Vec3::new(-60.0, 0.0, -25.0), Vec3::X);
        for _ in 0..secs(1.0) {
            g.tick();
        }
        g.lay_mine(owner, 1, on_floor(-60.0, -25.0));
        for _ in 0..secs(0.5) {
            g.tick();
        }
        assert_eq!(g.mines.len(), 2, "not live yet");
        for _ in 0..secs(0.7) {
            g.tick();
        }
        assert_eq!(g.mines.len(), 1, "it should have gone off");
        let e = &g.ships[&enemy];
        println!("the mine took {:.0} hit points off the rover", lost(&g, enemy));
        assert!(lost(&g, enemy) > 40.0, "and hurt it");
        assert_eq!(e.last_attacker, Some(owner), "the kill would be the layer's");
        // no more than three down: the fourth takes the place of the oldest
        let first = g.mines[0].id;
        for x in [-30.0, -20.0, -10.0] {
            g.lay_mine(owner, 1, on_floor(x, -60.0));
        }
        assert_eq!(g.mines.len(), 3);
        assert!(g.mines.iter().all(|m| m.id != first));
    }

    /// A missile `target` is after, from off to one side.
    fn missile_at(g: &mut Game, owner: u16, target: u16) {
        let pos = g.ships[&target].kin.pos + Vec3::new(-40.0, 3.0, -18.0);
        g.bolts.push(Bolt { id: 900, pos, prev: pos, vel: Vec3::X * 120.0, team: g.ships[&owner].team, owner, weapon: BlockType::Missile, dmg: 30.0, life: 5.0, rewind: 0, target: Some(target), speed: 120.0 });
    }

    #[test]
    fn smoke_makes_a_missile_lose_its_target() {
        let hit = |smoke: bool| {
            let mut g = bare_cage();
            let target = put(&mut g, preset("rover"), 2, Vec3::new(-60.0, 0.0, -25.0), Vec3::X);
            let shooter = put(&mut g, preset("scorpion"), 1, Vec3::new(-100.0, 0.0, -60.0), Vec3::X);
            if smoke {
                g.clouds.push(Cloud { pos: g.ships[&target].kin.pos, radius: 7.0, until: g.tick + secs(8.0) });
            }
            missile_at(&mut g, shooter, target);
            for _ in 0..secs(1.5) {
                g.tick();
            }
            lost(&g, target)
        };
        let (clear, smoked) = (hit(false), hit(true));
        println!("lost {clear:.0} to a missile it couldn't hide from, {smoked:.0} in smoke");
        assert!(clear > 10.0, "the missile should find it");
        assert_eq!(smoked, 0.0, "it shouldn't in smoke");
    }

    #[test]
    fn a_bot_with_a_missile_after_it_lets_off_smoke_and_then_cant_be_seen() {
        let mut g = bare_cage();
        let at = Vec3::new(-60.0, 0.0, -25.0);
        let bot = g.add_ship(Arc::new(preset("scorpion")), "bot".into(), 2, None, at, Quat::IDENTITY, Some(Skill::ffa_bot()));
        let foe = put(&mut g, preset("rover"), 1, Vec3::new(-100.0, 0.0, -60.0), Vec3::X);
        missile_at(&mut g, foe, bot);
        for _ in 0..3 {
            g.tick();
        }
        assert_eq!(g.clouds.len(), 1, "it should let off smoke");
        assert!(g.bolts.iter().all(|b| b.target.is_none()), "and the missile lose it");
        // a bot in the open sees it; one outside the cloud looking in doesn't
        let watcher = Vec3::new(-100.0, 1.5, -25.0);
        assert!(hidden(&g.clouds, watcher, g.ships[&bot].kin.pos, g.tick));
        assert!(!hidden(&g.clouds, watcher, g.ships[&foe].kin.pos, g.tick));
    }

    #[test]
    fn level_on_kills_and_deaths_is_a_draw() {
        let (mut g, mut rxs) = room(&["甲", "乙"]);
        frag(&mut g, 1, 2);
        frag(&mut g, 2, 1);
        end_round(&mut g);
        let said = announces(&mut rxs[1]);
        assert!(said.iter().any(|(t, s)| t == "平局" && s.contains("甲、乙 并列第一")), "{said:?}");
    }
}
