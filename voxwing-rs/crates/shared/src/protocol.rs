//! Wire format. Reliable messages travel on one ordered QUIC stream per direction; snapshots and
//! inputs travel as unreliable QUIC datagrams.

use crate::blocks::Design;
use crate::flight::{InputCmd, Kin};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const ALPN: &[u8] = b"voxwing/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Everyone against waves of AI ships.
    Coop,
    /// Free-for-all, topped up with bots.
    Ffa,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Coop => "合作波次",
            Mode::Ffa => "自由混战",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    Hello { version: u32, name: String, design: Design },
    Leave,
    /// Join to watch: no ship, every ship in the snapshots.
    Watch { version: u32, name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreRow {
    pub pid: u16,
    pub name: String,
    pub kills: u32,
    pub deaths: u32,
    pub score: u32,
    pub alive: bool,
}

/// Score rows for free-for-all bots use pids from here up (player pids are small).
pub const BOT_PID_BASE: u16 = 0xF000;

impl ScoreRow {
    pub fn is_bot(&self) -> bool {
        self.pid >= BOT_PID_BASE
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome { pid: u16, tick: u32, mode: Mode, asteroid_seed: u64 },
    Reject { reason: String },
    ShipSpawn { id: u16, team: u16, name: String, owner: Option<u16>, design: Design, kin: Kin },
    /// Which ship this client controls (None while dead or spectating).
    YourShip { id: Option<u16> },
    ShipKilled { id: u16, killer: Option<u16> },
    ShipRemoved { id: u16 },
    ShipRepaired { id: u16 },
    /// Non-lethal hits: (block index, remaining hp as 0..=255).
    BlocksDamaged { ship: u16, hits: Vec<(u16, u8)> },
    /// Blocks shot off this tick. Clients then run the same `ShipBody::settle` as the server.
    BlocksDestroyed { ship: u16, blocks: Vec<u16> },
    ShieldHit { ship: u16, pos: Vec3 },
    Explosion { pos: Vec3, size: f32 },
    Crash { pos: Vec3 },
    Announce { title: String, sub: String },
    Feed { text: String, bad: bool },
    Scores { rows: Vec<ScoreRow>, wave: u32, enemies: u32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputPacket {
    /// The newest few inputs; repeats cover lost datagrams.
    pub cmds: Vec<InputCmd>,
}

pub const FLAG_FIRING: u8 = 1;
pub const FLAG_BOOST: u8 = 2;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ShipSnap {
    pub id: u16,
    pub pos: Vec3,
    pub rot: [i16; 4],
    pub vel: Vec3,
    pub flags: u8,
    pub throttle: u8,
    pub shield: u8,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MissileSnap {
    pub id: u16,
    pub pos: Vec3,
    pub vel: Vec3,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct OwnState {
    pub ship: u16,
    pub kin: Kin,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: u32,
    /// Last of this client's input ticks the server has applied (0 if none yet).
    pub ack: u32,
    /// Newest input tick received minus `tick`: how far ahead the client's inputs arrive.
    pub lead: i16,
    pub own: Option<OwnState>,
    pub ships: Vec<ShipSnap>,
    pub missiles: Vec<MissileSnap>,
}

pub fn encode<T: Serialize>(t: &T) -> Vec<u8> {
    postcard::to_allocvec(t).expect("postcard encoding cannot fail for these types")
}

pub fn decode<T: DeserializeOwned>(b: &[u8]) -> Option<T> {
    postcard::from_bytes(b).ok()
}

pub fn quantize_quat(q: Quat) -> [i16; 4] {
    let q = if q.w < 0.0 { -q } else { q };
    [q.x, q.y, q.z, q.w].map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16)
}

pub fn dequantize_quat(a: [i16; 4]) -> Quat {
    Quat::from_xyzw(a[0] as f32, a[1] as f32, a[2] as f32, a[3] as f32).normalize()
}
