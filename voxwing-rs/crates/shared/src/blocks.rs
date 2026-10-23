use glam::IVec3;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum BlockType {
    Cockpit,
    Wing,
    Engine,
    Hull,
    Armor,
    Laser,
    Cannon,
    Plasma,
    Missile,
    Reactor,
    Shield,
    Gyro,
    // added after the web version: new types only ever go at the end, the index is the wire and
    // share-code format
    Frame,
    Lamp,
    Fin,
    Thruster,
    Rcs,
    Railgun,
    Flak,
    Capacitor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Core,
    Structure,
    Mobility,
    Weapon,
    System,
}

impl Category {
    pub const ALL: [Category; 5] = [Category::Core, Category::Structure, Category::Mobility, Category::Weapon, Category::System];
    pub fn label(self) -> &'static str {
        match self {
            Category::Core => "核心部件 · 必需",
            Category::Structure => "结构",
            Category::Mobility => "机动",
            Category::Weapon => "武器",
            Category::System => "系统",
        }
    }
}

pub struct TypeInfo {
    pub name: &'static str,
    pub cat: Category,
    pub mass: f32,
    pub hp: f32,
    pub color: u32,
    pub info: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponSfx {
    Laser,
    Cannon,
    Plasma,
    Missile,
    Rail,
    Flak,
}

pub struct WeaponSpec {
    pub rate: f32,
    pub dmg: f32,
    pub speed: f32,
    pub life: f32,
    pub energy: f32,
    pub color: u32,
    pub len: f32,
    pub width: f32,
    pub spread: f32,
    /// Splash radius in blocks; 0 means a single-block hit.
    pub splash: f32,
    pub homing: bool,
    /// Projectiles per shot (each with its own spread).
    pub pellets: u8,
    /// Blocks one projectile goes through, each taking `PIERCE_FALLOFF` of the damage before it.
    pub pierce: u8,
    pub sfx: WeaponSfx,
}

/// Damage left after a piercing round passes through a block.
pub const PIERCE_FALLOFF: f32 = 0.7;

static TYPE_INFO: [TypeInfo; 20] = [
    TypeInfo { name: "驾驶舱", cat: Category::Core, mass: 2.0, hp: 45.0, color: 0xe6ebf1, info: "必需，只能有 1 个。顶部或正前方至少一个方向视野通畅。被打掉就坠毁。" },
    TypeInfo { name: "机翼", cat: Category::Core, mass: 0.5, hp: 14.0, color: 0xc4ccd6, info: "必需。每片能托起 6 质量，也提供转向力：离重心越远，翻滚（左右）或俯仰（前后）越有力。不用对称，但两侧升力不平衡会让飞机一直侧滚。上下不能都被包住。" },
    TypeInfo { name: "引擎", cat: Category::Core, mass: 2.0, hp: 28.0, color: 0x6b7380, info: "必需。每台提供 26 推力；尾喷口正后方必须空着。推力线要对准重心，否则全速时机头会被拉偏。" },
    TypeInfo { name: "船体", cat: Category::Structure, mass: 1.0, hp: 22.0, color: 0xb9c3cf, info: "轻型结构块，用来连接和塑形。" },
    TypeInfo { name: "装甲", cat: Category::Structure, mass: 3.0, hp: 75.0, color: 0x59626d, info: "耐打 3 倍多，但很重：拖慢速度，离重心越远越拖慢转向。" },
    TypeInfo { name: "激光炮", cat: Category::Weapon, mass: 1.5, hp: 18.0, color: 0x8a939e, info: "弹速快、射速高，每发消耗能量。正前方必须空着。" },
    TypeInfo { name: "机炮", cat: Category::Weapon, mass: 2.0, hp: 22.0, color: 0x7c7468, info: "高射速散布弹幕，不耗能量。正前方必须空着。" },
    TypeInfo { name: "等离子炮", cat: Category::Weapon, mass: 3.0, hp: 26.0, color: 0x6d7f74, info: "弹速慢、伤害高，命中后小范围爆炸。很耗能量。" },
    TypeInfo { name: "导弹舱", cat: Category::Weapon, mass: 2.5, hp: 24.0, color: 0x8c8f96, info: "锁定目标后右键发射追踪导弹，范围伤害。" },
    TypeInfo { name: "反应堆", cat: Category::System, mass: 3.0, hp: 30.0, color: 0x6e6a5c, info: "能量上限 +60，回复 +12/秒。" },
    TypeInfo { name: "护盾发生器", cat: Category::System, mass: 3.0, hp: 30.0, color: 0x5f6e86, info: "护盾 +40，3 秒未受伤后自动回复，先于船体吸收伤害。" },
    TypeInfo { name: "陀螺仪", cat: Category::Mobility, mass: 2.0, hp: 24.0, color: 0x6f6684, info: "提供俯仰、偏航、翻滚三个方向的转向力，放哪都一样有效；但它也重，靠近重心放最划算。" },
    TypeInfo { name: "骨架", cat: Category::Structure, mass: 0.4, hp: 10.0, color: 0x9aa4b0, info: "很轻的空心骨架：拉长机身、把尾翼和推进器伸远时几乎不增加转动惯量，但不耐打。" },
    TypeInfo { name: "灯块", cat: Category::Structure, mass: 0.3, hp: 8.0, color: 0x7fe0ff, info: "装饰用，按涂装颜色发光。很轻，也很脆。" },
    TypeInfo { name: "尾翼", cat: Category::Mobility, mass: 0.3, hp: 10.0, color: 0xc4ccd6, info: "竖直的舵面，不产生升力，主要加强偏航（左右甩机头）；离重心的前后距离越远越有力。左右两侧不能都被挡住。" },
    TypeInfo { name: "小推进器", cat: Category::Mobility, mass: 0.8, hp: 12.0, color: 0x6b7380, info: "小号引擎，推力 10，也算引擎；尾喷口正后方必须空着。适合补一点推力，或把推力中心调正。" },
    TypeInfo { name: "姿态推进器", cat: Category::Mobility, mass: 0.8, hp: 12.0, color: 0x7d8794, info: "三个方向都提供转向力，离重心越远越有力，和陀螺仪正相反。适合装在翼尖、机头、机尾。" },
    TypeInfo { name: "轨道炮", cat: Category::Weapon, mass: 3.0, hp: 24.0, color: 0x6e7a88, info: "弹速极快、射程最远的穿甲弹，一发能连穿 3 个方块（伤害依次递减）。射速慢，很耗能量。正前方必须空着。" },
    TypeInfo { name: "霰弹炮", cat: Category::Weapon, mass: 2.0, hp: 22.0, color: 0x7a6e60, info: "一次打出 7 发散开的弹丸，贴脸威力大，射程短。不耗能量。正前方必须空着。" },
    TypeInfo { name: "电容", cat: Category::System, mass: 1.5, hp: 18.0, color: 0x5c6f78, info: "能量上限 +90，但不回复能量。让武器和加速能连续用得更久。" },
];

static LASER: WeaponSpec = WeaponSpec { rate: 5.0, dmg: 7.0, speed: 520.0, life: 1.25, energy: 1.5, color: 0xff3344, len: 3.4, width: 0.2, spread: 0.002, splash: 0.0, homing: false, pellets: 1, pierce: 1, sfx: WeaponSfx::Laser };
static CANNON: WeaponSpec = WeaponSpec { rate: 9.0, dmg: 4.0, speed: 420.0, life: 1.45, energy: 0.0, color: 0xffd040, len: 1.8, width: 0.15, spread: 0.012, splash: 0.0, homing: false, pellets: 1, pierce: 1, sfx: WeaponSfx::Cannon };
static PLASMA: WeaponSpec = WeaponSpec { rate: 1.1, dmg: 26.0, speed: 260.0, life: 2.6, energy: 9.0, color: 0x44ff99, len: 1.2, width: 1.1, spread: 0.0, splash: 1.8, homing: false, pellets: 1, pierce: 1, sfx: WeaponSfx::Plasma };
static MISSILE: WeaponSpec = WeaponSpec { rate: 0.45, dmg: 40.0, speed: 150.0, life: 6.0, energy: 0.0, color: 0xfff2e0, len: 1.6, width: 0.36, spread: 0.0, splash: 2.4, homing: true, pellets: 1, pierce: 1, sfx: WeaponSfx::Missile };
static RAILGUN: WeaponSpec = WeaponSpec { rate: 0.6, dmg: 45.0, speed: 1000.0, life: 0.95, energy: 15.0, color: 0x9fe8ff, len: 7.0, width: 0.12, spread: 0.0, splash: 0.0, homing: false, pellets: 1, pierce: 3, sfx: WeaponSfx::Rail };
static FLAK: WeaponSpec = WeaponSpec { rate: 1.5, dmg: 4.0, speed: 380.0, life: 0.7, energy: 0.0, color: 0xffb060, len: 0.9, width: 0.16, spread: 0.06, splash: 0.0, homing: false, pellets: 7, pierce: 1, sfx: WeaponSfx::Flak };

impl BlockType {
    pub const ALL: [BlockType; 20] = [
        BlockType::Cockpit,
        BlockType::Wing,
        BlockType::Engine,
        BlockType::Hull,
        BlockType::Armor,
        BlockType::Laser,
        BlockType::Cannon,
        BlockType::Plasma,
        BlockType::Missile,
        BlockType::Reactor,
        BlockType::Shield,
        BlockType::Gyro,
        BlockType::Frame,
        BlockType::Lamp,
        BlockType::Fin,
        BlockType::Thruster,
        BlockType::Rcs,
        BlockType::Railgun,
        BlockType::Flak,
        BlockType::Capacitor,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }

    pub fn info(self) -> &'static TypeInfo {
        &TYPE_INFO[self as usize]
    }

    pub fn weapon(self) -> Option<&'static WeaponSpec> {
        match self {
            BlockType::Laser => Some(&LASER),
            BlockType::Cannon => Some(&CANNON),
            BlockType::Plasma => Some(&PLASMA),
            BlockType::Missile => Some(&MISSILE),
            BlockType::Railgun => Some(&RAILGUN),
            BlockType::Flak => Some(&FLAK),
            _ => None,
        }
    }

    /// Engine thrust; engines and thrusters need the cell behind them clear.
    pub fn thrust(self) -> f32 {
        match self {
            BlockType::Engine => 26.0,
            BlockType::Thruster => 10.0,
            _ => 0.0,
        }
    }

    /// New blocks of these types take the selected paint color instead of their default color.
    pub fn paintable(self) -> bool {
        matches!(self, BlockType::Hull | BlockType::Armor | BlockType::Wing | BlockType::Frame | BlockType::Lamp | BlockType::Fin)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub x: i8,
    pub y: i8,
    pub z: i8,
    pub ty: BlockType,
    pub color: u32,
}

impl Block {
    pub fn new(x: i32, y: i32, z: i32, ty: BlockType, color: Option<u32>) -> Self {
        Block { x: x as i8, y: y as i8, z: z as i8, ty, color: color.unwrap_or(ty.info().color) }
    }

    pub fn pos(&self) -> IVec3 {
        IVec3::new(self.x as i32, self.y as i32, self.z as i32)
    }
}

/// A ship blueprint. Forward is -Z, up is +Y, right is +X.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Design {
    pub name: String,
    pub blocks: Vec<Block>,
}

pub const NAME_MAX_CHARS: usize = 14;

impl Design {
    pub fn sanitized_name(&self) -> String {
        let n: String = self.name.trim().chars().filter(|c| !c.is_control()).take(NAME_MAX_CHARS).collect();
        if n.is_empty() { "无名战机".to_string() } else { n }
    }
}

pub fn rgb_u8(c: u32) -> [u8; 3] {
    [((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8]
}
