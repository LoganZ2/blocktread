use crate::ARENA_R;
use crate::flight::{BTN_BOOST, BTN_FIRE, BTN_FLIP, BTN_MISSILE, InputCmd, Kin, NO_LOCK, q_axis, quantize_aim};
use crate::rng::Rng;
use crate::ship::ShipBody;
use crate::weapons::intercept;
use crate::world::Asteroid;
use glam::Vec3;

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
    pub fn for_wave(w: u32) -> Skill {
        let w = w as f32;
        Skill { aim_err: (0.04 - w * 0.005).max(0.012), cone: 0.05, react: 5.0 + w * 0.6, dmg_mul: (0.42 + w * 0.1).min(1.0) }
    }
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
/// round rocks, back out when stuck and right itself when it rolls over.
#[derive(Clone, Debug)]
pub struct AiPilot {
    pub skill: Skill,
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
    steer: f32,
    rng: Rng,
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
            steer: 0.0,
            rng,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn think(&mut self, me: &Kin, body: &ShipBody, recent_dmg: f32, my_team: u16, others: &[Contact], rocks: &[Asteroid], dt: f32) -> InputCmd {
        let k = self.skill;
        let fwd = me.fwd();
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
        let mut cmd = InputCmd::default();
        // with no one to shoot, the guns look ahead
        let mut aim = me.pos + fwd * 120.0 + me.up() * 2.0;
        if let Some(t) = tgt {
            let to = t.pos - me.pos;
            let dist = to.length();
            let lead = intercept(me.pos, me.vel, t.pos, t.vel, body.bolt_speed) + self.wob * dist * k.aim_err;
            let to_lead = (lead - me.pos).normalize_or(fwd);
            if self.state == State::Attack {
                // close in, then circle the target at gun range
                des = if dist > 70.0 { to_lead } else { (to_lead + to.cross(Vec3::Y).normalize_or(fwd) * 1.2).normalize_or(fwd) };
                if dist > 250.0 && me.energy > body.stats.energy_cap * 0.5 {
                    cmd.buttons |= BTN_BOOST;
                }
                self.chase += dt;
                if self.chase > self.chase_max || (recent_dmg > 30.0 && self.rng.f32() < dt * 0.7) {
                    self.state = State::Evade;
                    self.t = self.rng.range(2.5, 4.0);
                    self.chase = 0.0;
                    self.chase_max = self.rng.range(9.0, 14.0);
                    self.away = (self.rng.unit().with_y(0.0) - to / dist.max(1.0)).normalize_or(-fwd);
                }
                // the guns turn to the lead point by themselves: fire once in range
                if body.gun_range > 0.0 && dist < body.gun_range {
                    cmd.buttons |= BTN_FIRE;
                }
                if body.has_missile {
                    if dist < 800.0 { self.lock += dt } else { self.lock = 0.0 }
                    if self.lock > 1.4 && self.missile_t <= 0.0 {
                        cmd.buttons |= BTN_MISSILE;
                        self.missile_t = self.rng.range(3.0, 6.0);
                    }
                }
                aim = lead;
                cmd.lock = t.id;
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
                drive = -1.0;
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
        // spread out from teammates
        for o in others.iter().filter(|o| o.team == my_team) {
            let rel = (me.pos - o.pos).with_y(0.0);
            let d = rel.length();
            if d > 0.01 && d < 20.0 + body.radius + o.radius {
                des += rel * (0.9 / d);
            }
        }
        let dl = me.pos.with_y(0.0).length();
        if dl > ARENA_R * 0.75 {
            des -= me.pos.with_y(0.0) * (1.6 / dl);
        }
        // stuck: pushing with the engine but going nowhere
        if self.state != State::Reverse && me.vel.length() < 1.5 && me.up().y > 0.5 {
            self.stuck += dt;
            if self.stuck > 1.5 {
                self.state = State::Reverse;
                self.t = self.rng.range(1.2, 2.0);
                self.stuck = 0.0;
            }
        } else {
            self.stuck = 0.0;
        }
        if me.up().y < 0.4 {
            cmd.buttons |= BTN_FLIP;
        }
        let mut steer = (heading_to(fwd, des) * 2.5).clamp(-1.0, 1.0);
        if drive < 0.0 {
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
