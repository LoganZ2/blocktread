//! The cage: its walls stop vehicles and rounds, its ramps lead up onto the hill, a fast run
//! clears a pit from its kicker, and the layout leaves every spawn clear.

use std::sync::Arc;
use blocktread_shared::DT;
use blocktread_shared::ai::{AiPilot, Contact, Skill};
use blocktread_shared::arena::{CAGE_HALF, HILL_HALF, HILL_TOP, KILL_Y, PIT_HALF_W, PIT_LONG, PIT_NEAR, RIM};
use blocktread_shared::drive::{spawn_height, step_drive};
use blocktread_shared::flight::{BTN_BOOST, InputCmd, Kin};
use blocktread_shared::glam::{Quat, Vec3};
use blocktread_shared::presets::{PRESETS, preset};
use blocktread_shared::ship::ShipBody;
use blocktread_shared::terrain::Terrain;

/// Set down at (x, z) facing along `dir` (on the ground).
fn set_down(body: &ShipBody, ground: &Terrain, x: f32, z: f32, dir: Vec3) -> Kin {
    let rot = Quat::from_rotation_arc(Vec3::NEG_Z, dir.normalize());
    let y = spawn_height(body, ground, x, z, rot);
    Kin::new(Vec3::new(x, y, z), rot, &body.stats)
}

#[test]
fn a_wall_stops_a_vehicle_at_full_speed() {
    let ground = Terrain::cage(1);
    for p in PRESETS {
        let body = ShipBody::new(Arc::new(preset(p.key)));
        let mut k = set_down(&body, &ground, 60.0, 40.0, Vec3::X);
        let mut hardest: f32 = 0.0;
        let mut most_x = f32::MIN;
        for t in 0..(9.0 / DT) as u32 {
            let knock = step_drive(&mut k, &body, &InputCmd { tick: t, drive: 127, buttons: BTN_BOOST, ..Default::default() }, &ground, DT);
            hardest = hardest.max(knock.map_or(0.0, |i| i.speed));
            most_x = most_x.max(k.pos.x);
            assert!(k.pos.is_finite() && k.vel.length() < 80.0, "{} blew up at tick {t}: {k:?}", p.key);
        }
        println!("{}: hit the wall at {hardest:.1} m/s, got to x = {most_x:.2} (wall at {CAGE_HALF}), ended {:.1} m/s", p.key, k.vel.length());
        assert!(most_x + body.radius * 0.5 < CAGE_HALF, "{} went into the wall", p.key);
        assert!(hardest > 8.0, "{} should feel the wall", p.key);
    }
}

#[test]
fn a_ramp_leads_up_onto_the_hill() {
    let ground = Terrain::cage(1);
    let a = ground.arena.as_ref().unwrap();
    for p in PRESETS {
        let body = ShipBody::new(Arc::new(preset(p.key)));
        // from the south, at a crawl, up the ramp and onto the top
        let mut k = set_down(&body, &ground, 0.0, -34.0, Vec3::Z);
        let mut on_top = 0.0;
        for t in 0..(12.0 / DT) as u32 {
            // brake once over the top's near edge
            let ahead = k.vel.dot(k.fwd());
            let drive = if k.pos.z < -6.0 { 70 } else if ahead > 0.3 { -127 } else { 0 };
            step_drive(&mut k, &body, &InputCmd { tick: t, drive, ..Default::default() }, &ground, DT);
            let floor = ground.surface(k.pos.x, k.pos.z);
            assert!(k.pos.y > floor - 0.6, "{} sank into the hill at tick {t}: {:.2} under {floor:.2}", p.key, k.pos.y);
            if a.on_hill(k.pos) {
                on_top += DT;
            }
        }
        println!("{}: {on_top:.1} s on the hill, ended at {:.1?}, up {:.2}", p.key, k.pos, k.up().y);
        assert!(on_top > 2.0, "{} should get up onto the hill", p.key);
        assert!(k.pos.y > HILL_TOP && k.up().y > 0.9, "{} should be sitting on top", p.key);
    }
}

#[test]
fn a_kicker_jumps_the_pit_at_speed() {
    let ground = Terrain::cage(1);
    for p in PRESETS {
        let body = ShipBody::new(Arc::new(preset(p.key)));
        // a run-up from beside the hill, straight at the kicker on the north side
        let mut k = set_down(&body, &ground, 0.0, 14.0, Vec3::Z);
        let (mut launch, mut fell, mut air) = (0.0, false, 0.0f32);
        for t in 0..(8.0 / DT) as u32 {
            step_drive(&mut k, &body, &InputCmd { tick: t, drive: 127, buttons: BTN_BOOST, ..Default::default() }, &ground, DT);
            if k.pos.z < PIT_NEAR {
                launch = k.vel.length();
            }
            if (PIT_NEAR..PIT_NEAR + PIT_LONG).contains(&k.pos.z) {
                air = air.max(k.pos.y);
            }
            fell |= k.pos.y < KILL_Y;

            if fell || k.pos.z > 80.0 {
                break;
            }
        }
        println!("{}: {:.1} m/s off the kicker, {air:.1} m over the pit, {}", p.key, launch, if fell { "fell in".to_string() } else { format!("landed, z = {:.0}", k.pos.z) });
        // the quick ones make it; the heavy ones aren't meant to
        if launch > 17.0 {
            assert!(!fell && k.pos.z > 80.0 && k.up().y > 0.5, "{} should clear the pit and drive on", p.key);
        }
    }
}

#[test]
fn the_cage_leaves_its_spawns_clear_and_stops_rounds_at_its_walls() {
    let ground = Terrain::cage(1);
    let a = ground.arena.as_ref().unwrap();
    for s in a.spawns.iter().chain(&a.team_spawns[0]).chain(&a.team_spawns[1]) {
        for dx in [-6.0, 0.0, 6.0] {
            for dz in [-6.0, 0.0, 6.0] {
                let (x, z) = (s.x + dx, s.z + dz);
                assert_eq!(ground.surface(x, z), 0.0, "something stands by the spawn at {s}");
                assert!(!a.in_pit(x, z));
            }
        }
    }
    // the ramps meet the ground: no step at the foot of any of them
    for w in &a.hill_ways {
        assert!(ground.surface(w.x, w.z).abs() < 0.01, "the way up at {w} isn't level ground");
    }
    // a round fired at the wall stops at its face; one over the hill flies on
    let t = ground.segment_hit(Vec3::new(100.0, 2.0, 0.0), Vec3::new(130.0, 2.0, 0.0)).unwrap();
    assert!((100.0 + t * 30.0 - CAGE_HALF).abs() < 0.01);
    assert!(ground.segment_hit(Vec3::new(-30.0, 9.0, 0.0), Vec3::new(30.0, 9.0, 0.0)).is_none());
    // into a pit's floor, not its rim
    let t = ground.segment_hit(Vec3::new(0.0, 5.0, 56.0), Vec3::new(0.0, -20.0, 56.0)).unwrap();
    assert!((5.0 - t * 25.0 + 14.0).abs() < 0.1, "should reach the pit's floor");
}

#[test]
fn nothing_sits_hung_over_a_pit_side() {
    // set down along a pit with a side over it, someone to chase across it: every vehicle
    // either drives off the rim or drops in (or hangs over it long enough that the server calls
    // it lost), rather than sitting on it
    let ground = Terrain::cage(1);
    let arena = ground.arena.as_ref().unwrap();
    let z = PIT_NEAR + PIT_LONG * 0.5;
    for p in PRESETS {
        let body = ShipBody::new(Arc::new(preset(p.key)));
        for over in [0.5, 1.5] {
            let x = PIT_HALF_W + body.radius * 0.5 - over;
            let mut k = set_down(&body, &ground, x, z, Vec3::Z);
            let mut ai = AiPilot::new(Skill::ffa_bot(), 3);
            let mut hung = 0.0;
            let them = [Contact { id: 9, team: 9, pos: Vec3::new(-40.0, 1.0, z), vel: Vec3::ZERO, radius: 5.0 }];
            for t in 0..(10.0 / DT) as u32 {
                let cmd = ai.think(&k, &body, 0.0, 1, &them, &[], &ground, t);
                step_drive(&mut k, &body, &cmd, &ground, DT);
                hung = if arena.in_pit(k.pos.x, k.pos.z) { hung + DT } else { 0.0 };
                if k.pos.y < KILL_Y || hung > 2.0 {
                    break;
                }
            }
            let off = k.pos.x.abs() - PIT_HALF_W > RIM || (k.pos.z - z).abs() > PIT_LONG;
            println!("{} from x = {x:.1}: ended at {:.1?}", p.key, k.pos);
            assert!(k.pos.y < KILL_Y || hung > 2.0 || off, "{} sat on the rim at {:.1?}", p.key, k.pos);
        }
    }
}

#[test]
fn a_bot_sent_to_the_hill_gets_up_and_stays() {
    // every vehicle, from every 2v2 spawn, with no one about: up a ramp (past the vents at their
    // feet) and round the top
    let ground = Terrain::cage(1);
    let a = ground.arena.as_ref().unwrap();
    let goal = (a.hill.centre().with_y(HILL_TOP), HILL_HALF - 3.0);
    let mut worst: f32 = 0.0;
    for p in PRESETS {
        let body = ShipBody::new(Arc::new(preset(p.key)));
        for (side, s) in a.team_spawns.iter().enumerate().flat_map(|(i, v)| v.iter().map(move |s| (i, s))) {
            let mut k = set_down(&body, &ground, s.x, s.z, if side == 0 { Vec3::X } else { Vec3::NEG_X });
            let mut ai = AiPilot::new(Skill::ffa_bot(), 7);
            ai.goal = Some(goal);
            let (mut up_at, mut on) = (None, 0.0);
            for t in 0..(40.0 / DT) as u32 {
                let cmd = ai.think(&k, &body, 0.0, 1, &[], &[], &ground, t);
                step_drive(&mut k, &body, &cmd, &ground, DT);
                if a.on_hill(k.pos) {
                    up_at.get_or_insert(t as f32 * DT);
                    on += DT;
                }
            }
            println!("{} from {s}: up after {:?} s, on top {on:.0} s of 40", p.key, up_at.map(|t| t.round()));
            let took = up_at.unwrap_or(f32::INFINITY);
            worst = worst.max(took);
            assert!(took < 30.0, "{} from {s} should get up onto the hill", p.key);
            assert!(on > (40.0 - took) * 0.7, "{} from {s} should stay up there", p.key);
        }
    }
    println!("slowest up: {worst:.0} s");
}

