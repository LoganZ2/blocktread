//! Wire format. Reliable messages travel on one ordered QUIC stream per direction; snapshots and
//! inputs travel as unreliable QUIC datagrams.

use crate::blocks::Design;
use crate::flight::{InputCmd, Kin};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Still the game's old name: the connection handshake has to match what released clients and
/// servers send, so an outdated one still connects far enough to be told to update.
pub const ALPN: &[u8] = b"voxwing/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    /// Everyone against waves of AI ships.
    Coop,
    /// Free-for-all: one room on the server that players drive in and out of, no AI. Its clock
    /// runs while two or more are in; each round is ranked by kills.
    Ffa,
    /// Two teams of two.
    Teams,
    /// The test range: one player, nobody else, no limits, on the same map every time.
    Solo,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Coop => "合作波次",
            Mode::Ffa => "自由混战",
            Mode::Teams => "2v2 团队战",
            Mode::Solo => "试车场",
        }
    }

    /// Seats in a match of this mode; AI fills the ones players don't. The free-for-all room
    /// takes players one at a time, and the test range is one player's.
    pub fn slots(self) -> usize {
        match self {
            Mode::Teams | Mode::Coop => 4,
            Mode::Ffa | Mode::Solo => 1,
        }
    }
}

/// Where the free-for-all room's round stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Round {
    /// No rounds here (2v2, co-op, the test range).
    #[default]
    None,
    /// Fewer than two players in: no clock.
    Waiting,
    /// The clock runs.
    Running,
    /// The round is over; the next one starts after a short break.
    Over,
}

/// One seat in a match, as everyone sees it during the ship pick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotInfo {
    pub name: String,
    pub team: u16,
    pub bot: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// The first message on a connection. `balance` is `balance::hash()`: both sides must
    /// simulate with the same numbers.
    Hello { version: u32, name: String, balance: u64 },
    Leave,
    /// Instead of `Hello`: watch a match (no ship, every ship in the snapshots).
    Watch { version: u32, name: String, balance: u64 },
    /// Look for a match of this mode, optionally limited to a budget tier (`balance.ron` key).
    Queue { mode: Mode, tier: Option<String> },
    Unqueue,
    /// During the ship pick: the ship this player would fly (can change until `Ready`).
    Pick { design: Design },
    Ready,
}

/// The version in a hello this build can't otherwise read (an older client's), so the server can
/// still tell it why it can't join. Every hello starts with its variant and the version.
pub fn hello_version(b: &[u8]) -> Option<u32> {
    postcard::take_from_bytes::<(u32, u32)>(b).ok().map(|((_, v), _)| v)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreRow {
    pub pid: u16,
    pub name: String,
    pub kills: u32,
    pub deaths: u32,
    pub score: u32,
    pub alive: bool,
    pub team: u16,
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
    /// The game starts (or a spectator is let in): the client's simulation begins here.
    Welcome { pid: u16, tick: u32, mode: Mode, asteroid_seed: u64 },
    Reject { reason: String },
    /// The connection is accepted; this is the player's id.
    Connected { pid: u16 },
    /// In the queue: how many are waiting for this mode, and seconds until AI fills the rest.
    Queued { players: u32, fill_in: f32 },
    /// A match is made: its seats (`you` is this player's) and how long the ship pick lasts.
    MatchFound { mode: Mode, tier: Option<String>, you: u8, slots: Vec<SlotInfo>, pick_secs: f32 },
    /// A seat's ship during the pick, and whether it's locked in.
    SlotPick { slot: u8, design: Option<Design>, ready: bool },
    /// The match ended: who won, and the final scoreboard.
    MatchOver { winner: String, rows: Vec<ScoreRow> },
    /// Something to tell the player (a pick refused, say).
    Notice { text: String },
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
    /// `limit`: kills that win the match (0 = none); `left`: seconds until it ends (0 = no clock),
    /// or in the free-for-all room until the round (or the break after it) ends.
    Scores { rows: Vec<ScoreRow>, wave: u32, enemies: u32, limit: u32, left: f32, round: Round },
    /// To the shooter only: one of their rounds landed on `target`, on its shield or its hull.
    /// Last, so the variants before it keep their numbers and older clients skip it unread.
    HitConfirm { target: u16, shield: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputPacket {
    /// The newest few inputs; repeats cover lost datagrams.
    pub cmds: Vec<InputCmd>,
}

pub const FLAG_FIRING: u8 = 1;
pub const FLAG_BOOST: u8 = 2;
/// Its thrusters push (on the throttle, or boosting).
pub const FLAG_THRUST: u8 = 4;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ShipSnap {
    pub id: u16,
    pub pos: Vec3,
    pub rot: [i16; 4],
    pub vel: Vec3,
    pub flags: u8,
    pub throttle: u8,
    pub shield: u8,
    /// Where its guns aim (as in `InputCmd::aim`), so other clients turn its guns the same way.
    pub aim: [i16; 3],
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
