//! The vehicle model on its own: the test buggy settles on its springs, drives, steers, brakes,
//! and keeps its wheels on the generated ground.

use std::sync::Arc;
use blocktread_shared::DT;
use blocktread_shared::drive::{performance, spawn_height, step_drive};
use blocktread_shared::flight::{BTN_FLIP, InputCmd, Kin, q_axis};
use blocktread_shared::glam::{Quat, Vec3};
use blocktread_shared::presets::rover;
use blocktread_shared::rng::Rng;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::terrain::Terrain;

fn buggy() -> ShipBody {
    ShipBody::new(Arc::new(rover()))
}

fn set_down(body: &ShipBody, ground: &Terrain, x: f32, z: f32) -> Kin {
    let y = spawn_height(body, ground, x, z, Quat::IDENTITY);
    Kin::new(Vec3::new(x, y, z), Quat::IDENTITY, &body.stats)
}

fn run(k: &mut Kin, body: &ShipBody, ground: &Terrain, secs: f32, cmd: InputCmd) {
    for t in 0..(secs / DT) as u32 {
        step_drive(k, body, &InputCmd { tick: t, ..cmd }, ground, DT);
    }
}

#[test]
fn the_buggy_settles_on_its_springs_and_stays_put() {
    let (body, ground) = (buggy(), Terrain::flat());
    let (accel, top) = performance(&body);
    println!("buggy: mass {:.1}, {} wheels, accel {accel:.1} m/s², top {top:.1} m/s", body.stats.mass, body.movers.len());
    assert_eq!(body.movers.len(), 4);
    let mut k = set_down(&body, &ground, 0.0, 0.0);
    run(&mut k, &body, &ground, 3.0, InputCmd::default());
    let y = k.pos.y;
    run(&mut k, &body, &ground, 2.0, InputCmd::default());
    println!("rest height {:.3}, drift {:.4} m, speed {:.4}", k.pos.y, k.pos.with_y(0.0).length(), k.vel.length());
    assert!((k.pos.y - y).abs() < 0.01, "still bouncing: {y} -> {}", k.pos.y);
    assert!(k.vel.length() < 0.05 && k.ang.length() < 0.02, "should sit still: {k:?}");
    assert!(k.up().y > 0.999, "level on level ground");
}

#[test]
fn it_drives_steers_and_brakes() {
    let (body, ground) = (buggy(), Terrain::flat());
    let (_, top) = performance(&body);
    let mut k = set_down(&body, &ground, 0.0, 0.0);
    run(&mut k, &body, &ground, 1.0, InputCmd::default());
    run(&mut k, &body, &ground, 8.0, InputCmd { drive: 127, ..Default::default() });
    let v = k.vel.dot(k.fwd());
    println!("after 8 s full ahead: {v:.1} m/s (top {top:.1}), {:.0} m", -k.pos.z);
    assert!(v > top * 0.8, "should be near top speed");
    assert!(k.up().y > 0.99);
    // full lock at speed: it turns, and stays on its wheels
    let heading = k.fwd();
    let mut min_up: f32 = 1.0;
    for t in 0..(3.0 / DT) as u32 {
        step_drive(&mut k, &body, &InputCmd { tick: t, drive: 127, steer: 127, ..Default::default() }, &ground, DT);
        min_up = min_up.min(k.up().y);
    }
    let turned = heading.angle_between(k.fwd()).to_degrees();
    println!("3 s of full right lock: turned {turned:.0}°, most lean {:.1}°", min_up.acos().to_degrees());
    assert!(turned > 60.0, "should turn");
    assert!(heading.cross(k.fwd()).y < 0.0, "to the right");
    assert!(min_up > 0.85, "shouldn't roll over in a turn");
    // brakes: reverse until stopped
    let mut stop = 0.0;
    while k.vel.length() > 0.5 && stop < 10.0 {
        step_drive(&mut k, &body, &InputCmd { drive: -127, ..Default::default() }, &ground, DT);
        stop += DT;
    }
    println!("stopped from speed in {stop:.1} s");
    assert!(stop < 4.0);
}

#[test]
fn random_driving_over_the_hills_keeps_it_sane() {
    let ground = Terrain::generate(42);
    let body = buggy();
    let mut rng = Rng::new(9);
    let mut k = set_down(&body, &ground, 0.0, 0.0);
    let mut cmd = InputCmd::default();
    let mut upright = 0;
    let n = (90.0 / DT) as u32;
    for t in 0..n {
        if t % 60 == 0 {
            cmd = InputCmd { drive: q_axis(rng.range(-0.4, 1.0)), steer: q_axis(rng.range(-1.0, 1.0)), ..Default::default() };
        }
        // a roll-over gets righted, like a player would
        let flip = if k.up().y < 0.3 { BTN_FLIP } else { 0 };
        step_drive(&mut k, &body, &InputCmd { tick: t, buttons: flip, ..cmd }, &ground, DT);
        assert!(k.pos.is_finite() && k.vel.is_finite() && k.ang.is_finite(), "blew up at tick {t}: {k:?}");
        let g = ground.height(k.pos.x, k.pos.z);
        assert!(k.pos.y > g - 0.5, "sank into the ground at tick {t}: {:.2} under {g:.2}", k.pos.y);
        assert!(k.vel.length() < 60.0 && k.ang.length() < 12.0, "flung at tick {t}: {k:?}");
        if k.up().y > 0.7 {
            upright += 1;
        }
    }
    println!("upright {:.0}% of the time, ended {:.0} m out", upright as f32 * 100.0 / n as f32, k.pos.with_y(0.0).length());
    assert!(upright as f32 > n as f32 * 0.9);
}

#[test]
fn the_same_inputs_give_the_same_drive() {
    let ground = Terrain::generate(3);
    let body = buggy();
    let go = |k: &mut Kin| {
        for t in 0..600 {
            step_drive(k, &body, &InputCmd { tick: t, drive: 127, steer: q_axis(((t / 90) % 3) as f32 - 1.0), ..Default::default() }, &ground, DT);
        }
    };
    let (mut a, mut b) = (set_down(&body, &ground, 10.0, 20.0), set_down(&body, &ground, 10.0, 20.0));
    go(&mut a);
    go(&mut b);
    assert_eq!(a, b);
}

#[test]
fn the_presets_drive_and_fit_their_tiers() {
    use blocktread_shared::analysis::analyze;
    use blocktread_shared::presets::{PRESETS, preset};
    let ground = Terrain::flat();
    for (p, budget) in PRESETS.iter().zip([80.0, 80.0, 160.0, 160.0, 350.0]) {
        let d = preset(p.key);
        let a = analyze(&d);
        let body = ShipBody::new(Arc::new(d.clone()));
        let (accel, top) = performance(&body);
        let mut k = set_down(&body, &ground, 0.0, 0.0);
        run(&mut k, &body, &ground, 2.0, InputCmd::default());
        run(&mut k, &body, &ground, 10.0, InputCmd { drive: 127, ..Default::default() });
        let v = k.vel.dot(k.fwd());
        let mut lean: f32 = 0.0;
        for t in 0..(3.0 / DT) as u32 {
            step_drive(&mut k, &body, &InputCmd { tick: t, drive: 127, steer: 127, ..Default::default() }, &ground, DT);
            lean = lean.max(k.up().angle_between(Vec3::Y).to_degrees());
        }
        let problems: Vec<String> = a.checks.iter().filter(|c| c.level != blocktread_shared::analysis::Level::Ok).map(|c| format!("{}: {}", c.title, c.msg)).collect();
        println!(
            "{} {}: cost {:.0}/{budget}, mass {:.0}, {} blocks, {} wheels, power/weight {:.2}, accel {accel:.1}, top {:.0} km/h (10 s: {:.0}), tip {:.0}°, lean in a hard turn {lean:.1}°, dps {:.0} · {problems:?}",
            p.key, p.name, a.stats.cost, a.stats.mass, d.blocks.len(), a.stats.grounded.len(), a.stats.power / a.stats.mass, top * 3.6, v * 3.6, a.stats.tip, a.stats.dps
        );
        assert!(a.ok(), "{} fails its checks: {problems:?}", p.key);
        assert!(a.stats.cost <= budget, "{} costs {:.0}, over {budget}", p.key, a.stats.cost);
        assert!(v > top * 0.75, "{} should get near its top speed", p.key);
        assert!(lean < 25.0 && k.up().y > 0.9, "{} leans {lean:.0}° in a hard turn", p.key);
    }
}

#[test]
fn hover_pads_turn_fast_and_drop_when_the_energy_runs_out() {
    use blocktread_shared::presets::preset;
    let ground = Terrain::flat();
    let turn_rate = |key: &str| {
        let body = ShipBody::new(Arc::new(preset(key)));
        let mut k = set_down(&body, &ground, 0.0, 0.0);
        run(&mut k, &body, &ground, 1.0, InputCmd::default());
        run(&mut k, &body, &ground, 4.0, InputCmd { drive: 80, ..Default::default() });
        let h0 = k.fwd();
        run(&mut k, &body, &ground, 1.0, InputCmd { drive: 80, steer: 127, ..Default::default() });
        h0.angle_between(k.fwd()).to_degrees()
    };
    let (hover, wheels) = (turn_rate("wasp"), turn_rate("rover"));
    println!("one second of full lock: hover pads {hover:.0}°, wheels {wheels:.0}°");
    assert!(hover > wheels * 1.5, "hover pads should out-turn wheels");
    // floating: the hull stays clear of the ground; out of energy, it settles onto it
    let body = ShipBody::new(Arc::new(preset("wasp")));
    let mut k = set_down(&body, &ground, 0.0, 0.0);
    run(&mut k, &body, &ground, 2.0, InputCmd::default());
    let floating = k.pos.y;
    for t in 0..120 {
        k.energy = 0.0;
        step_drive(&mut k, &body, &InputCmd { tick: t, ..Default::default() }, &ground, DT);
    }
    println!("hover height {floating:.2}, after the energy ran out {:.2}", k.pos.y);
    assert!(k.pos.y < floating - 0.5, "should drop without energy");
}

#[test]
fn an_empty_battery_locks_the_boost_until_shift_is_pressed_again() {
    use blocktread_shared::flight::BTN_BOOST;
    let (body, ground) = (buggy(), Terrain::flat());
    let mut k = set_down(&body, &ground, 0.0, 0.0);
    run(&mut k, &body, &ground, 1.0, InputCmd::default());
    let boost = InputCmd { drive: 127, buttons: BTN_BOOST, ..Default::default() };
    let mut t = 0;
    while !k.boost_hold && t < 600 {
        step_drive(&mut k, &body, &InputCmd { tick: t, ..boost }, &ground, DT);
        assert!(k.boosting, "boosts while there is energy");
        t += 1;
    }
    println!("battery ran dry after {:.1} s of boost", t as f32 * DT);
    assert!(k.boost_hold && k.boost_lock > 0.9);
    // still holding Shift: no boost, however long, though the battery refills
    run(&mut k, &body, &ground, 3.0, boost);
    assert!(!k.boosting && k.energy > 20.0, "held through the lock: still off");
    // let go and press again: it boosts
    step_drive(&mut k, &body, &InputCmd::default(), &ground, DT);
    step_drive(&mut k, &body, &boost, &ground, DT);
    assert!(k.boosting);
    // released at once after running dry: the lock still holds for a second
    while !k.boost_hold {
        step_drive(&mut k, &body, &boost, &ground, DT);
    }
    step_drive(&mut k, &body, &InputCmd::default(), &ground, DT);
    step_drive(&mut k, &body, &boost, &ground, DT);
    assert!(!k.boosting, "within the second after running dry");
    run(&mut k, &body, &ground, 1.0, InputCmd::default());
    step_drive(&mut k, &body, &boost, &ground, DT);
    assert!(k.boosting, "after the lock, a fresh press boosts");
}
