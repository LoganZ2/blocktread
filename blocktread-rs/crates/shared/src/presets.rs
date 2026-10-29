use crate::blocks::{Block, BlockType, Design};
use BlockType::*;

pub struct Preset {
    pub key: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
}

pub const PRESETS: [Preset; 5] = [
    Preset { key: "rover", name: "越野车", desc: "轻型侦察车" },
    Preset { key: "wasp", name: "黄蜂", desc: "悬浮侦察车" },
    Preset { key: "bison", name: "野牛", desc: "六轮装甲车" },
    Preset { key: "scorpion", name: "毒蝎", desc: "导弹车" },
    Preset { key: "rhino", name: "犀牛", desc: "重型炮车" },
];

struct Mk(Vec<Block>);

impl Mk {
    fn add(&mut self, x: i32, y: i32, z: i32, t: BlockType, c: Option<u32>) {
        let b = Block::new(x, y, z, t, c);
        if let Some(o) = self.0.iter_mut().find(|o| o.pos() == b.pos()) {
            *o = b;
        } else {
            self.0.push(b);
        }
    }
    fn sym(&mut self, x: i32, y: i32, z: i32, t: BlockType, c: Option<u32>) {
        self.add(x, y, z, t, c);
        if x != 0 {
            self.add(-x, y, z, t, c);
        }
    }
}

/// A preset vehicle (an unknown key gives a blank one: just the cab). Forward is -z; wheels sit
/// on the deck's sides in its lowest row.
pub fn preset(key: &str) -> Design {
    let mut m = Mk(Vec::new());
    let name = PRESETS.iter().find(|p| p.key == key).map_or("新战车", |p| p.name);
    let lamp = Some(0xffe9b0);
    match key {
        // a six-metre buggy: steering wheels in front, twin tyres driving at the back, an engine,
        // two cannons by the cab, and two jump jets in its floor
        "rover" => {
            let (o, d, a) = (Some(0x77805a), Some(0x3b4030), Some(0xd9822b));
            for z in -4..=4 {
                for x in -1..=1 {
                    m.add(x, 0, z, Hull, if x == 0 { d } else { o });
                }
            }
            for (z, w) in [(-3, Wheel), (3, DualWheel)] {
                m.sym(2, 0, z, Hull, d);
                m.sym(3, 0, z, w, None);
            }
            m.sym(1, 0, -5, Lamp, lamp);
            m.add(0, 0, -5, Hull, a);
            m.add(0, 0, -2, JumpJet, None);
            m.add(0, 0, 2, JumpJet, None);
            m.add(0, 1, -1, Cockpit, None);
            m.add(0, 1, 0, Hull, o);
            m.sym(1, 1, -1, Hull, o);
            m.sym(1, 1, 0, Hull, o);
            m.sym(1, 1, -2, Cannon, None);
            m.add(0, 1, 3, Engine, None);
            m.sym(1, 1, 3, Hull, a);
        }
        // four hover pads under a small hull, pushed by an engine between two small thrusters at
        // the back: two lasers, a reactor to keep it all in the air
        "wasp" => {
            let (o, d, a) = (Some(0x5c6f78), Some(0x2e3438), Some(0xe8c547));
            for z in -2..=2 {
                for x in -1..=1 {
                    m.add(x, 0, z, Hull, if x == 0 { d } else { o });
                }
            }
            for z in [-2, 2] {
                m.sym(2, 0, z, Hover, None);
            }
            m.sym(1, 0, -3, Lamp, lamp);
            m.add(0, 0, -3, Hull, a);
            m.add(0, 1, -1, Cockpit, None);
            m.add(0, 1, 0, Reactor, None);
            m.add(0, 1, 1, Hull, d);
            m.sym(1, 1, 1, Laser, None);
            m.add(0, 0, 3, Engine, None);
            m.sym(1, 0, 3, Thruster, None);
        }
        // six twin-tyred wheels under an armoured cab, none of them steering: it turns by its sides
        // like a tank. A ram across its nose, cannons forward, a railgun on the roof
        "bison" => {
            let (o, d, a) = (Some(0x6f7a4a), Some(0x3b4030), Some(0xd9822b));
            for z in -4..=4 {
                for x in -2..=2 {
                    m.add(x, 0, z, Hull, if x.abs() == 2 { d } else { o });
                }
            }
            for z in [-3, 0, 3] {
                m.sym(3, 0, z, DualWheel, None);
            }
            m.add(0, 0, -5, Ram, None);
            m.sym(1, 0, -5, Ram, None);
            m.sym(2, 0, -5, Lamp, lamp);
            for z in -2..=0 {
                m.sym(2, 1, z, Armor, None);
            }
            m.add(0, 1, -2, Cockpit, None);
            m.sym(1, 1, -2, Hull, o);
            for z in -1..=3 {
                for x in -1..=1 {
                    m.add(x, 1, z, Hull, if z >= 3 { a } else { o });
                }
            }
            m.add(0, 1, 4, Engine, None);
            m.sym(1, 1, -3, Cannon, None);
            m.add(0, 2, 0, Railgun, None);
        }
        // a rack of four missiles on a long six-wheeled bed, a laser under the cab, smoke to cover
        // its getaway
        "scorpion" => {
            let (o, d, a) = (Some(0x8a7d55), Some(0x3e3a2c), Some(0xb5432f));
            for z in -4..=5 {
                for x in -1..=1 {
                    m.add(x, 0, z, Hull, if x == 0 { d } else { o });
                }
            }
            for (z, w) in [(-3, Wheel), (0, Wheel), (4, DualWheel)] {
                m.sym(2, 0, z, Hull, d);
                m.sym(3, 0, z, w, None);
            }
            m.sym(1, 0, -5, Lamp, lamp);
            m.add(0, 0, -5, Hull, a);
            m.add(0, 1, -3, Cockpit, None);
            m.sym(1, 1, -3, Hull, o);
            m.add(0, 1, -4, Laser, None);
            for z in -2..=4 {
                m.add(0, 1, z, Hull, d);
            }
            m.add(0, 1, 5, Engine, None);
            // the rack: four tubes abreast on a crossbeam
            for x in 1..=2 {
                m.sym(x, 1, 1, Hull, o);
                m.sym(x, 2, 1, Missile, None);
            }
            m.sym(1, 1, 2, Hull, o);
            m.sym(1, 1, 0, Hull, a);
            m.add(0, 2, 4, Smoke, None);
        }
        // the heavy: a wide hull on four big steering wheels and two giant ones at the back, in
        // armour; a railgun between two flak guns on the roof, cannons forward, a shield and
        // reactors to feed it all, and mines for whoever comes up behind
        "rhino" => {
            let (o, d, a) = (Some(0x5c6442), Some(0x33372a), Some(0xd9822b));
            for z in -5..=5 {
                for x in -3..=3 {
                    m.add(x, 0, z, Hull, if x.abs() == 3 { d } else { o });
                }
            }
            for z in [-4, -1] {
                m.sym(4, 0, z, BigWheel, None);
            }
            m.sym(4, 0, 3, GiantWheel, None);
            for x in -2..=2 {
                m.add(x, 0, -6, Armor, None);
            }
            m.sym(3, 0, -6, Lamp, lamp);
            for z in -4..=3 {
                m.sym(3, 1, z, Armor, None);
            }
            m.add(0, 1, -3, Cockpit, None);
            for z in -2..=5 {
                for x in -2..=2 {
                    m.add(x, 1, z, Hull, if z >= 4 { a } else { o });
                }
            }
            m.sym(1, 1, -3, Hull, o);
            m.sym(2, 1, -3, Hull, o);
            m.sym(1, 1, 5, Engine, None);
            m.sym(1, 1, 1, Reactor, None);
            m.sym(2, 1, -4, Cannon, None);
            m.add(0, 2, -1, Railgun, None);
            m.sym(1, 2, -1, Flak, None);
            m.add(0, 2, 2, Shield, None);
            for z in [0, 1, 3] {
                m.add(0, 2, z, Hull, d);
            }
            m.add(0, 2, 4, MineLayer, None);
        }
        _ => m.add(0, 0, 0, Cockpit, None),
    }
    Design { name: name.to_string(), blocks: m.0 }
}

/// The buggy everybody drives where no design is given (tests, the duel simulator).
pub fn rover() -> Design {
    preset("rover")
}

/// The garage's paints: field olive and sand, greys, then markings (signal amber, rust, red,
/// navy) and a light and a dark.
pub const PAINTS: [u32; 12] = [0x77805a, 0x3b4030, 0xa39a6b, 0xc9c1a0, 0x8d96a3, 0x4d5662, 0xd9822b, 0x9a5432, 0xb5432f, 0x4b6b8a, 0xe8e4d6, 0x22241e];

/// A design repainted the way a player might have: each colour on its paintable blocks swapped
/// for a different garage paint.
pub fn repaint(d: &Design, rng: &mut crate::rng::Rng) -> Design {
    let mut out = d.clone();
    let mut map: Vec<(u32, u32)> = Vec::new();
    let mut free: Vec<u32> = PAINTS.to_vec();
    for b in out.blocks.iter_mut().filter(|b| b.ty.paintable()) {
        let to = match map.iter().find(|m| m.0 == b.color) {
            Some(m) => m.1,
            None => {
                let c = if free.is_empty() { PAINTS[rng.below(PAINTS.len())] } else { free.swap_remove(rng.below(free.len())) };
                map.push((b.color, c));
                c
            }
        };
        b.color = to;
    }
    out
}
