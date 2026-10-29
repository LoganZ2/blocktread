//! The blocks added after the web version: what each one does to a vehicle.

use std::sync::Arc;
use blocktread_shared::analysis::{Stats, analyze, compute_stats};
use blocktread_shared::blocks::{Block, BlockType, Design};
use blocktread_shared::flight::{BTN_FIRE, InputCmd, Kin};
use blocktread_shared::glam::{Quat, Vec3};
use blocktread_shared::presets::preset;
use blocktread_shared::protocol::{ServerMsg, decode, encode};
use blocktread_shared::ship::ShipBody;
use blocktread_shared::weapons::{WeaponState, step_weapons};
use blocktread_shared::DT;

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
    let d = with(preset("rover"), &[(0, -1, 1, BlockType::Capacitor), (0, 2, 3, BlockType::Lamp)]);
    let msg = ServerMsg::ShipSpawn { id: 1, team: 0, name: "x".into(), design: d.clone(), kin: Kin::new(Vec3::ZERO, Quat::IDENTITY, &stats(&d)) };
    match decode::<ServerMsg>(&encode(&msg)).unwrap() {
        ServerMsg::ShipSpawn { design, .. } => assert_eq!(design, d),
        _ => panic!(),
    }
}

#[test]
fn thrusters_boost_and_wheels_drive() {
    let base = stats(&preset("rover"));
    let more = stats(&with(preset("rover"), &[(0, -1, 4, BlockType::Thruster)]));
    assert!((more.thrust - base.thrust - 10.0).abs() < 1e-4, "a small thruster adds its thrust to the boost");
    assert!((more.power - base.power).abs() < 1e-4, "but nothing to the drive");
    // the drive is the wheels': a bigger wheel pulls harder, a small one less
    let big = stats(&with(preset("rover"), &[(-3, 0, 0, BlockType::BigWheel)]));
    let small = stats(&with(preset("rover"), &[(-3, 0, 0, BlockType::SmallWheel)]));
    assert!(big.power > small.power && small.power > base.power);
    // no thruster at all is allowed: it just can't boost
    let mut d = preset("rover");
    d.blocks.retain(|b| b.ty != BlockType::Engine);
    let a = analyze(&d);
    assert!(a.ok(), "{:?}", a.first_error());
    assert_eq!(a.stats.thrust, 0.0);
}

#[test]
fn capacitors_extend_how_long_the_guns_run() {
    let base = stats(&preset("bison"));
    let cap = stats(&with(preset("bison"), &[(0, 2, 3, BlockType::Capacitor)]));
    assert!((cap.energy_cap - base.energy_cap - 90.0).abs() < 1e-4);
    assert!((cap.energy_regen - base.energy_regen).abs() < 1e-4, "no regen");
    assert!(cap.fire_endurance.unwrap() > base.fire_endurance.unwrap() * 2.0);
}

#[test]
fn flak_fires_a_spread_of_pellets() {
    let d = with(preset("rover"), &[(0, -2, -2, BlockType::Flak)]);
    let body = ShipBody::new(Arc::new(d));
    let mut k = Kin::new(Vec3::ZERO, Quat::IDENTITY, &body.stats);
    let mut ws = WeaponState::new(&body);
    ws.cds.iter_mut().for_each(|c| *c = 0.0);
    let mut shots = Vec::new();
    // aimed straight down where it rests, so it fires on the first tick
    let i = body.design.blocks.iter().position(|b| b.ty == BlockType::Flak).unwrap();
    let (yaw, pitch) = ws.aim[i];
    let aim = blocktread_shared::flight::quantize_aim(body.local(i) + blocktread_shared::weapons::barrel_dir(yaw, pitch) * 100.0);
    step_weapons(&mut k, &body, &mut ws, &InputCmd { tick: 5, buttons: BTN_FIRE, aim, ..Default::default() }, DT, 5, false, &mut shots);
    let pellets: Vec<_> = shots.iter().filter(|s| s.weapon == BlockType::Flak).collect();
    assert_eq!(pellets.len(), 7);
    let dirs: Vec<Vec3> = pellets.iter().map(|s| s.vel.normalize()).collect();
    assert!(dirs.windows(2).all(|w| w[0].angle_between(w[1]) > 1e-4), "pellets should spread");
    assert!(body.stats.dps > stats(&preset("rover")).dps + 40.0);
}

#[test]
fn railgun_rounds_go_through_several_blocks() {
    let body = ShipBody::new(Arc::new(preset("rover")));
    // straight down the spine from the nose: hull blocks in a row
    let hits = body.ray_through(Vec3::ZERO, Quat::IDENTITY, Vec3::new(0.0, 0.0, -60.0) - body.com0 * Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 60.0), 3);
    assert_eq!(hits.len(), 3);
    let zs: Vec<i8> = hits.iter().map(|&i| body.design.blocks[i].z).collect();
    assert_eq!(zs, vec![-5, -4, -3], "nearest first");
}

#[test]
fn reactors_decide_how_long_the_guns_keep_firing() {
    // the six-wheeler's railgun and cannons draw a little more than its battery refills
    let s = stats(&preset("bison"));
    let lasts = s.fire_endurance.expect("bison should run dry at full fire");
    assert!(s.dps_sustained < s.dps);
    assert!((lasts - s.energy_cap / (s.energy_draw - s.energy_regen)).abs() < 1e-3);
    // a reactor on the engine deck keeps them fed
    let fed = stats(&with(preset("bison"), &[(0, 2, 4, BlockType::Reactor)]));
    assert!(fed.fire_endurance.is_none());
    assert!((fed.dps_sustained - fed.dps).abs() < 1e-3);
    // every weapon draws energy, so reactors matter to every loadout
    for t in BlockType::ALL {
        if let Some(w) = t.weapon() {
            assert!(w.energy > 0.0, "{} fires for free", t.info().name);
        }
    }
}

#[test]
fn saved_codes_never_move() {
    // vehicles.json and share codes keep these numbers; the flying version's parts left gaps
    for (t, code) in [(BlockType::Cockpit, 0), (BlockType::Engine, 2), (BlockType::Frame, 12), (BlockType::Thruster, 15), (BlockType::Railgun, 17), (BlockType::Smoke, 31)] {
        assert_eq!(t.code(), code);
        assert_eq!(BlockType::from_code(code as usize), Some(t));
    }
    for gone in [1, 11, 14, 16, 32] {
        assert_eq!(BlockType::from_code(gone), None);
    }
    for t in BlockType::ALL {
        assert_eq!(BlockType::from_code(t.code() as usize), Some(t));
    }
}
