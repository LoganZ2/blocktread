//! Game rules and simulation shared by the client (for prediction) and the server (authoritative).
//! Nothing in here depends on Bevy, so engine upgrades never touch the core simulation.

pub mod ai;
pub mod analysis;
pub mod balance;
pub mod blocks;
pub mod drive;
pub mod flight;
pub mod grid;
pub mod presets;
pub mod protocol;
pub mod rng;
pub mod ship;
pub mod terrain;
pub mod weapons;
pub mod world;

pub use glam;

/// Simulation ticks per second, on both sides.
pub const TICK_RATE: u32 = 60;
pub const DT: f32 = 1.0 / TICK_RATE as f32;
/// The server sends a snapshot every this many ticks (30 Hz).
pub const SNAPSHOT_INTERVAL: u32 = 2;
/// 2: block types after the web version's twelve, and spectators.
/// 3: the balance table's hash in the hello, build budgets and the new damage rules.
/// 4: matchmaking: queue, ship pick, matches.
/// 5: ground vehicles: drive and steer instead of the flight stick.
/// 6: guns turn to an aim point; wheels and hover pads drive, thrusters boost.
pub const PROTOCOL_VERSION: u32 = 6;
pub const DEFAULT_PORT: u16 = 4433;
/// The arena's reach from its centre (the ground is a square of twice this).
pub const ARENA_R: f32 = terrain::HALF;
/// Server keeps this many ticks of ship transforms for lag compensation.
pub const HISTORY_TICKS: usize = 64;
