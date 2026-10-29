//! Wire format. Reliable messages travel on one ordered QUIC stream per direction; snapshots and
//! inputs travel as unreliable QUIC datagrams.

use crate::blocks::Design;
use crate::flight::{InputCmd, Kin};
use crate::terrain::Map;
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    /// Free-for-all: one room on the server that players drive in and out of, no AI. Its clock
    /// runs while two or more are in; each round is ranked by kills.
    Ffa,
    /// Two teams of two, fighting over the hill in the middle of the arena.
    Teams,
    /// The test range: one player, nobody else, no limits, on the same map every time.
    Solo,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ffa => "自由混战",
            Mode::Teams => "2v2 占点战",
            Mode::Solo => "试车场",
        }
    }

    /// Seats in a match of this mode; AI fills the ones players don't. The free-for-all room
    /// takes players one at a time, and the test range is one player's.
    pub fn slots(self) -> usize {
        match self {
            Mode::Teams => 4,
            Mode::Ffa | Mode::Solo => 1,
        }
    }
}

/// Where the free-for-all room's round stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Round {
    /// No rounds here (2v2, the test range).
    #[default]
    None,
    /// Fewer than two players in: no clock.
    Waiting,
    /// The clock runs.
    Running,
    /// The round is over; the next one starts after a short break.
    Over,
}

/// 2v2's hill: each side's points (team 1's, team 2's), the team alone on top if one is, and
/// whether both are (then neither scores).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HillState {
    pub points: [u32; 2],
    pub holder: Option<u16>,
    pub contested: bool,
}

/// One seat in a match, as everyone sees it during the ship pick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlotInfo {
    pub name: String,
    pub team: u16,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMsg {
    /// The game starts (or a spectator is let in): the client's simulation begins here.
    Welcome { pid: u16, tick: u32, mode: Mode, seed: u64, map: Map },
    Reject { reason: String },
    /// The connection is accepted; this is the player's id.
    Connected { pid: u16 },
    /// In the queue: how many are waiting for this mode.
    Queued { players: u32 },
    /// A match is made: its seats (`you` is this player's) and how long the ship pick lasts.
    MatchFound { mode: Mode, tier: Option<String>, you: u8, slots: Vec<SlotInfo>, pick_secs: f32 },
    /// A seat's ship during the pick, and whether it's locked in.
    SlotPick { slot: u8, design: Option<Design>, ready: bool },
    /// The match ended: who won, and the final scoreboard.
    MatchOver { winner: String, rows: Vec<ScoreRow> },
    /// Something to tell the player (a pick refused, say).
    Notice { text: String },
    ShipSpawn { id: u16, team: u16, name: String, design: Design, kin: Kin },
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
    /// `limit`: kills (on the hill, points) that win the match (0 = none); `left`: seconds until
    /// it ends (0 = no clock), or in the free-for-all room until the round (or the break after
    /// it) ends.
    Scores { rows: Vec<ScoreRow>, limit: u32, left: f32, round: Round, hill: Option<HillState> },
    /// To the shooter only: one of their rounds landed on `target`, on its shield or its hull.
    /// Last, so the variants before it keep their numbers and older clients skip it unread.
    HitConfirm { target: u16, shield: bool },
    /// The arena's pick-up pad `pad` (its index in the layout) was taken and is back at server
    /// tick `back_at`; a newcomer gets one for every pad that is down.
    PadState { pad: u8, back_at: u32 },
    /// Ship `id` is in overdrive until server tick `until`.
    ShipBuff { id: u16, until: u32 },
    /// A mine laid by `team` lies at `pos`, live from server tick `armed_at`; a newcomer gets one
    /// for every mine down.
    MineDropped { id: u16, pos: Vec3, team: u16, armed_at: u32 },
    /// The mine is gone: set off (the explosion comes on its own), or cleared away.
    MineGone { id: u16, exploded: bool },
    /// A cloud of smoke `radius` round `pos`, until server tick `until`; a newcomer gets
    /// every cloud still hanging.
    Smoke { pos: Vec3, radius: f32, until: u32 },
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
/// Its saw is biting into something.
pub const FLAG_CUT: u8 = 8;
/// Its flipper is up: it threw within the last `FLIP_SHOWN` seconds.
pub const FLAG_FLIP: u8 = 16;
pub const FLIP_SHOWN: f32 = 0.45;
/// Its jump jets fire: it jumped within the last `JUMP_SHOWN` seconds.
pub const FLAG_JUMP: u8 = 32;
pub const JUMP_SHOWN: f32 = 0.35;

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
    /// The team that fired it: it flies through that team's ships, so they needn't be warned.
    pub team: u16,
    /// The ship that fired it: that ship's client draws it on its own (predicted) clock.
    pub owner: u16,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct OwnState {
    pub ship: u16,
    pub kin: Kin,
    /// As `ShipSnap::flags`: what it shows that only the server knows (its saw biting).
    pub flags: u8,
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
