use std::sync::Arc;
use voxwing_shared::analysis::{analyze, place_reason};
use voxwing_shared::blocks::BlockType;
use voxwing_shared::flight::{InputCmd, Kin, step_flight};
use voxwing_shared::glam::{IVec3, Quat, Vec3};
use voxwing_shared::presets::{PRESETS, preset};
use voxwing_shared::protocol::*;
use voxwing_shared::ship::ShipBody;
use voxwing_shared::weapons::{WeaponState, step_weapons};

#[test]
fn presets_pass_all_checks() {
    for p in PRESETS {
        let a = analyze(&preset(p.key));
        assert!(a.ok(), "{} failed: {:?}", p.key, a.first_error());
    }
    assert!(!analyze(&preset("blank")).ok());
}

#[test]
fn falcon_stats_match_web_version() {
    let s = analyze(&preset("falcon")).stats;
    assert_eq!(s.n, 55);
    assert!((s.mass - 52.0).abs() < 0.01, "mass {}", s.mass);
    assert_eq!(s.engines.len(), 3);
    assert_eq!((s.left, s.right), (10, 10));
    assert!((s.max_speed - 92.0).abs() < 1.0, "speed {}", s.max_speed);
    assert!((s.pitch_rate.to_degrees() - 76.0).abs() < 1.5, "turn {}", s.pitch_rate.to_degrees());
}

#[test]
fn barrels_and_nozzles_block_placement() {
    let d = preset("falcon");
    // lasers sit at (+-2, 0, -1); their barrel occupies (2, 0, -2)
    assert!(place_reason(&d.blocks, IVec3::new(2, 0, -2), BlockType::Hull).unwrap().contains("枪管"));
    // engines at z = 4; nozzle cell is z = 5
    assert!(place_reason(&d.blocks, IVec3::new(0, 0, 5), BlockType::Armor).unwrap().contains("尾喷口"));
    assert!(place_reason(&d.blocks, IVec3::new(0, -1, 0), BlockType::Hull).is_none());
}

#[test]
fn ray_hits_the_nose_first() {
    let body = ShipBody::new(Arc::new(preset("falcon")));
    let hit = body.ray(Vec3::ZERO, Quat::IDENTITY, Vec3::new(0.0, 0.0, -60.0) - body.com0 * Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 60.0));
    let (i, p) = hit.expect("should hit");
    let b = body.design.blocks[i];
    assert_eq!((b.x, b.y, b.z), (0, 0, -5));
    assert!(p.z < 0.0);
}

#[test]
fn cutting_the_wing_root_drops_the_outer_wing() {
    let mut body = ShipBody::new(Arc::new(preset("falcon")));
    let n0 = body.stats.n;
    let wings0 = body.stats.wings.len();
    // the left wing hangs off column x = -2; remove its four root blocks
    let roots: Vec<usize> = (0..body.design.blocks.len()).filter(|&i| body.design.blocks[i].x == -2 && body.design.blocks[i].ty == BlockType::Wing).collect();
    assert_eq!(roots.len(), 4);
    for i in roots {
        body.destroy(i);
    }
    let pieces = body.settle();
    assert_eq!(pieces.len(), 1, "outer left wing should fall off as one piece");
    assert_eq!(pieces[0].len(), 6);
    assert_eq!(body.stats.n, n0 - 10);
    assert_eq!(body.stats.wings.len(), wings0 - 10);
    assert!(body.stats.roll_drift < 0.0, "fewer left wings should roll the ship left");
    assert!(body.cockpit_alive());
}

#[test]
fn flight_and_weapons_are_deterministic() {
    let body = ShipBody::new(Arc::new(preset("falcon")));
    let run = || {
        let mut k = Kin::new(Vec3::new(0.0, 0.0, 600.0), Quat::IDENTITY, &body.stats);
        let mut ws = WeaponState::new(&body);
        let mut shots = Vec::new();
        for t in 0..300u32 {
            let inp = InputCmd { tick: t, pitch: 40, yaw: -20, roll: 10, throttle: 200, buttons: 1, ..Default::default() };
            step_flight(&mut k, &body.stats, &inp, voxwing_shared::DT, 1.0);
            step_weapons(&mut k, &body, &mut ws, &inp, voxwing_shared::DT, t, false, &mut shots);
        }
        (k, shots.len())
    };
    let (a, na) = run();
    let (b, nb) = run();
    assert_eq!(a, b);
    assert_eq!(na, nb);
    assert!(na > 20, "guns should fire, got {na}");
}

#[test]
fn protocol_roundtrip() {
    let d = preset("rhino");
    let body = ShipBody::new(Arc::new(d.clone()));
    let msg = ServerMsg::ShipSpawn { id: 3, team: 1, name: "敌方·犀牛".into(), owner: None, design: d.clone(), kin: Kin::new(Vec3::ONE, Quat::IDENTITY, &body.stats) };
    let bytes = encode(&msg);
    match decode::<ServerMsg>(&bytes).unwrap() {
        ServerMsg::ShipSpawn { design, name, .. } => {
            assert_eq!(design, d);
            assert_eq!(name, "敌方·犀牛");
        }
        _ => panic!(),
    }
    let q = Quat::from_euler(voxwing_shared::glam::EulerRot::XYZ, 0.3, -1.2, 2.0);
    assert!(dequantize_quat(quantize_quat(q)).angle_between(q) < 0.001);
    let snap = Snapshot { tick: 9, ack: 7, lead: 2, own: None, ships: vec![ShipSnap { id: 1, pos: Vec3::ONE, rot: quantize_quat(q), vel: Vec3::X, flags: 1, throttle: 200, shield: 0 }; 16], missiles: vec![] };
    let sb = encode(&snap);
    assert!(sb.len() < 1100, "16-ship snapshot should fit one datagram, was {}", sb.len());
    println!("rhino design bytes {}, 16-ship snapshot bytes {}", bytes.len(), sb.len());
}

#[test]
fn spectators_join_with_their_own_message() {
    let w = ClientMsg::Watch { version: voxwing_shared::PROTOCOL_VERSION, name: "看客".into(), balance: voxwing_shared::balance::hash() };
    match decode::<ClientMsg>(&encode(&w)).unwrap() {
        ClientMsg::Watch { name, .. } => assert_eq!(name, "看客"),
        _ => panic!(),
    }
    // every hello starts with its version, so even one this build can't parse says which it is
    let h = ClientMsg::Hello { version: 7, name: "x".into(), balance: 1 };
    assert_eq!(hello_version(&encode(&h)), Some(7));
    // a version 3 client's hello still carried its ship
    #[derive(serde::Serialize)]
    enum OldHello {
        Hello { version: u32, name: String, design: voxwing_shared::blocks::Design, balance: u64 },
    }
    let old = encode(&OldHello::Hello { version: 3, name: "x".into(), design: preset("falcon"), balance: 1 });
    assert!(decode::<ClientMsg>(&old).is_none_or(|m| !matches!(m, ClientMsg::Hello { version: 4.., .. })));
    assert_eq!(hello_version(&old), Some(3));
    let w = ClientMsg::Watch { version: 9, name: "x".into(), balance: 1 };
    assert_eq!(hello_version(&encode(&w)), Some(9));
}
