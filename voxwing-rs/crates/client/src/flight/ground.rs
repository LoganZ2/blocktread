//! The ground the vehicles drive on: a daytime sky, the sun and its shadows, haze, and the terrain
//! mesh, cut into the very triangles the physics drives over.

use crate::Screen;
use crate::art::srgb;
use bevy::asset::RenderAssetUsages;
use bevy::light::{CascadeShadowConfigBuilder, Skybox};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension};
use voxwing_shared::glam::Vec3 as GVec3;
use voxwing_shared::blocks::{BlockType, Design};
use voxwing_shared::drive::WHEEL_R;
use voxwing_shared::rng::hash01;
use voxwing_shared::world::fbm3;
use voxwing_shared::terrain::{CELL, HALF, N, Terrain};

/// Towards the sun.
pub const SUN: Vec3 = Vec3::new(-0.45, 0.62, -0.64);
/// The haze on the horizon, which the fog fades into.
pub const HAZE: u32 = 0xc3cdd4;

#[derive(Resource)]
pub struct DaySky(pub Handle<Image>);

/// The drawn terrain (one per match).
#[derive(Component)]
pub struct Ground;

fn linear(c: u32) -> Vec3 {
    let l = srgb(c).to_linear();
    Vec3::new(l.red, l.green, l.blue)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Sky colour (linear) looking along `d`: haze at the horizon deepening to blue overhead, and the
/// sun with a glow round it.
fn sky(d: Vec3) -> Vec3 {
    let (horizon, zenith) = (linear(HAZE), linear(0x5f8fc8));
    let mut c = horizon.lerp(zenith, d.y.max(0.0).powf(0.5));
    if d.y < 0.0 {
        c = horizon * (1.0 + d.y * 0.3);
    }
    let s = d.dot(SUN.normalize()).max(0.0);
    c + Vec3::new(1.0, 0.92, 0.75) * (s.powf(1500.0) * 6.0 + s.powf(40.0) * 0.35 + s.powf(6.0) * 0.08)
}

/// Six 256×256 faces; the sky is smooth, so that is plenty.
pub fn build_day_sky(images: &mut Assets<Image>) -> Handle<Image> {
    let s = 256usize;
    let mut data = vec![0u8; s * s * 6 * 4];
    for face in 0..6 {
        for y in 0..s {
            for x in 0..s {
                let u = x as f32 / (s - 1) as f32 * 2.0 - 1.0;
                let v = y as f32 / (s - 1) as f32 * 2.0 - 1.0;
                let c = sky(super::env::face_dir(face, u, v));
                let to8 = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
                let i = ((face * s + y) * s + x) * 4;
                data[i..i + 4].copy_from_slice(&[to8(c.x), to8(c.y), to8(c.z), 255]);
            }
        }
    }
    let mut img = Image::new(Extent3d { width: s as u32, height: (s * 6) as u32, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    img.reinterpret_stacked_2d_as_array(6).expect("six square faces");
    img.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
    images.add(img)
}

/// How bright the day sky is drawn.
pub const SKY_BRIGHTNESS: f32 = 2500.0;

/// The sun (with shadows near the camera) and a sky-blue fill, for as long as `screen` lasts.
pub fn spawn_daylight(commands: &mut Commands, screen: Screen) {
    commands.spawn((
        DirectionalLight { illuminance: 20000.0, color: srgb(0xfff1dc), shadow_maps_enabled: true, ..default() },
        Transform::from_translation(SUN * 100.0).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder { num_cascades: 3, maximum_distance: 280.0, first_cascade_far_bound: 35.0, ..default() }.build(),
        DespawnOnExit(screen),
    ));
    commands.insert_resource(GlobalAmbientLight { color: Color::srgb(0.72, 0.8, 0.95), brightness: 1100.0, ..default() });
}

/// Haze that swallows the far side of the arena.
pub fn fog() -> DistanceFog {
    haze(160.0, 950.0)
}

fn haze(start: f32, end: f32) -> DistanceFog {
    DistanceFog { color: srgb(HAZE), directional_light_color: Color::srgba(1.0, 0.93, 0.78, 0.45), directional_light_exponent: 24.0, falloff: FogFalloff::Linear { start, end } }
}

/// The terrain mesh: one vertex per height sample, two triangles per cell split the way
/// `Terrain::height` splits them. Dry grass on the flats, bare earth on slopes, rock where it's
/// steep.
fn terrain_mesh(ground: &Terrain) -> Mesh {
    let (grass, dry, earth, stone) = (linear(0x667542), linear(0x9a9160), linear(0x86694a), linear(0x77736c));
    let mut pos = Vec::with_capacity(N * N);
    let mut nrm = Vec::with_capacity(N * N);
    let mut col = Vec::with_capacity(N * N);
    let at = |i: isize, j: isize| ground.sample(i.clamp(0, N as isize - 1) as usize, j.clamp(0, N as isize - 1) as usize);
    for j in 0..N {
        for i in 0..N {
            let (ii, jj) = (i as isize, j as isize);
            let (x, z) = (-HALF + i as f32 * CELL, -HALF + j as f32 * CELL);
            pos.push([x, at(ii, jj), z]);
            let n = Vec3::new(at(ii - 1, jj) - at(ii + 1, jj), 2.0 * CELL, at(ii, jj - 1) - at(ii, jj + 1)).normalize();
            nrm.push(n.to_array());
            // broad patches of green and dry grass, earth on slopes, rock where it's steep
            let steep = 1.0 - n.y;
            let patch = smoothstep(0.35, 0.6, fbm3(GVec3::new(x / 90.0, 3.7, z / 90.0), 3));
            let mut c = grass.lerp(dry, patch).lerp(earth, smoothstep(0.04, 0.13, steep)).lerp(stone, smoothstep(0.16, 0.3, steep));
            c *= 0.94 + hash01(i as u32, j as u32 + 7) * 0.12;
            col.push([c.x, c.y, c.z, 1.0]);
        }
    }
    let mut idx = Vec::with_capacity((N - 1) * (N - 1) * 6);
    for j in 0..N - 1 {
        for i in 0..N - 1 {
            let a = (j * N + i) as u32;
            let (b, c, d) = (a + 1, a + N as u32, a + N as u32 + 1);
            idx.extend_from_slice(&[a, d, b, a, c, d]);
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
        .with_inserted_indices(Indices::U32(idx))
}

fn ground_material(mats: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
    mats.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.97, reflectance: 0.2, ..default() })
}

/// The match's terrain (replacing any from an earlier welcome).
pub fn spawn_terrain(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>, ground: &Terrain, old: impl Iterator<Item = Entity>) {
    for e in old {
        commands.entity(e).despawn();
    }
    let mat = ground_material(mats);
    commands.spawn((Mesh3d(meshes.add(terrain_mesh(ground))), MeshMaterial3d(mat), Transform::IDENTITY, Ground, DespawnOnExit(Screen::Flight)));
}

/// The ground the menus park vehicles on: a level patch at the origin of a fixed stretch of
/// hills. Built once; every menu screen spawns it.
#[derive(Resource)]
pub struct Stage {
    mesh: Handle<Mesh>,
    mat: Handle<StandardMaterial>,
    /// Where the terrain sits so that the patch is at the origin, at height 0.
    at: Vec3,
}

/// Radius of the level patch: room for the lobby's vehicles to slide past.
const STAGE_FLAT: f32 = 55.0;
const STAGE_SEED: u64 = 1729;

pub fn build_stage(meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>) -> Stage {
    let mut ground = Terrain::generate(STAGE_SEED);
    // a patch out towards a corner, so the rim's ridges close the view behind it
    let (x, z) = (-HALF * 0.5, -HALF * 0.52);
    let y = ground.height(x, z);
    ground.level(x, z, STAGE_FLAT, y);
    Stage { mesh: meshes.add(terrain_mesh(&ground)), mat: ground_material(mats), at: Vec3::new(-x, -y, -z) }
}

/// A menu screen's world: daylight, haze, the sky, and the stage's ground with its level patch at
/// height `floor`.
pub fn spawn_stage(commands: &mut Commands, stage: &Stage, sky: &DaySky, camera: Entity, screen: Screen, floor: f32) {
    // lighter haze than in battle: the ridges behind should read
    commands.entity(camera).insert((Skybox { image: Some(sky.0.clone()), brightness: SKY_BRIGHTNESS, ..default() }, haze(260.0, 1500.0)));
    spawn_daylight(commands, screen.clone());
    commands.spawn((Mesh3d(stage.mesh.clone()), MeshMaterial3d(stage.mat.clone()), Transform::from_translation(stage.at + Vec3::Y * floor), DespawnOnExit(screen)));
}

/// How high to lift a design so that it stands on the ground at height 0: on its wheels if it has
/// any, else on its lowest blocks. `com` is where its blocks are drawn from.
pub fn park_height(design: &Design, com: Vec3) -> f32 {
    let bottom = design.blocks.iter().map(|b| b.pos().as_vec3().y - com.y - if b.ty == BlockType::Wheel { WHEEL_R } else { 0.5 }).fold(f32::MAX, f32::min);
    if bottom == f32::MAX { 0.0 } else { -bottom }
}
