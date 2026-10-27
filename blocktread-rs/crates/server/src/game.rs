use crate::Outgoing;
use bytes::Bytes;
use glam::{Quat, Vec3};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use blocktread_shared::ai::{AiPilot, Contact, Skill};
use blocktread_shared::analysis::analyze;
use blocktread_shared::balance::balance;
use blocktread_shared::blocks::{BlockType, DamageKind, Design};
use blocktread_shared::drive::{spawn_height, step_drive};
use blocktread_shared::flight::{InputCmd, Kin, NO_LOCK, collide_asteroids, look_rotation};
use blocktread_shared::presets::{PRESETS, enemy_colors, preset};
use blocktread_shared::protocol::*;
use blocktread_shared::rng::Rng;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::weapons::{Shot, WeaponState, step_weapons};
use blocktread_shared::terrain::Terrain;
use blocktread_shared::world::{Asteroid, PLAYER_SPAWN, generate_boulders, tick_time};
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
    order: u32,
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
    collide_cd: f32,
    recent_dmg: f32,
    disabled_t: Option<f32>,
    dmg_mul: f32,
    destroyed: Vec<u16>,
    damaged: Vec<(u16, u8)>,
    killed: bool,
    is_bot: bool,
    /// Free-for-all bot seat this ship flies for.
    seat: Option<usize>,
}

/// A bot identity. Each respawn flies the same ship for the same seat, so its score carries over.
struct BotSeat {
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
    StartWave,
    RepairPlayers,
    Restart,
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
    rocks: Vec<Asteroid>,
    seed: u64,
    wave: u32,
    between: bool,
    wiped: bool,
    timers: Vec<(u32, Timer)>,
    pending_bots: u32,
    seats: Vec<BotSeat>,
    events: Vec<ServerMsg>,
    scores_dirty: bool,
    joins: u32,
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
}

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

    /// A game whose asteroids and randomness come from `seed` (for reproducible simulations).
    pub fn with_seed(cfg: GameConfig, seed: u64) -> Self {
        let terrain = Arc::new(Terrain::generate(seed));
        Game {
            cfg,
            tick: 0,
            rng: Rng::new(seed ^ 0xA5A5),
            players: BTreeMap::new(),
            ships: BTreeMap::new(),
            next_ship: 1,
            bolts: Vec::new(),
            next_bolt: 1,
            rocks: generate_boulders(seed, &terrain),
            terrain,
            seed,
            wave: 0,
            between: true,
            wiped: false,
            timers: Vec::new(),
            pending_bots: 0,
            seats: Vec::new(),
            events: Vec::new(),
            scores_dirty: true,
            joins: 0,
            shots: Vec::new(),
            team_kills: BTreeMap::new(),
            limit: 0,
            ends_at: None,
            rounds: None,
            round: Round::None,
            round_end: 0,
        }
    }

    pub fn now(&self) -> u32 {
        self.tick
    }

    pub fn team_kills(&self) -> &BTreeMap<u16, u32> {
        &self.team_kills
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
        self.joins += 1;
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
            order: self.joins,
        };
        p.send(&ServerMsg::Welcome { pid, tick: self.tick, mode: self.cfg.mode, asteroid_seed: self.seed });
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
            Mode::Coop => {
                if !self.wiped {
                    self.spawn_player(pid);
                }
                if self.wave == 0 && self.between && !self.timers.iter().any(|t| matches!(t.1, Timer::StartWave)) {
                    self.announce("准备战斗", "第一波敌车即将抵达".into());
                    self.after(3.0, Timer::StartWave);
                } else if !self.wiped {
                    let sub = format!("第 {} 波进行中", self.wave);
                    if let Some(p) = self.players.get(&pid) {
                        p.send(&ServerMsg::Announce { title: "合作波次".into(), sub });
                    }
                }
            }
            Mode::Ffa | Mode::Teams => {
                self.spawn_player(pid);
                let sub = match (self.cfg.mode, self.limit) {
                    _ if self.rounds.is_some() => self.room_status(),
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
            collide_cd: 0.0,
            recent_dmg: 0.0,
            disabled_t: None,
            dmg_mul,
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

    /// A bot that flies `design` for a match, on `team`; it takes off at once and respawns.
    pub fn add_seat(&mut self, name: String, design: Design, team: Option<u16>) {
        self.seats.push(BotSeat { name, design: Arc::new(enemy_colors(&design)), team, ship: None, kills: 0, deaths: 0, score: 0 });
        self.cfg.bots += 1;
        self.spawn_seat(self.seats.len() - 1);
    }

    fn spawn_seat(&mut self, seat: usize) {
        let s = &self.seats[seat];
        let team = s.team.unwrap_or(2000 + self.next_ship);
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
        let design = Arc::new(enemy_colors(&preset(PRESETS[k].key)));
        self.seats.push(BotSeat { name: format!("AI·{}", PRESETS[k].name), design, team: None, ship: None, kills: 0, deaths: 0, score: 0 });
        self.seats.len() - 1
    }

    fn spawn_player(&mut self, pid: u16) {
        let Some(p) = self.players.get(&pid) else { return };
        let Some(design) = p.design.clone() else { return };
        if p.ship.is_some() {
            return;
        }
        let (name, order) = (p.name.clone(), p.order);
        let team = p.team.unwrap_or(match self.cfg.mode {
            Mode::Coop => 0,
            Mode::Ffa | Mode::Teams | Mode::Solo => 1000 + pid,
        });
        let (pos, rot) = match self.cfg.mode {
            Mode::Coop => {
                let side = (order as f32 - 1.0) * 28.0 * if order % 2 == 0 { 1.0 } else { -1.0 };
                (PLAYER_SPAWN + Vec3::new(side, (order % 3) as f32 * 8.0, 0.0), Quat::IDENTITY)
            }
            Mode::Ffa | Mode::Teams => self.spawn_point(team),
            // the test range: always the same spot, facing the middle of the map
            Mode::Solo => (PLAYER_SPAWN, Quat::IDENTITY),
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
        (PLAYER_SPAWN, Quat::IDENTITY)
    }

    /// No rock within `gap` of (x, z).
    fn clear_ground(&self, pos: Vec3, gap: f32) -> bool {
        self.rocks.iter().all(|a| a.pos.with_y(0.0).distance(pos.with_y(0.0)) > a.r + gap)
    }

    fn start_wave(&mut self) {
        if !self.players.values().any(|p| p.ship.is_some()) {
            return;
        }
        self.wave += 1;
        let w = self.wave;
        let mut budget = 1.5 + w as f32 * 1.3 + (self.pilots().saturating_sub(1)) as f32 * 1.5;
        let mut picks: Vec<usize> = Vec::new();
        let cap = (w as usize + self.pilots().max(1)).min(10);
        while picks.len() < cap {
            let opts: Vec<usize> = (0..PRESETS.len()).filter(|&i| PRESETS[i].cost <= budget && (PRESETS[i].key != "rhino" || w >= 3)).collect();
            if opts.is_empty() {
                break;
            }
            let i = opts[self.rng.below(opts.len())];
            budget -= PRESETS[i].cost;
            picks.push(i);
        }
        if picks.is_empty() {
            picks.push(1);
        }
        let centre = self.player_centre();
        let fwd = Vec3::NEG_Z;
        let side = Vec3::X;
        let n = picks.len();
        for (k, &i) in picks.iter().enumerate() {
            let off = k as f32 - (n as f32 - 1.0) / 2.0;
            let mut pos = centre + fwd * self.rng.range(650.0, 850.0) + side * (off * 60.0 + self.rng.range(-20.0, 20.0)) + Vec3::Y * self.rng.range(-60.0, 60.0);
            if pos.length() > blocktread_shared::ARENA_R * 0.9 {
                pos = pos.normalize() * blocktread_shared::ARENA_R * 0.85;
            }
            let rot = look_rotation(pos, centre, Vec3::Y);
            let d = Arc::new(enemy_colors(&preset(PRESETS[i].key)));
            self.add_ship(d, format!("敌方·{}", PRESETS[i].name), 1, None, pos, rot, Some(Skill::for_wave(w)));
        }
        self.between = false;
        self.announce(&format!("第 {w} 波"), format!("{n} 辆敌车来袭"));
        self.scores_dirty = true;
    }

    fn player_centre(&self) -> Vec3 {
        let ps: Vec<Vec3> = self.players.values().filter_map(|p| p.ship).filter_map(|id| self.ships.get(&id)).map(|s| s.kin.pos).collect();
        if ps.is_empty() { PLAYER_SPAWN } else { ps.iter().copied().sum::<Vec3>() / ps.len() as f32 }
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
                Timer::StartWave => self.start_wave(),
                Timer::RepairPlayers => {
                    self.repair_players();
                    self.feed("战车已完全修复".into(), false);
                }
                Timer::Restart => {
                    self.wiped = false;
                    self.wave = 0;
                    let pids: Vec<u16> = self.players.keys().copied().collect();
                    for pid in pids {
                        self.spawn_player(pid);
                    }
                    self.announce("重新开始", "第一波敌车即将抵达".into());
                    self.after(3.0, Timer::StartWave);
                }
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

        // bots
        let contacts: Vec<Contact> = self.ships.values().filter(|s| !s.killed).map(|s| s.contact()).collect();
        for s in self.ships.values_mut() {
            if let Some(ai) = s.ai.as_mut() {
                let others: Vec<Contact> = contacts.iter().filter(|c| c.id != s.id).copied().collect();
                let mut cmd = ai.think(&s.kin, &s.body, s.recent_dmg, s.team, &others, &self.rocks, DT);
                cmd.tick = t;
                s.input = cmd;
            }
        }

        // flight + weapons
        let mut crashes = Vec::new();
        for s in self.ships.values_mut() {
            step_drive(&mut s.kin, &s.body, &s.input, &self.terrain, DT);
            if let Some(imp) = collide_asteroids(&mut s.kin, &s.body, &self.rocks, tick_time(t))
                && s.collide_cd <= 0.0
            {
                s.collide_cd = 0.3;
                let (pos, rot) = (s.kin.pos, s.kin.rot);
                s.splash(pos, rot, imp.point, 1.7, imp.speed * 0.9, DamageKind::Kinetic, None);
                crashes.push(imp.point);
            }
            s.collide_cd -= DT;
            s.recent_dmg *= (-1.5 * DT).exp();
            self.shots.clear();
            step_weapons(&mut s.kin, &s.body, &mut s.ws, &s.input, DT, t, false, &mut self.shots);
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
                    dmg: spec.dmg * s.dmg_mul,
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

        self.ship_collisions();
        self.update_bolts(t);
        self.settle_ships();

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
            self.kill(id, by);
        }

        match self.cfg.mode {
            Mode::Coop => self.coop_logic(),
            Mode::Ffa | Mode::Teams | Mode::Solo => self.ffa_logic(),
        }
        self.flush(t);
    }

    fn ship_collisions(&mut self) {
        let ids: Vec<u16> = self.ships.keys().copied().collect();
        for a in 0..ids.len() {
            for b in a + 1..ids.len() {
                let (ia, ib) = (ids[a], ids[b]);
                let (sa, sb) = {
                    let sa = &self.ships[&ia];
                    let sb = &self.ships[&ib];
                    if sa.kin.pos.distance(sb.kin.pos) > sa.body.radius + sb.body.radius {
                        continue;
                    }
                    (sa, sb)
                };
                let mut contact = None;
                for i in 0..sa.body.alive.len() {
                    if !sa.body.alive[i] {
                        continue;
                    }
                    let w = sa.body.world(sa.kin.pos, sa.kin.rot, i);
                    let g = sb.body.to_grid(sb.kin.pos, sb.kin.rot, w).round().as_ivec3();
                    if sb.body.block_at(g).is_some() {
                        contact = Some(w);
                        break;
                    }
                }
                let Some(cp) = contact else { continue };
                let n = (sa.kin.pos - sb.kin.pos).normalize_or(Vec3::Y);
                let (ma, mb) = (sa.body.stats.mass.max(1.0), sb.body.stats.mass.max(1.0));
                let rel = (sa.kin.vel - sb.kin.vel).dot(n);
                let cd_ready = sa.collide_cd <= 0.0 || sb.collide_cd <= 0.0;
                let (team_a, team_b) = (sa.team, sb.team);
                {
                    let sa = self.ships.get_mut(&ia).unwrap();
                    sa.kin.pos += n * 0.8 * mb / (ma + mb);
                }
                {
                    let sb = self.ships.get_mut(&ib).unwrap();
                    sb.kin.pos -= n * 0.8 * ma / (ma + mb);
                }
                if rel < 0.0 {
                    let j = -1.4 * rel / (1.0 / ma + 1.0 / mb);
                    self.ships.get_mut(&ia).unwrap().kin.vel += n * (j / ma);
                    self.ships.get_mut(&ib).unwrap().kin.vel -= n * (j / mb);
                    if cd_ready {
                        let dmg = (-rel * 1.2).min(90.0);
                        let friendly = team_a == team_b;
                        let f = if friendly { 0.3 } else { 1.0 };
                        for (id, other, share) in [(ia, ib, mb / (ma + mb)), (ib, ia, ma / (ma + mb))] {
                            let s = self.ships.get_mut(&id).unwrap();
                            s.collide_cd = 0.35;
                            let (pos, rot) = (s.kin.pos, s.kin.rot);
                            s.splash(pos, rot, cp, 1.9, dmg * 2.0 * share * f, DamageKind::Kinetic, Some(other));
                        }
                        self.events.push(ServerMsg::Crash { pos: cp });
                    }
                }
            }
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
                if let Some(tid) = b.target
                    && let Some(tg) = self.ships.get(&tid)
                {
                    let des = (tg.kin.pos - b.pos).normalize_or(cur);
                    let ang = cur.angle_between(des);
                    if ang > 1e-4 {
                        dir = cur.lerp(des, (3.4 * DT / ang).min(1.0)).normalize_or(cur);
                    }
                }
                b.speed = (b.speed + 200.0 * DT).min(380.0);
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
            if !dead && b.pos.y < self.terrain.height(b.pos.x, b.pos.z) {
                // into the ground
                if spec.splash > 0.0 {
                    self.events.push(ServerMsg::Explosion { pos: b.pos, size: 0.7 });
                }
                dead = true;
            }
            if !dead {
                for a in &self.rocks {
                    if let Some(tt) = a.hit_segment(b.prev, b.pos, a.rotation(tick_time(t))) {
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
            self.kill(id, by);
        }
    }

    fn kill(&mut self, id: u16, by: Option<u16>) {
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
            if matches!(self.cfg.mode, Mode::Ffa | Mode::Teams | Mode::Solo) {
                p.respawn_at = Some(self.tick + secs(4.0));
            }
        }
        let text = match killer_name {
            Some(k) => format!("{k} 击毁了 {}", victim.name),
            None => format!("{} 报废了", victim.name),
        };
        self.feed(text, victim.owner.is_some());
        self.scores_dirty = true;
    }

    fn coop_logic(&mut self) {
        // spectators alone don't keep a co-op game going: the waves only fight pilots
        if self.pilots() == 0 {
            if self.wave > 0 || !self.ships.is_empty() {
                let ids: Vec<u16> = self.ships.keys().copied().collect();
                for id in ids {
                    self.ships.remove(&id);
                    self.events.push(ServerMsg::ShipRemoved { id });
                }
                self.bolts.clear();
                self.timers.clear();
                self.wave = 0;
                self.between = true;
                self.wiped = false;
            }
            return;
        }
        let players_alive = self.players.values().any(|p| p.ship.is_some());
        if !players_alive && !self.wiped && self.wave > 0 {
            self.wiped = true;
            self.between = true;
            self.timers.retain(|t| !matches!(t.1, Timer::StartWave | Timer::RepairPlayers));
            let w = self.wave;
            self.announce("全军覆没", format!("坚持到第 {w} 波，6 秒后重新开始"));
            let ids: Vec<u16> = self.ships.keys().copied().collect();
            for id in ids {
                self.ships.remove(&id);
                self.events.push(ServerMsg::ShipRemoved { id });
            }
            self.bolts.clear();
            self.after(6.0, Timer::Restart);
            return;
        }
        if self.between || self.wiped {
            return;
        }
        if self.ships.values().any(|s| s.team != 0) {
            return;
        }
        self.between = true;
        let bonus = 200 * self.wave;
        for p in self.players.values_mut() {
            if p.ship.is_some() {
                p.score += bonus;
            }
        }
        self.scores_dirty = true;
        self.announce("波次清除", format!("奖励 +{bonus} · 正在修复战车"));
        self.after(2.2, Timer::RepairPlayers);
        self.after(5.5, Timer::StartWave);
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
            let delay = if self.tick < secs(5.0) || bots == 0 { 1.5 } else { 5.0 };
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
        self.round_end = self.tick + secs(len);
        self.scores_dirty = true;
        self.announce("开战", format!("{} 人混战 · {} 内击毁最多的人获胜", self.pilots(), span(len)));
    }

    fn end_round(&mut self) {
        self.round = Round::Over;
        self.round_end = self.tick + secs(ROUND_BREAK_SECS);
        self.scores_dirty = true;
        let rows = self.scores();
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
        let mode = self.cfg.mode;
        let mut rows: Vec<ScoreRow> = self
            .players
            .iter()
            .filter(|(_, p)| !p.watching())
            .map(|(&pid, p)| {
                let team = p.team.unwrap_or(if mode == Mode::Coop { 0 } else { 1000 + pid });
                ScoreRow { pid, name: p.name.clone(), kills: p.kills, deaths: p.deaths, score: p.score, alive: p.ship.is_some(), team }
            })
            .chain(self.seats.iter().enumerate().map(|(i, s)| ScoreRow {
                pid: BOT_PID_BASE + i as u16,
                name: s.name.clone(),
                kills: s.kills,
                deaths: s.deaths,
                score: s.score,
                alive: s.ship.is_some(),
                team: s.team.unwrap_or(BOT_PID_BASE + i as u16),
            }))
            .collect();
        // co-op by points; the others by kills, then fewer deaths
        if mode == Mode::Coop {
            rows.sort_by(|a, b| b.score.cmp(&a.score).then(b.kills.cmp(&a.kills)));
        } else {
            rows.sort_by(|a, b| b.kills.cmp(&a.kills).then(a.deaths.cmp(&b.deaths)).then(b.score.cmp(&a.score)));
        }
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
            let enemies = self.ships.values().filter(|s| s.owner.is_none()).count() as u32;
            let end = match self.round {
                Round::Running | Round::Over => Some(self.round_end),
                Round::Waiting => None,
                Round::None => self.ends_at,
            };
            let left = end.map_or(0.0, |e| e.saturating_sub(self.tick) as f32 / TICK_RATE as f32);
            self.events.push(ServerMsg::Scores { rows, wave: self.wave, enemies, limit: self.limit, left, round: self.round });
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
                flags: (if s.input.has(blocktread_shared::flight::BTN_FIRE) { FLAG_FIRING } else { 0 })
                    | (if s.kin.boosting { FLAG_BOOST } else { 0 })
                    | (if s.body.stats.thrust > 0.0 && blocktread_shared::drive::thrust_push(&s.input, s.kin.boosting) > 0.0 { FLAG_THRUST } else { 0 }),
                throttle: (s.input.throttle_f() * 255.0) as u8,
                shield: if s.body.stats.shield_cap > 0.0 { (s.kin.shield / s.body.stats.shield_cap * 255.0) as u8 } else { 0 },
                aim: s.input.aim,
            })
            .collect();
        let missiles: Vec<MissileSnap> = self.bolts.iter().filter(|b| b.weapon == BlockType::Missile).map(|b| MissileSnap { id: b.id, pos: b.pos, vel: b.vel }).collect();
        for p in self.players.values() {
            let own = p.ship.and_then(|id| self.ships.get(&id).map(|s| OwnState { ship: id, kin: s.kin }));
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
/// other in the normal arena, asteroids and all, until one goes down or `max_secs` pass. Run it
/// many times with different seeds; one fight says little.
pub fn duel(a: &Design, b: &Design, seed: u64, max_secs: f32) -> Duel {
    let mut g = Game::with_seed(GameConfig { mode: Mode::Ffa, bots: 0, budget: None, verbose: false }, seed);
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
    ServerMsg::ShipSpawn { id: s.id, team: s.team, name: s.name.clone(), owner: s.owner, design: (*s.body.design).clone(), kin: s.kin }
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
        g.kill(v, Some(k));
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
    fn level_on_kills_and_deaths_is_a_draw() {
        let (mut g, mut rxs) = room(&["甲", "乙"]);
        frag(&mut g, 1, 2);
        frag(&mut g, 2, 1);
        end_round(&mut g);
        let said = announces(&mut rxs[1]);
        assert!(said.iter().any(|(t, s)| t == "平局" && s.contains("甲、乙 并列第一")), "{said:?}");
    }
}
