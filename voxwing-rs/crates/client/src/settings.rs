//! Player settings and saved designs, stored as JSON under ~/.voxwing.
//! Designs use the same `VW1.` share code format as the web version.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use voxwing_shared::blocks::{Block, BlockType, Design};
use voxwing_shared::grid::in_grid;

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    pub server: String,
    /// "coop" or "ffa", used when this client hosts a room.
    pub host_mode: String,
    pub bots: u32,
    pub invert_y: bool,
    pub sound: bool,
    pub volume: f32,
    /// Mouse steering sensitivity multiplier.
    pub sensitivity: f32,
    pub bloom: bool,
    /// "windowed", "borderless" (a fullscreen window) or "fullscreen" (exclusive: changes the
    /// monitor's video mode).
    pub display: String,
    /// Window size in logical pixels, in windowed mode.
    pub window_size: [u32; 2],
    /// Exclusive fullscreen resolution in physical pixels; [0, 0] keeps the monitor's mode.
    pub fullscreen_size: [u32; 2],
    pub vsync: bool,
    /// Frame rate cap; 0 = none.
    pub fps_cap: u32,
    /// Replaced by `display`; still read so older settings files keep their fullscreen choice.
    #[serde(skip_serializing)]
    pub fullscreen: bool,
    pub show_fps: bool,
    pub show_help: bool,
    /// Recently joined server addresses, newest first.
    pub recent: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { name: "飞行员".into(), server: format!("127.0.0.1:{}", voxwing_shared::DEFAULT_PORT), host_mode: "coop".into(), bots: 4, invert_y: false, sound: true, volume: 0.8, sensitivity: 1.0, bloom: true, display: "windowed".into(), window_size: [1440, 900], fullscreen_size: [0, 0], vsync: true, fps_cap: 0, fullscreen: false, show_fps: false, show_help: true, recent: Vec::new() }
    }
}

fn dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let d = PathBuf::from(home).join(".voxwing");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

fn read<T: for<'a> Deserialize<'a>>(file: &str) -> Option<T> {
    let s = std::fs::read_to_string(dir()?.join(file)).ok()?;
    serde_json::from_str(&s).ok()
}

fn write<T: Serialize>(file: &str, v: &T) {
    if let Some(d) = dir()
        && let Ok(s) = serde_json::to_string_pretty(v)
    {
        let _ = std::fs::write(d.join(file), s);
    }
}

pub fn load_settings() -> Settings {
    let mut s: Settings = read("settings.json").unwrap_or_default();
    if s.fullscreen && s.display == "windowed" {
        s.display = "borderless".into();
    }
    s.fullscreen = false;
    s
}

pub fn save_settings(s: &Settings) {
    write("settings.json", s);
}

impl Settings {
    pub fn remember_server(&mut self, addr: &str) {
        self.recent.retain(|a| a != addr);
        self.recent.insert(0, addr.to_string());
        self.recent.truncate(5);
    }
}

pub fn delete_design(name: &str) {
    let mut all: BTreeMap<String, Packed> = read("designs.json").unwrap_or_default();
    all.remove(name);
    write("designs.json", &all);
}

/// Compact design format shared with the web version: {"n": name, "b": [[x,y,z,type,color],...]}
#[derive(Serialize, Deserialize)]
struct Packed {
    n: String,
    b: Vec<(i32, i32, i32, usize, u32)>,
}

fn pack(d: &Design) -> Packed {
    Packed { n: d.name.clone(), b: d.blocks.iter().map(|b| (b.x as i32, b.y as i32, b.z as i32, b.ty.index(), b.color)).collect() }
}

fn unpack(p: Packed) -> Design {
    let blocks = p
        .b
        .into_iter()
        .filter_map(|(x, y, z, t, c)| {
            let ty = BlockType::from_index(t)?;
            in_grid(voxwing_shared::glam::IVec3::new(x, y, z)).then(|| Block::new(x, y, z, ty, Some(c)))
        })
        .collect();
    Design { name: p.n, blocks }
}

pub fn load_current() -> Option<Design> {
    read::<Packed>("current.json").map(unpack)
}

pub fn save_current(d: &Design) {
    write("current.json", &pack(d));
}

pub fn saved_designs() -> BTreeMap<String, Design> {
    read::<BTreeMap<String, Packed>>("designs.json").unwrap_or_default().into_iter().map(|(k, v)| (k, unpack(v))).collect()
}

pub fn save_design(d: &Design) {
    let mut all: BTreeMap<String, Packed> = read("designs.json").unwrap_or_default();
    all.insert(d.name.clone(), pack(d));
    write("designs.json", &all);
}

pub fn encode_share(d: &Design) -> String {
    format!("VW1.{}", B64.encode(serde_json::to_vec(&pack(d)).unwrap_or_default()))
}

pub fn decode_share(code: &str) -> Result<Design, String> {
    let s: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let body = s.strip_prefix("VW1.").ok_or("分享码应该以 VW1. 开头")?;
    let bytes = B64.decode(body).map_err(|_| "分享码不完整")?;
    let p: Packed = serde_json::from_slice(&bytes).map_err(|_| "分享码内容无法识别")?;
    let d = unpack(p);
    if d.blocks.is_empty() {
        return Err("分享码里没有方块".into());
    }
    Ok(d)
}
