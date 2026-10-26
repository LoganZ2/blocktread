//! Rocks on the battlefield, and the cube-face directions the generated skies are built on.

use super::Session;
use crate::Screen;
use crate::art::srgb;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use blocktread_shared::glam::Vec3 as GVec3;
use blocktread_shared::world::{Asteroid, rock_radius};

/// A drawn rock: its index in the client core's rock list.
#[derive(Component)]
pub struct Rock(usize);

pub(crate) fn face_dir(face: usize, u: f32, v: f32) -> Vec3 {
    match face {
        0 => Vec3::new(1.0, -v, -u),
        1 => Vec3::new(-1.0, -v, u),
        2 => Vec3::new(u, 1.0, v),
        3 => Vec3::new(u, -1.0, -v),
        4 => Vec3::new(u, -v, 1.0),
        _ => Vec3::new(-u, -v, -1.0),
    }
    .normalize()
}

pub fn spawn_rocks(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>, rocks: &[Asteroid]) {
    let cols = [0x6b6157u32, 0x585a5e, 0x77695a];
    let rmats: Vec<_> = cols.iter().map(|&c| mats.add(StandardMaterial { base_color: srgb(c), perceptual_roughness: 0.95, metallic: 0.05, ..default() })).collect();
    for (i, a) in rocks.iter().enumerate() {
        let mut mesh = Sphere::new(1.0).mesh().ico(3).unwrap();
        if let Some(VertexAttributeValues::Float32x3(ps)) = mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION) {
            for p in ps.iter_mut() {
                let d = GVec3::from(*p).normalize();
                let r = rock_radius(d, a.shape_seed);
                *p = [d.x * r * a.scale.x, d.y * r * a.scale.y, d.z * r * a.scale.z];
            }
        }
        mesh.duplicate_vertices();
        mesh.compute_flat_normals();
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(rmats[a.tint as usize % 3].clone()),
            Transform::from_translation(Vec3::from(a.pos.to_array())).with_scale(Vec3::splat(a.size)),
            Rock(i),
            DespawnOnExit(Screen::Flight),
        ));
    }
}

/// Turn every rock to the same server time the collisions use, so what you see is what you hit.
pub fn orient_rocks(session: Res<Session>, mut q: Query<(&mut Transform, &Rock)>) {
    let core = &session.core;
    let t = core.rock_time();
    for (mut tf, r) in &mut q {
        if let Some(a) = core.rocks.get(r.0) {
            tf.rotation = Quat::from_array(a.rotation(t).to_array());
        }
    }
}
