//! Asteroid collisions must happen where the rock is drawn.

use std::sync::Arc;
use voxwing_shared::flight::{InputCmd, Kin, collide_asteroids, step_flight};
use voxwing_shared::glam::{Quat, Vec3};
use voxwing_shared::presets::preset;
use voxwing_shared::ship::ShipBody;
use voxwing_shared::world::{generate_asteroids, rock_radius, tick_time};
use voxwing_shared::DT;

/// Directions spread evenly over the sphere (golden spiral).
fn directions(n: usize) -> impl Iterator<Item = Vec3> {
    (0..n).map(move |i| {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - y * y).sqrt();
        let phi = i as f32 * 2.399_963;
        Vec3::new(r * phi.cos(), y, r * phi.sin())
    })
}

#[test]
fn surface_matches_rendered_mesh() {
    for a in generate_asteroids(42).iter().take(16) {
        let rot = a.rotation(tick_time(123_456));
        for d in directions(300) {
            // the vertex exactly where the renderer puts it: unit direction pushed out by
            // rock_radius, stretched by scale, then the entity's size and rotation
            let v = a.pos + rot * (d * rock_radius(d, a.shape_seed) * a.scale * a.size);
            let (dist, n) = a.surface(v, rot);
            assert!(dist.abs() < 1e-3 * a.size, "vertex off the surface by {dist}");
            assert!(a.surface(v + n, rot).0 > 0.0, "a metre out along the normal should be outside");
            assert!(a.surface(v - n, rot).0 < 0.0, "a metre in along the normal should be inside");
            assert!(v.distance(a.pos) <= a.r, "bounding radius {} too small for {}", a.r, v.distance(a.pos));
        }
    }
}

#[test]
fn ships_stop_at_the_drawn_surface() {
    let body = ShipBody::new(Arc::new(preset("falcon")));
    let rocks = generate_asteroids(7);
    let mut deepest = 0.0f32;
    for (ai, a) in rocks.iter().enumerate().take(10) {
        for (di, dir) in directions(12).enumerate() {
            // fly flat out straight at the rock from outside its bounding sphere
            let rot = Quat::from_rotation_arc(Vec3::NEG_Z, -dir);
            let mut k = Kin::new(a.pos + dir * (a.r + 25.0), rot, &body.stats);
            k.vel = -dir * 160.0;
            let start = (ai * 1000 + di * 50) as u32;
            for tick in start..start + 180 {
                let cmd = InputCmd { tick, throttle: 255, ..Default::default() };
                step_flight(&mut k, &body.stats, &cmd, DT, 1.0);
                let t = tick_time(tick);
                collide_asteroids(&mut k, &body, std::slice::from_ref(a), t);
                let r = a.rotation(t);
                for i in 0..body.alive.len() {
                    deepest = deepest.min(a.surface(body.world(k.pos, k.rot, i), r).0);
                }
            }
            assert!(k.pos.distance(a.pos) > a.r * 0.4, "ship ended up inside rock {ai} from direction {di}");
        }
    }
    // block centres may graze the surface (they are half a metre across) but never sink in
    assert!(deepest > -0.6, "a block centre sank {:.2} m into a rock", -deepest);
}
