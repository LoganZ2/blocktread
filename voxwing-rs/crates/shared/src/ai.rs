use crate::ARENA_R;
use crate::flight::{BTN_BOOST, BTN_FIRE, BTN_MISSILE, InputCmd, Kin, NO_LOCK, q_axis};
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
    Evade,
}

/// Dogfighting bot: chase and lead the nearest enemy, break off now and then, dodge rocks.
#[derive(Clone, Debug)]
pub struct AiPilot {
    pub skill: Skill,
    state: State,
    t: f32,
    evade: Vec3,
    wob: Vec3,
    wob_t: f32,
    lock: f32,
    missile_t: f32,
    chase: f32,
    chase_max: f32,
    yaw: f32,
    pitch: f32,
    roll: f32,
    rng: Rng,
}

impl AiPilot {
    pub fn new(skill: Skill, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        AiPilot {
            skill,
            state: State::Attack,
            t: 0.0,
            evade: Vec3::Z,
            wob: Vec3::ZERO,
            wob_t: 0.0,
            lock: 0.0,
            missile_t: rng.range(2.0, 5.0),
            chase: 0.0,
            chase_max: rng.range(7.0, 11.0),
            yaw: 0.0,
            pitch: 0.0,
            roll: 0.0,
            rng,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn think(&mut self, me: &Kin, body: &ShipBody, recent_dmg: f32, my_team: u16, others: &[Contact], rocks: &[Asteroid], dt: f32) -> InputCmd {
        let k = self.skill;
        let st = &body.stats;
        let fwd = me.fwd();
        let mut des = fwd;
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
        let mut cmd = InputCmd { throttle: 150, ..Default::default() };
        let mut aim_dist = 320.0;
        if let Some(t) = tgt {
            let to = t.pos - me.pos;
            let dist = to.length();
            let lead = intercept(me.pos, me.vel, t.pos, t.vel, body.bolt_speed) + self.wob * dist * k.aim_err;
            let to_lead = (lead - me.pos).normalize_or(fwd);
            match self.state {
                State::Attack => {
                    des = to_lead;
                    cmd.throttle = if dist > 420.0 { 255 } else if dist < 140.0 { 140 } else { 217 };
                    if dist > 750.0 && me.energy > st.energy_cap * 0.5 {
                        cmd.buttons |= BTN_BOOST;
                    }
                    self.chase += dt;
                    if dist < 38.0 + body.radius + t.radius || (recent_dmg > 30.0 && self.rng.f32() < dt * 0.9) {
                        self.state = State::Evade;
                        self.t = self.rng.range(1.4, 2.6);
                        self.chase = 0.0;
                        self.evade = (self.rng.unit() - to / dist.max(1.0) * 1.3).normalize_or(Vec3::Y);
                    } else if self.chase > self.chase_max {
                        // break off and extend past the target, then come back around
                        self.state = State::Evade;
                        self.t = self.rng.range(2.2, 3.6);
                        self.chase = 0.0;
                        self.chase_max = self.rng.range(7.0, 11.0);
                        self.evade = (self.rng.unit() + fwd * 1.6).normalize_or(fwd);
                    }
                    let ang = fwd.angle_between(to_lead);
                    let fire = body.gun_range > 0.0 && dist < body.gun_range && ang < k.cone * 0.6 + (t.radius * 0.9).atan2(dist.max(1.0));
                    if fire {
                        cmd.buttons |= BTN_FIRE;
                    }
                    if body.has_missile {
                        if ang < 0.35 && dist < 800.0 { self.lock += dt } else { self.lock = 0.0 }
                        if self.lock > 1.4 && self.missile_t <= 0.0 {
                            cmd.buttons |= BTN_MISSILE;
                            self.missile_t = self.rng.range(3.0, 6.0);
                        }
                    }
                    aim_dist = (lead - me.pos).length();
                    cmd.lock = t.id;
                }
                State::Evade => {
                    des = self.evade;
                    cmd.throttle = 255;
                    if me.energy > 20.0 {
                        cmd.buttons |= BTN_BOOST;
                    }
                    if self.t <= 0.0 {
                        self.state = State::Attack;
                    }
                }
            }
        }
        // rocks ahead
        let look = me.vel.length().max(30.0) * 2.2;
        for a in rocks {
            let rel = a.pos - me.pos;
            let ahead = rel.dot(fwd);
            if ahead < -a.r || ahead > look + a.r {
                continue;
            }
            let perp = rel - fwd * ahead;
            let pl = perp.length();
            let safe = a.r + body.radius + 14.0;
            if pl < safe {
                let push = (1.0 - pl / safe) * 3.5;
                if pl < 0.01 { des += me.up() * push } else { des -= perp * (push / pl) }
            }
        }
        // spread out from teammates
        for o in others.iter().filter(|o| o.team == my_team) {
            let rel = me.pos - o.pos;
            let d = rel.length();
            if d > 0.01 && d < 35.0 + body.radius + o.radius {
                des += rel * (0.9 / d);
            }
        }
        let dl = me.pos.length();
        if dl > ARENA_R * 0.8 {
            des -= me.pos * (1.6 / dl);
        }
        let des = des.normalize_or(fwd);
        let l = me.rot.inverse() * des;
        // angle controller: command a turn rate proportional to the remaining angle so aim settles
        let off = l.x.hypot(l.y).atan2(-l.z);
        let lift_pen = if st.lift_ratio < 1.0 { st.lift_ratio.max(0.3) } else { 1.0 };
        let (tp, ty) = ((st.pitch_rate * lift_pen).max(0.2), (st.yaw_rate * lift_pen).max(0.16));
        let (yaw, pitch) = if off > 1.2 {
            let m = l.x.hypot(l.y);
            if m < 0.05 { (0.0, 1.0) } else { (l.x / m, l.y / m) }
        } else {
            ((l.x.atan2(-l.z) * 4.5 / ty).clamp(-1.0, 1.0), (l.y.atan2(-l.z) * 4.5 / tp).clamp(-1.0, 1.0))
        };
        let bank = ((off - 0.35) / 0.5).clamp(0.0, 1.0);
        let roll = (l.x.atan2(l.y) * 1.1).clamp(-1.0, 1.0) * bank - (me.ang.z / st.roll_rate.max(0.5)).clamp(-1.0, 1.0) * 0.3 * (1.0 - bank);
        let r = (dt * k.react).min(1.0);
        self.yaw += (yaw - self.yaw) * r;
        self.pitch += (pitch - self.pitch) * r;
        self.roll += (roll - self.roll) * r;
        cmd.yaw = q_axis(self.yaw);
        cmd.pitch = q_axis(self.pitch);
        cmd.roll = q_axis(self.roll);
        cmd.aim_dist = aim_dist.clamp(30.0, 2000.0) as u16;
        if tgt.is_none() {
            cmd.lock = NO_LOCK;
        }
        cmd
    }
}
