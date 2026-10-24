//! The balance rules: build cost and tiers, cockpit hit points, armour and shields against each
//! kind of hit. The numbers come from balance.ron; these check the rules hold, not exact values.

use std::sync::Arc;
use voxwing_shared::analysis::{Stats, analyze, compute_stats};
use voxwing_shared::balance::{balance, parse};
use voxwing_shared::blocks::{Block, BlockType, DamageKind, Design};
use voxwing_shared::presets::preset;
use voxwing_shared::ship::ShipBody;

fn stats(d: &Design) -> Stats {
    compute_stats(&d.blocks, &vec![true; d.blocks.len()])
}

#[test]
fn the_table_parses_and_names_every_block() {
    parse(include_str!("../balance.ron")).expect("balance.ron should parse");
    for t in BlockType::ALL {
        assert!(!t.info().name.is_empty());
        assert!(t.info().cost >= 0.0 && t.info().mass > 0.0 && t.info().hp > 0.0);
    }
    let tiers = &balance().tiers;
    assert!(tiers.windows(2).all(|w| w[0].budget < w[1].budget), "tiers cheapest first");
}

#[test]
fn presets_land_in_their_tiers() {
    let tier = |key: &str| balance().tier_for(analyze(&preset(key)).stats.cost).map(|t| t.key.clone());
    assert_eq!(tier("hummingbird").as_deref(), Some("light"));
    assert_eq!(tier("falcon").as_deref(), Some("medium"));
    assert_eq!(tier("stinger").as_deref(), Some("medium"));
    assert_eq!(tier("rhino").as_deref(), Some("heavy"));
}

#[test]
fn every_extra_gun_costs_more_than_the_last() {
    // the same laser added to a gunless ship and then to one with several guns
    let mut d = preset("falcon");
    d.blocks.retain(|b| b.ty.weapon().is_none());
    let bare = stats(&d).cost;
    let spots = [(3, 0, 0), (-3, 0, 0), (0, -1, 0), (0, -1, 1)];
    let mut costs = vec![bare];
    for &(x, y, z) in &spots {
        d.blocks.push(Block::new(x, y, z, BlockType::Laser, None));
        costs.push(stats(&d).cost);
    }
    let steps: Vec<f32> = costs.windows(2).map(|w| w[1] - w[0]).collect();
    assert!((steps[0] - BlockType::Laser.info().cost).abs() < 1e-3, "the first gun costs its price");
    assert!(steps.windows(2).all(|w| w[1] > w[0]), "each further gun costs more: {steps:?}");
}

#[test]
fn bigger_ships_have_tougher_cockpits() {
    let cockpit = |key: &str| {
        let body = ShipBody::new(Arc::new(preset(key)));
        let i = body.design.blocks.iter().position(|b| b.ty == BlockType::Cockpit).unwrap();
        body.block_max[i]
    };
    assert!(cockpit("rhino") > cockpit("falcon") * 1.5);
    assert!(cockpit("falcon") > BlockType::Cockpit.info().hp * 1.5, "no longer one hit from any big gun");
    // the ship's total includes it, and repairs restore it
    let mut body = ShipBody::new(Arc::new(preset("falcon")));
    assert!((body.max_hp - body.block_max.iter().sum::<f32>()).abs() < 1e-2);
    body.hp.iter_mut().for_each(|h| *h *= 0.5);
    body.repair();
    assert_eq!(body.hp, body.block_max);
}

#[test]
fn armour_stops_small_kinetic_rounds_and_little_else() {
    let b = balance();
    let armor = BlockType::Armor.info().armor;
    assert!(armor > 0.0);
    // a cannon round is mostly stopped, but never to nothing
    let cannon = b.through_armor(4.0, armor, DamageKind::Kinetic);
    assert!(cannon < 2.0 && cannon > 0.0, "cannon through armour {cannon}");
    // lasers and explosions ignore armour; a railgun slug barely notices it
    assert_eq!(b.through_armor(8.0, armor, DamageKind::Energy), 8.0);
    assert_eq!(b.through_armor(32.0, armor, DamageKind::Explosive), 32.0);
    assert!(b.through_armor(45.0, armor, DamageKind::Pierce) > 44.0);
    // plain hull has no armour at all
    assert_eq!(b.through_armor(4.0, BlockType::Hull.info().armor, DamageKind::Kinetic), 4.0);
}

#[test]
fn shields_shrug_off_energy_and_crack_under_explosions() {
    let b = balance();
    assert!(b.on_shield(10.0, DamageKind::Energy) < 10.0);
    assert_eq!(b.on_shield(10.0, DamageKind::Kinetic), 10.0);
    assert!(b.on_shield(10.0, DamageKind::Explosive) > 10.0);
}
