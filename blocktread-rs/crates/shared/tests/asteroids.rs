//! Rock collisions must happen where the rock is drawn.

use std::sync::Arc;
use blocktread_shared::drive::{spawn_height, step_drive};
use blocktread_shared::flight::{InputCmd, Kin, collide_asteroids, look_rotation};
use blocktread_shared::glam::Vec3;
use blocktread_shared::presets::rover;
use blocktread_shared::ship::ShipBody;
use blocktread_shared::terrain::Terrain;
use blocktread_shared::world::{generate_boulders, rock_radius, tick_time};
use blocktread_shared::DT;

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
    for a in generate_boulders(42, &Terrain::generate(42)).iter().take(16) {
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
fn vehicles_stop_at_the_drawn_surface() {
    let body = ShipBody::new(Arc::new(rover()));
    let ground = Terrain::generate(7);
    let rocks = generate_boulders(7, &ground);
    let mut deepest = 0.0f32;
    for (ai, a) in rocks.iter().enumerate().take(10) {
        for di in 0..8 {
            // drive flat out at the rock from outside its bounding sphere
            let ang = di as f32 * std::f32::consts::TAU / 8.0;
            let dir = Vec3::new(ang.cos(), 0.0, ang.sin());
            let at = a.pos + dir * (a.r + 25.0);
            let rot = look_rotation(Vec3::ZERO, -dir, Vec3::Y);
            let mut k = Kin::new(at.with_y(spawn_height(&body, &ground, at.x, at.z, rot)), rot, &body.stats);
            let start = (ai * 1000 + di * 50) as u32;
            for tick in start..start + 360 {
                let cmd = InputCmd { tick, drive: 127, ..Default::default() };
                step_drive(&mut k, &body, &cmd, &ground, DT);
                let t = tick_time(tick);
                collide_asteroids(&mut k, &body, std::slice::from_ref(a), t);
                let r = a.rotation(t);
                for i in 0..body.alive.len() {
                    deepest = deepest.min(a.surface(body.world(k.pos, k.rot, i), r).0);
                }
            }
            assert!(k.pos.distance(a.pos) > a.r * 0.4, "vehicle ended up inside rock {ai} from direction {di}");
        }
    }
    // block centres may graze the surface (they are half a metre across) but never sink in
    assert!(deepest > -0.6, "a block centre sank {:.2} m into a rock", -deepest);
}
