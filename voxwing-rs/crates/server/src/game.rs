use crate::{NetEvent, Outgoing};
use bytes::Bytes;
use crossbeam_channel::Receiver;
use glam::{Quat, Vec3};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use voxwing_shared::ai::{AiPilot, Contact, Skill};
use voxwing_shared::blocks::{BlockType, Design, PIERCE_FALLOFF};
use voxwing_shared::flight::{InputCmd, Kin, NO_LOCK, collide_asteroids, look_rotation, step_flight};
use voxwing_shared::presets::{PRESETS, enemy_colors, preset};
use voxwing_shared::protocol::*;
use voxwing_shared::rng::Rng;
use voxwing_shared::ship::ShipBody;
use voxwing_shared::weapons::{Shot, WeaponState, step_weapons};
use voxwing_shared::world::{Asteroid, PLAYER_SPAWN, generate_asteroids, tick_time};
use voxwing_shared::{DT, HISTORY_TICKS, SNAPSHOT_INTERVAL, TICK_RATE};

pub struct GameConfig {
    pub mode: Mode,
    pub bots: u32,
    pub verbose: bool,
}

const MAX_DATAGRAM: usize = 1100;
const PLAYER_TURN_BONUS: f32 = 1.12;
const MAX_REWIND_TICKS: u32 = 30;

struct Player {
    name: String,
    /// None for a spectator, who never gets a ship.
    design: Option<Arc<Design>>,
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

/// A free-for-all bot identity. Each respawn flies for the same seat, so its score carries over.
struct BotSeat {
    name: String,
    preset: usize,
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

    /// Apply damage to one block. Shields soak hits first; the caller handles shield checks.
    fn hit_block(&mut self, i: usize, dmg: f32, src: Option<u16>) {
        if !self.body.alive[i] {
            return;
        }
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

    fn splash(&mut self, pos: Vec3, rot: Quat, p: Vec3, r: f32, dmg: f32, src: Option<u16>) {
        for (i, d) in self.body.within(pos, rot, p, r) {
            self.hit_block(i, dmg * (1.0 - 0.65 * d / r), src);
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
}

fn secs(s: f32) -> u32 {
    (s * TICK_RATE as f32) as u32
}

fn seg_sphere(a: Vec3, b: Vec3, c: Vec3, r: f32) -> Option<f32> {
    let ab = b - a;
    let l2 = ab.length_squared();
    let t = if l2 > 0.0 { ((c - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    ((a + ab * t).distance_squared(c) <= r * r).then_some(t)
}

impl Game {
    pub fn new(cfg: GameConfig) -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
        Game {
            cfg,
            tick: 0,
            rng: Rng::new(seed ^ 0xA5A5),
            players: BTreeMap::new(),
            ships: BTreeMap::new(),
            next_ship: 1,
            bolts: Vec::new(),
            next_bolt: 1,
            rocks: generate_asteroids(seed),
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

    pub fn handle(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Join { pid, name, design, out } => self.join(pid, name, design, out),
            NetEvent::Input { pid, pkt } => {
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
            NetEvent::Leave { pid } => {
                if let Some(p) = self.players.remove(&pid) {
                    if let Some(sid) = p.ship {
                        self.ships.remove(&sid);
                        self.events.push(ServerMsg::ShipRemoved { id: sid });
                    }
                    self.feed(format!("{} {}", p.name, if p.watching() { "结束了观战" } else { "离开了战场" }), false);
                    self.scores_dirty = true;
                }
            }
        }
    }

    /// Players who fly (not spectators).
    fn pilots(&self) -> usize {
        self.players.values().filter(|p| !p.watching()).count()
    }

    fn join(&mut self, pid: u16, name: String, design: Option<Design>, out: tokio::sync::mpsc::UnboundedSender<Outgoing>) {
        let mut name: String = name.trim().chars().filter(|c| !c.is_control()).take(12).collect();
        if name.is_empty() {
            name = format!("飞行员{pid}");
        }
        self.joins += 1;
        let p = Player {
            name: name.clone(),
            design: design.map(|d| Arc::new(Design { name: d.sanitized_name(), blocks: d.blocks })),
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
                p.send(&ServerMsg::Announce { title: "观战".into(), sub: format!("{} · 附身到任意一架飞机上看", self.cfg.mode.label()) });
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
                    self.announce("准备战斗", "第一波敌机即将抵达".into());
                    self.after(3.0, Timer::StartWave);
                } else if !self.wiped {
                    let sub = format!("第 {} 波进行中", self.wave);
                    if let Some(p) = self.players.get(&pid) {
                        p.send(&ServerMsg::Announce { title: "合作波次".into(), sub });
                    }
                }
            }
            Mode::Ffa => {
                self.spawn_player(pid);
                if let Some(p) = self.players.get(&pid) {
                    p.send(&ServerMsg::Announce { title: "自由混战".into(), sub: "击落任何其他飞机".into() });
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
        let mut kin = Kin::new(pos, rot, &body.stats);
        kin.vel = rot * Vec3::NEG_Z * 40.0;
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
            input: InputCmd { throttle: 180, ..Default::default() },
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

    /// A bot seat with no ship in the air, creating one (with an unused preset if any) if needed.
    fn free_seat(&mut self) -> usize {
        let flying = |s: &BotSeat, i: usize| s.ship.and_then(|id| self.ships.get(&id)).is_some_and(|sh| sh.seat == Some(i));
        if let Some(i) = self.seats.iter().enumerate().position(|(i, s)| !flying(s, i)) {
            return i;
        }
        let free: Vec<usize> = (0..PRESETS.len()).filter(|p| !self.seats.iter().any(|s| s.preset == *p)).collect();
        let preset = if free.is_empty() { self.rng.below(PRESETS.len()) } else { free[self.rng.below(free.len())] };
        self.seats.push(BotSeat { name: format!("AI·{}", PRESETS[preset].name), preset, ship: None, kills: 0, deaths: 0, score: 0 });
        self.seats.len() - 1
    }

    fn spawn_player(&mut self, pid: u16) {
        let Some(p) = self.players.get(&pid) else { return };
        let Some(design) = p.design.clone() else { return };
        if p.ship.is_some() {
            return;
        }
        let (name, order) = (p.name.clone(), p.order);
        let (pos, rot) = match self.cfg.mode {
            Mode::Coop => {
                let side = (order as f32 - 1.0) * 28.0 * if order % 2 == 0 { 1.0 } else { -1.0 };
                (PLAYER_SPAWN + Vec3::new(side, (order % 3) as f32 * 8.0, 0.0), Quat::IDENTITY)
            }
            Mode::Ffa => self.ffa_spawn_point(),
        };
        let team = match self.cfg.mode {
            Mode::Coop => 0,
            Mode::Ffa => 1000 + pid,
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

    fn ffa_spawn_point(&mut self) -> (Vec3, Quat) {
        for _ in 0..30 {
            let mut d = self.rng.unit();
            d.y *= 0.4;
            let pos = d.normalize_or(Vec3::Z) * self.rng.range(450.0, 750.0);
            let clear = self.rocks.iter().all(|a| a.pos.distance(pos) > a.r + 40.0) && self.ships.values().all(|s| s.kin.pos.distance(pos) > 150.0);
            if clear {
                let rot = look_rotation(pos, Vec3::ZERO, Vec3::Y);
                return (pos, rot);
            }
        }
        (PLAYER_SPAWN, Quat::IDENTITY)
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
            if pos.length() > voxwing_shared::ARENA_R * 0.9 {
                pos = pos.normalize() * voxwing_shared::ARENA_R * 0.85;
            }
            let rot = look_rotation(pos, centre, Vec3::Y);
            let d = Arc::new(enemy_colors(&preset(PRESETS[i].key)));
            self.add_ship(d, format!("敌方·{}", PRESETS[i].name), 1, None, pos, rot, Some(Skill::for_wave(w)));
        }
        self.between = false;
        self.announce(&format!("第 {w} 波"), format!("{n} 架敌机来袭"));
        self.scores_dirty = true;
    }

    fn player_centre(&self) -> Vec3 {
        let ps: Vec<Vec3> = self.players.values().filter_map(|p| p.ship).filter_map(|id| self.ships.get(&id)).map(|s| s.kin.pos).collect();
        if ps.is_empty() { PLAYER_SPAWN } else { ps.iter().copied().sum::<Vec3>() / ps.len() as f32 }
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
                    self.feed("战机已完全修复".into(), false);
                }
                Timer::Restart => {
                    self.wiped = false;
                    self.wave = 0;
                    let pids: Vec<u16> = self.players.keys().copied().collect();
                    for pid in pids {
                        self.spawn_player(pid);
                    }
                    self.announce("重新开始", "第一波敌机即将抵达".into());
                    self.after(3.0, Timer::StartWave);
                }
                Timer::SpawnBot => {
                    self.pending_bots = self.pending_bots.saturating_sub(1);
                    let seat = self.free_seat();
                    let k = &PRESETS[self.seats[seat].preset];
                    let (pos, rot) = self.ffa_spawn_point();
                    let team = 2000 + self.next_ship;
                    let d = Arc::new(enemy_colors(&preset(k.key)));
                    let id = self.add_ship(d, self.seats[seat].name.clone(), team, None, pos, rot, Some(Skill::ffa_bot()));
                    if let Some(s) = self.ships.get_mut(&id) {
                        s.seat = Some(seat);
                    }
                    self.seats[seat].ship = Some(id);
                    self.scores_dirty = true;
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
            let bonus = if s.owner.is_some() { PLAYER_TURN_BONUS } else { 1.0 };
            step_flight(&mut s.kin, &s.body.stats, &s.input, DT, bonus);
            if let Some(imp) = collide_asteroids(&mut s.kin, &s.body, &self.rocks, tick_time(t))
                && s.collide_cd <= 0.0
            {
                s.collide_cd = 0.3;
                let (pos, rot) = (s.kin.pos, s.kin.rot);
                s.splash(pos, rot, imp.point, 1.7, imp.speed * 0.9, None);
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

        // ships that lost every engine and every gun give up after a few seconds
        let mut give_up = Vec::new();
        for s in self.ships.values_mut() {
            if s.body.stats.thrust == 0.0 && s.body.stats.weapons.is_empty() {
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
            Mode::Ffa => self.ffa_logic(),
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
                            s.splash(pos, rot, cp, 1.9, dmg * 2.0 * share * f, Some(other));
                        }
                        self.events.push(ServerMsg::Crash { pos: cp });
                    }
                }
            }
        }
    }

    fn update_bolts(&mut self, t: u32) {
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
                        dir = cur.lerp(des, (2.6 * DT / ang).min(1.0)).normalize_or(cur);
                    }
                }
                b.speed = (b.speed + 150.0 * DT).min(270.0);
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
                        s.kin.shield = (s.kin.shield - b.dmg).max(0.0);
                        s.kin.shield_delay = 3.0;
                        s.recent_dmg += b.dmg * 0.5;
                        s.last_attacker = Some(b.owner);
                        self.events.push(ServerMsg::ShieldHit { ship: s.id, pos: pt });
                        if spec.splash > 0.0 {
                            self.events.push(ServerMsg::Explosion { pos: pt, size: 0.6 });
                        }
                        dead = true;
                        break;
                    }
                    if let Some((blk, pt)) = s.body.ray(sp, sr, b.prev, b.pos) {
                        if spec.splash > 0.0 {
                            self.events.push(ServerMsg::Explosion { pos: pt, size: spec.splash * 0.55 });
                            s.splash(sp, sr, pt, spec.splash, b.dmg, Some(b.owner));
                        } else if spec.pierce > 1 {
                            // on through the ship: the next blocks along the line, each hit softer
                            let dir = (b.pos - b.prev).normalize_or(Vec3::NEG_Z);
                            let mut dmg = b.dmg;
                            for i in s.body.ray_through(sp, sr, b.prev, b.pos + dir * s.body.radius * 2.0, spec.pierce as usize) {
                                s.hit_block(i, dmg, Some(b.owner));
                                dmg *= PIERCE_FALLOFF;
                            }
                        } else {
                            s.hit_block(blk, b.dmg, Some(b.owner));
                        }
                        dead = true;
                        break;
                    }
                }
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
    }

    fn settle_ships(&mut self) {
        let mut kills = Vec::new();
        for s in self.ships.values_mut() {
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
                if !s.body.cockpit_alive() {
                    kills.push((s.id, s.last_attacker));
                }
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
        let pts = 100 + (victim.body.max_hp / 10.0) as u32;
        if let Some(pid) = killer_owner
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
            p.deaths += 1;
            p.ship = None;
            p.send(&ServerMsg::YourShip { id: None });
            if self.cfg.mode == Mode::Ffa {
                p.respawn_at = Some(self.tick + secs(4.0));
            }
        }
        let text = match killer_name {
            Some(k) => format!("{k} 击毁了 {}", victim.name),
            None => format!("{} 坠毁了", victim.name),
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
        self.announce("波次清除", format!("奖励 +{bonus} · 正在修复战机"));
        self.after(2.2, Timer::RepairPlayers);
        self.after(5.5, Timer::StartWave);
    }

    fn ffa_logic(&mut self) {
        let now = self.tick;
        let due: Vec<u16> = self.players.iter().filter(|(_, p)| p.respawn_at.is_some_and(|t| t <= now)).map(|(&k, _)| k).collect();
        for pid in due {
            self.spawn_player(pid);
        }
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

    fn flush(&mut self, t: u32) {
        if self.scores_dirty && t % 15 == 0 {
            self.scores_dirty = false;
            let mut rows: Vec<ScoreRow> = self
                .players
                .iter()
                .filter(|(_, p)| !p.watching())
                .map(|(&pid, p)| ScoreRow { pid, name: p.name.clone(), kills: p.kills, deaths: p.deaths, score: p.score, alive: p.ship.is_some() })
                .chain(self.seats.iter().enumerate().map(|(i, s)| ScoreRow {
                    pid: BOT_PID_BASE + i as u16,
                    name: s.name.clone(),
                    kills: s.kills,
                    deaths: s.deaths,
                    score: s.score,
                    alive: s.ship.is_some(),
                }))
                .collect();
            rows.sort_by(|a, b| b.score.cmp(&a.score).then(b.kills.cmp(&a.kills)));
            let enemies = self.ships.values().filter(|s| s.owner.is_none()).count() as u32;
            self.events.push(ServerMsg::Scores { rows, wave: self.wave, enemies });
        }
        if !self.events.is_empty() {
            let encoded: Vec<Bytes> = self.events.drain(..).map(|m| Bytes::from(encode(&m))).collect();
            for p in self.players.values() {
                for b in &encoded {
                    let _ = p.out.send(Outgoing::Reliable(b.clone()));
                }
            }
        }
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
                flags: (if s.input.has(voxwing_shared::flight::BTN_FIRE) { FLAG_FIRING } else { 0 }) | (if s.kin.boosting { FLAG_BOOST } else { 0 }),
                throttle: s.input.throttle,
                shield: if s.body.stats.shield_cap > 0.0 { (s.kin.shield / s.body.stats.shield_cap * 255.0) as u8 } else { 0 },
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

fn spawn_msg(s: &Ship) -> ServerMsg {
    ServerMsg::ShipSpawn { id: s.id, team: s.team, name: s.name.clone(), owner: s.owner, design: (*s.body.design).clone(), kin: s.kin }
}

pub fn run_loop(cfg: GameConfig, rx: Receiver<NetEvent>, stop: Arc<AtomicBool>) {
    let verbose = cfg.verbose;
    let mut g = Game::new(cfg);
    let step = Duration::from_secs_f64(1.0 / TICK_RATE as f64);
    let mut next = Instant::now();
    let (mut worst, mut total, mut n) = (Duration::ZERO, Duration::ZERO, 0u32);
    let mut report = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        while let Ok(ev) = rx.try_recv() {
            g.handle(ev);
        }
        let t0 = Instant::now();
        g.tick();
        let el = t0.elapsed();
        worst = worst.max(el);
        total += el;
        n += 1;
        if verbose && report.elapsed() > Duration::from_secs(10) {
            println!(
                "[server] tick {} · players {} · ships {} · bolts {} · tick avg {:.2} ms, worst {:.2} ms",
                g.tick,
                g.players.len(),
                g.ships.len(),
                g.bolts.len(),
                total.as_secs_f64() * 1000.0 / n as f64,
                worst.as_secs_f64() * 1000.0
            );
            report = Instant::now();
            worst = Duration::ZERO;
            total = Duration::ZERO;
            n = 0;
        }
        next += step;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else if now - next > Duration::from_millis(250) {
            next = now;
        }
    }
}
