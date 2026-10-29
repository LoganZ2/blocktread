//! Names for AI drivers. They read like the names players give themselves, and nothing in what a
//! client is told sets an AI apart from a player.

use blocktread_shared::rng::Rng;

const NAMES: &[&str] = &[
    "坦克老王", "钢铁洪流", "履带侠", "铁憨憨", "开车不看路", "老司机", "小破车", "装甲兵张三", "炮塔转转",
    "不服就干", "夜行者", "铁拳", "车神", "慢慢开", "沙漠之狐", "北风", "柴油味", "螺丝刀", "大轮子",
    "轮子哥", "火力不足", "冲冲冲", "别打我", "隔壁老李", "一发入魂", "钢板", "铁锈", "野狗", "暴走卡车",
    "HeavyMetal", "Rustbucket", "TankGirl", "Gearhead", "Sputnik", "Bolt", "Ironside", "Torque",
    "Mudflap", "Diesel", "Scrapyard", "NoBrakes", "Kaboom", "Wrench", "Tread", "Pistonhead", "Ramjet",
];

/// A fresh name `taken` turns down for none: one from the list, sometimes with a number after it,
/// or now and then the default a player who never named themselves gets (`车长` and their id).
pub fn pick(rng: &mut Rng, pid: u16, taken: &dyn Fn(&str) -> bool) -> String {
    for _ in 0..40 {
        let name = match rng.below(10) {
            0 => format!("车长{pid}"),
            1 | 2 => format!("{}{}", NAMES[rng.below(NAMES.len())], rng.below(99) + 1),
            _ => NAMES[rng.below(NAMES.len())].to_string(),
        };
        if !taken(&name) {
            return name;
        }
    }
    fallback(pid, taken)
}

/// `车长` and the id, numbered on if even that is taken: the name of last resort.
pub fn fallback(pid: u16, taken: &dyn Fn(&str) -> bool) -> String {
    (0..).map(|k| if k == 0 { format!("车长{pid}") } else { format!("车长{pid}-{k}") }).find(|n| !taken(n)).unwrap()
}
