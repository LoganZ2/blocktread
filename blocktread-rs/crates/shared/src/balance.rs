//! The balance table: block numbers and global knobs from `balance.ron`.
//!
//! The file is built into the program. Setting `BLOCKTREAD_BALANCE` to a path loads that file instead,
//! for trying changes without rebuilding (the duel simulator does this). Clients and servers
//! compare `hash()` when a player joins, so both sides always simulate with the same numbers. The
//! hash ignores line endings: a Windows checkout's CRLF file is the same table.

use crate::blocks::{BlockType, DamageKind, TypeInfo};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The table as written in the file.
#[derive(Deserialize)]
struct File {
    damage_scale: f32,
    hp_scale: f32,
    base_energy_cap: f32,
    base_energy_regen: f32,
    pierce_falloff: f32,
    cockpit_hp_per_cost: f32,
    collapse_below: f32,
    shield_vs: HashMap<DamageKind, f32>,
    armor_vs: HashMap<DamageKind, f32>,
    min_damage_share: f32,
    splash_total: f32,
    weapon_cost_growth: f32,
    tiers: Vec<Tier>,
    blocks: HashMap<BlockType, TypeInfo>,
}

/// A room's build budget.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Tier {
    /// As given on the command line and kept in the settings: light, medium, heavy.
    pub key: String,
    pub name: String,
    pub budget: f32,
}

/// The table in use, with the global scales already applied to the blocks.
pub struct Balance {
    /// Indexed by `BlockType::index()`.
    pub types: Vec<TypeInfo>,
    pub base_energy_cap: f32,
    pub base_energy_regen: f32,
    pub pierce_falloff: f32,
    pub cockpit_hp_per_cost: f32,
    pub collapse_below: f32,
    shield_vs: HashMap<DamageKind, f32>,
    armor_vs: HashMap<DamageKind, f32>,
    min_damage_share: f32,
    pub splash_total: f32,
    pub weapon_cost_growth: f32,
    /// Cheapest first.
    pub tiers: Vec<Tier>,
    hash: u64,
}

impl Balance {
    /// This table's fingerprint (what `hash()` returns for the table in use).
    pub fn fingerprint(&self) -> u64 {
        self.hash
    }

    pub fn tier(&self, key: &str) -> Option<&Tier> {
        self.tiers.iter().find(|t| t.key == key)
    }

    /// The cheapest tier a ship of this cost fits in.
    pub fn tier_for(&self, cost: f32) -> Option<&Tier> {
        self.tiers.iter().find(|t| cost <= t.budget + 1e-3)
    }

    /// A block's full hit points on a ship of this cost (cockpits grow with the ship).
    pub fn block_hp(&self, ty: BlockType, ship_cost: f32) -> f32 {
        ty.info().hp + if ty == BlockType::Cockpit { self.cockpit_hp_per_cost * ship_cost } else { 0.0 }
    }

    /// What a shield takes from a hit of this kind.
    pub fn on_shield(&self, dmg: f32, kind: DamageKind) -> f32 {
        dmg * self.shield_vs.get(&kind).copied().unwrap_or(1.0)
    }

    /// What gets through a block's armour.
    pub fn through_armor(&self, dmg: f32, armor: f32, kind: DamageKind) -> f32 {
        let a = armor * self.armor_vs.get(&kind).copied().unwrap_or(1.0);
        if a <= 0.0 { dmg } else { (dmg - a).max(dmg * self.min_damage_share) }
    }
}

static BALANCE: OnceLock<Balance> = OnceLock::new();

pub fn balance() -> &'static Balance {
    BALANCE.get_or_init(|| {
        let text = match std::env::var("BLOCKTREAD_BALANCE") {
            Ok(path) => std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("BLOCKTREAD_BALANCE={path}: {e}")),
            Err(_) => include_str!("../balance.ron").to_string(),
        };
        parse(&text).unwrap_or_else(|e| panic!("balance table: {e}"))
    })
}

/// Fingerprint of the numbers in use (FNV-1a over the file text).
pub fn hash() -> u64 {
    balance().hash
}

pub fn parse(text: &str) -> Result<Balance, String> {
    // the same table whatever line endings the checkout gave it
    let text = &text.replace("\r\n", "\n");
    let f: File = ron::from_str(text).map_err(|e| e.to_string())?;
    let mut types = Vec::with_capacity(BlockType::ALL.len());
    for t in BlockType::ALL {
        let mut info = f.blocks.get(&t).cloned().ok_or_else(|| format!("no entry for {t:?}"))?;
        info.hp *= f.hp_scale;
        if let Some(w) = info.weapon.as_mut() {
            w.dmg *= f.damage_scale;
        }
        types.push(info);
    }
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    let mut tiers = f.tiers;
    tiers.sort_by(|a, b| a.budget.total_cmp(&b.budget));
    Ok(Balance {
        types,
        base_energy_cap: f.base_energy_cap,
        base_energy_regen: f.base_energy_regen,
        pierce_falloff: f.pierce_falloff,
        cockpit_hp_per_cost: f.cockpit_hp_per_cost * f.hp_scale,
        collapse_below: f.collapse_below,
        shield_vs: f.shield_vs,
        armor_vs: f.armor_vs,
        min_damage_share: f.min_damage_share,
        splash_total: f.splash_total,
        weapon_cost_growth: f.weapon_cost_growth,
        tiers,
        hash,
    })
}
