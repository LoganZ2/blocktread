use std::sync::Arc;
use blocktread_shared::analysis::{analyze, place_reason};
use blocktread_shared::blocks::BlockType;
use blocktread_shared::flight::{InputCmd, Kin};
use blocktread_shared::glam::{IVec3, Quat, Vec3};
use blocktread_shared::presets::{PRESETS, preset};
use blocktread_shared::protocol::*;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::weapons::{WeaponState, step_weapons};

#[test]
fn presets_pass_all_checks() {
    for p in PRESETS {
        let a = analyze(&preset(p.key));
        assert!(a.ok(), "{} failed: {:?}", p.key, a.first_error());
    }
    assert!(!analyze(&preset("blank")).ok());
}

#[test]
fn the_buggys_numbers() {
    let s = analyze(&preset("rover")).stats;
    assert_eq!(s.n, 49);
    assert!((s.mass - 51.6).abs() < 0.01, "mass {}", s.mass);
    assert_eq!((s.wheels.len(), s.grounded.len()), (4, 4));
    assert!((s.cost - 76.8).abs() < 0.05, "cost {}", s.cost);
    // four wheels drive it; the thruster only boosts
    assert!((s.power - 32.0).abs() < 1e-4);
    assert!((s.thrust - 26.0).abs() < 1e-4);
    assert!((60.0..75.0).contains(&s.tip), "tip {}", s.tip);
}

#[test]
fn nozzles_block_placement_but_barrels_dont() {
    let d = preset("rover");
    // the thruster sits at (0, 1, 3); its nozzle needs (0, 1, 4) clear
    assert!(place_reason(&d.blocks, IVec3::new(0, 1, 4), BlockType::Armor).unwrap().contains("尾喷口"));
    // guns turn, so the cell in front of one takes a block (its mount then avoids it)
    assert!(place_reason(&d.blocks, IVec3::new(1, 1, -3), BlockType::Hull).is_none());
    assert!(place_reason(&d.blocks, IVec3::new(0, -1, 0), BlockType::Hull).is_none());
}

#[test]
fn ray_hits_the_nose_first() {
    let body = ShipBody::new(Arc::new(preset("rover")));
    let hit = body.ray(Vec3::ZERO, Quat::IDENTITY, Vec3::new(0.0, 0.0, -60.0) - body.com0 * Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 60.0));
    let (i, p) = hit.expect("should hit");
    let b = body.design.blocks[i];
    assert_eq!((b.x, b.y, b.z), (0, 0, -5));
    assert!(p.z < 0.0);
}

#[test]
fn shooting_off_an_axle_drops_its_wheel() {
    let mut body = ShipBody::new(Arc::new(preset("rover")));
    let n0 = body.stats.n;
    // the front left wheel hangs off the axle stub at (-2, 0, -3)
    let axle = (0..body.design.blocks.len()).find(|&i| body.design.blocks[i].pos() == IVec3::new(-2, 0, -3)).unwrap();
    body.destroy(axle);
    let pieces = body.settle();
    assert_eq!(pieces.len(), 1, "the wheel should fall off");
    assert_eq!(body.design.blocks[pieces[0][0]].ty, BlockType::Wheel);
    assert_eq!(body.stats.n, n0 - 2);
    assert_eq!(body.movers.len(), 3);
    assert!(body.cockpit_alive());
}

#[test]
fn driving_and_weapons_are_deterministic() {
    let body = ShipBody::new(Arc::new(blocktread_shared::presets::rover()));
    let ground = blocktread_shared::terrain::Terrain::generate(5);
    let run = || {
        let y = blocktread_shared::drive::spawn_height(&body, &ground, 0.0, 0.0, Quat::IDENTITY);
        let mut k = Kin::new(Vec3::new(0.0, y, 0.0), Quat::IDENTITY, &body.stats);
        let mut ws = WeaponState::new(&body);
        let mut shots = Vec::new();
        for t in 0..300u32 {
            // the guns track a point out ahead and fire whenever they bear on it
            let inp = InputCmd { tick: t, drive: 110, steer: -40, buttons: 1, aim: blocktread_shared::flight::quantize_aim(Vec3::new(-40.0, 5.0, -200.0)), ..Default::default() };
            blocktread_shared::drive::step_drive(&mut k, &body, &inp, &ground, blocktread_shared::DT);
            step_weapons(&mut k, &body, &mut ws, &inp, blocktread_shared::DT, t, false, &mut shots);
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
    let q = Quat::from_euler(blocktread_shared::glam::EulerRot::XYZ, 0.3, -1.2, 2.0);
    assert!(dequantize_quat(quantize_quat(q)).angle_between(q) < 0.001);
    let snap = Snapshot { tick: 9, ack: 7, lead: 2, own: None, ships: vec![ShipSnap { id: 1, pos: Vec3::ONE, rot: quantize_quat(q), vel: Vec3::X, flags: 1, throttle: 200, shield: 0, aim: [1200, -40, 30000] }; 16], missiles: vec![] };
    let sb = encode(&snap);
    assert!(sb.len() < 1100, "16-ship snapshot should fit one datagram, was {}", sb.len());
    println!("rhino design bytes {}, 16-ship snapshot bytes {}", bytes.len(), sb.len());
}

#[test]
fn spectators_join_with_their_own_message() {
    let w = ClientMsg::Watch { version: blocktread_shared::PROTOCOL_VERSION, name: "看客".into(), balance: blocktread_shared::balance::hash() };
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
        Hello { version: u32, name: String, design: blocktread_shared::blocks::Design, balance: u64 },
    }
    let old = encode(&OldHello::Hello { version: 3, name: "x".into(), design: preset("rover"), balance: 1 });
    assert!(decode::<ClientMsg>(&old).is_none_or(|m| !matches!(m, ClientMsg::Hello { version: 4.., .. })));
    assert_eq!(hello_version(&old), Some(3));
    let w = ClientMsg::Watch { version: 9, name: "x".into(), balance: 1 };
    assert_eq!(hello_version(&encode(&w)), Some(9));
}

#[test]
fn hit_confirms_come_last_so_older_clients_skip_them() {
    // postcard numbers enum variants in order: the messages an older client knows keep theirs,
    // and the new one carries a number it has never seen, which its decode turns down
    let scores = ServerMsg::Scores { rows: vec![], wave: 0, enemies: 0, limit: 0, left: 0.0, round: Round::None };
    assert_eq!(encode(&scores)[0], 20);
    let hit = encode(&ServerMsg::HitConfirm { target: 7, shield: true });
    assert_eq!(hit[0], 21);
    match decode::<ServerMsg>(&hit).unwrap() {
        ServerMsg::HitConfirm { target, shield } => assert_eq!((target, shield), (7, true)),
        _ => panic!(),
    }
}
