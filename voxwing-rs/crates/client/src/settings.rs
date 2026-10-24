//! Player settings and saved designs, stored as JSON under ~/.voxwing.
//! Designs use the same `VW1.` share code format as the web version.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use voxwing_shared::blocks::{Block, BlockType, Design};
use voxwing_shared::presets::{PRESETS, preset};
use voxwing_shared::grid::in_grid;

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub name: String,
    /// The mode to queue for: "ffa" or "2v2".
    pub host_mode: String,
    /// Budget tier to queue in (`balance.ron` key); empty = no limit.
    pub host_budget: String,
    /// The server picked in the lobby (`REGIONS` key); empty = the one with the lowest ping.
    pub region: String,
    pub invert_y: bool,
    pub sound: bool,
    pub volume: f32,
    /// Background music level (times `volume`); 0 = off.
    pub music: f32,
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { name: "飞行员".into(), host_mode: "ffa".into(), host_budget: String::new(), region: String::new(), invert_y: false, sound: true, volume: 0.8, music: 0.6, sensitivity: 1.0, bloom: true, display: "windowed".into(), window_size: [1440, 900], fullscreen_size: [0, 0], vsync: true, fps_cap: 0, fullscreen: false, show_fps: false, show_help: true }
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

/// A game server players pick from on the entry page.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// What the settings remember the pick by.
    pub key: String,
    pub name: String,
    pub code: String,
    pub addr: String,
}

/// The built-in servers (`deploy/servers.json` lists the same machines for CI): key, name, code,
/// address.
const BUILT_IN: &[(&str, &str, &str, &str)] = &[("na", "北美", "NA", "99.246.198.62:4433"), ("asia", "亚洲", "ASIA", "38.92.8.110:4433")];

/// The servers on offer: the built-in ones, as changed or added to by `~/.voxwing/servers.json`.
#[derive(Resource, Clone, Debug)]
pub struct Regions(pub Vec<Region>);

/// One entry of `servers.json`: with a built-in key it changes that server (any field given), with a
/// new key it adds a server (`addr` needed).
#[derive(Deserialize)]
struct RegionFile {
    key: String,
    addr: Option<String>,
    name: Option<String>,
    code: Option<String>,
}

fn built_in() -> Vec<Region> {
    BUILT_IN.iter().map(|&(key, name, code, addr)| Region { key: key.into(), name: name.into(), code: code.into(), addr: addr.into() }).collect()
}

pub fn load_regions() -> Regions {
    let Some(path) = dir().map(|d| d.join("servers.json")) else { return Regions(built_in()) };
    let Ok(text) = std::fs::read_to_string(&path) else { return Regions(built_in()) };
    match regions_from(&text) {
        Ok(list) => Regions(list),
        Err(e) => {
            eprintln!("{} 读不了，用内置的服务器列表：{e}", path.display());
            Regions(built_in())
        }
    }
}

/// The built-in servers changed and added to by a `servers.json` text.
fn regions_from(text: &str) -> Result<Vec<Region>, String> {
    let entries: Vec<RegionFile> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut list = built_in();
    for e in entries {
        match list.iter_mut().find(|r| r.key == e.key) {
            Some(r) => {
                if let Some(a) = e.addr {
                    r.addr = a;
                }
                if let Some(n) = e.name {
                    r.name = n;
                }
                if let Some(c) = e.code {
                    r.code = c;
                }
            }
            None => match e.addr {
                Some(addr) => list.push(Region { name: e.name.unwrap_or_else(|| e.key.clone()), code: e.code.unwrap_or_else(|| e.key.to_uppercase()), key: e.key, addr }),
                None => eprintln!("servers.json：新服务器「{}」没有 addr，跳过", e.key),
            },
        }
    }
    Ok(list)
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

/// Your ships in the order the hangar shows them, and which one is selected (`ships.json`).
#[derive(Clone, Debug)]
pub struct Fleet {
    pub ships: Vec<Design>,
    pub current: usize,
}

#[derive(Serialize, Deserialize)]
struct FleetFile {
    current: usize,
    ships: Vec<Packed>,
}

pub fn load_fleet() -> Fleet {
    let mut fleet = match read::<FleetFile>("ships.json") {
        Some(f) => Fleet { ships: f.ships.into_iter().map(unpack).collect(), current: f.current },
        None => old_fleet(),
    };
    if fleet.ships.is_empty() {
        fleet.ships = PRESETS.iter().map(|p| preset(p.key)).collect();
    }
    fleet.current = fleet.current.min(fleet.ships.len() - 1);
    fleet
}

pub fn save_fleet(f: &Fleet) {
    write("ships.json", &FleetFile { current: f.current, ships: f.ships.iter().map(pack).collect() });
}

/// Before ships.json: named saves (designs.json) plus the ship last on the bench (current.json).
fn old_fleet() -> Fleet {
    let mut ships: Vec<Design> = read::<BTreeMap<String, Packed>>("designs.json").unwrap_or_default().into_values().map(unpack).collect();
    let mut current = 0;
    if let Some(c) = read::<Packed>("current.json").map(unpack).filter(|d| !d.blocks.is_empty()) {
        match ships.iter().position(|s| s.name == c.name) {
            Some(i) => {
                ships[i] = c;
                current = i;
            }
            None => ships.insert(0, c),
        }
    }
    Fleet { ships, current }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_json_changes_and_adds_servers() {
        let list = regions_from(r#"[{"key": "na", "addr": "10.0.0.10:4433"}, {"key": "lab", "addr": "127.0.0.1:4499"}]"#).unwrap();
        let na = list.iter().find(|r| r.key == "na").unwrap();
        // only what the entry gives changes
        assert_eq!((na.addr.as_str(), na.name.as_str(), na.code.as_str()), ("10.0.0.10:4433", "北美", "NA"));
        assert_eq!(list.iter().find(|r| r.key == "asia").unwrap().addr, "38.92.8.110:4433");
        let lab = list.iter().find(|r| r.key == "lab").unwrap();
        assert_eq!((lab.name.as_str(), lab.code.as_str()), ("lab", "LAB"));
        // a new server without an address is left out; a broken file is an error
        assert_eq!(regions_from(r#"[{"key": "x"}]"#).unwrap().len(), BUILT_IN.len());
        assert!(regions_from("{ not json").is_err());
    }
}
