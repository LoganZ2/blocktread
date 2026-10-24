use crate::balance::balance;
use crate::blocks::{Block, BlockType, Design};
use crate::grid::{DIRS6, Occupancy, in_grid};
use glam::{IVec3, Vec2, Vec3};

pub const MAX_BLOCKS: usize = 380;

// Turning. Each axis turns at a rate set by its control torque over its moment of inertia, so
// where blocks sit matters: weight far from the centre of mass makes that axis sluggish, wings
// steer harder the longer their lever arm, gyros push the same wherever they are.


/// Turn-rate curve: `scale * (0.6 + 0.55 * sqrt(AGILITY * torque / inertia))`. The square root
/// gives diminishing returns, the 0.6 keeps even sluggish ships flyable.
const AGILITY: f32 = 11.0;
/// Yaw turns slower than pitch, roll faster.
const AXIS_SCALE: Vec3 = Vec3::new(1.0, 0.85, 1.25);
/// Engine torque (thrust x offset in metres) in control-torque units.
const THRUST_LEVER: f32 = 1.0;
/// The flight computer trims out a thrust line up to this far (metres) from the centre of mass,
/// so a ship that is a little top-heavy flies straight; only the offset beyond it pulls the nose.
/// Lift off to one side is trimmed the same way.
pub const AUTO_TRIM: f32 = 0.25;
/// Lift torque (total lift x sideways offset in metres) in control-torque units.
const LIFT_LEVER: f32 = 0.14;
/// Lopsided lift can use up at most this share of the roll authority, so a ship that lost a
/// whole wing still answers the stick.
const MAX_ROLL_DRIFT: f32 = 0.75;

/// Derived flight and combat numbers for a set of (alive) blocks. Indices refer to `Design::blocks`.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub n: usize,
    pub mass: f32,
    pub hp: f32,
    /// Build cost (see `balance.ron`): the blocks' costs, weapons dearer the more there are.
    pub cost: f32,
    pub counts: [u16; BlockType::ALL.len()],
    pub com: Vec3,
    pub cockpits: Vec<usize>,
    pub engines: Vec<usize>,
    pub eng_blocked: Vec<usize>,
    pub weapons: Vec<usize>,
    pub wpn_blocked: Vec<usize>,
    pub wings: Vec<usize>,
    pub wings_buried: Vec<usize>,
    /// Fins with air on at least one side, and those boxed in on both.
    pub fins: Vec<usize>,
    pub fins_buried: Vec<usize>,
    pub left: u32,
    pub right: u32,
    pub w_min_z: i32,
    pub w_max_z: i32,
    pub radius: f32,
    pub thrust: f32,
    pub twr: f32,
    pub lift_ratio: f32,
    /// Moment of inertia about the centre of mass: x = pitch, y = yaw, z = roll.
    pub inertia: Vec3,
    /// Control torque on each axis (same order).
    pub control: Vec3,
    /// Top turn rates (rad/s).
    pub pitch_rate: f32,
    pub yaw_rate: f32,
    pub roll_rate: f32,
    /// Roll rate (rad/s, positive rolls right) that lift off to one side of the centre of mass
    /// forces on the ship.
    pub roll_drift: f32,
    /// Untrimmed engine torque at full throttle as a fraction of the control torque: x pitches the
    /// nose up, y yaws it right. Non-zero when the engines' thrust line misses the centre of mass.
    pub thrust_torque: Vec2,
    pub max_speed: f32,
    pub accel: f32,
    pub energy_cap: f32,
    pub energy_regen: f32,
    pub shield_cap: f32,
    pub shield_regen: f32,
    /// Damage per second with every weapon firing and a full battery.
    pub dps: f32,
    /// Damage per second once the battery is empty: energy weapons fire only as fast as the
    /// reactors refill it.
    pub dps_sustained: f32,
    /// Energy per second the guns use at full fire.
    pub energy_draw: f32,
    /// Seconds of full fire before the battery runs dry, if the reactors can't keep up.
    pub fire_endurance: Option<f32>,
}

impl Stats {
    pub fn count(&self, t: BlockType) -> u16 {
        self.counts[t.index()]
    }

    /// Turn rates (rad/s, x = pitch up, y = yaw right) the engines force on the ship at `push`
    /// (throttle, more while boosting).
    pub fn thrust_drift(&self, push: f32) -> Vec2 {
        self.thrust_torque * push * Vec2::new(self.pitch_rate, self.yaw_rate)
    }
}

pub fn occupancy(blocks: &[Block], alive: &[bool]) -> Occupancy {
    let mut occ = Occupancy::default();
    for (i, b) in blocks.iter().enumerate() {
        if alive[i] {
            occ.set(b.pos(), Some(i));
        }
    }
    occ
}

pub fn compute_stats(blocks: &[Block], alive: &[bool]) -> Stats {
    let occ = occupancy(blocks, alive);
    let mut s = Stats::default();
    let (mut lo, mut hi) = (IVec3::splat(i32::MAX), IVec3::splat(i32::MIN));
    let mut c = Vec3::ZERO;
    for (i, b) in blocks.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        let info = b.ty.info();
        s.n += 1;
        s.mass += info.mass;
        s.hp += info.hp;
        c += b.pos().as_vec3() * info.mass;
        s.counts[b.ty.index()] += 1;
        if b.ty == BlockType::Cockpit {
            s.cockpits.push(i);
        }
        lo = lo.min(b.pos());
        hi = hi.max(b.pos());
    }
    s.com = if s.mass > 0.0 { c / s.mass } else { Vec3::ZERO };
    let within = |p: IVec3| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y && p.z >= lo.z && p.z <= hi.z;
    let clear = |b: &Block, d: IVec3| {
        let mut p = b.pos() + d;
        while within(p) {
            if occ.has(p) {
                return false;
            }
            p += d;
        }
        true
    };
    for (i, b) in blocks.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        if b.ty.thrust() > 0.0 {
            if clear(b, IVec3::Z) { s.engines.push(i) } else { s.eng_blocked.push(i) }
        } else if b.ty.weapon().is_some() {
            if clear(b, IVec3::NEG_Z) { s.weapons.push(i) } else { s.wpn_blocked.push(i) }
        } else if b.ty == BlockType::Wing {
            let buried = occ.has(b.pos() + IVec3::Y) && occ.has(b.pos() - IVec3::Y);
            if buried { s.wings_buried.push(i) } else { s.wings.push(i) }
        } else if b.ty == BlockType::Fin {
            // a vertical surface: it needs air on its sides
            let buried = occ.has(b.pos() + IVec3::X) && occ.has(b.pos() - IVec3::X);
            if buried { s.fins_buried.push(i) } else { s.fins.push(i) }
        }
    }
    s.w_min_z = i32::MAX;
    s.w_max_z = i32::MIN;
    for &w in &s.wings {
        let b = &blocks[w];
        let side = b.x as f32 - s.com.x;
        if side < -0.5 {
            s.left += 1;
        } else if side > 0.5 {
            s.right += 1;
        }
        s.w_min_z = s.w_min_z.min(b.z as i32);
        s.w_max_z = s.w_max_z.max(b.z as i32);
    }
    let mut r: f32 = 0.0;
    for (i, b) in blocks.iter().enumerate() {
        if alive[i] {
            r = r.max(b.pos().as_vec3().distance(s.com));
        }
    }
    s.radius = r + 0.9;
    s.thrust = s.engines.iter().map(|&e| blocks[e].ty.thrust()).sum();
    s.twr = if s.mass > 0.0 { s.thrust / s.mass } else { 0.0 };
    let lift: f32 = s.wings.iter().map(|&w| blocks[w].ty.info().lift).sum();
    s.lift_ratio = if s.mass > 0.0 { lift / s.mass } else { 0.0 };

    // every block a solid unit cube: point mass at its centre plus its own m/6 about each axis
    for (i, b) in blocks.iter().enumerate() {
        if alive[i] {
            let r = b.pos().as_vec3() - s.com;
            let m = b.ty.info().mass;
            s.inertia += m * (Vec3::new(r.y * r.y + r.z * r.z, r.x * r.x + r.z * r.z, r.x * r.x + r.y * r.y) + 1.0 / 6.0);
        }
    }
    // wings: a base share each, plus their fore-aft arm for pitch and yaw (tails steer) and their
    // sideways arm for roll (long wings roll)
    // gyros: the same torque wherever they sit
    s.control = Vec3::splat(blocks.iter().enumerate().filter(|&(i, _)| alive[i]).map(|(_, b)| b.ty.info().gyro).sum());
    for &w in &s.wings {
        let r = blocks[w].pos().as_vec3() - s.com;
        s.control += Vec3::new(1.0 + r.z.abs(), 1.0 + r.z.abs(), 1.0 + r.x.abs());
    }
    // fins are rudders: mostly yaw, by their fore-aft arm; a little pitch, and roll by their height
    for &f in &s.fins {
        let r = blocks[f].pos().as_vec3() - s.com;
        s.control += Vec3::new(0.5 + 0.5 * r.z.abs(), 2.0 + 2.0 * r.z.abs(), 0.5 + 0.5 * r.y.abs());
    }
    // attitude thrusters push by their distance from each axis: the farther out, the stronger
    for (i, b) in blocks.iter().enumerate() {
        let rcs = b.ty.info().rcs;
        if alive[i] && rcs > 0.0 {
            let r = b.pos().as_vec3() - s.com;
            s.control += rcs * Vec3::new(r.y.hypot(r.z), r.x.hypot(r.z), r.x.hypot(r.y));
        }
    }
    let rate = |axis: usize| AXIS_SCALE[axis] * (0.6 + 0.55 * (AGILITY * s.control[axis] / s.inertia[axis].max(0.5)).sqrt());
    s.pitch_rate = rate(0).clamp(0.35, 2.4);
    s.yaw_rate = rate(1).clamp(0.3, 2.0);
    s.roll_rate = rate(2).clamp(0.6, 4.3);
    // thrust along -Z through a line r off the centre of mass turns the ship by r x F: below it
    // lifts the nose, right of it swings the nose left
    if s.thrust > 0.0 {
        let line = s.engines.iter().map(|&e| blocks[e].pos().as_vec3() * blocks[e].ty.thrust()).sum::<Vec3>() / s.thrust - s.com;
        let off = |d: f32| (d.abs() - AUTO_TRIM).max(0.0).copysign(d);
        let torque = Vec2::new(-off(line.y), -off(line.x)) * s.thrust * THRUST_LEVER;
        s.thrust_torque = torque / Vec2::new(s.control.x, s.control.y).max(Vec2::ONE);
    }

    // lift centred right of the centre of mass raises the right side, rolling the ship left (and
    // the heavy side drops). No symmetry rule: it just flies that way.
    if lift > 0.0 {
        let side = s.wings.iter().map(|&w| blocks[w].x as f32 * blocks[w].ty.info().lift).sum::<f32>() / lift - s.com.x;
        let off = (side.abs() - AUTO_TRIM).max(0.0).copysign(side);
        let share = (-off * lift * LIFT_LEVER / s.control.z.max(1.0)).clamp(-MAX_ROLL_DRIFT, MAX_ROLL_DRIFT);
        s.roll_drift = share * s.roll_rate;
    }
    s.max_speed = if s.thrust > 0.0 { (28.0 + 52.0 * s.twr.sqrt()).min(125.0) } else { 0.0 };
    s.accel = (18.0 + 30.0 * s.twr).min(90.0);
    let bal = balance();
    let (mut energy_cap, mut energy_regen, mut shield_cap, mut shield_regen) = (bal.base_energy_cap, bal.base_energy_regen, 0.0, 0.0);
    for (i, b) in blocks.iter().enumerate() {
        if alive[i] {
            let info = b.ty.info();
            energy_cap += info.energy_cap;
            energy_regen += info.energy_regen;
            shield_cap += info.shield_cap;
            shield_regen += info.shield_regen;
        }
    }
    (s.energy_cap, s.energy_regen, s.shield_cap, s.shield_regen) = (energy_cap, energy_regen, shield_cap, shield_regen);
    // cost: each weapon beyond the first makes all of them `weapon_cost_growth` dearer
    let (mut plain, mut guns, mut n_guns) = (0.0, 0.0, 0u32);
    for (i, b) in blocks.iter().enumerate() {
        if alive[i] {
            let c = b.ty.info().cost;
            if b.ty.weapon().is_some() {
                guns += c;
                n_guns += 1;
            } else {
                plain += c;
            }
        }
    }
    s.cost = plain + guns * (1.0 + bal.weapon_cost_growth * n_guns.saturating_sub(1) as f32);
    s.hp += s.cockpits.len() as f32 * bal.cockpit_hp_per_cost * s.cost;
    let mut energy_dps = 0.0;
    for &w in &s.weapons {
        let spec = blocks[w].ty.weapon().unwrap();
        let dps = spec.rate * spec.dmg * spec.pellets as f32 * if spec.homing { 0.6 } else { 1.0 };
        s.dps += dps;
        if spec.energy > 0.0 {
            s.energy_draw += spec.rate * spec.energy;
            energy_dps += dps;
        }
    }
    // with the battery empty, energy weapons fire as fast as the reactors refill it
    let keep_up = if s.energy_draw > s.energy_regen { s.energy_regen / s.energy_draw } else { 1.0 };
    s.dps_sustained = s.dps - energy_dps * (1.0 - keep_up);
    s.fire_endurance = (s.energy_draw > s.energy_regen).then(|| s.energy_cap / (s.energy_draw - s.energy_regen));
    s
}

/// Blocks not face-connected to `root`.
pub fn disconnected(blocks: &[Block], alive: &[bool], root: usize) -> Vec<usize> {
    let occ = occupancy(blocks, alive);
    let mut seen = vec![false; blocks.len()];
    seen[root] = true;
    let mut stack = vec![root];
    while let Some(i) = stack.pop() {
        for d in DIRS6 {
            if let Some(j) = occ.get(blocks[i].pos() + d)
                && !seen[j]
            {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    (0..blocks.len()).filter(|&i| alive[i] && !seen[i]).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Err,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub level: Level,
    pub title: &'static str,
    pub msg: String,
    pub cells: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub stats: Stats,
    pub checks: Vec<Check>,
    pub errors: usize,
    pub warns: usize,
}

impl Analysis {
    pub fn ok(&self) -> bool {
        self.errors == 0
    }
    pub fn first_error(&self) -> Option<&Check> {
        self.checks.iter().find(|c| c.level == Level::Err)
    }
}

fn fmt(n: f32) -> String {
    if (n - n.round()).abs() < 1e-4 { format!("{}", n.round() as i64) } else { format!("{n:.1}") }
}

/// Why a block of type `t` may not go into `cell`, given the current blocks. Barrels stick one
/// cell forward (-z) and engine nozzles one cell back (+z); those cells must stay empty.
pub fn place_reason(blocks: &[Block], cell: IVec3, t: BlockType) -> Option<String> {
    if !in_grid(cell) {
        return Some("超出搭建范围".into());
    }
    let at = |p: IVec3| blocks.iter().find(|b| b.pos() == p);
    if at(cell).is_some() {
        return Some("这里已经有方块了".into());
    }
    let behind = at(cell + IVec3::Z);
    let ahead = at(cell - IVec3::Z);
    if let Some(b) = behind
        && b.ty.weapon().is_some()
    {
        return Some(format!("这一格是{}的枪管位置，不能放方块", b.ty.info().name));
    }
    if let Some(a) = ahead
        && a.ty.thrust() > 0.0
    {
        return Some(format!("这一格是{}尾喷口的位置，不能放方块", a.ty.info().name));
    }
    if t.weapon().is_some() && ahead.is_some() {
        return Some(format!("{}的枪管前方已经有方块，放不下", t.info().name));
    }
    if t.thrust() > 0.0 && behind.is_some() {
        return Some(format!("{}尾喷口后方已经有方块，放不下", t.info().name));
    }
    None
}

pub fn analyze(design: &Design) -> Analysis {
    let blocks = &design.blocks;
    let alive = vec![true; blocks.len()];
    let s = compute_stats(blocks, &alive);
    let mut checks: Vec<Check> = Vec::new();
    let mut add = |level: Level, title: &'static str, msg: String, cells: Vec<usize>| checks.push(Check { level, title, msg, cells });

    // structural sanity (matters for designs received over the network)
    let outside: Vec<usize> = (0..blocks.len()).filter(|&i| !in_grid(blocks[i].pos())).collect();
    if !outside.is_empty() {
        add(Level::Err, "范围", format!("{} 个方块超出了搭建范围", outside.len()), outside);
    }
    let mut dup = Vec::new();
    for i in 0..blocks.len() {
        if blocks[..i].iter().any(|b| b.pos() == blocks[i].pos()) {
            dup.push(i);
        }
    }
    if !dup.is_empty() {
        add(Level::Err, "重叠", format!("{} 个方块和别的方块挤在同一格", dup.len()), dup);
    }

    if s.cockpits.is_empty() {
        add(Level::Err, "驾驶舱", "缺少驾驶舱：每架飞机需要且只能有 1 个".into(), vec![]);
    } else if s.cockpits.len() > 1 {
        add(Level::Err, "驾驶舱", format!("只能有 1 个驾驶舱（现在有 {} 个）", s.cockpits.len()), s.cockpits.clone());
    } else {
        let c = &blocks[s.cockpits[0]];
        let occ = occupancy(blocks, &alive);
        let clear_dir = |d: IVec3| {
            let mut p = c.pos() + d;
            while in_grid(p) {
                if occ.has(p) {
                    return false;
                }
                p += d;
            }
            true
        };
        if clear_dir(IVec3::Y) || clear_dir(IVec3::NEG_Z) {
            add(Level::Ok, "驾驶舱", "视野通畅".into(), vec![]);
        } else {
            add(Level::Err, "驾驶舱视野", "顶部和正前方都被挡住了，飞行员看不见路。清空其中一个方向".into(), vec![s.cockpits[0]]);
        }
    }
    if s.cockpits.len() == 1 {
        let root = s.cockpits[0];
        let dis = disconnected(blocks, &alive, root);
        if dis.is_empty() {
            add(Level::Ok, "结构连接", format!("{} 个方块全部相连", blocks.len()), vec![]);
        } else {
            add(Level::Err, "结构连接", format!("{} 个方块没有连到驾驶舱（方块之间要面贴面）", dis.len()), dis);
        }
        let occ = occupancy(blocks, &alive);
        let nb = DIRS6.iter().filter(|&&d| occ.has(blocks[root].pos() + d)).count();
        if nb == 1 && blocks.len() > 4 {
            add(Level::Warn, "结构", "驾驶舱只有 1 个面和机身相连，那块被打掉整架飞机就会散架。多加几个支撑".into(), vec![root]);
        }
    }
    if s.count(BlockType::Engine) + s.count(BlockType::Thruster) == 0 {
        add(Level::Err, "引擎", "缺少引擎：至少需要 1 台".into(), vec![]);
    } else if s.engines.is_empty() {
        add(Level::Err, "引擎", "所有引擎的尾喷口都被挡住了，引擎正后方必须空着".into(), s.eng_blocked.clone());
    } else if s.twr < 0.35 {
        add(Level::Err, "推力", format!("推力不足：推重比 {:.2}，至少 0.35。加引擎或减重", s.twr), s.engines.clone());
    } else {
        add(Level::Ok, "引擎", format!("{} 台 · 推重比 {:.2}", s.engines.len(), s.twr), vec![]);
    }
    if !s.eng_blocked.is_empty() && !s.engines.is_empty() {
        add(Level::Warn, "引擎", format!("{} 台引擎尾喷口被挡住，不产生推力", s.eng_blocked.len()), s.eng_blocked.clone());
    }
    let drift = s.thrust_drift(1.0);
    if drift.length() > 2f32.to_radians() {
        let mut dir = String::new();
        if drift.y.abs() > 1e-3 {
            dir.push(if drift.y > 0.0 { '右' } else { '左' });
        }
        if drift.x.abs() > 1e-3 {
            dir.push(if drift.x > 0.0 { '上' } else { '下' });
        }
        add(Level::Warn, "推力偏心", format!("引擎的推力中心没对准重心，全速时机头会往{dir}偏 {:.0} 度/秒，加速时更明显。挪动引擎或配重，让推力中心和重心对齐", drift.length().to_degrees()), s.engines.clone());
    }
    if s.count(BlockType::Wing) == 0 {
        add(Level::Err, "机翼", "缺少机翼：至少需要 1 片".into(), vec![]);
    } else if s.wings.is_empty() {
        add(Level::Err, "机翼", "所有机翼上下都被包住了，无法产生升力".into(), s.wings_buried.clone());
    } else {
        add(Level::Ok, "机翼", format!("左 {} 片 / 右 {} 片", s.left, s.right), vec![]);
    }
    if !s.fins_buried.is_empty() {
        add(Level::Warn, "尾翼", format!("{} 片尾翼左右都被挡住，不起作用", s.fins_buried.len()), s.fins_buried.clone());
    }
    if !s.wings_buried.is_empty() && !s.wings.is_empty() {
        add(Level::Warn, "机翼", format!("{} 片机翼上下都被包住，不产生升力", s.wings_buried.len()), s.wings_buried.clone());
    }
    if !s.wings.is_empty() {
        let need = (s.mass / BlockType::Wing.info().lift).ceil() as usize;
        if s.wings.len() < need {
            add(Level::Err, "升力", format!("升力不足：质量 {} 至少需要 {} 片有效机翼（现有 {}）", fmt(s.mass), need, s.wings.len()), vec![]);
        } else {
            add(Level::Ok, "升力", format!("升力比 {:.2}（需要 ≥ 1.00）", s.lift_ratio), vec![]);
        }
        if s.com.z < s.w_min_z as f32 - 3.0 || s.com.z > s.w_max_z as f32 + 3.0 {
            add(Level::Err, "重心", "重心离机翼太远，机翼要放在重心前后 3 格以内".into(), s.wings.clone());
        } else {
            add(Level::Ok, "重心", "重心落在机翼区域内".into(), vec![]);
        }
    }
    if s.roll_drift.abs() > 2f32.to_radians() {
        let (side, key) = if s.roll_drift > 0.0 { ("右", "A") } else { ("左", "D") };
        add(Level::Warn, "升力偏心", format!("升力中心没对准重心，飞机会一直往{side}侧滚 {:.0} 度/秒，要压着 {key} 修正。两侧的机翼和重物配平一些就好", s.roll_drift.abs().to_degrees()), s.wings.clone());
    }
    let mut clash = Vec::new();
    for b in blocks.iter() {
        let probe = if b.ty.weapon().is_some() {
            Some(b.pos() - IVec3::Z)
        } else if b.ty.thrust() > 0.0 {
            Some(b.pos() + IVec3::Z)
        } else {
            None
        };
        if let Some(p) = probe
            && let Some(j) = blocks.iter().position(|o| o.pos() == p)
        {
            clash.push(j);
        }
    }
    if !clash.is_empty() {
        add(Level::Err, "重叠", format!("{} 个方块插进了枪管或尾喷口，把它们删掉", clash.len()), clash);
    }
    if !s.wpn_blocked.is_empty() {
        add(Level::Warn, "武器", format!("{} 门武器正前方被挡住，无法开火", s.wpn_blocked.len()), s.wpn_blocked.clone());
    }
    if s.weapons.is_empty() {
        add(Level::Warn, "武器", "没有能开火的武器，只能靠撞".into(), vec![]);
    }
    if let Some(t) = s.fire_endurance
        && s.dps_sustained < s.dps * 0.6
    {
        add(Level::Warn, "能量", format!("武器耗能太快：满火力 {t:.0} 秒后能量耗尽，之后只剩 {:.0}% 火力。加反应堆，或少装激光和等离子炮", s.dps_sustained / s.dps * 100.0), vec![]);
    }
    if blocks.len() > MAX_BLOCKS {
        add(Level::Err, "方块数量", format!("方块太多（{}/{}）", blocks.len(), MAX_BLOCKS), vec![]);
    }
    let errors = checks.iter().filter(|c| c.level == Level::Err).count();
    let warns = checks.iter().filter(|c| c.level == Level::Warn).count();
    Analysis { stats: s, checks, errors, warns }
}
