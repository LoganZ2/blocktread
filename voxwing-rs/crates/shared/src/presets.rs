use crate::blocks::{Block, BlockType, Design};
use BlockType::*;

pub struct Preset {
    pub key: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    /// Relative cost when the server builds enemy waves.
    pub cost: f32,
}

pub const PRESETS: [Preset; 4] = [
    Preset { key: "falcon", name: "游隼", desc: "均衡战斗机", cost: 2.0 },
    Preset { key: "hummingbird", name: "蜂鸟", desc: "高速拦截机", cost: 1.0 },
    Preset { key: "stinger", name: "毒刺", desc: "导弹艇", cost: 2.0 },
    Preset { key: "rhino", name: "犀牛", desc: "重型炮艇", cost: 4.0 },
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

pub fn preset(key: &str) -> Design {
    let mut m = Mk(Vec::new());
    let name = PRESETS.iter().find(|p| p.key == key).map_or("空白机体", |p| p.name);
    match key {
        "falcon" => {
            let (h, a, d) = (Some(0xc9d2dc), Some(0xff8a3d), Some(0x3b4452));
            for z in -5..=3 {
                m.add(0, 0, z, Hull, h);
            }
            m.add(0, 1, -2, Cockpit, None);
            m.add(0, 1, -1, Hull, h);
            m.sym(1, 1, -2, Hull, d);
            for z in -2..=3 {
                m.sym(1, 0, z, Hull, if z < 0 { h } else { d });
            }
            for (x, zs) in [(2, &[0, 1, 2, 3][..]), (3, &[1, 2, 3][..]), (4, &[2, 3][..]), (5, &[3][..])] {
                for &z in zs {
                    m.sym(x, 0, z, Wing, if x == 5 || (x == 4 && z == 2) { a } else { h });
                }
            }
            m.add(0, 0, 4, Engine, None);
            m.sym(1, 0, 4, Engine, None);
            m.sym(2, 0, -1, Laser, None);
            m.sym(1, -1, -2, Cannon, None);
            m.add(0, 1, 2, Hull, a);
            m.add(0, 1, 3, Hull, a);
            m.add(0, 2, 3, Hull, a);
        }
        "hummingbird" => {
            let (h, a) = (Some(0xe8ecf0), Some(0x5fd4ff));
            m.add(0, 0, -3, Hull, a);
            m.add(0, 0, -2, Cockpit, None);
            m.add(0, 0, -1, Hull, h);
            m.add(0, 0, 0, Hull, h);
            m.add(0, 0, 1, Hull, h);
            m.add(0, 0, 2, Engine, None);
            m.sym(1, 0, 2, Engine, None);
            m.sym(1, 0, -1, Laser, None);
            m.sym(1, 0, 0, Wing, h);
            m.sym(1, 0, 1, Wing, h);
            m.sym(2, 0, 0, Wing, h);
            m.sym(2, 0, 1, Wing, a);
            m.sym(3, 0, 1, Wing, a);
            m.add(0, -1, 0, Gyro, None);
            m.add(0, -1, -2, Hull, h);
            m.add(0, -1, -1, Hull, h);
        }
        "stinger" => {
            let (h, a, d) = (Some(0xd8dde3), Some(0x57e39a), Some(0x2f3742));
            for z in -5..=3 {
                m.add(0, 0, z, Hull, if z < -3 { a } else { h });
            }
            m.add(0, 1, -3, Cockpit, None);
            m.add(0, 1, -2, Hull, d);
            m.sym(1, 0, -2, Missile, None);
            m.sym(1, 1, -2, Missile, None);
            for (x, zs) in [(1, &[-1, 0, 1, 2][..]), (2, &[0, 1, 2][..]), (3, &[1, 2][..]), (4, &[2][..])] {
                for &z in zs {
                    m.sym(x, 0, z, Wing, if x == 4 { a } else { h });
                }
            }
            m.add(0, 0, 4, Engine, None);
            m.sym(1, 0, 3, Engine, None);
            m.add(0, -1, 0, Shield, None);
            m.add(0, -1, -4, Laser, None);
            m.add(0, 1, 2, Hull, d);
            m.add(0, 1, 3, Hull, d);
        }
        "rhino" => {
            let (h, a, ar) = (Some(0x8d9aa8), Some(0xffae42), Some(0x4d5662));
            for z in -4..=4 {
                for x in -1..=1 {
                    for y in -1..=0 {
                        let arm = x != 0 && y == 0;
                        m.add(x, y, z, if arm { Armor } else { Hull }, if arm { ar } else { h });
                    }
                }
            }
            m.add(0, 1, -3, Cockpit, None);
            m.add(0, 1, -2, Hull, h);
            m.sym(1, 1, -3, Armor, ar);
            m.add(0, -1, 0, Reactor, None);
            m.add(0, -1, 1, Reactor, None);
            m.add(0, 1, 1, Shield, None);
            m.add(0, 1, 2, Shield, None);
            m.sym(1, -1, 5, Engine, None);
            m.sym(1, 0, 5, Engine, None);
            for (x, zs) in [(2, &[0, 1, 2, 3][..]), (3, &[0, 1, 2, 3][..]), (4, &[1, 2, 3][..]), (5, &[2, 3][..]), (6, &[3][..])] {
                for &z in zs {
                    m.sym(x, -1, z, Wing, if x >= 5 { a } else { h });
                }
            }
            m.sym(2, 0, -2, Plasma, None);
            m.sym(2, -1, -1, Missile, None);
            m.add(0, -2, -4, Cannon, None);
            m.sym(1, -2, -3, Cannon, None);
            m.sym(1, 1, 3, Hull, a);
            m.sym(1, 1, 4, Hull, a);
            m.sym(1, 2, 4, Hull, a);
        }
        _ => m.add(0, 0, 0, Cockpit, None),
    }
    Design { name: name.to_string(), blocks: m.0 }
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
