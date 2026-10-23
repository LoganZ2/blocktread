//! The blocks added after the web version: what each one does to a ship.

use std::sync::Arc;
use voxwing_shared::analysis::{Stats, analyze, compute_stats, place_reason};
use voxwing_shared::blocks::{Block, BlockType, Design};
use voxwing_shared::flight::{BTN_FIRE, InputCmd, Kin};
use voxwing_shared::glam::{IVec3, Quat, Vec3};
use voxwing_shared::presets::preset;
use voxwing_shared::protocol::{ServerMsg, decode, encode};
use voxwing_shared::ship::ShipBody;
use voxwing_shared::weapons::{WeaponState, step_weapons};
use voxwing_shared::DT;

fn stats(d: &Design) -> Stats {
    compute_stats(&d.blocks, &vec![true; d.blocks.len()])
}

fn with(mut d: Design, extra: &[(i32, i32, i32, BlockType)]) -> Design {
    for &(x, y, z, t) in extra {
        d.blocks.push(Block::new(x, y, z, t, None));
    }
    d
}

#[test]
fn every_type_has_a_name_and_round_trips() {
    for (i, t) in BlockType::ALL.iter().enumerate() {
        assert_eq!(t.index(), i);
        assert_eq!(BlockType::from_index(i), Some(*t));
        assert!(!t.info().name.is_empty());
    }
    // the wire format carries the new types
    let d = with(preset("falcon"), &[(0, -1, 1, BlockType::Capacitor), (0, 2, 3, BlockType::Fin)]);
    let msg = ServerMsg::ShipSpawn { id: 1, team: 0, name: "x".into(), owner: None, design: d.clone(), kin: Kin::new(Vec3::ZERO, Quat::IDENTITY, &stats(&d)) };
    match decode::<ServerMsg>(&encode(&msg)).unwrap() {
        ServerMsg::ShipSpawn { design, .. } => assert_eq!(design, d),
        _ => panic!(),
    }
}

#[test]
fn thrusters_are_small_engines() {
    let base = stats(&preset("falcon"));
    let more = stats(&with(preset("falcon"), &[(0, -1, 4, BlockType::Thruster)]));
    assert!((more.thrust - base.thrust - 10.0).abs() < 1e-4);
    assert_eq!(more.engines.len(), base.engines.len() + 1);
    // a ship with only thrusters still has engines
    let mut d = preset("falcon");
    d.blocks.retain(|b| b.ty != BlockType::Engine);
    for x in -2..=2 {
        d.blocks.push(Block::new(x, -1, 3, BlockType::Thruster, None));
    }
    let a = analyze(&d);
    assert!(!a.checks.iter().any(|c| c.title == "引擎" && c.level == voxwing_shared::analysis::Level::Err), "{:?}", a.first_error());
    // and they need their nozzle clear like an engine
    assert!(place_reason(&d.blocks, IVec3::new(0, -1, 4), BlockType::Hull).unwrap().contains("尾喷口"));
}

#[test]
fn fins_steer_the_nose_sideways() {
    let base = stats(&preset("falcon"));
    let tail = stats(&with(preset("falcon"), &[(0, 3, 3, BlockType::Fin), (0, 4, 3, BlockType::Fin)]));
    assert!(tail.yaw_rate > base.yaw_rate * 1.05, "yaw {} -> {}", base.yaw_rate, tail.yaw_rate);
    assert!((tail.lift_ratio * tail.mass - base.lift_ratio * base.mass).abs() < 1e-3, "fins carry no lift");
    // boxed in on both sides, a fin does nothing
    let boxed = analyze(&with(preset("falcon"), &[(0, 1, 0, BlockType::Fin), (1, 1, 0, BlockType::Hull), (-1, 1, 0, BlockType::Hull)]));
    assert_eq!((boxed.stats.fins.len(), boxed.stats.fins_buried.len()), (0, 1));
    assert!(boxed.checks.iter().any(|c| c.title == "尾翼"));
}

#[test]
fn attitude_thrusters_work_best_far_out() {
    // what one thruster adds to the roll torque, near the axis and out on the wingtip
    let gain = |at: (i32, i32, i32)| stats(&with(preset("falcon"), &[(at.0, at.1, at.2, BlockType::Rcs)])).control.z - stats(&preset("falcon")).control.z;
    let (near, far) = (gain((0, -1, 0)), gain((6, 0, 3)));
    assert!(far > near * 4.0, "roll torque far {far} vs near {near}");
    // and on the wingtips they out-turn a gyro at the centre, at well under half its mass
    let tips = stats(&with(preset("falcon"), &[(6, 0, 3, BlockType::Rcs), (-6, 0, 3, BlockType::Rcs)]));
    let gyro = stats(&with(preset("falcon"), &[(0, -1, 0, BlockType::Gyro)]));
    assert!(tips.roll_rate > gyro.roll_rate, "rcs {} vs gyro {}", tips.roll_rate, gyro.roll_rate);
}

#[test]
fn capacitors_extend_how_long_the_guns_run() {
    let base = stats(&preset("falcon"));
    let cap = stats(&with(preset("falcon"), &[(0, -1, 1, BlockType::Capacitor)]));
    assert!((cap.energy_cap - base.energy_cap - 90.0).abs() < 1e-4);
    assert!((cap.energy_regen - base.energy_regen).abs() < 1e-4, "no regen");
    assert!(cap.fire_endurance.unwrap() > base.fire_endurance.unwrap() * 2.0);
}

#[test]
fn flak_fires_a_spread_of_pellets() {
    let d = with(preset("hummingbird"), &[(0, -2, -2, BlockType::Flak)]);
    let body = ShipBody::new(Arc::new(d));
    let mut k = Kin::new(Vec3::ZERO, Quat::IDENTITY, &body.stats);
    let mut ws = WeaponState::new(&body);
    ws.cds.iter_mut().for_each(|c| *c = 0.0);
    let mut shots = Vec::new();
    step_weapons(&mut k, &body, &mut ws, &InputCmd { tick: 5, buttons: BTN_FIRE, ..Default::default() }, DT, 5, false, &mut shots);
    let pellets: Vec<_> = shots.iter().filter(|s| s.weapon == BlockType::Flak).collect();
    assert_eq!(pellets.len(), 7);
    let dirs: Vec<Vec3> = pellets.iter().map(|s| s.vel.normalize()).collect();
    assert!(dirs.windows(2).all(|w| w[0].angle_between(w[1]) > 1e-4), "pellets should spread");
    assert!(body.stats.dps > stats(&preset("hummingbird")).dps + 40.0);
}

#[test]
fn railgun_rounds_go_through_several_blocks() {
    let body = ShipBody::new(Arc::new(preset("falcon")));
    // straight down the spine from the nose: hull blocks in a row
    let hits = body.ray_through(Vec3::ZERO, Quat::IDENTITY, Vec3::new(0.0, 0.0, -60.0) - body.com0 * Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 60.0), 3);
    assert_eq!(hits.len(), 3);
    let zs: Vec<i8> = hits.iter().map(|&i| body.design.blocks[i].z).collect();
    assert_eq!(zs, vec![-5, -4, -3], "nearest first");
}
