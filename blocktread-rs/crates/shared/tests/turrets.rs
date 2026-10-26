//! Guns turn to the aim point, every one of them all the way round and up and down, but never
//! point (or fire) through their own vehicle: where they can't get, they stop at the limit and hold
//! their fire. Each sits on a mount on the face that holds it.

use std::sync::Arc;
use blocktread_shared::DT;
use blocktread_shared::blocks::{Block, BlockType, Design};
use blocktread_shared::flight::{BTN_FIRE, InputCmd, Kin, quantize_aim};
use blocktread_shared::glam::{IVec3, Quat, Vec3};
use blocktread_shared::presets::preset;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::weapons::{Mount, Shot, WeaponState, barrel_dir, step_weapons};

fn with(mut d: Design, extra: &[(i32, i32, i32, BlockType)]) -> Design {
    for &(x, y, z, t) in extra {
        d.blocks.push(Block::new(x, y, z, t, None));
    }
    d
}

fn mount_at(body: &ShipBody, x: i8, y: i8, z: i8) -> &Mount {
    body.mounts.iter().find(|m| {
        let b = body.design.blocks[m.block];
        (b.x, b.y, b.z) == (x, y, z)
    }).unwrap()
}

#[test]
fn a_gun_mounts_on_the_face_that_holds_it() {
    // on the deck: plate underneath; hung off the side of the deck: plate on that side; under the
    // deck: plate on top
    let d = with(preset("rover"), &[(-2, 0, 1, BlockType::Laser), (0, -1, 1, BlockType::Laser)]);
    let body = ShipBody::new(Arc::new(d));
    assert_eq!(mount_at(&body, 1, 1, -2).face, IVec3::NEG_Y);
    assert_eq!(mount_at(&body, -2, 0, 1).face, IVec3::X);
    assert_eq!(mount_at(&body, 0, -1, 1).face, IVec3::Y);
}

/// Run `secs` of aiming at `target` (world), firing; returns the weapon state and the shots.
fn aim(body: &ShipBody, target: Vec3, secs: f32) -> (WeaponState, Vec<Shot>) {
    let mut k = Kin::new(Vec3::ZERO, Quat::IDENTITY, &body.stats);
    let mut ws = WeaponState::new(body);
    let mut shots = Vec::new();
    for t in 0..(secs / DT) as u32 {
        k.energy = 1e6;
        step_weapons(&mut k, body, &mut ws, &InputCmd { tick: t, buttons: BTN_FIRE, aim: quantize_aim(target), ..Default::default() }, DT, t, false, &mut shots);
    }
    (ws, shots)
}

fn crosses_itself(body: &ShipBody, s: &Shot) -> bool {
    body.ray(Vec3::ZERO, Quat::IDENTITY, s.pos, s.pos + s.vel.normalize() * 14.0).is_some()
}

#[test]
fn guns_follow_the_aim_but_never_through_their_own_vehicle() {
    let body = ShipBody::new(Arc::new(preset("bison")));
    // off to the right and up: every gun gets there (the roof railgun exactly, up and all)
    let target = Vec3::new(60.0, 20.0, -40.0);
    let (ws, shots) = aim(&body, target, 2.0);
    for m in &body.mounts {
        let (yaw, pitch) = ws.aim[m.block];
        let dir = barrel_dir(yaw, pitch);
        let want = (target - body.local(m.block)).normalize();
        assert!(dir.angle_between(want) < 0.03, "{:?} points {dir} for {want}", body.design.blocks[m.block].ty);
    }
    assert!(!shots.is_empty());
    assert!(!shots.iter().any(|s| crosses_itself(&body, s)), "no round leaves through its own blocks");
    // straight behind: the roof railgun turns all the way round; the cannons down by the cab
    // can't see past it, so they turn as far as is clear and hold their fire
    let (ws, shots) = aim(&body, Vec3::new(0.0, 1.0, 80.0), 3.0);
    assert!(!shots.iter().any(|s| crosses_itself(&body, s)));
    let railgun = body.mounts.iter().find(|m| body.design.blocks[m.block].ty == BlockType::Railgun).unwrap();
    assert!(ws.aim[railgun.block].0.abs() > 2.9, "the roof gun turns all the way round");
    assert!(shots.iter().any(|s| s.weapon == BlockType::Railgun));
    assert!(!shots.iter().any(|s| s.weapon == BlockType::Cannon), "the cannons can't bear: they don't fire");
}

#[test]
fn a_gun_that_cant_get_there_stops_at_its_limit() {
    // a wall right beside a deck gun, and the target straight out past the wall at its height
    let d = with(preset("rover"), &[(2, 1, -2, BlockType::Hull)]);
    let body = ShipBody::new(Arc::new(d));
    let gun = mount_at(&body, 1, 1, -2).block;
    let target = body.local(gun) + Vec3::new(100.0, 0.0, 0.0);
    let (ws, shots) = aim(&body, target, 2.0);
    let (yaw, pitch) = ws.aim[gun];
    println!("stopped at yaw {:.0}°, pitch {:.0}°", yaw.to_degrees(), pitch.to_degrees());
    // it turns toward the wall and stops there: no climbing over it or swinging round behind
    assert!(yaw > 0.3 && yaw < 1.4, "turned right, short of the wall: {yaw}");
    assert!(pitch.abs() < 0.01, "stays level: {pitch}");
    assert!(!shots.iter().any(|s| s.block == gun), "it can't bear, so it holds its fire");
    // once the target comes out from behind the wall, it gets there and fires
    let (ws, shots) = aim(&body, body.local(gun) + Vec3::new(60.0, 0.0, -100.0), 2.0);
    assert!(barrel_dir(ws.aim[gun].0, ws.aim[gun].1).angle_between(Vec3::new(60.0, 0.0, -100.0).normalize()) < 0.03);
    assert!(shots.iter().any(|s| s.block == gun));
}

#[test]
fn a_boxed_in_gun_is_warned_about() {
    // a laser walled in on every side
    let mut d = preset("rover");
    d.blocks.push(Block::new(0, 2, 1, BlockType::Laser, None));
    for (x, y, z) in [(1, 2, 1), (-1, 2, 1), (0, 3, 1), (0, 2, 0), (0, 2, 2)] {
        d.blocks.push(Block::new(x, y, z, BlockType::Hull, None));
    }
    let body = ShipBody::new(Arc::new(d.clone()));
    assert!(mount_at(&body, 0, 2, 1).boxed_in());
    let a = blocktread_shared::analysis::analyze(&d);
    assert!(a.checks.iter().any(|c| c.title == "武器" && c.msg.contains("转不出去")));
}
