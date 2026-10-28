//! Close quarters: rams, saws and flippers on a vehicle, and the arena's saw blades.

use std::sync::Arc;
use blocktread_shared::DT;
use blocktread_shared::analysis::{compute_stats, place_reason};
use blocktread_shared::arena::SAW_R;
use blocktread_shared::blocks::{Block, BlockType, Design};
use blocktread_shared::drive::{saw_bites, spawn_height, step_drive};
use blocktread_shared::flight::{BTN_FLIPPER, InputCmd, Kin};
use blocktread_shared::glam::{IVec3, Quat, Vec3};
use blocktread_shared::presets::preset;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::terrain::Terrain;
use blocktread_shared::weapons::{WeaponState, step_weapons};
use blocktread_shared::world::tick_time;

/// The rover with a row of `t` across its nose.
fn nosed(t: BlockType) -> Design {
    let mut d = preset("rover");
    for x in -1..=1 {
        d.blocks.push(Block::new(x, 0, -6, t, None));
    }
    d
}

#[test]
fn saws_and_flippers_need_the_cell_ahead_clear() {
    let d = preset("rover");
    // the nose row is at z = -5: a saw behind it has its blade in the hull, one in front is fine
    assert!(place_reason(&d.blocks, IVec3::new(0, 0, 4 + 2), BlockType::Saw).is_none(), "nothing behind the tail");
    assert!(place_reason(&d.blocks, IVec3::new(0, 1, 1), BlockType::Saw).is_some(), "the cab's hull is ahead of it");
    let saw = nosed(BlockType::Saw);
    assert!(place_reason(&saw.blocks, IVec3::new(0, 0, -7), BlockType::Hull).is_some(), "nothing goes where the blade cuts");
    // a ram minds nothing
    assert!(place_reason(&d.blocks, IVec3::new(0, 1, 1), BlockType::Ram).is_none());
    let s = compute_stats(&saw.blocks, &vec![true; saw.blocks.len()]);
    assert_eq!((s.melee.len(), s.melee_blocked.len()), (3, 0));
    // one boxed in behind another works for nothing
    let mut boxed = saw.clone();
    boxed.blocks.push(Block::new(0, 0, -7, BlockType::Hull, None));
    let s = compute_stats(&boxed.blocks, &vec![true; boxed.blocks.len()]);
    assert_eq!((s.melee.len(), s.melee_blocked.len()), (2, 1));
}

#[test]
fn a_flipper_throws_on_e_then_waits_for_its_cooldown() {
    let body = ShipBody::new(Arc::new(nosed(BlockType::Flipper)));
    let mut k = Kin::new(Vec3::ZERO, Quat::IDENTITY, &body.stats);
    let mut ws = WeaponState::new(&body);
    let spec = BlockType::Flipper.melee().unwrap();
    let mut shots = Vec::new();
    let mut flips = Vec::new();
    for t in 0..(4.0 / DT) as u32 {
        // the battery refills as it drives (`step_drive`); here it is simply kept full
        k.energy = body.stats.energy_cap;
        let before = k.energy;
        step_weapons(&mut k, &body, &mut ws, &InputCmd { tick: t, buttons: BTN_FLIPPER, ..Default::default() }, DT, t, false, &mut shots);
        if ws.flipped {
            assert!((before - k.energy - spec.energy * 3.0).abs() < 1e-3, "each of the three flippers draws its energy");
            flips.push(t as f32 * DT);
        }
    }
    assert_eq!(flips.len(), 2, "{flips:?}");
    assert!((flips[1] - flips[0] - spec.cooldown).abs() < 2.0 * DT, "{flips:?}");
    // not without the energy
    let mut ws = WeaponState::new(&body);
    k.energy = spec.energy;
    step_weapons(&mut k, &body, &mut ws, &InputCmd { tick: 1, buttons: BTN_FLIPPER, ..Default::default() }, DT, 1, false, &mut shots);
    assert!(!ws.flipped);
}

#[test]
fn a_saw_blade_throws_off_what_touches_it() {
    let ground = Terrain::cage(1);
    let arena = ground.arena.as_ref().unwrap();
    let body = ShipBody::new(Arc::new(preset("rover")));
    // the block out on its right side nearest the middle (a wheel)
    let side = (0..body.design.blocks.len()).map(|i| body.local(i)).max_by(|a, b| (a.x - a.z.abs() * 0.1).total_cmp(&(b.x - b.z.abs() * 0.1))).unwrap();
    for (n, saw) in arena.saws.iter().enumerate() {
        // as the blade passes the middle of its slot, clear of anything at its ends
        let tick = ((2.0 - saw.phase) * saw.period / DT).round() as u32;
        let hub = saw.at(tick_time(tick));
        let across = saw.dir.cross(Vec3::Y);
        // alongside the blade, facing along it, that block just into it
        let rot = Quat::from_rotation_arc(Vec3::NEG_Z, saw.dir);
        let at = hub - across * 0.2 - (rot * side).with_y(0.0);
        let mut k = Kin::new(at.with_y(spawn_height(&body, &ground, at.x, at.z, rot)), rot, &body.stats);
        assert!(!saw_bites(&k, &body, saw, tick_time(tick)).is_empty(), "saw {n} should be touching it");
        let start = k.pos;
        for t in tick..tick + 40 {
            step_drive(&mut k, &body, &InputCmd { tick: t, ..Default::default() }, &ground, DT);
            // nothing of it gets through to the far side
            let hub = saw.at(tick_time(t));
            for i in 0..body.design.blocks.len() {
                let w = body.world(k.pos, k.rot, i);
                let flat = (w - hub).with_y(0.0);
                assert!(flat.length() > SAW_R + 0.5 || flat.dot(across) < 0.4, "saw {n} cut through it at tick {t}");
            }
        }
        let away = (k.pos - start).dot(-across);
        println!("saw {n}: thrown {away:.1} m clear");
        assert!(away > 1.5, "saw {n} should throw it clear: {:?}", k.pos - start);
    }
}
