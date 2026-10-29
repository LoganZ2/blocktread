//! Progression: what a match pays, what parts cost, what levels open, from `economy.ron` (built
//! in). The server decides with it; the client shows it.
//!
//! Coins unlock parts, once each. Levels come from XP and open the bigger budget tiers and 2v2.
//! Nothing makes a vehicle stronger than another of the same cost: progression opens choices.

use crate::blocks::{BlockType, Design};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
pub struct Economy {
    /// Parts everyone has from the start.
    pub starter: Vec<BlockType>,
    /// Coins to unlock each of the other parts.
    pub prices: HashMap<BlockType, u32>,
    pub level_base: u32,
    pub level_step: u32,
    pub level_up_coins: u32,
    /// The level each budget tier (by key) opens at; one not listed is open from the start.
    pub tier_levels: HashMap<String, u32>,
    pub teams_level: u32,
    pub rewards: Rewards,
}

#[derive(Debug, Deserialize)]
pub struct Rewards {
    pub per_minute: u32,
    pub per_kill: u32,
    pub win: u32,
    pub places: Vec<u32>,
    pub daily_first_win: u32,
    pub xp_per_coin: f32,
}

/// How one player did in a match (or a free-for-all round), for its reward.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Showing {
    /// Seconds they were in it.
    pub secs: f32,
    pub kills: u32,
    /// 2v2: on the winning side.
    pub won: bool,
    /// Free-for-all: where they finished (0 = first).
    pub place: Option<usize>,
}

/// What a match pays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reward {
    pub coins: u32,
    pub xp: u32,
}

/// Parts unlocked beyond the starter set, one bit per `BlockType::code`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unlocks(pub u64);

impl Unlocks {
    pub fn has(self, ty: BlockType) -> bool {
        economy().is_starter(ty) || self.0 & (1 << ty.code()) != 0
    }

    pub fn with(self, ty: BlockType) -> Unlocks {
        Unlocks(self.0 | 1 << ty.code())
    }
}

impl Economy {
    pub fn is_starter(&self, ty: BlockType) -> bool {
        self.starter.contains(&ty)
    }

    /// Coins to unlock a part; None for a starter part.
    pub fn price(&self, ty: BlockType) -> Option<u32> {
        if self.is_starter(ty) { None } else { Some(self.prices.get(&ty).copied().unwrap_or(1000)) }
    }

    /// XP from `level` to the next.
    pub fn level_span(&self, level: u32) -> u32 {
        self.level_base + self.level_step * level.saturating_sub(1)
    }

    /// The level `xp` comes to, the XP into it, and the XP the next one needs.
    pub fn level(&self, xp: u32) -> (u32, u32, u32) {
        let (mut level, mut left) = (1, xp);
        while left >= self.level_span(level) {
            left -= self.level_span(level);
            level += 1;
        }
        (level, left, self.level_span(level))
    }

    /// Whether a budget tier is open at `level` (a match with no tier: always).
    pub fn tier_open(&self, tier: Option<&str>, level: u32) -> bool {
        tier.and_then(|k| self.tier_levels.get(k)).is_none_or(|&need| level >= need)
    }

    /// The most a vehicle may cost at `level` where no tier is set (the free-for-all room): the
    /// budget of the biggest tier open.
    pub fn budget_at(&self, level: u32) -> Option<f32> {
        let open: Vec<f32> = crate::balance::balance().tiers.iter().filter(|t| self.tier_open(Some(&t.key), level)).map(|t| t.budget).collect();
        if open.len() == crate::balance::balance().tiers.len() { None } else { open.into_iter().reduce(f32::max) }
    }

    /// The part types in `design` not unlocked.
    pub fn locked(&self, design: &Design, unlocks: Unlocks) -> Vec<BlockType> {
        let mut out: Vec<BlockType> = design.blocks.iter().map(|b| b.ty).filter(|&t| !unlocks.has(t)).collect();
        out.sort();
        out.dedup();
        out
    }

    /// What a showing pays; `first_today`: it is the player's first win (or top three) today.
    pub fn reward(&self, s: &Showing, first_today: bool) -> Reward {
        let r = &self.rewards;
        let placed = s.place.and_then(|p| r.places.get(p)).copied().unwrap_or(0);
        let mut coins = (s.secs / 60.0 * r.per_minute as f32) as u32 + s.kills * r.per_kill + if s.won { r.win } else { 0 } + placed;
        if first_today && Self::counts_as_win(s) {
            coins += r.daily_first_win;
        }
        Reward { coins, xp: (coins as f32 * r.xp_per_coin) as u32 }
    }

    /// A 2v2 win, or a top-three free-for-all round.
    pub fn counts_as_win(s: &Showing) -> bool {
        s.won || s.place.is_some_and(|p| p < 3)
    }
}

static ECONOMY: OnceLock<Economy> = OnceLock::new();

pub fn economy() -> &'static Economy {
    ECONOMY.get_or_init(|| parse(include_str!("../economy.ron")).unwrap_or_else(|e| panic!("economy table: {e}")))
}

pub fn parse(text: &str) -> Result<Economy, String> {
    ron::from_str(text).map_err(|e| e.to_string())
}
