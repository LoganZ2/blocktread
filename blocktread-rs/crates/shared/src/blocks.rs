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
    Wheel,
    SmallWheel,
    BigWheel,
    Hover,
    DualWheel,
    GiantWheel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum Category {
    Core,
    Structure,
    /// Thrusters: the boost.
    Mobility,
    Weapon,
    System,
    /// What the vehicle stands and drives on: wheels, hover pads.
    Move,
}

impl Category {
    /// In the garage's order.
    pub const ALL: [Category; 6] = [Category::Core, Category::Move, Category::Mobility, Category::Structure, Category::Weapon, Category::System];
    pub fn label(self) -> &'static str {
        match self {
            Category::Core => "核心部件 · 必需",
            Category::Structure => "结构",
            Category::Mobility => "推进",
            Category::Move => "行动",
            Category::Weapon => "武器",
            Category::System => "系统",
        }
    }
}

/// What a block type is and does. Loaded from `balance.ron` (see `crate::balance`); every field
/// past `info` is an ability, zero (or none) when the block doesn't have it.
#[derive(Clone, Debug, Deserialize)]
pub struct TypeInfo {
    pub name: String,
    pub cat: Category,
    pub mass: f32,
    pub hp: f32,
    /// Build cost, counted against a room's budget.
    pub cost: f32,
    pub color: u32,
    pub info: String,
    /// Engine thrust; engines need the cell behind them clear.
    #[serde(default)]
    pub thrust: f32,
    /// Mass one (unburied) wing holds up.
    #[serde(default)]
    pub lift: f32,
    /// Control torque on every axis, wherever the block sits (a gyro).
    #[serde(default)]
    pub gyro: f32,
    /// Control torque per metre of lever arm from each axis (an attitude thruster).
    #[serde(default)]
    pub rcs: f32,
    #[serde(default)]
    pub energy_cap: f32,
    #[serde(default)]
    pub energy_regen: f32,
    #[serde(default)]
    pub shield_cap: f32,
    #[serde(default)]
    pub shield_regen: f32,
    /// Damage taken off every hit, depending on its kind (see the balance's `armor_vs`).
    #[serde(default)]
    pub armor: f32,
    #[serde(default)]
    pub weapon: Option<WeaponSpec>,
    /// A wheel or hover pad: what it stands on the ground with and drives by.
    #[serde(default)]
    pub drive: Option<DriveSpec>,
}

/// A block the vehicle stands and drives on.
#[derive(Clone, Debug, Deserialize)]
pub struct DriveSpec {
    /// Wheel radius; for a hover pad, the gap it holds above the ground.
    pub radius: f32,
    /// Drive force it adds (the vehicle's acceleration is the sum over its mass). Hover pads have
    /// none: they only hold the vehicle up, and thrusters push it.
    #[serde(default)]
    pub power: f32,
    /// Top speed it drives at (m/s).
    #[serde(default)]
    pub top: f32,
    /// Grip: how hard it can push along the ground, per unit of load. Hover pads have no tyres to
    /// grip with: they slide.
    #[serde(default)]
    pub grip: f32,
    /// Steering lock at a standstill (rad), for a wheel ahead of the centre of mass; 0 for a wheel
    /// that doesn't steer.
    #[serde(default)]
    pub steer: f32,
    /// Hover pads: energy drawn per second while lifting, and the turn rate they give (rad/s).
    #[serde(default)]
    pub hover: bool,
    #[serde(default)]
    pub drain: f32,
    #[serde(default)]
    pub turn: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum WeaponSfx {
    Laser,
    Cannon,
    Plasma,
    Missile,
    Rail,
    Flak,
}

/// What a hit does to shields and armour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum DamageKind {
    Energy,
    Kinetic,
    Pierce,
    Explosive,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WeaponSpec {
    pub kind: DamageKind,
    pub rate: f32,
    pub dmg: f32,
    pub speed: f32,
    pub life: f32,
    pub energy: f32,
    pub color: u32,
    pub len: f32,
    pub width: f32,
    #[serde(default)]
    pub spread: f32,
    /// Splash radius in blocks; 0 means a single-block hit.
    #[serde(default)]
    pub splash: f32,
    #[serde(default)]
    pub homing: bool,
    /// Projectiles per shot (each with its own spread).
    #[serde(default = "one")]
    pub pellets: u8,
    /// Blocks one projectile goes through, each taking the balance's `pierce_falloff` of the
    /// damage before it.
    #[serde(default = "one")]
    pub pierce: u8,
    pub sfx: WeaponSfx,
}

fn one() -> u8 {
    1
}

impl BlockType {
    pub const ALL: [BlockType; 26] = [
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
        BlockType::Wheel,
        BlockType::SmallWheel,
        BlockType::BigWheel,
        BlockType::Hover,
        BlockType::DualWheel,
        BlockType::GiantWheel,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }

    pub fn info(self) -> &'static TypeInfo {
        &crate::balance::balance().types[self as usize]
    }

    pub fn weapon(self) -> Option<&'static WeaponSpec> {
        self.info().weapon.as_ref()
    }

    /// Thrust (the boost); thrusters need the cell behind them clear.
    pub fn thrust(self) -> f32 {
        self.info().thrust
    }

    /// What it drives with, if it is a wheel or a hover pad.
    pub fn drive(self) -> Option<&'static DriveSpec> {
        self.info().drive.as_ref()
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
        if n.is_empty() { "无名战车".to_string() } else { n }
    }
}

pub fn rgb_u8(c: u32) -> [u8; 3] {
    [((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8]
}
