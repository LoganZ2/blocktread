//! The progression table: what is free, what costs what, the level curve, what a match pays.

use blocktread_shared::blocks::BlockType;
use blocktread_shared::economy::{Showing, Unlocks, economy};
use blocktread_shared::presets::preset;

#[test]
fn every_part_is_free_or_has_a_price() {
    let e = economy();
    for t in BlockType::ALL {
        assert!(e.is_starter(t) != e.prices.contains_key(&t), "{t:?} must be either a starter or for sale, not both or neither");
    }
}

#[test]
fn a_newcomer_can_drive_the_buggy_but_not_the_rest_of_the_presets() {
    let e = economy();
    assert!(e.locked(&preset("rover"), Unlocks::default()).is_empty(), "{:?}", e.locked(&preset("rover"), Unlocks::default()));
    assert_eq!(e.locked(&preset("wasp"), Unlocks::default()), [BlockType::Hover]);
    assert!(e.locked(&preset("wasp"), Unlocks::default().with(BlockType::Hover)).is_empty());
}

#[test]
fn levels_climb_by_a_growing_step() {
    let e = economy();
    assert_eq!(e.level(0), (1, 0, 200));
    assert_eq!(e.level(199).0, 1);
    assert_eq!(e.level(200), (2, 0, 300));
    assert_eq!(e.level(500).0, 3);
    assert!(e.tier_open(Some("light"), 1) && !e.tier_open(Some("medium"), 2) && e.tier_open(Some("medium"), 3));
    assert!(e.tier_open(None, 1));
    // the room's cap follows the biggest tier open
    assert_eq!(e.budget_at(1), Some(80.0));
    assert_eq!(e.budget_at(3), Some(160.0));
    assert_eq!(e.budget_at(6), None);
}

#[test]
fn a_match_pays_for_time_kills_and_winning_and_the_pace_is_about_a_hundred_matches() {
    let e = economy();
    let full = Showing { secs: 480.0, kills: 3, won: true, place: None };
    let r = e.reward(&full, false);
    assert_eq!(r.coins, 80 + 30 + 40);
    assert_eq!(r.xp, r.coins);
    assert_eq!(e.reward(&full, true).coins, r.coins + 150, "the day's first win pays extra");
    assert_eq!(e.reward(&Showing { secs: 480.0, kills: 0, won: false, place: None }, true).coins, 80, "a loss isn't a first win");
    assert_eq!(e.reward(&Showing { secs: 240.0, kills: 1, won: false, place: Some(0) }, false).coins, 40 + 10 + 40);
    let all: u32 = e.prices.values().sum();
    let average = (e.reward(&full, false).coins + e.reward(&Showing { secs: 480.0, kills: 2, won: false, place: None }, false).coins) / 2;
    let matches = all / average;
    assert!((80..=130).contains(&matches), "every part in about a hundred matches, not {matches}");
}
