//! Where blocks sit changes how a ship flies: inertia per axis, engines off the centre-of-mass
//! line, and how long the reactors keep the guns fed.

use std::sync::Arc;
use voxwing_shared::analysis::{Stats, analyze, compute_stats};
use voxwing_shared::blocks::{Block, BlockType, Design};
use voxwing_shared::flight::{InputCmd, Kin, step_flight};
use voxwing_shared::glam::{Quat, Vec3};
use voxwing_shared::presets::{PRESETS, preset};
use voxwing_shared::ship::ShipBody;
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
fn symmetric_presets_fly_straight() {
    for p in PRESETS {
        let s = analyze(&preset(p.key)).stats;
        assert!(s.thrust_drift(1.8).length() < 1e-4, "{} drifts {:?}", p.key, s.thrust_drift(1.8));
    }
}

#[test]
fn weight_far_out_slows_the_axis_it_spins_about() {
    let base = stats(&preset("falcon"));
    // the same two armour blocks, once on the wingtips and once along the spine
    let tips = stats(&with(preset("falcon"), &[(6, 0, 3, BlockType::Armor), (-6, 0, 3, BlockType::Armor)]));
    let spine = stats(&with(preset("falcon"), &[(0, -1, 1, BlockType::Armor), (0, -1, 2, BlockType::Armor)]));
    assert!(tips.roll_rate < base.roll_rate * 0.85, "wingtip weight should cost roll: {} -> {}", base.roll_rate, tips.roll_rate);
    assert!(spine.roll_rate > tips.roll_rate * 1.1, "the same weight near the axis should roll better");
    assert!((tips.mass - spine.mass).abs() < 1e-3);
    // weight at both ends (the centre of mass stays put) makes pitch and yaw heavy but leaves
    // roll mostly alone
    let ends = stats(&with(preset("falcon"), &[(0, -1, -5, BlockType::Armor), (0, -1, 6, BlockType::Armor)]));
    assert!((ends.com.z - base.com.z).abs() < 0.1);
    assert!(ends.pitch_rate < base.pitch_rate * 0.9, "pitch {} -> {}", base.pitch_rate, ends.pitch_rate);
    // yaw less so: the wingspan already makes up much of its inertia
    assert!(ends.yaw_rate < base.yaw_rate * 0.94, "yaw {} -> {}", base.yaw_rate, ends.yaw_rate);
    assert!(ends.roll_rate > base.roll_rate * 0.97);
}

#[test]
fn gyros_steer_wherever_they_sit_but_weigh_more_far_out() {
    let base = stats(&preset("falcon"));
    let centre = stats(&with(preset("falcon"), &[(0, -1, 0, BlockType::Gyro)]));
    let tail = stats(&with(preset("falcon"), &[(0, 1, 4, BlockType::Gyro)]));
    assert!(centre.pitch_rate > base.pitch_rate * 1.04, "pitch {} -> {}", base.pitch_rate, centre.pitch_rate);
    assert!(centre.pitch_rate > tail.pitch_rate, "a gyro at the tail adds inertia it then has to turn");
}

#[test]
fn losing_an_engine_yaws_towards_the_dead_side() {
    let mut body = ShipBody::new(Arc::new(preset("falcon")));
    let right = body.stats.engines.iter().copied().max_by_key(|&i| body.design.blocks[i].x).unwrap();
    body.destroy(right);
    body.settle();
    let d = body.stats.thrust_drift(1.0);
    assert!(d.y > 0.1, "nose should swing right, drift {d:?}");
    // fly hands-off at full throttle: the heading wanders right, more on boost
    let fly = |buttons: u8| {
        let mut k = Kin::new(Vec3::ZERO, Quat::IDENTITY, &body.stats);
        for t in 0..120 {
            let cmd = InputCmd { tick: t, throttle: 255, buttons, ..Default::default() };
            step_flight(&mut k, &body.stats, &cmd, DT, 1.0);
        }
        k.fwd().x
    };
    let (cruise, boost) = (fly(0), fly(voxwing_shared::flight::BTN_BOOST));
    assert!(cruise > 0.1, "heading should drift right, fwd.x = {cruise}");
    assert!(boost > cruise, "boost pushes harder, so it should drift further");
    // and a ship with a clean thrust line holds its heading
    let clean = ShipBody::new(Arc::new(preset("falcon")));
    let mut k = Kin::new(Vec3::ZERO, Quat::IDENTITY, &clean.stats);
    for t in 0..120 {
        step_flight(&mut k, &clean.stats, &InputCmd { tick: t, throttle: 255, ..Default::default() }, DT, 1.0);
    }
    assert!(k.fwd().x.abs() < 1e-3);
}

#[test]
fn reactors_decide_how_long_the_guns_keep_firing() {
    let s = stats(&preset("falcon"));
    assert!(s.dps_sustained < s.dps, "two lasers outrun one battery");
    let lasts = s.fire_endurance.expect("falcon should run dry at full fire");
    assert!((lasts - 12.0).abs() < 0.5, "battery lasts {lasts}");
    let fed = stats(&with(preset("falcon"), &[(0, -1, 1, BlockType::Reactor)]));
    assert!(fed.fire_endurance.is_none());
    assert!((fed.dps_sustained - fed.dps).abs() < 1e-3);
    // missiles and cannons need no energy
    let s = stats(&preset("stinger"));
    assert!((s.dps_sustained - s.dps).abs() < 1e-3);
}

#[test]
fn lopsided_designs_may_fly_but_roll_towards_the_short_side() {
    // a falcon with no left wing at all: no longer refused, just warned about
    let mut d = preset("falcon");
    d.blocks.retain(|b| !(b.ty == BlockType::Wing && b.x < 0));
    let a = analyze(&d);
    assert!(a.ok(), "should be allowed to take off: {:?}", a.first_error());
    assert!(a.checks.iter().any(|c| c.title == "升力偏心"), "should warn about the roll");
    let s = a.stats;
    assert!(s.roll_drift < 0.0, "lift all on the right lifts the right side: roll left");
    assert!(s.roll_drift.abs() <= s.roll_rate * 0.75 + 1e-4, "the stick must still be able to hold it level");
    // the heavy side drops: armour hung under the right wing rolls the ship right
    let heavy = analyze(&with(preset("falcon"), &[(3, -1, 1, BlockType::Armor), (3, -1, 2, BlockType::Armor), (4, -1, 2, BlockType::Armor), (4, -1, 3, BlockType::Armor)])).stats;
    assert!(heavy.roll_drift > 0.0, "drift {}", heavy.roll_drift);
}

#[test]
fn small_asymmetry_is_trimmed_out() {
    let a = analyze(&with(preset("falcon"), &[(6, 0, 3, BlockType::Wing)]));
    assert!(a.ok());
    assert_eq!(a.stats.roll_drift, 0.0);
    assert!(!a.checks.iter().any(|c| c.title == "升力偏心"));
}
