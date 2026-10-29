use crate::ARENA_R;
use crate::DT;
use crate::blocks::{BlockType, Gadget};
use crate::flight::{BTN_BOOST, BTN_FIRE, BTN_FLIP, BTN_FLIPPER, BTN_JUMP, BTN_MINE, BTN_MISSILE, BTN_SMOKE, InputCmd, Kin, NO_LOCK, q_axis, quantize_aim};
use crate::arena::{Arena, Rect};
use crate::rng::Rng;
use crate::nav::Nav;
use crate::ship::ShipBody;
use crate::solid::Solid;
use crate::terrain::Terrain;
use crate::weapons::intercept;
use crate::world::{Rock, tick_time};
use glam::{Quat, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Skill {
    /// Aim wobble as a fraction of distance.
    pub aim_err: f32,
    pub cone: f32,
    /// Input smoothing rate (1/s); higher reacts faster.
    pub react: f32,
    pub dmg_mul: f32,
}

impl Skill {
    pub fn ffa_bot() -> Skill {
        Skill { aim_err: 0.022, cone: 0.05, react: 7.0, dmg_mul: 0.8 }
    }
}

/// Another ship as the AI sees it.
#[derive(Clone, Copy, Debug)]
pub struct Contact {
    pub id: u16,
    pub team: u16,
    pub pos: Vec3,
    pub vel: Vec3,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Attack,
    /// Drive off in `away` for a while, then come back around.
    Evade,
    /// Stuck against something: back out, wheel turned the other way.
    Reverse,
}

/// Driving bot: run at the nearest enemy, circle it at gun range, break away now and then, steer
/// round rocks and the arena's walls and pits, hold fire while a wall is in the way, back out when
/// stuck and right itself when it rolls over. Given somewhere to be, it makes for that instead,
/// fighting only what comes close, and once there holds it.
#[derive(Clone, Debug)]
pub struct AiPilot {
    pub skill: Skill,
    /// Where it would rather be than wherever its fight takes it (the hill, a pad), and how near
    /// counts as there; set by the game every tick.
    pub goal: Option<(Vec3, f32)>,
    /// A missile is after it (the game says so every tick): time for smoke, if it has some.
    pub hunted: bool,
    state: State,
    t: f32,
    away: Vec3,
    wob: Vec3,
    wob_t: f32,
    lock: f32,
    missile_t: f32,
    chase: f32,
    chase_max: f32,
    stuck: f32,
    /// Its speed when it last seemed stuck, to tell stuck from slowly getting going.
    stuck_v: f32,
    steer: f32,
    /// Which way it last turned to get round something (±1), so it keeps going round it the same
    /// way rather than dithering.
    dodge: f32,
    /// How long it has sat tipped up (on its side, its tail, a wall).
    tipped: f32,
    /// Which way it last turned towards where it wanted to go (±1): a way straight behind it is
    /// as near one way round as the other, so it keeps going round the way it was.
    last_turn: f32,
    /// The way round whatever blocks the straight way, held while it stays open so that turning
    /// a big vehicle round in a few goes all turns it the same way.
    detour: Option<Vec3>,
    /// The heading a turn in a few goes (back, forward, back) is bringing it round to, and how
    /// long it has left: held until it is done, so that every go turns it the same way.
    turning: Option<(Vec3, f32)>,
    /// The ramp it is going up the hill by (an index into the arena's `hill_ways`), and whether it
    /// is on that ramp's line yet.
    ramp: Option<usize>,
    on_line: bool,
    /// Seconds into turning round where it stands, in goes forward and back (below 0: it isn't).
    kturn: f32,
    /// How tight it turns with the wheel hard over at a crawl (the circle's radius, m), as it has
    /// seen itself turn; faster, the wheel turns less and the circle is wider (see `turn_at`).
    turn_r: f32,
    /// The planned way to where it is going, how far along it it has got, when it next plans,
    /// and where it planned to.
    path: Vec<Vec3>,
    path_at: usize,
    plan_t: f32,
    plan_goal: Vec3,
    rng: Rng,
}

/// Probes look ahead in steps this long.
const PROBE_STEP: f32 = 2.0;
/// A rise from one probe to the next that a vehicle can't drive up (a wall, a crate, a ramp's
/// side), and a drop it won't drive off.
const PROBE_CLIMB: f32 = 0.9;
const PROBE_DROP: f32 = 3.0;
/// Room it wants past its nose to turn round in; with less, it backs up to turn.
const ROOM: f32 = 4.0;

/// How far a vehicle standing on ground at height `floor` can drive from `from` along `dir`
/// (level) before something stops it: a step too high, a drop too deep, a pit or the kickers
/// that lead into it, a saw's slot, a vent that is burning at `now`.
#[allow(clippy::too_many_arguments)]
fn free_run(ground: &Terrain, near: &[&Solid], from: Vec3, dir: Vec3, floor: f32, reach: f32, now: f64) -> f32 {
    let surface = |p: Vec3| near.iter().filter_map(|s| s.top(p.x, p.z)).fold(ground.height(p.x, p.z), f32::max);
    let arena = ground.arena.as_ref();
    // already by a pit (knocked there, say): anywhere but into it; and a side hanging over one
    // already is no reason not to drive off
    let by_pit = arena.is_some_and(|a| a.near_pit(from.x, from.z));
    let over_pit = arena.is_some_and(|a| a.in_pit(from.x, from.z));
    let in_hazard = |p: Vec3| arena.is_some_and(|a| a.in_hazard(p.x, p.z, Some(now)));
    let by_hazard = in_hazard(from);
    let mut prev = floor;
    let mut d = 0.0;
    while d < reach {
        let p = from + dir * (d + PROBE_STEP);
        let in_pit = arena.is_some_and(|a| a.in_pit(p.x, p.z));
        if !over_pit && (in_pit || (!by_pit && arena.is_some_and(|a| a.near_pit(p.x, p.z)))) {
            return d;
        }
        if !by_hazard && in_hazard(p) {
            return d;
        }
        // the whole step at knee height, so that it can't step over a thin wall's corner
        let knee = Vec3::Y * (prev + PROBE_CLIMB);
        if near.iter().any(|s| s.clip((from + dir * d).with_y(0.0) + knee, p.with_y(0.0) + knee).is_some()) {
            return d;
        }
        if !in_pit {
            let h = surface(p);
            if h - prev > PROBE_CLIMB || prev - h > PROBE_DROP {
                return d;
            }
            prev = h;
        }
        d += PROBE_STEP;
    }
    reach
}

/// A vent across the way from `from` along `dir` within `reach`, burning at `now` or about to,
/// that a vehicle `half_w` either side of its middle isn't on already.
fn vent_ahead(ground: &Terrain, from: Vec3, dir: Vec3, reach: f32, half_w: f32, now: f64) -> bool {
    let Some(a) = ground.arena.as_ref() else { return false };
    let dir = dir.with_y(0.0).normalize_or_zero();
    a.vents.iter().filter(|v| v.hot(now)).any(|v| {
        let r = v.area;
        let wide = Rect { x0: r.x0 - half_w, x1: r.x1 + half_w, z0: r.z0 - half_w, z1: r.z1 + half_w };
        !wide.contains(from.x, from.z) && (1..=(reach / PROBE_STEP).ceil() as i32).map(|k| from + dir * (k as f32 * PROBE_STEP)).any(|p| wide.contains(p.x, p.z))
    })
}

/// How far the vehicle's blocks reach from its centre: ahead, behind and to either side.
fn extent(body: &ShipBody) -> (f32, f32, f32) {
    (0..body.design.blocks.len()).filter(|&i| body.alive[i]).map(|i| body.local(i)).fold((0.0f32, 0.0f32, 0.0f32), |(f, b, w), p| {
        (f.max(0.5 - p.z), b.max(0.5 + p.z), w.max(0.5 + p.x.abs()))
    })
}

/// Signed angle (rad) from `a` to `b` around the vertical, both flattened: positive is right.
fn heading_to(a: Vec3, b: Vec3) -> f32 {
    let (a, b) = (a.with_y(0.0), b.with_y(0.0));
    (a.x * b.z - a.z * b.x).atan2(a.x * b.x + a.z * b.z)
}

impl AiPilot {
    pub fn new(skill: Skill, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        AiPilot {
            skill,
            goal: None,
            hunted: false,
            state: State::Attack,
            t: 0.0,
            away: Vec3::Z,
            wob: Vec3::ZERO,
            wob_t: 0.0,
            lock: 0.0,
            missile_t: rng.range(2.0, 5.0),
            chase: 0.0,
            chase_max: rng.range(9.0, 14.0),
            stuck: 0.0,
            stuck_v: 0.0,
            steer: 0.0,
            dodge: 1.0,
            tipped: 0.0,
            last_turn: 1.0,
            detour: None,
            turning: None,
            ramp: None,
            on_line: false,
            kturn: -1.0,
            turn_r: 8.0,
            path: Vec::new(),
            path_at: 0,
            plan_t: 0.0,
            plan_goal: Vec3::ZERO,
            rng,
        }
    }

    /// The heading nearest `des` that the vehicle can drive along for a while (`des` itself when
    /// nothing is in the way), and how much room there is past its nose and past its tail.
    fn clear_heading(&mut self, me: &Kin, body: &ShipBody, ground: &Terrain, des: Vec3, now: f64) -> (Vec3, f32, f32) {
        let flat = |v: Vec3| v.with_y(0.0).normalize_or(Vec3::NEG_Z);
        let want = flat(des.with_y(0.0).normalize_or(me.fwd()));
        let (nose, tail, half_w) = extent(body);
        let reach = (me.vel.length() * 1.6).clamp(10.0, 40.0).max(nose.max(tail) + ROOM + PROBE_STEP);
        let near: Vec<&Solid> = ground.solids_near(me.pos, reach + body.radius + PROBE_STEP).collect();
        let floor = near.iter().filter_map(|s| s.top(me.pos.x, me.pos.z)).filter(|&h| h < me.pos.y).fold(ground.height(me.pos.x, me.pos.z), f32::max);
        // probes across its width with a little to spare, close enough that the end of a wall
        // can't slip between them
        let lines = ((half_w + 0.5) / 1.2).ceil() as i32;
        let free = |dir: Vec3| {
            let side = dir.cross(Vec3::Y) * ((half_w + 0.5) / lines as f32);
            (-lines..=lines).map(|k| free_run(ground, &near, me.pos + side * k as f32, dir, floor, reach, now)).fold(reach, f32::min)
        };
        let (ahead, behind) = (free(flat(me.fwd())) - nose, free(-flat(me.fwd())) - tail);
        if free(want) >= reach {
            self.detour = None;
            return (des, ahead, behind);
        }
        if let Some(way) = self.detour.filter(|&w| w.dot(want) > -0.2 && free(w) >= reach * 0.75) {
            return (way, ahead, behind);
        }
        let mut best = (want, f32::MIN, self.dodge);
        for k in 1..=8 {
            let a = k as f32 * 0.35;
            for s in [self.dodge, -self.dodge] {
                let dir = Quat::from_rotation_y(-s * a) * want;
                let f = free(dir);
                if f >= reach {
                    self.dodge = s;
                    self.detour = Some(dir);
                    return (dir, ahead, behind);
                }
                // the most room, for the least turn
                if f - a * 4.0 > best.1 {
                    best = (dir, f - a * 4.0, s);
                }
            }
        }
        self.dodge = best.2;
        self.detour = Some(best.0);
        (best.0, ahead, behind)
    }

    /// Which way to head for `goal` along the way planned across the arena (None off one).
    fn way_to(&mut self, me: &Kin, body: &ShipBody, ground: &Terrain, goal: Vec3, dt: f32) -> Option<Vec3> {
        let nav = ground.nav.as_ref()?;
        let flat = |p: Vec3| p.with_y(0.0);
        self.plan_t -= dt;
        let lost = self.path.get(self.path_at).is_none_or(|p| flat(*p).distance(flat(me.pos)) > 8.0);
        if self.plan_t <= 0.0 || lost || flat(goal).distance(flat(self.plan_goal)) > 8.0 {
            self.path = nav.path(me.pos, goal, Nav::room_for(extent(body).2));
            self.path_at = 0;
            self.plan_t = 0.8;
            self.plan_goal = goal;
        }
        // as far along it as it has got, then a few cells on, more at speed
        let near = |p: &Vec3| flat(*p).distance(flat(me.pos));
        while self.path_at + 1 < self.path.len() && near(&self.path[self.path_at + 1]) <= near(&self.path[self.path_at]) {
            self.path_at += 1;
        }
        let ahead = (3.0 + me.vel.length() * 0.3).min(8.0) as usize;
        let p = self.path.get((self.path_at + ahead).min(self.path.len().saturating_sub(1)))?;
        Some(flat(*p - me.pos).normalize_or(me.fwd()))
    }

    /// The way up onto the hill: round to the nearest ramp's line, well back from its foot, then
    /// along that line and up the ramp. (A vehicle that came at a ramp from the side would run off
    /// its edge.) And whether it is on that line, which needs no steering round things, or turning
    /// round where it stands at the line's far end to come along it; and the most speed it should
    /// come in at, slow enough to turn onto the line.
    fn up_the_hill(&mut self, me: &Kin, body: &ShipBody, ground: &Terrain, a: &Arena, dt: f32) -> (Vec3, bool, bool, f32) {
        let p = me.pos.with_y(0.0);
        let far = |w: &Vec3| w.with_y(0.0).distance(p);
        let nearest = (0..a.hill_ways.len()).min_by(|&x, &y| far(&a.hill_ways[x]).total_cmp(&far(&a.hill_ways[y]))).unwrap_or(0);
        // keep to the one it chose, unless another is a good deal nearer
        let k = match self.ramp {
            Some(k) if k < a.hill_ways.len() && far(&a.hill_ways[k]) < far(&a.hill_ways[nearest]) + 15.0 => k,
            _ => nearest,
        };
        self.ramp = Some(k);
        let foot = a.hill_ways[k].with_y(0.0);
        let out = foot.normalize_or(Vec3::Z);
        let along = p.dot(out);
        let side = (p - out * along).length();
        let heading = me.fwd().with_y(0.0).normalize_or(-out);
        let facing = heading.dot(-out);
        // onto the line through a funnel that narrows to the ramp's foot, not heading away from
        // the hill; then on it until it strays out of the funnel. On it, make for a point on it a
        // little nearer the hill (further on, the further off it is), which brings it onto the
        // line and turns it along it.
        // how far back along the line it is clear (of a pit's kickers, a saw): room to come round
        let room = (1..=30).map(|k| k as f32 * 2.0).take_while(|s| {
            let q = out * (foot.length() + s);
            !a.near_pit(q.x, q.z) && !a.by_saw(q.x, q.z)
        });
        let room = room.last().unwrap_or(2.0);
        let past = along - foot.length();
        let width = 6.0 + past.max(0.0) * 0.8;
        // turning in from here, it must neither swing across past the line nor still be turning
        // at the foot: a wide turner comes onto it further out, or from the far end
        let r = self.turn_at(me.vel.dot(me.fwd()).abs().max(4.0));
        let swing = r * (1.0 - facing);
        let closing = (p - out * along).normalize_or_zero().dot(-heading) >= 0.0;
        let fits = !closing || swing < side + 2.0;
        let in_time = past > r * (1.0 - facing * facing).max(0.0).sqrt() - 2.0;
        let funnel = past < room + 2.0 && side < width && facing > -0.2 && fits && in_time;
        // once on it, it stays on while it heads in towards it, however wide it came on (and
        // whether it is moving or stopped for a vent)
        let nearing = (p - out * along).normalize_or_zero().dot(-heading) > 0.2;
        self.on_line = (self.on_line && past < room + 6.0 && (side < width + 3.0 || nearing)) || funnel;
        if self.on_line {
            return ((out * (along - (r * 0.6).max(8.0) - side * 0.5) - p).normalize_or(-out), true, false, f32::MAX);
        }
        // otherwise out to the far end of the line, to turn round there and come along it
        let stage = out * (foot.length() + (room - 2.0).max(6.0));
        if (self.kturn >= 0.0 || p.distance(stage) < 8.0) && facing < 0.9 && self.kturn < 16.0 {
            return (-out, false, true, f32::MAX);
        }
        let way = self.way_to(me, body, ground, stage, dt).unwrap_or((stage - p).normalize_or(-out));
        (way, false, false, 6.0 + p.distance(stage) * 0.3)
    }

    /// How wide it turns at `speed` (the circle's radius, m): the wheels turn less the faster it
    /// goes (see `drive::steer_angle`).
    fn turn_at(&self, speed: f32) -> f32 {
        self.turn_r * (1.0 + speed / 18.0)
    }

    /// The input for server tick `tick`.
    #[allow(clippy::too_many_arguments)]
    pub fn think(&mut self, me: &Kin, body: &ShipBody, recent_dmg: f32, my_team: u16, others: &[Contact], rocks: &[Rock], ground: &Terrain, tick: u32) -> InputCmd {
        let (dt, now) = (DT, tick_time(tick));
        let k = self.skill;
        let fwd = me.fwd();
        let hover_only = !body.movers.is_empty() && body.movers.iter().all(|&i| body.design.blocks[i].ty.drive().is_some_and(|d| d.hover));
        let mut des = fwd;
        let mut drive = 1.0;
        self.t -= dt;
        self.missile_t -= dt;
        self.wob_t -= dt;
        if self.wob_t <= 0.0 {
            self.wob_t = self.rng.range(0.5, 1.2);
            self.wob = self.rng.unit();
        }
        let tgt = others
            .iter()
            .filter(|o| o.team != my_team)
            .min_by(|a, b| a.pos.distance_squared(me.pos).total_cmp(&b.pos.distance_squared(me.pos)));
        let mut cmd = InputCmd { tick, ..Default::default() };
        // somewhere to be: where it is, how near counts, how far off it is
        let goal = self.goal.map(|(g, r)| (g, r, (g - me.pos).with_y(0.0).length()));
        let mut engaged = false;
        // stopped short of a vent on its way, until the vent goes out; or on the straight run up a
        // ramp
        let mut waiting = false;
        let mut straight = false;
        // rams and saws work up close: a vehicle with them goes in rather than circling
        let blocks = &body.design.blocks;
        let brawler = body.stats.melee.iter().any(|&i| blocks[i].ty != BlockType::Flipper);
        let flipper = body.stats.melee.iter().any(|&i| blocks[i].ty == BlockType::Flipper);
        // with no one to shoot, the guns look ahead
        let mut aim = me.pos + fwd * 120.0 + me.up() * 2.0;
        if let Some(t) = tgt {
            let to = t.pos - me.pos;
            let dist = to.length();
            // over the ground and past the walls in between
            let seen = ground.segment_hit(me.pos + me.up() * 1.5, t.pos + Vec3::Y).is_none();
            let lead = intercept(me.pos, me.vel, t.pos, t.vel, body.bolt_speed) + self.wob * dist * k.aim_err;
            let to_lead = (lead - me.pos).normalize_or(fwd);
            if self.state == State::Attack {
                // on its way somewhere it only shoots; there, it turns on what comes to it
                engaged = goal.is_none_or(|(g, r, d)| d <= r && t.pos.distance(g) < r + 12.0);
                // close in (round whatever is in the way), then circle the target at gun range
                if engaged {
                    des = if dist > 70.0 || !seen {
                        self.way_to(me, body, ground, t.pos, dt).unwrap_or(to_lead)
                    } else if brawler || (flipper && dist < 25.0) {
                        // straight at it, to where it is going
                        (t.pos + t.vel * (dist / me.vel.length().max(8.0)).min(1.5) - me.pos).with_y(0.0).normalize_or(fwd)
                    } else {
                        (to_lead + to.cross(Vec3::Y).normalize_or(fwd) * 1.2).normalize_or(fwd)
                    };
                }
                if brawler && dist < 40.0 && seen && heading_to(fwd, to).abs() < 0.3 && me.energy > body.stats.energy_cap * 0.3 {
                    cmd.buttons |= BTN_BOOST;
                }
                // the flipper, once it is right in front
                if flipper && heading_to(fwd, to).abs() < 0.5 && dist < extent(body).0 + t.radius * 0.6 + BlockType::Flipper.melee().unwrap().reach {
                    cmd.buttons |= BTN_FLIPPER;
                }
                if dist > 250.0 && me.energy > body.stats.energy_cap * 0.5 {
                    cmd.buttons |= BTN_BOOST;
                }
                // with somewhere to be it doesn't break away
                self.chase += dt;
                if goal.is_none() && (self.chase > self.chase_max || (recent_dmg > 30.0 && self.rng.f32() < dt * 0.7)) {
                    self.state = State::Evade;
                    self.t = self.rng.range(2.5, 4.0);
                    self.chase = 0.0;
                    self.chase_max = self.rng.range(9.0, 14.0);
                    self.away = (self.rng.unit().with_y(0.0) - to / dist.max(1.0)).normalize_or(-fwd);
                }
                // the guns turn to the lead point by themselves: fire once in range
                if body.gun_range > 0.0 && dist < body.gun_range && seen {
                    cmd.buttons |= BTN_FIRE;
                }
                if body.has_missile {
                    if dist < 800.0 && seen { self.lock += dt } else { self.lock = 0.0 }
                    if self.lock > 1.4 && self.missile_t <= 0.0 {
                        cmd.buttons |= BTN_MISSILE;
                        self.missile_t = self.rng.range(3.0, 6.0);
                    }
                }
                aim = lead;
                cmd.lock = t.id;
            }
        }
        if let Some((g, r, d)) = goal
            && self.state == State::Attack
        {
            let to_g = (g - me.pos).with_y(0.0);
            if d > r && !engaged {
                let (mut round, mut easy) = (false, f32::MAX);
                match ground.arena.as_ref().filter(|a| a.on_hill(g)) {
                    Some(a) => (des, straight, round, easy) = self.up_the_hill(me, body, ground, a, dt),
                    None => des = self.way_to(me, body, ground, g, dt).unwrap_or(to_g.normalize_or(fwd)),
                }
                let v = me.vel.dot(fwd);
                if !round {
                    self.kturn = -1.0;
                }
                let turn = heading_to(fwd, des).abs();
                let (nose, _, half_w) = extent(body);
                if round {
                    // turning round where it stands: stopped first, then two seconds forward with
                    // the wheel one way, two back with it the other, never fast, until it faces
                    // the right way; a go with no room that way (a wall, a saw) is cut short
                    self.kturn = self.kturn.max(0.0);
                    waiting = true;
                    if self.kturn < 1.0 && v.abs() > 1.5 {
                        drive = -v.signum();
                    } else {
                        self.kturn = self.kturn.max(1.0) + dt;
                        let (_, ahead, behind) = self.clear_heading(me, body, ground, fwd, now);
                        let go = |k: f32| if ((k - 1.0) / 2.0) as i32 % 2 == 0 { 1.0 } else { -1.0 };
                        if (go(self.kturn) > 0.0 && ahead < 3.0) || (go(self.kturn) < 0.0 && behind < 3.0) {
                            self.kturn = 3.0 + ((self.kturn - 1.0) / 2.0).floor() * 2.0;
                        }
                        let go = go(self.kturn);
                        drive = if v * go > 4.0 { 0.0 } else { go * 0.7 };
                    }
                } else if straight && turn < 0.6 && vent_ahead(ground, me.pos, des, nose + 4.0 + v.max(0.0) * 0.8, half_w, now) {
                    // a vent across the way (at the foot of a ramp): wait for it to go out rather
                    // than going round, the long way to another
                    waiting = true;
                    drive = if v > 0.5 { -1.0 } else { 0.0 };
                } else if v > 8.0 && turn > 0.6 {
                    // not so fast that it can't make the turn onto a ramp's line
                    drive = if turn > 1.2 { -0.6 } else { 0.0 };
                } else if v > if straight { 7.0 } else { (8.0 + d * 0.4).min(easy) } {
                    // nor so fast along it that it runs off, or can't stop up there
                    drive = if v > 10.0 && straight { -1.0 } else { -0.3 };
                } else if straight && v > 4.0 && turn > 0.5 {
                    // and slow to turn onto it
                    drive = 0.0;
                }
            } else if d <= r {
                // there: fight what comes, kept on it, or with no one to fight sit in the middle;
                // never so fast that it runs off (it rolls a long way if it only lets go)
                let v = me.vel.dot(fwd);
                if engaged {
                    des = des.with_y(0.0).normalize_or(fwd) + to_g / r.max(1.0) * 0.9;
                    if v > 5.0 {
                        drive = -0.6;
                    }
                } else {
                    des = to_g.normalize_or(fwd);
                    waiting = true;
                    drive = if d > 2.5 && v < 3.0 {
                        0.5
                    } else if v > 0.5 {
                        -0.6
                    } else if v < -0.5 {
                        0.6
                    } else {
                        0.0
                    };
                }
            }
        }
        match self.state {
            State::Evade => {
                des = self.away;
                if self.t <= 0.0 {
                    self.state = State::Attack;
                }
            }
            State::Reverse => {
                // on hover pads reverse only creeps back: turn on the spot instead
                drive = if hover_only { 0.0 } else { -1.0 };
                if self.t <= 0.0 {
                    self.state = State::Attack;
                }
            }
            State::Attack => {}
        }
        // rocks in the way: steer round them
        let look = me.vel.length().max(10.0) * 2.5;
        for a in rocks {
            let rel = (a.pos - me.pos).with_y(0.0);
            let ahead = rel.dot(fwd);
            if ahead < 0.0 || ahead > look + a.r {
                continue;
            }
            let perp = rel - fwd.with_y(0.0) * ahead;
            let pl = perp.length();
            let safe = a.r + body.radius + 4.0;
            if pl < safe {
                let push = (1.0 - pl / safe) * 3.0;
                if pl < 0.01 { des += fwd.cross(Vec3::Y) * push } else { des -= perp * (push / pl) }
            }
        }
        // spread out from teammates (but not off a ramp's line)
        for o in others.iter().filter(|o| o.team == my_team && !straight) {
            let rel = (me.pos - o.pos).with_y(0.0);
            let d = rel.length();
            if d > 0.01 && d < 20.0 + body.radius + o.radius {
                des += rel * (0.9 / d);
            }
        }
        // out in the open, keep off the edge (in an arena its walls are just more walls)
        let dl = me.pos.with_y(0.0).length();
        if ground.arena.is_none() && dl > ARENA_R * 0.75 {
            des -= me.pos.with_y(0.0) * (1.6 / dl);
        }
        // walls, cover, a step too high, a pit: turn to the nearest way that's clear
        if !goal.is_some_and(|(_, r, d)| d > r) {
            self.ramp = None;
            self.on_line = false;
            self.kturn = -1.0;
        }
        if ground.arena.is_some() && !waiting && !straight {
            let (way, ahead, behind) = self.clear_heading(me, body, ground, des, now);
            self.turning = self.turning.filter(|&(w, t)| t > 0.0 && heading_to(fwd, w).abs() > 0.3).map(|(w, t)| (w, t - dt));
            let way = self.turning.map_or(way, |(w, _)| w);
            des = way;
            let turn = heading_to(fwd, way).abs();
            if self.state != State::Reverse && ahead < ROOM && turn > 0.5 && (hover_only || behind > 2.0) {
                // no room to turn that far: back up with the wheel the other way, then go
                self.state = State::Reverse;
                self.t = 1.5;
                self.turning.get_or_insert((way, 4.0));
            } else if self.state == State::Reverse && ((behind < 1.0 && !hover_only && me.vel.dot(fwd) < -1.0) || (self.t < 0.7 && turn < 0.5 && ahead > ROOM)) {
                // pointing the right way with room to go, or about to back into something (one
                // that is wedged in still tries, as the wheel turned the other way may free it)
                self.state = State::Attack;
            }
        }
        // stuck: pushing with the engine but going nowhere, not even slowly speeding up
        let v = me.vel.length();
        if self.state != State::Reverse && !waiting && v < 1.5 && me.up().y > 0.5 {
            if self.stuck == 0.0 || v < self.stuck_v {
                self.stuck_v = v;
            } else if v > self.stuck_v + 0.6 {
                self.stuck = 0.0;
                self.stuck_v = v;
            }
            self.stuck += dt;
            if self.stuck > 1.0 {
                self.state = State::Reverse;
                self.t = self.rng.range(1.2, 2.0);
                self.stuck = 0.0;
                if ground.arena.is_some() {
                    self.turning.get_or_insert((des.with_y(0.0).normalize_or(fwd), 4.0));
                }
            }
        } else {
            self.stuck = 0.0;
        }
        // the gadgets: smoke when a missile is after it, mines for whoever is on its tail (or
        // closing on the place it holds), a jump when it is stuck
        if self.hunted && body.gadget(Gadget::Smoke) > 0 {
            cmd.buttons |= BTN_SMOKE;
        }
        if body.gadget(Gadget::Mine) > 0
            && let Some(t) = tgt
        {
            let to = t.pos - me.pos;
            let holding = goal.is_some_and(|(_, r, d)| d <= r);
            if (to.length() < 30.0 && heading_to(fwd, to).abs() > 2.2) || (holding && to.length() < 60.0) {
                cmd.buttons |= BTN_MINE;
            }
        }
        if body.gadget(Gadget::Jump) > 0 && self.stuck > 0.6 {
            cmd.buttons |= BTN_JUMP;
        }
        // over, or stuck tipped up for a moment: right it
        if me.up().y < 0.8 && me.vel.length() < 3.0 {
            self.tipped += dt;
        } else if me.up().y > 0.95 {
            self.tipped = 0.0;
        }
        if me.up().y < 0.4 || self.tipped > 1.0 {
            cmd.buttons |= BTN_FLIP;
        }
        let mut h = heading_to(fwd, des);
        if h.abs() > 2.6 {
            h = h.abs() * self.last_turn;
        } else if h.abs() > 0.2 {
            self.last_turn = h.signum();
        }
        let mut steer = (h * 2.5).clamp(-1.0, 1.0);
        // how tight it turns, from how it turned with the wheel hard over just now
        let ahead = me.vel.dot(fwd).abs();
        if self.steer.abs() > 0.8 && ahead > 2.0 && me.ang.y.abs() > 0.02 {
            let r = (ahead / me.ang.y.abs()).min(40.0) / (1.0 + ahead / 18.0);
            self.turn_r += (r - self.turn_r) * (dt * 2.0).min(1.0);
        }
        // backing up (not braking) on wheels the steering turns the other way; on hover pads it
        // doesn't
        if drive < 0.0 && !hover_only && (self.state == State::Reverse || me.vel.dot(fwd) < 0.0) {
            steer = -steer;
        }
        self.steer += (steer - self.steer) * (dt * k.react).min(1.0);
        cmd.drive = q_axis(drive);
        cmd.steer = q_axis(self.steer);
        cmd.aim = quantize_aim(aim);
        if tgt.is_none() {
            cmd.lock = NO_LOCK;
        }
        cmd
    }
}
