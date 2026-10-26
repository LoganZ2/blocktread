use crate::balance::balance;
use crate::blocks::{Block, BlockType, Design};
use crate::grid::{DIRS6, Occupancy, in_grid};
use glam::{IVec3, Vec2, Vec3};

pub const MAX_BLOCKS: usize = 380;

/// Least power-to-weight a vehicle needs to get going (and up a slope).
pub const MIN_POWER: f32 = 0.2;
/// Below this tip-over angle (degrees) the checks warn that a vehicle rolls too easily.
pub const TIP_WARN: f32 = 35.0;

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
    /// Wheels and hover pads ("movers"); those in the lowest row are the ones on the ground.
    pub wheels: Vec<usize>,
    pub grounded: Vec<usize>,
    /// The drive power the movers add up to.
    pub power: f32,
    /// Energy per second the hover pads draw.
    pub hover_drain: f32,
    /// How far the vehicle leans before it tips over (degrees), side to side or nose to tail,
    /// whichever comes first (0 when it has fewer than three wheels on the ground).
    pub tip: f32,
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
            s.weapons.push(i);
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
    // driving: wheels and hover pads, their power, and how far the vehicle leans before it tips
    s.wheels = (0..blocks.len()).filter(|&i| alive[i] && blocks[i].ty.drive().is_some()).collect();
    let wheel_row = s.wheels.iter().map(|&i| blocks[i].y).min();
    s.grounded = s.wheels.iter().copied().filter(|&i| Some(blocks[i].y) == wheel_row).collect();
    s.power = s.wheels.iter().map(|&i| blocks[i].ty.drive().unwrap().power).sum();
    s.hover_drain = s.wheels.iter().map(|&i| blocks[i].ty.drive().unwrap()).filter(|d| d.hover).map(|d| d.drain).sum();
    (s.accel, s.max_speed) = crate::drive::performance_of(s.wheels.iter().map(|&i| blocks[i].ty.drive().unwrap()), s.mass);
    if s.grounded.len() >= 3 {
        let span = |f: fn(&Block) -> i8| {
            let v = s.grounded.iter().map(|&i| f(&blocks[i]) as f32);
            (v.clone().fold(f32::MAX, f32::min), v.fold(f32::MIN, f32::max))
        };
        let ((x0, x1), (z0, z1)) = (span(|b| b.x), span(|b| b.z));
        // the centre of mass over the ground: wheels sit REST_DROP below their blocks
        let reach = blocks[s.grounded[0]].ty.drive().unwrap().radius;
        let h = (s.com.y - wheel_row.unwrap() as f32 + reach + crate::drive::REST_DROP).max(0.1);
        let reach = (s.com.x - x0).min(x1 - s.com.x).min(s.com.z - z0).min(z1 - s.com.z);
        s.tip = reach.max(0.0).atan2(h).to_degrees();
    }
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

/// Why a block of type `t` may not go into `cell`, given the current blocks. A thruster's nozzle
/// sticks one cell back (+z); that cell must stay empty.
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
    if let Some(a) = ahead
        && a.ty.thrust() > 0.0
    {
        return Some(format!("这一格是{}尾喷口的位置，不能放方块", a.ty.info().name));
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
        add(Level::Err, "驾驶室", "缺少驾驶室：每辆车需要且只能有 1 个".into(), vec![]);
    } else if s.cockpits.len() > 1 {
        add(Level::Err, "驾驶室", format!("只能有 1 个驾驶室（现在有 {} 个）", s.cockpits.len()), s.cockpits.clone());
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
            add(Level::Ok, "驾驶室", "视野通畅".into(), vec![]);
        } else {
            add(Level::Err, "驾驶室视野", "顶部和正前方都被挡住了，车长看不见路。清空其中一个方向".into(), vec![s.cockpits[0]]);
        }
    }
    if s.cockpits.len() == 1 {
        let root = s.cockpits[0];
        let dis = disconnected(blocks, &alive, root);
        if dis.is_empty() {
            add(Level::Ok, "结构连接", format!("{} 个方块全部相连", blocks.len()), vec![]);
        } else {
            add(Level::Err, "结构连接", format!("{} 个方块没有连到驾驶室（方块之间要面贴面）", dis.len()), dis);
        }
        let occ = occupancy(blocks, &alive);
        let nb = DIRS6.iter().filter(|&&d| occ.has(blocks[root].pos() + d)).count();
        if nb == 1 && blocks.len() > 4 {
            add(Level::Warn, "结构", "驾驶室只有 1 个面和车身相连，那块被打掉整辆车就会散架。多加几个支撑".into(), vec![root]);
        }
    }
    // driving: wheels and hover pads
    let row = s.grounded.first().map(|&i| blocks[i].y);
    if s.grounded.len() < 3 {
        let msg = if s.wheels.is_empty() { "缺少车轮或悬浮器：至少需要 3 个".to_string() } else { format!("着地的车轮和悬浮器只有 {} 个，至少需要 3 个（要装在同一层，最低的那层）", s.grounded.len()) };
        add(Level::Err, "行动", msg, s.grounded.clone());
    } else {
        add(Level::Ok, "行动", format!("{} 个，都能着地", s.grounded.len()), vec![]);
    }
    let floating: Vec<usize> = s.wheels.iter().copied().filter(|i| !s.grounded.contains(i)).collect();
    if !floating.is_empty() {
        add(Level::Warn, "行动", format!("{} 个车轮或悬浮器比最低的那层高，碰不到地", floating.len()), floating);
    }
    if let Some(row) = row {
        let low: Vec<usize> = (0..blocks.len()).filter(|&i| blocks[i].y < row).collect();
        if !low.is_empty() {
            add(Level::Err, "离地间隙", format!("{} 个方块比车轮还低，会拖在地上。车轮和悬浮器要装在最低的一层", low.len()), low);
        }
    }
    if !s.wheels.is_empty() {
        if s.power / s.mass.max(1.0) < MIN_POWER {
            add(Level::Err, "功重比", format!("动力不足：功重比 {:.2}，至少 {MIN_POWER:.2}。多装车轮、换越野轮，或者减重", s.power / s.mass.max(1.0)), vec![]);
        } else {
            add(Level::Ok, "功重比", format!("{:.2}（需要 ≥ {MIN_POWER:.2}）", s.power / s.mass.max(1.0)), vec![]);
        }
    }
    if s.grounded.len() >= 3 {
        // hover pads turn the vehicle by themselves; wheels need one ahead of the centre of mass
        let hovers = s.grounded.iter().any(|&i| blocks[i].ty.drive().is_some_and(|d| d.hover));
        if !hovers && !s.grounded.iter().any(|&i| (blocks[i].z as f32) < s.com.z - 0.5) {
            add(Level::Err, "转向", "重心前面没有车轮，转不了向。前面的车轮负责转向".into(), vec![]);
        }
        let (xs, zs): (Vec<f32>, Vec<f32>) = s.grounded.iter().map(|&i| (blocks[i].x as f32, blocks[i].z as f32)).unzip();
        let inside = |v: &[f32], c: f32| c > v.iter().copied().fold(f32::MAX, f32::min) + 0.25 && c < v.iter().copied().fold(f32::MIN, f32::max) - 0.25;
        if !(inside(&xs, s.com.x) && inside(&zs, s.com.z)) {
            add(Level::Err, "重心", "重心不在车轮围成的范围里，车会翻。把重物挪到车轮中间".into(), vec![]);
        } else if s.tip < TIP_WARN {
            add(Level::Warn, "侧翻角", format!("{:.0}°，太容易翻（最好 ≥ {TIP_WARN:.0}°）。降低重心，或者把车轮装得更开", s.tip), vec![]);
        } else {
            add(Level::Ok, "重心", format!("在车轮围成的范围里 · 侧翻角 {:.0}°", s.tip), vec![]);
        }
    }
    if s.hover_drain > s.energy_regen {
        add(Level::Warn, "悬浮耗能", format!("悬浮器每秒耗能 {:.0}，超过能量回复 {:.0}：能量会慢慢耗尽，然后落地。加反应堆", s.hover_drain, s.energy_regen), vec![]);
    }
    // thrusters: the boost
    if s.count(BlockType::Engine) + s.count(BlockType::Thruster) == 0 {
        add(Level::Ok, "推进器", "没有推进器 · 不能加速".into(), vec![]);
    } else if s.engines.is_empty() {
        add(Level::Warn, "推进器", "所有推进器的尾喷口都被挡住了，不能加速。推进器正后方必须空着".into(), s.eng_blocked.clone());
    } else {
        add(Level::Ok, "推进器", format!("{} 台 · 推力 {:.0}", s.engines.len(), s.thrust), vec![]);
        if !s.eng_blocked.is_empty() {
            add(Level::Warn, "推进器", format!("{} 台推进器尾喷口被挡住，不出力", s.eng_blocked.len()), s.eng_blocked.clone());
        }
    }
    // weapons: how each one turns, and whether it can point anywhere at all
    let ms = crate::weapons::mounts(blocks, &alive, &occupancy(blocks, &alive));
    let boxed: Vec<usize> = ms.iter().filter(|m| m.boxed_in()).map(|m| m.block).collect();
    if !boxed.is_empty() {
        add(Level::Warn, "武器", format!("{} 门武器四周都被挡住，转不出去也开不了火", boxed.len()), boxed);
    }
    if !ms.is_empty() {
        add(Level::Ok, "武器", format!("{} 门 · 都跟着准星转", ms.len()), vec![]);
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
