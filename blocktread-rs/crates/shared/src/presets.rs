use crate::blocks::{Block, BlockType, Design};
use BlockType::*;

pub struct Preset {
    pub key: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    /// Relative cost when the server builds enemy waves.
    pub cost: f32,
}

pub const PRESETS: [Preset; 5] = [
    Preset { key: "rover", name: "越野车", desc: "轻型侦察车", cost: 1.0 },
    Preset { key: "wasp", name: "黄蜂", desc: "悬浮侦察车", cost: 1.0 },
    Preset { key: "bison", name: "野牛", desc: "六轮装甲车", cost: 2.0 },
    Preset { key: "scorpion", name: "毒蝎", desc: "导弹车", cost: 2.0 },
    Preset { key: "rhino", name: "犀牛", desc: "重型炮车", cost: 4.0 },
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
        // two cannons by the cab
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
        // like a tank. Cannons forward, a railgun on the roof
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
            m.add(0, 0, -5, Hull, a);
            m.sym(1, 0, -5, Lamp, lamp);
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
        // a rack of four missiles on a long six-wheeled bed, a laser under the cab
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
        }
        // the heavy: a wide hull on four big steering wheels and two giant ones at the back, in
        // armour; a railgun between two flak guns on the roof, cannons forward, a shield and
        // reactors to feed it all
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
        }
        _ => m.add(0, 0, 0, Cockpit, None),
    }
    Design { name: name.to_string(), blocks: m.0 }
}

/// The buggy everybody drives where no design is given (tests, the duel simulator).
pub fn rover() -> Design {
    preset("rover")
}

/// Recolour a design in the red enemy scheme, keeping light/dark structure.
pub fn enemy_colors(d: &Design) -> Design {
    let mut out = d.clone();
    for b in &mut out.blocks {
        let [r, g, bl] = crate::blocks::rgb_u8(b.color);
        let (r, g, bl) = (r as f32 / 255.0, g as f32 / 255.0, bl as f32 / 255.0);
        let mx = r.max(g).max(bl);
        let mn = r.min(g).min(bl);
        let l = (mx + mn) / 2.0;
        let s = if mx == mn { 0.0 } else if l > 0.5 { (mx - mn) / (2.0 - mx - mn) } else { (mx - mn) / (mx + mn) };
        b.color = if s > 0.45 && l > 0.3 {
            0xff5a2e
        } else if l > 0.62 {
            0x9a2f38
        } else if l > 0.38 {
            0x5a3036
        } else {
            0x2a2c33
        };
    }
    out
}
