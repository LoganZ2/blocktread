//! Space backdrop: a generated cubemap (nebula + stars), a planet, the sun, and the asteroid field.

use super::Session;
use crate::Screen;
use crate::art::srgb;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension};
use voxwing_shared::glam::Vec3 as GVec3;
use voxwing_shared::rng::hash01;
use voxwing_shared::world::{Asteroid, fbm3, rock_radius};

pub const SUN_DIR: Vec3 = Vec3::new(-0.42, 0.45, -0.79);

#[derive(Resource)]
pub struct SkyImage(pub Handle<Image>);

/// A drawn asteroid: its index in the client core's rock list.
#[derive(Component)]
pub struct Rock(usize);

fn face_dir(face: usize, u: f32, v: f32) -> Vec3 {
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

fn nebula(d: Vec3) -> Vec3 {
    let g = GVec3::new(d.x, d.y, d.z);
    let n = fbm3(g * 2.3 + GVec3::splat(3.1), 4);
    let n2 = fbm3(g * 4.6 + GVec3::splat(n * 2.0), 4);
    let bd = d.y * 2.0 + n * 0.9 - 0.25;
    let band = (-bd * bd * 2.5).exp();
    let mut c = Vec3::new(0.004, 0.006, 0.014);
    c += Vec3::new(0.16, 0.045, 0.24) * n2.powi(3) * band * 1.6;
    c += Vec3::new(0.02, 0.09, 0.2) * n.powf(3.5) * 1.4;
    c += Vec3::new(0.22, 0.08, 0.03) * fbm3(g * 3.0 + GVec3::splat(7.0), 3).powi(5) * 1.2;
    c
}

/// Six 512x512 faces. Nebula is evaluated on a coarse grid and interpolated; stars are per pixel.
pub fn build_sky(images: &mut Assets<Image>) -> Handle<Image> {
    let s = 1024usize;
    let coarse = 64usize;
    let mut data = vec![0u8; s * s * 6 * 4];
    for face in 0..6 {
        let mut grid = vec![Vec3::ZERO; (coarse + 1) * (coarse + 1)];
        for gy in 0..=coarse {
            for gx in 0..=coarse {
                let u = gx as f32 / coarse as f32 * 2.0 - 1.0;
                let v = gy as f32 / coarse as f32 * 2.0 - 1.0;
                grid[gy * (coarse + 1) + gx] = nebula(face_dir(face, u, v));
            }
        }
        for y in 0..s {
            for x in 0..s {
                let fx = x as f32 / (s - 1) as f32 * coarse as f32;
                let fy = y as f32 / (s - 1) as f32 * coarse as f32;
                let (x0, y0) = ((fx as usize).min(coarse - 1), (fy as usize).min(coarse - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let g = |a: usize, b: usize| grid[b * (coarse + 1) + a];
                let mut c = g(x0, y0).lerp(g(x0 + 1, y0), tx).lerp(g(x0, y0 + 1).lerp(g(x0 + 1, y0 + 1), tx), ty);
                let h = hash01((face * s * s + y * s + x) as u32, 91);
                if h > 0.9988 {
                    let b = ((h - 0.9988) / 0.0012).powi(3) * 2.2 + 0.3;
                    let tint = hash01(x as u32, (y + face * s) as u32);
                    let col = if tint < 0.15 { Vec3::new(1.0, 0.8, 0.6) } else if tint < 0.35 { Vec3::new(0.7, 0.8, 1.0) } else { Vec3::ONE };
                    c += col * b;
                }
                let i = ((face * s + y) * s + x) * 4;
                let to8 = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
                data[i..i + 4].copy_from_slice(&[to8(c.x), to8(c.y), to8(c.z), 255]);
            }
        }
    }
    let mut img = Image::new(
        Extent3d { width: s as u32, height: (s * 6) as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.reinterpret_stacked_2d_as_array(6).expect("six square faces");
    img.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
    images.add(img)
}

fn planet_texture(images: &mut Assets<Image>) -> Handle<Image> {
    let (w, h) = (512usize, 256usize);
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let lon = x as f32 / w as f32 * std::f32::consts::TAU;
            let lat = (y as f32 / h as f32 - 0.5) * std::f32::consts::PI;
            let p = GVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let b = p.y * 7.0 + fbm3(p * 3.0, 4) * 2.6;
            let c1 = Vec3::new(0.05, 0.16, 0.3);
            let c2 = Vec3::new(0.32, 0.55, 0.66);
            let c3 = Vec3::new(0.02, 0.07, 0.16);
            let mut c = c1.lerp(c2, 0.5 + 0.5 * b.sin());
            let s = fbm3(p * 8.0 + GVec3::splat(3.0), 4);
            c = c.lerp(c3, ((s - 0.55) / 0.25).clamp(0.0, 1.0));
            let to8 = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8;
            let i = (y * w + x) * 4;
            px[i..i + 4].copy_from_slice(&[to8(c.x), to8(c.y), to8(c.z), 255]);
        }
    }
    images.add(Image::new(Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 }, TextureDimension::D2, px, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD))
}

/// Planet and sun meshes/materials, built once.
#[derive(Resource)]
pub struct EnvAssets {
    planet: (Handle<Mesh>, Handle<StandardMaterial>),
    sun: (Handle<Mesh>, Handle<StandardMaterial>),
}

pub fn build_env_assets(meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> EnvAssets {
    let tex = planet_texture(images);
    EnvAssets {
        planet: (meshes.add(Sphere::new(1100.0).mesh().uv(96, 48)), mats.add(StandardMaterial { base_color_texture: Some(tex), perceptual_roughness: 0.9, ..default() })),
        sun: (meshes.add(Sphere::new(260.0).mesh().ico(3).unwrap()), mats.add(StandardMaterial { base_color: Color::LinearRgba(LinearRgba::rgb(60.0, 52.0, 40.0)), unlit: true, ..default() })),
    }
}

/// Sun, fill light, ambient, planet. `origin` shifts the backdrop (menus put it near the ship).
pub fn spawn_env(commands: &mut Commands, env: &EnvAssets, scope: Screen, origin: Vec3) {
    let scope = DespawnOnExit(scope);
    let sun = SUN_DIR.normalize();
    commands.spawn((DirectionalLight { illuminance: 15000.0, color: srgb(0xfff0dc), ..default() }, Transform::from_translation(sun * 100.0).looking_at(Vec3::ZERO, Vec3::Y), scope.clone()));
    commands.spawn((DirectionalLight { illuminance: 3500.0, color: srgb(0x8fa6d8), ..default() }, Transform::from_translation(-sun * 100.0).looking_at(Vec3::ZERO, Vec3::Y), scope.clone()));
    commands.insert_resource(GlobalAmbientLight { color: Color::srgb(0.7, 0.76, 0.9), brightness: 420.0, ..default() });
    commands.spawn((Mesh3d(env.planet.0.clone()), MeshMaterial3d(env.planet.1.clone()), Transform::from_translation(origin + Vec3::new(3600.0, -1300.0, -5200.0)), scope.clone()));
    commands.spawn((Mesh3d(env.sun.0.clone()), MeshMaterial3d(env.sun.1.clone()), Transform::from_translation(origin + sun * 9000.0), scope));
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
