//! Part thumbnails for the hangar palette: every block type is rendered once by its own small
//! off-screen camera, so the palette shows the real parts exactly as they look on a ship.
//!
//! The rigs (block, lights, cameras) live on a render layer of their own, so the game camera never
//! sees them and their lights touch nothing else. They render for a few seconds while pipelines
//! compile, then despawn; the images keep their last frame.

use crate::art::{BlockArt, srgb};
use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use blocktread_shared::blocks::{Block, BlockType};

const LAYER: usize = 7;
/// Thumbnail size in pixels (tiles show them at about 60 logical px, 120 on a Retina screen).
const SIZE: u32 = 160;
/// Seconds the rigs keep rendering before they retire.
const RENDER_FOR: f32 = 4.0;

/// Thumbnail images, indexed by `BlockType::index()`.
#[derive(Resource)]
pub struct PartThumbs(pub Vec<Handle<Image>>);

#[derive(Component)]
struct ThumbRig;

pub struct ThumbsPlugin;

impl Plugin for ThumbsPlugin {
    fn build(&self, app: &mut App) {
        // PostStartup: BlockArt is inserted by a Startup system
        app.add_systems(PostStartup, render_thumbs).add_systems(Update, retire_thumbs);
    }
}

fn render_thumbs(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut art: ResMut<BlockArt>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let layer = RenderLayers::layer(LAYER);
    // the hangar's lighting: a key light from above and a teal rim from behind
    commands.spawn((DirectionalLight { illuminance: 9000.0, ..default() }, Transform::from_xyz(3.0, 6.0, -2.0).looking_at(Vec3::ZERO, Vec3::Y), layer.clone(), ThumbRig));
    commands.spawn((DirectionalLight { illuminance: 2600.0, color: srgb(0x3ee6d0), ..default() }, Transform::from_xyz(-4.0, 2.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y), layer.clone(), ThumbRig));
    let mut thumbs = Vec::new();
    for (i, ty) in BlockType::ALL.iter().enumerate() {
        let image = images.add(Image::new_target_texture(SIZE, SIZE, TextureFormat::Rgba8UnormSrgb, None));
        // far from the arena and from each other, so each camera frames only its own block
        let origin = Vec3::new(4000.0 + i as f32 * 30.0, 4000.0, 0.0);
        let root = commands.spawn((Transform::from_translation(origin), Visibility::default(), layer.clone(), ThumbRig)).id();
        let b = Block::new(0, 0, 0, *ty, None);
        let (mut parts, _) = art.spawn_block(&mut commands, &mut mats, root, &b, Vec3::ZERO, 3);
        if ty.weapon().is_some() {
            parts.extend(art.spawn_mount(&mut commands, &mut mats, root, &b, Vec3::ZERO, 3, IVec3::NEG_Y));
        }
        for e in parts {
            commands.entity(e).insert(layer.clone());
        }
        // three-quarter view from the front, so barrels point at the viewer; engines from behind,
        // so the nozzle shows. Framing follows each part's extent: the long-barrelled guns need
        // the widest view, a lone cube the tightest.
        let back = if ty.thrust() > 0.0 { 1.0 } else { -1.0 };
        let (centre_z, fov): (f32, f32) = match ty {
            BlockType::Laser | BlockType::Cannon => (-0.5, 26.0),
            BlockType::Railgun => (-0.55, 27.0),
            BlockType::Flak => (-0.35, 26.5),
            BlockType::Engine => (0.12, 27.5),
            BlockType::Thruster => (0.05, 25.0),
            BlockType::Missile | BlockType::Plasma => (-0.1, 27.0),
            // a lone cube: leave the top-right corner free for the shortcut key
            _ => (0.0, 26.5),
        };
        let focus = origin + Vec3::new(0.0, 0.0, centre_z);
        commands.spawn((
            Camera3d::default(),
            Camera { order: -20 - i as isize, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
            RenderTarget::Image(image.clone().into()),
            Projection::Perspective(PerspectiveProjection { fov: fov.to_radians(), ..default() }),
            Transform::from_translation(focus + Vec3::new(2.8, 2.2, 2.3 * back)).looking_at(focus, Vec3::Y),
            layer.clone(),
            ThumbRig,
        ));
        thumbs.push(image);
    }
    commands.insert_resource(PartThumbs(thumbs));
}

fn retire_thumbs(mut commands: Commands, time: Res<Time>, rigs: Query<Entity, With<ThumbRig>>, mut done: Local<bool>) {
    if *done || time.elapsed_secs() < RENDER_FOR {
        return;
    }
    *done = true;
    for e in &rigs {
        commands.entity(e).despawn();
    }
}
