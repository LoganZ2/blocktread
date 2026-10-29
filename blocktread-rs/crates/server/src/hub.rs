//! The server's lobby: connections, a queue per mode (and budget tier), the matches they turn
//! into, and the free-for-all room.
//!
//! 2v2: a player connects and queues. When a queue holds a full match, or its first player has
//! waited `queue_wait`, a match is made: players get seats and AI takes the rest. Everyone picks
//! a ship and sees the others' picks; once all have locked in, or the pick time is up, the match
//! plays out in a `Game` of its own, the two sides fighting over the arena's hill, until one has
//! the points to win or time runs out. Then everyone gets the results and is back in the lobby.
//!
//! Free-for-all: one room per server, no AI, no seat limit. Queueing for it gets a one-seat pick
//! (the client sends the vehicle in view at once) and then drives straight into the room, which
//! opens with its first player and closes with its last. Players come and go as they like; the
//! room's `Game` runs its rounds (see `Game::rounds`).
//!
//! The test range is the same one-seat pick, into a game of its own with nobody else.

use crate::game::{Game, GameConfig, SOLO_SEED};
use crate::{NetEvent, Outgoing};
use bytes::Bytes;
use std::collections::{BTreeMap, HashMap};
use blocktread_shared::analysis::analyze;
use blocktread_shared::arena::HILL_WIN;
use blocktread_shared::balance::balance;
use blocktread_shared::blocks::Design;
use blocktread_shared::presets::{PRESETS, preset, rover};
use blocktread_shared::protocol::*;
use blocktread_shared::rng::Rng;
use blocktread_shared::TICK_RATE;

/// Seconds everyone gets to pick a ship.
pub const PICK_SECS: f32 = 30.0;
/// The two teams of a 2v2 match.
pub const TEAMS: [u16; 2] = [1, 2];

pub fn team_name(team: u16) -> &'static str {
    if team == TEAMS[0] { "蓝队" } else { "红队" }
}

#[derive(Clone, Debug)]
pub struct HubConfig {
    /// Seconds a queue waits for more players before AI fills a match (0 for local play).
    pub queue_wait: f32,
    /// A 2v2 match's length (and a free-for-all round's), and the points that win a 2v2.
    pub match_secs: f32,
    pub win_points: u32,
}

impl Default for HubConfig {
    fn default() -> Self {
        HubConfig { queue_wait: 15.0, match_secs: 480.0, win_points: HILL_WIN }
    }
}

type Out = tokio::sync::mpsc::UnboundedSender<Outgoing>;
/// Who waits for what: (mode, tier) → (queued since, pid), oldest first.
type Queues = HashMap<(Mode, Option<String>), Vec<(u32, u16)>>;

fn send(out: &Out, msg: &ServerMsg) {
    let _ = out.send(Outgoing::Reliable(Bytes::from(encode(msg))));
}

/// An AI seat's name, numbered when another seat already flies the same ship.
fn ai_name(slots: &[Slot], ship: &str) -> String {
    let base = format!("AI·{ship}");
    let taken = |n: &str| slots.iter().any(|s| s.name == n);
    (1..).map(|i| if i == 1 { base.clone() } else { format!("{base} {i}") }).find(|n| !taken(n)).unwrap()
}

fn secs(s: f32) -> u32 {
    (s * TICK_RATE as f32) as u32
}

/// What whoever didn't pick in time drives: the first preset that fits the budget.
fn fallback(budget: Option<f32>) -> Design {
    PRESETS.iter().map(|p| preset(p.key)).find(|d| budget.is_none_or(|b| analyze(d).stats.cost <= b + 1e-3)).unwrap_or_else(rover)
}

#[derive(Clone, Debug, PartialEq)]
enum Where {
    Idle,
    /// In a queue since this tick.
    Queued { mode: Mode, tier: Option<String>, since: u32 },
    Match(u32),
}

struct Conn {
    name: String,
    out: Out,
    at: Where,
}

struct Slot {
    name: String,
    team: u16,
    /// The player in this seat; None for AI (or a player who left before the start).
    pid: Option<u16>,
    design: Option<Design>,
    ready: bool,
}

enum Phase {
    Pick { until: u32 },
    Play(Box<Game>),
}

struct Match {
    mode: Mode,
    tier: Option<String>,
    slots: Vec<Slot>,
    /// Spectators.
    watchers: Vec<u16>,
    phase: Phase,
}

impl Match {
    fn budget(&self) -> Option<f32> {
        self.tier.as_deref().and_then(|k| balance().tier(k)).map(|t| t.budget)
    }
}

pub struct Hub {
    cfg: HubConfig,
    tick: u32,
    conns: BTreeMap<u16, Conn>,
    matches: BTreeMap<u32, Match>,
    next_match: u32,
    rng: Rng,
}

impl Hub {
    pub fn new(cfg: HubConfig) -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
        Hub { cfg, tick: 0, conns: BTreeMap::new(), matches: BTreeMap::new(), next_match: 1, rng: Rng::new(seed) }
    }

    /// For the server log: connections, matches, players waiting.
    pub fn census(&self) -> (usize, usize, usize) {
        (self.conns.len(), self.matches.len(), self.conns.values().filter(|c| matches!(c.at, Where::Queued { .. })).count())
    }

    pub fn handle(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Connect { pid, name, watch, out } => {
                let mut name: String = name.trim().chars().filter(|c| !c.is_control()).take(12).collect();
                if name.is_empty() {
                    name = format!("车长{pid}");
                }
                send(&out, &ServerMsg::Connected { pid });
                self.conns.insert(pid, Conn { name, out, at: Where::Idle });
                if watch {
                    self.watch(pid);
                }
            }
            NetEvent::Msg { pid, msg } => match msg {
                ClientMsg::Queue { mode, tier } => self.queue(pid, mode, tier),
                ClientMsg::Unqueue => {
                    if let Some(c) = self.conns.get_mut(&pid)
                        && matches!(c.at, Where::Queued { .. })
                    {
                        c.at = Where::Idle;
                    }
                }
                ClientMsg::Pick { design } => self.pick(pid, design),
                ClientMsg::Ready => self.ready(pid),
                // a spectator whose match ended watches the next one
                ClientMsg::Watch { .. } if self.conns.get(&pid).is_some_and(|c| c.at == Where::Idle) => self.watch(pid),
                _ => {}
            },
            NetEvent::Input { pid, pkt } => {
                if let Some(Where::Match(id)) = self.conns.get(&pid).map(|c| &c.at)
                    && let Some(Match { phase: Phase::Play(g), .. }) = self.matches.get_mut(id)
                {
                    g.input(pid, pkt);
                }
            }
            NetEvent::Leave { pid } => self.leave(pid),
        }
    }

    fn queue(&mut self, pid: u16, mode: Mode, tier: Option<String>) {
        let now = self.tick;
        let Some(c) = self.conns.get_mut(&pid) else { return };
        if c.at != Where::Idle {
            return;
        }
        // only 2v2 is matched by budget: the room and the test range take any vehicle
        let tier = tier.filter(|k| mode == Mode::Teams && balance().tier(k).is_some());
        c.at = Where::Queued { mode, tier, since: now };
        self.queue_status();
    }

    pub fn tick(&mut self) {
        self.tick += 1;
        if self.tick % 15 == 0 {
            self.matchmake();
        }
        if self.tick % TICK_RATE == 0 {
            self.queue_status();
        }
        let ids: Vec<u32> = self.matches.keys().copied().collect();
        for id in ids {
            self.step_match(id);
        }
    }

    /// Everyone queued, grouped by what they queued for, oldest first.
    fn queues(&self) -> Queues {
        let mut q = Queues::new();
        for (&pid, c) in &self.conns {
            if let Where::Queued { mode, tier, since } = &c.at {
                q.entry((*mode, tier.clone())).or_default().push((*since, pid));
            }
        }
        for v in q.values_mut() {
            v.sort();
        }
        q
    }

    fn queue_status(&self) {
        for ((_, _), waiting) in self.queues() {
            let fill_in = (self.cfg.queue_wait - (self.tick - waiting[0].0) as f32 / TICK_RATE as f32).max(0.0);
            for &(_, pid) in &waiting {
                send(&self.conns[&pid].out, &ServerMsg::Queued { players: waiting.len() as u32, fill_in });
            }
        }
    }

    fn matchmake(&mut self) {
        let wait = secs(self.cfg.queue_wait);
        for ((mode, tier), mut waiting) in self.queues() {
            let full = mode.slots();
            while waiting.len() >= full {
                let rest = waiting.split_off(full);
                self.make_match(mode, tier.clone(), waiting.iter().map(|w| w.1).collect());
                waiting = rest;
            }
            // nobody else came in time: AI fills the seats
            if let Some(first) = waiting.first()
                && self.tick - first.0 >= wait
            {
                self.make_match(mode, tier.clone(), waiting.iter().map(|w| w.1).collect());
            }
        }
    }

    /// A preset that fits the budget for an AI seat, a different one for each seat while they last.
    fn ai_ship(&mut self, budget: Option<f32>, taken: &[Slot]) -> Design {
        let fits: Vec<usize> = (0..PRESETS.len()).filter(|&p| budget.is_none_or(|b| analyze(&preset(PRESETS[p].key)).stats.cost <= b + 1e-3)).collect();
        let pool = if fits.is_empty() { (0..PRESETS.len()).collect() } else { fits };
        let fresh: Vec<usize> = pool.iter().copied().filter(|&p| !taken.iter().any(|s| s.design.as_ref().is_some_and(|d| d.name == PRESETS[p].name))).collect();
        let pick = if fresh.is_empty() { pool[self.rng.below(pool.len())] } else { fresh[self.rng.below(fresh.len())] };
        preset(PRESETS[pick].key)
    }

    fn make_match(&mut self, mode: Mode, tier: Option<String>, pids: Vec<u16>) {
        let id = self.next_match;
        self.next_match += 1;
        let budget = tier.as_deref().and_then(|k| balance().tier(k)).map(|t| t.budget);
        // seats: teams take turns in 2v2, everyone their own side in free-for-all
        let n = mode.slots();
        let team_of = |i: usize| if mode == Mode::Teams { TEAMS[i % 2] } else { 100 + i as u16 };
        let mut slots: Vec<Slot> = Vec::with_capacity(n);
        for (i, &pid) in pids.iter().enumerate() {
            slots.push(Slot { name: self.conns[&pid].name.clone(), team: team_of(i), pid: Some(pid), design: None, ready: false });
        }
        for i in pids.len()..n {
            let design = self.ai_ship(budget, &slots);
            slots.push(Slot { name: ai_name(&slots, &design.name), team: team_of(i), pid: None, design: Some(design), ready: true });
        }
        // list each team together
        slots.sort_by_key(|s| s.team);
        let infos: Vec<SlotInfo> = slots.iter().map(|s| SlotInfo { name: s.name.clone(), team: s.team, bot: s.pid.is_none() }).collect();
        for (k, s) in slots.iter().enumerate() {
            let Some(pid) = s.pid else { continue };
            let c = self.conns.get_mut(&pid).unwrap();
            c.at = Where::Match(id);
            send(&c.out, &ServerMsg::MatchFound { mode, tier: tier.clone(), you: k as u8, slots: infos.clone(), pick_secs: PICK_SECS });
        }
        let m = Match { mode, tier, slots, watchers: Vec::new(), phase: Phase::Pick { until: self.tick + secs(PICK_SECS) } };
        // the AI seats' ships are known from the start
        for (k, s) in m.slots.iter().enumerate().filter(|(_, s)| s.pid.is_none()) {
            self.tell(&m, &ServerMsg::SlotPick { slot: k as u8, design: s.design.clone(), ready: true });
        }
        self.matches.insert(id, m);
    }

    /// Send to everyone in a match during the pick (players and spectators).
    fn tell(&self, m: &Match, msg: &ServerMsg) {
        for pid in m.slots.iter().filter_map(|s| s.pid).chain(m.watchers.iter().copied()) {
            if let Some(c) = self.conns.get(&pid) {
                send(&c.out, msg);
            }
        }
    }

    fn picking(&mut self, pid: u16) -> Option<(u32, usize)> {
        let Where::Match(id) = self.conns.get(&pid)?.at else { return None };
        let m = self.matches.get(&id)?;
        if !matches!(m.phase, Phase::Pick { .. }) {
            return None;
        }
        Some((id, m.slots.iter().position(|s| s.pid == Some(pid))?))
    }

    fn pick(&mut self, pid: u16, design: Design) {
        let Some((id, k)) = self.picking(pid) else { return };
        let m = &self.matches[&id];
        if m.slots[k].ready {
            return;
        }
        let a = analyze(&design);
        let refuse = match (a.first_error(), m.budget()) {
            (Some(e), _) => Some(format!("这辆没有通过出车检查：{}", e.msg)),
            (None, Some(b)) if a.stats.cost > b + 1e-3 => Some(format!("这辆造价 {:.0}，超出本局上限 {b:.0}", a.stats.cost)),
            _ => None,
        };
        if let Some(text) = refuse {
            send(&self.conns[&pid].out, &ServerMsg::Notice { text });
            return;
        }
        let design = Design { name: design.sanitized_name(), blocks: design.blocks };
        let m = self.matches.get_mut(&id).unwrap();
        m.slots[k].design = Some(design.clone());
        let m = &self.matches[&id];
        self.tell(m, &ServerMsg::SlotPick { slot: k as u8, design: Some(design), ready: false });
    }

    fn ready(&mut self, pid: u16) {
        let Some((id, k)) = self.picking(pid) else { return };
        let m = self.matches.get_mut(&id).unwrap();
        if m.slots[k].design.is_none() || m.slots[k].ready {
            return;
        }
        m.slots[k].ready = true;
        let design = m.slots[k].design.clone();
        let m = &self.matches[&id];
        self.tell(m, &ServerMsg::SlotPick { slot: k as u8, design, ready: true });
    }

    /// Watch the free-for-all room while anyone drives there, else a 2v2 being played, else the
    /// room (opened empty if need be: players who come in later show up).
    fn watch(&mut self, pid: u16) {
        let playing = |mode: Mode, m: &Match| m.mode == mode && matches!(m.phase, Phase::Play(_)) && !m.slots.is_empty();
        let found = self.matches.iter().find(|(_, m)| playing(Mode::Ffa, m)).or_else(|| self.matches.iter().find(|(_, m)| playing(Mode::Teams, m))).map(|(&id, _)| id);
        let id = found.unwrap_or_else(|| self.room());
        let c = self.conns.get_mut(&pid).unwrap();
        c.at = Where::Match(id);
        let (name, out) = (c.name.clone(), c.out.clone());
        let m = self.matches.get_mut(&id).unwrap();
        m.watchers.push(pid);
        if let Phase::Play(g) = &mut m.phase {
            g.join(pid, name, None, None, out);
        }
    }

    fn leave(&mut self, pid: u16) {
        let Some(c) = self.conns.remove(&pid) else { return };
        let Where::Match(id) = c.at else { return };
        let Some(m) = self.matches.get_mut(&id) else { return };
        m.watchers.retain(|&w| w != pid);
        let Some(k) = m.slots.iter().position(|s| s.pid == Some(pid)) else {
            if let Phase::Play(g) = &mut m.phase {
                g.leave(pid);
            }
            return;
        };
        match (m.mode, &mut m.phase) {
            // the test range is that one player's, and so is a way into the room: they go with them
            (Mode::Solo, _) | (Mode::Ffa, Phase::Pick { .. }) => {
                self.matches.remove(&id);
                return;
            }
            // the room just has one fewer (it closes once nobody is left: see `step_match`)
            (Mode::Ffa, Phase::Play(g)) => {
                g.leave(pid);
                m.slots.remove(k);
                return;
            }
            _ => {}
        }
        // the seat stays filled: AI flies the ship that was picked (or would have been)
        let budget = m.budget();
        let slot = &mut m.slots[k];
        slot.pid = None;
        slot.name = format!("AI·{}", slot.name);
        slot.ready = true;
        if let Phase::Play(g) = &mut m.phase {
            g.leave(pid);
            let design = slot.design.clone().unwrap_or_else(rover);
            g.add_seat(slot.name.clone(), design, Some(slot.team));
            return;
        }
        if slot.design.is_none() {
            let d = self.ai_ship(budget, &[]);
            self.matches.get_mut(&id).unwrap().slots[k].design = Some(d);
        }
        let m = &self.matches[&id];
        let s = &m.slots[k];
        self.tell(m, &ServerMsg::SlotPick { slot: k as u8, design: s.design.clone(), ready: true });
    }

    fn step_match(&mut self, id: u32) {
        enum Next {
            Wait,
            Start,
            Finish,
            Drop,
        }
        let now = self.tick;
        let Some(m) = self.matches.get_mut(&id) else { return };
        let all_in = m.slots.iter().all(|s| s.ready);
        let next = match &mut m.phase {
            Phase::Pick { until } if all_in || now >= *until => Next::Start,
            Phase::Pick { .. } => Next::Wait,
            Phase::Play(g) => {
                g.tick();
                let best = g.points().values().copied().max().unwrap_or(0);
                if !g.has_players() {
                    // everyone left: nobody to finish it for
                    Next::Drop
                } else if (g.limit > 0 && best >= g.limit) || g.ends_at.is_some_and(|e| g.now() >= e) {
                    Next::Finish
                } else {
                    Next::Wait
                }
            }
        };
        match next {
            Next::Wait => {}
            Next::Start => self.start(id),
            Next::Finish => self.finish(id),
            Next::Drop => {
                self.matches.remove(&id);
            }
        }
    }

    fn start(&mut self, id: u32) {
        if self.matches[&id].mode == Mode::Ffa {
            return self.enter_room(id);
        }
        let budget = self.matches[&id].budget();
        let m = self.matches.get_mut(&id).unwrap();
        let cfg = GameConfig { mode: m.mode, bots: 0, budget, verbose: false };
        // the test range: the same map every time, and no end
        let mut game = Box::new(if m.mode == Mode::Solo { Game::with_seed(cfg, SOLO_SEED) } else { Game::new(cfg) });
        if m.mode != Mode::Solo {
            game.limit = self.cfg.win_points;
            game.ends_at = Some(secs(self.cfg.match_secs));
        }
        let slots = std::mem::take(&mut m.slots);
        let watchers = m.watchers.clone();
        for s in &slots {
            let design = s.design.clone().unwrap_or_else(|| fallback(budget));
            match s.pid.and_then(|p| self.conns.get(&p).map(|c| (p, c))) {
                Some((pid, c)) => game.join(pid, s.name.clone(), Some(design), Some(s.team), c.out.clone()),
                None => game.add_seat(s.name.clone(), design, Some(s.team)),
            }
        }
        for pid in watchers {
            if let Some(c) = self.conns.get(&pid) {
                game.join(pid, c.name.clone(), None, None, c.out.clone());
            }
        }
        let m = self.matches.get_mut(&id).unwrap();
        m.slots = slots;
        m.phase = Phase::Play(game);
    }

    /// The free-for-all room, opened (empty, no AI) if it isn't.
    fn room(&mut self) -> u32 {
        if let Some((&id, _)) = self.matches.iter().find(|(_, m)| m.mode == Mode::Ffa && matches!(m.phase, Phase::Play(_))) {
            return id;
        }
        let id = self.next_match;
        self.next_match += 1;
        let mut game = Game::new(GameConfig { mode: Mode::Ffa, bots: 0, budget: None, verbose: false });
        game.rounds = Some(self.cfg.match_secs);
        self.matches.insert(id, Match { mode: Mode::Ffa, tier: None, slots: Vec::new(), watchers: Vec::new(), phase: Phase::Play(Box::new(game)) });
        id
    }

    /// Someone's way into the room is through (vehicle locked in, or out of time to pick): in
    /// they drive.
    fn enter_room(&mut self, id: u32) {
        let Some(entry) = self.matches.remove(&id) else { return };
        let room = self.room();
        let m = self.matches.get_mut(&room).unwrap();
        let Phase::Play(g) = &mut m.phase else { return };
        for s in entry.slots {
            let Some(c) = s.pid.and_then(|p| self.conns.get_mut(&p)) else { continue };
            c.at = Where::Match(room);
            g.join(s.pid.unwrap(), s.name.clone(), Some(s.design.unwrap_or_else(|| fallback(None))), None, c.out.clone());
            m.slots.push(Slot { name: s.name, team: 0, pid: s.pid, design: None, ready: true });
        }
    }

    fn finish(&mut self, id: u32) {
        let Some(m) = self.matches.remove(&id) else { return };
        let Phase::Play(g) = m.phase else { return };
        let points = g.points();
        let best = points.values().copied().max().unwrap_or(0);
        let leaders: Vec<u16> = points.iter().filter(|&(_, &k)| k == best && best > 0).map(|(&t, _)| t).collect();
        let tally = |t: u16| points.get(&t).copied().unwrap_or(0);
        let winner = match (m.mode, leaders.as_slice()) {
            (_, [] | [_, _, ..]) => "平局".to_string(),
            (Mode::Teams, [t]) => format!("{}获胜 · {} : {}", team_name(*t), tally(*t), tally(if *t == TEAMS[0] { TEAMS[1] } else { TEAMS[0] })),
            (_, [t]) => m.slots.iter().find(|s| s.team == *t).map_or("平局".to_string(), |s| format!("{} 获胜", s.name)),
        };
        g.broadcast(&ServerMsg::MatchOver { winner, rows: g.scores() });
        for pid in m.slots.iter().filter_map(|s| s.pid).chain(m.watchers.iter().copied()) {
            if let Some(c) = self.conns.get_mut(&pid) {
                c.at = Where::Idle;
            }
        }
    }
}
