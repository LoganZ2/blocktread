//! How blocks look: per-type part meshes, procedural panel textures, and a material cache keyed by
//! paint colour and damage level.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2 as HP;
use voxwing_shared::blocks::{Block, BlockType};
use voxwing_shared::rng::Rng;

pub fn srgb(c: u32) -> Color {
    Color::srgb_u8(((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8)
}

pub fn glow(c: u32, k: f32) -> LinearRgba {
    srgb(c).to_linear() * k
}

#[derive(Clone)]
pub enum PartMat {
    Paint,
    Armor,
    /// Paint that glows in its own colour (lamps).
    Glow,
    Fixed(Handle<StandardMaterial>),
}

/// The painted surface of a block type, which is what darkens as it takes damage.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Finish {
    Paint,
    Armor,
    Glow,
}

impl Finish {
    pub fn of(t: BlockType) -> Finish {
        match t {
            BlockType::Armor => Finish::Armor,
            BlockType::Lamp => Finish::Glow,
            _ => Finish::Paint,
        }
    }
}

#[derive(Clone)]
pub struct Part {
    pub mesh: Handle<Mesh>,
    pub mat: PartMat,
    pub tf: Transform,
}

#[derive(Resource)]
pub struct BlockArt {
    pub parts: Vec<Vec<Part>>,
    pub cube: Handle<Mesh>,
    pub sphere: Handle<Mesh>,
    paint_tex: Handle<Image>,
    armor_tex: Handle<Image>,
    cache: HashMap<(u32, Finish, u8), Handle<StandardMaterial>>,
}

/// Damage level 0..=3 from remaining hit points (3 = undamaged).
pub fn dmg_level(frac: f32) -> u8 {
    if frac > 0.75 { 3 } else if frac > 0.5 { 2 } else if frac > 0.25 { 1 } else { 0 }
}

impl BlockArt {
    pub fn paint(&mut self, mats: &mut Assets<StandardMaterial>, color: u32, finish: Finish, level: u8) -> Handle<StandardMaterial> {
        let key = (color, finish, level);
        if let Some(h) = self.cache.get(&key) {
            return h.clone();
        }
        let burn = (3 - level.min(3)) as f32 / 3.0 * 0.75;
        let base = srgb(color).mix(&Color::srgb(0.03, 0.02, 0.02), burn);
        let armor = finish == Finish::Armor;
        let h = mats.add(match finish {
            // a lamp dims as it is shot up
            Finish::Glow => StandardMaterial { base_color: base, emissive: glow(color, 1.0 + 3.0 * level.min(3) as f32 / 3.0), perceptual_roughness: 0.3, ..default() },
            _ => StandardMaterial {
                base_color: base,
                base_color_texture: Some(if armor { self.armor_tex.clone() } else { self.paint_tex.clone() }),
                metallic: if armor { 0.3 } else { 0.08 },
                perceptual_roughness: if armor { 0.5 } else { 0.6 },
                ..default()
            },
        });
        self.cache.insert(key, h.clone());
        h
    }

    pub fn material_for(&mut self, mats: &mut Assets<StandardMaterial>, part: &Part, b: &Block, level: u8) -> Handle<StandardMaterial> {
        match &part.mat {
            PartMat::Paint => self.paint(mats, b.color, Finish::Paint, level),
            PartMat::Armor => self.paint(mats, b.color, Finish::Armor, level),
            PartMat::Glow => self.paint(mats, b.color, Finish::Glow, level),
            PartMat::Fixed(h) => h.clone(),
        }
    }

    /// Spawn one block's parts under `parent`, centred at `local`. Returns (all entities, paint entity).
    pub fn spawn_block(&mut self, commands: &mut Commands, mats: &mut Assets<StandardMaterial>, parent: Entity, b: &Block, local: Vec3, level: u8) -> (Vec<Entity>, Option<Entity>) {
        let mut out = Vec::new();
        let mut paint = None;
        let parts = self.parts[b.ty.index()].clone();
        for part in &parts {
            let mat = self.material_for(mats, part, b, level);
            let tf = Transform::from_translation(local) * part.tf;
            let e = commands.spawn((Mesh3d(part.mesh.clone()), MeshMaterial3d(mat), tf, ChildOf(parent))).id();
            if matches!(part.mat, PartMat::Paint | PartMat::Armor | PartMat::Glow) && paint.is_none() {
                paint = Some(e);
            }
            out.push(e);
        }
        (out, paint)
    }
}

fn panel_texture(armor: bool, images: &mut Assets<Image>) -> Handle<Image> {
    let s = 128usize;
    let mut px = vec![0u8; s * s * 4];
    let mut rng = Rng::new(if armor { 11 } else { 7 });
    let base: f32 = if armor { 0.74 } else { 0.84 };
    for y in 0..s {
        for x in 0..s {
            let mut v = base + (rng.f32() - 0.5) * 0.05;
            let edge = x.min(y).min(s - 1 - x).min(s - 1 - y);
            if edge < 3 {
                v *= 0.45;
            } else if edge < 6 {
                v = (v + 0.12).min(1.0);
            }
            if armor {
                for (cx, cy) in [(16, 16), (112, 16), (16, 112), (112, 112)] {
                    let d = ((x as f32 - cx as f32).powi(2) + (y as f32 - cy as f32).powi(2)).sqrt();
                    if d < 5.0 {
                        v *= 0.5;
                    }
                }
                if (y == 64 || y == 65) && (30..98).contains(&x) || (x == 64 || x == 65) && (30..98).contains(&y) {
                    v *= 0.65;
                }
            } else {
                if (y == 70 || y == 71) && edge >= 6 || (x == 56 || x == 57) && y < 70 && edge >= 6 {
                    v *= 0.72;
                }
                if (86..108).contains(&y) && (80..116).contains(&x) && (x - 80) % 9 < 5 {
                    v *= 0.7;
                }
            }
            let c = (v.clamp(0.0, 1.0) * 255.0) as u8;
            let i = (y * s + x) * 4;
            px[i..i + 4].copy_from_slice(&[c, c, c, 255]);
        }
    }
    images.add(with_mips(px, s))
}

/// A square RGBA8 sRGB texture with a full box-filtered mip chain and anisotropic filtering.
/// Without mips, the fine panel lines alias into a shimmer whenever a ship moves.
pub fn with_mips(level0: Vec<u8>, size: usize) -> Image {
    let mut data = level0.clone();
    let (mut prev, mut s, mut levels) = (level0, size, 1u32);
    while s > 1 {
        let n = s / 2;
        let mut next = vec![0u8; n * n * 4];
        for y in 0..n {
            for x in 0..n {
                for c in 0..4 {
                    let at = |dx: usize, dy: usize| prev[((2 * y + dy) * s + 2 * x + dx) * 4 + c] as u32;
                    next[(y * n + x) * 4 + c] = ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        (prev, s) = (next, n);
        levels += 1;
    }
    let mut image = Image::new_uninit(Extent3d { width: size as u32, height: size as u32, depth_or_array_layers: 1 }, TextureDimension::D2, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { anisotropy_clamp: 8, ..ImageSamplerDescriptor::linear() });
    image
}

pub fn build_block_art(meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> BlockArt {
    let paint_tex = panel_texture(false, images);
    let armor_tex = panel_texture(true, images);
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let mut m = |mesh: Mesh| meshes.add(mesh);
    let nozzle = m(ConicalFrustum { radius_top: 0.44, radius_bottom: 0.34, height: 0.32 }.into());
    let disc = m(Circle::new(0.33).into());
    let barrel = m(Cylinder::new(0.09, 1.0).into());
    let gun = m(Cylinder::new(0.065, 0.9).into());
    let tube = m(Cylinder::new(0.15, 0.12).into());
    let cone = m(Cone { radius: 0.11, height: 0.28 }.into());
    let ball = m(Sphere::new(0.24).mesh().ico(2).unwrap());
    let dome = m(Sphere::new(0.2).mesh().ico(2).unwrap());
    let ring = m(Torus { minor_radius: 0.07, major_radius: 0.3 }.into());
    let ring_s = m(Torus { minor_radius: 0.045, major_radius: 0.31 }.into());
    let ring_l = m(Torus { minor_radius: 0.045, major_radius: 0.43 }.into());
    let sphere = m(Sphere::new(1.0).mesh().ico(2).unwrap());
    let nozzle_s = m(ConicalFrustum { radius_top: 0.28, radius_bottom: 0.22, height: 0.24 }.into());
    let disc_s = m(Circle::new(0.21).into());
    let flak_barrel = m(Cylinder::new(0.19, 0.62).into());

    let mut fixed = |c: StandardMaterial| PartMat::Fixed(mats.add(c));
    let dark = fixed(StandardMaterial { base_color: srgb(0x262b33), metallic: 0.8, perceptual_roughness: 0.35, ..default() });
    let glass = fixed(StandardMaterial { base_color: srgb(0x0b2238), emissive: glow(0x2a7bff, 2.0), metallic: 0.9, perceptual_roughness: 0.1, ..default() });
    let red = fixed(StandardMaterial { base_color: srgb(0xd83a3a), metallic: 0.3, perceptual_roughness: 0.4, ..default() });
    // nozzle throat: white-hot but not blown out; the plume carries the colour
    let g_engine = fixed(StandardMaterial { base_color: Color::BLACK, emissive: glow(0xd8ecff, 2.5), ..default() });
    let g_red = fixed(StandardMaterial { base_color: Color::BLACK, emissive: glow(0xff3344, 40.0), ..default() });
    let g_green = fixed(StandardMaterial { base_color: Color::BLACK, emissive: glow(0x44ff99, 30.0), ..default() });
    let g_blue = fixed(StandardMaterial { base_color: Color::BLACK, emissive: glow(0x4aa8ff, 25.0), ..default() });
    let g_yellow = fixed(StandardMaterial { base_color: Color::BLACK, emissive: glow(0xffc84a, 25.0), ..default() });
    let g_purple = fixed(StandardMaterial { base_color: Color::BLACK, emissive: glow(0xb070ff, 25.0), ..default() });
    let g_cyan = fixed(StandardMaterial { base_color: Color::BLACK, emissive: glow(0x7fe8ff, 22.0), ..default() });

    let p = |mesh: &Handle<Mesh>, mat: &PartMat, pos: [f32; 3], rot: [f32; 3], scale: [f32; 3]| Part {
        mesh: mesh.clone(),
        mat: mat.clone(),
        tf: Transform { translation: Vec3::from(pos), rotation: Quat::from_euler(EulerRot::XYZ, rot[0], rot[1], rot[2]), scale: Vec3::from(scale) },
    };
    let (z, one) = ([0.0; 3], [1.0; 3]);
    let paint = PartMat::Paint;
    let mut parts: Vec<Vec<Part>> = vec![Vec::new(); BlockType::ALL.len()];
    // the twelve edges of a cube, `inset` from the centre and `t` thick; they stop 0.01 short of
    // the cell faces, so nothing is coplanar with a neighbouring block
    let edges = |mat: &PartMat, inset: f32, t: f32| {
        let mut v = Vec::new();
        for a in [-inset, inset] {
            for b in [-inset, inset] {
                v.push(p(&cube, mat, [0.0, a, b], z, [0.98, t, t]));
                v.push(p(&cube, mat, [a, 0.0, b], z, [t, 0.98, t]));
                v.push(p(&cube, mat, [a, b, 0.0], z, [t, t, 0.98]));
            }
        }
        v
    };
    use BlockType::*;
    parts[Hull.index()] = vec![p(&cube, &paint, z, z, one)];
    parts[Armor.index()] = vec![p(&cube, &PartMat::Armor, z, z, one)];
    parts[Cockpit.index()] = vec![
        p(&cube, &paint, [0.0, -0.2, 0.0], z, [1.0, 0.6, 1.0]),
        p(&cube, &glass, [0.0, 0.28, 0.04], z, [0.8, 0.44, 0.88]),
        // visor stands 0.01 proud of the hull face: a coplanar face would z-fight
        p(&cube, &dark, [0.0, 0.12, -0.47], z, [0.86, 0.12, 0.08]),
    ];
    // trailing-edge strip: 0.02 past the wing's back face and 1% wider, so no face of it is
    // coplanar with the wing (coplanar faces z-fight and flicker as the ship moves); the
    // overlap with a neighbouring strip is the same colour and invisible
    parts[Wing.index()] = vec![p(&cube, &paint, z, z, [1.0, 0.26, 1.0]), p(&cube, &dark, [0.0, 0.0, 0.44], z, [1.02, 0.28, 0.16])];
    parts[Engine.index()] = vec![
        p(&cube, &paint, [0.0, 0.0, -0.1], z, [0.94, 0.94, 0.8]),
        p(&nozzle, &dark, [0.0, 0.0, 0.42], [HP, 0.0, 0.0], one),
        p(&disc, &g_engine, [0.0, 0.0, 0.585], z, one),
    ];
    parts[Laser.index()] = vec![
        p(&cube, &paint, z, z, [0.8, 0.62, 0.9]),
        p(&barrel, &dark, [0.0, 0.0, -0.85], [HP, 0.0, 0.0], one),
        p(&cube, &g_red, [0.0, 0.0, -1.36], z, [0.13, 0.13, 0.1]),
    ];
    parts[Cannon.index()] = vec![
        p(&cube, &paint, z, z, [0.86, 0.72, 0.9]),
        p(&gun, &dark, [0.18, 0.0, -0.75], [HP, 0.0, 0.0], one),
        p(&gun, &dark, [-0.18, 0.0, -0.75], [HP, 0.0, 0.0], one),
        p(&cube, &dark, [0.0, 0.0, -0.5], z, [0.62, 0.22, 0.12]),
    ];
    parts[Plasma.index()] = vec![
        p(&cube, &paint, z, z, [0.92, 0.92, 0.92]),
        p(&ring, &dark, [0.0, 0.0, -0.52], [HP, 0.0, 0.0], one),
        p(&ball, &g_green, [0.0, 0.0, -0.5], z, one),
    ];
    let mut missile = vec![p(&cube, &paint, z, z, [0.94, 0.94, 0.96])];
    for (a, b) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        missile.push(p(&tube, &dark, [0.22 * a, 0.22 * b, -0.5], [HP, 0.0, 0.0], one));
        missile.push(p(&cone, &red, [0.22 * a, 0.22 * b, -0.64], [-HP, 0.0, 0.0], one));
    }
    parts[Missile.index()] = missile;
    parts[Reactor.index()] = vec![
        p(&cube, &paint, z, z, one),
        p(&cube, &g_yellow, z, z, [1.03, 0.2, 1.03]),
        p(&cube, &dark, [0.0, 0.5, 0.0], z, [0.55, 0.1, 0.55]),
    ];
    parts[Shield.index()] = vec![
        p(&cube, &paint, [0.0, -0.1, 0.0], z, [1.0, 0.8, 1.0]),
        p(&dome, &g_blue, [0.0, 0.3, 0.0], z, one),
        p(&ring_s, &dark, [0.0, 0.3, 0.0], z, one),
    ];
    parts[Gyro.index()] = vec![
        p(&cube, &paint, z, z, [0.84, 0.84, 0.84]),
        p(&ring_l, &g_purple, z, z, one),
        p(&ring_l, &dark, z, [0.0, 0.0, HP], one),
    ];
    // an open truss: edges plus a diagonal brace on each side
    let mut frame = edges(&paint, 0.42, 0.14);
    for x in [-0.42, 0.42] {
        frame.push(p(&cube, &paint, [x, 0.0, 0.0], [std::f32::consts::FRAC_PI_4, 0.0, 0.0], [0.1, 0.1, 1.12]));
    }
    parts[Frame.index()] = frame;
    let mut lamp = vec![p(&cube, &PartMat::Glow, z, z, [0.8, 0.8, 0.8])];
    lamp.extend(edges(&dark, 0.43, 0.12));
    parts[Lamp.index()] = lamp;
    parts[Fin.index()] = vec![p(&cube, &paint, z, z, [0.22, 1.0, 1.0]), p(&cube, &dark, [0.0, 0.0, 0.44], z, [0.24, 1.02, 0.16])];
    parts[Thruster.index()] = vec![
        p(&cube, &paint, [0.0, 0.0, -0.12], z, [0.72, 0.72, 0.76]),
        p(&cube, &dark, [0.0, 0.0, -0.42], z, [0.9, 0.9, 0.14]),
        p(&nozzle_s, &dark, [0.0, 0.0, 0.36], [HP, 0.0, 0.0], one),
        p(&disc_s, &g_engine, [0.0, 0.0, 0.485], z, one),
    ];
    // a body with a thruster pod sunk into each face
    let mut rcs = vec![p(&cube, &paint, z, z, [0.8, 0.8, 0.8])];
    for (pos, scale) in [([0.42, 0.0, 0.0], [0.1, 0.26, 0.26]), ([0.0, 0.42, 0.0], [0.26, 0.1, 0.26]), ([0.0, 0.0, 0.42], [0.26, 0.26, 0.1])] {
        let neg = [-pos[0], -pos[1], -pos[2]];
        rcs.push(p(&cube, &dark, pos, z, scale));
        rcs.push(p(&cube, &dark, neg, z, scale));
    }
    parts[Rcs.index()] = rcs;
    let mut rail = vec![p(&cube, &paint, z, z, [0.8, 0.62, 1.0]), p(&cube, &g_cyan, [0.0, 0.0, -0.92], z, [0.05, 0.05, 1.0]), p(&cube, &dark, [0.0, 0.0, -1.42], z, [0.42, 0.26, 0.08])];
    for x in [-0.13, 0.13] {
        rail.push(p(&cube, &dark, [x, 0.0, -0.92], z, [0.08, 0.16, 1.1]));
    }
    parts[Railgun.index()] = rail;
    parts[Flak.index()] = vec![
        p(&cube, &paint, z, z, [0.9, 0.76, 0.9]),
        p(&flak_barrel, &dark, [0.0, 0.0, -0.78], [HP, 0.0, 0.0], one),
        p(&cube, &dark, [0.0, 0.0, -1.12], z, [0.52, 0.18, 0.14]),
    ];
    let mut cap = vec![p(&cube, &paint, z, z, [0.92, 1.0, 0.92])];
    for y in [-0.28, 0.0, 0.28] {
        cap.push(p(&cube, &g_cyan, [0.0, y, 0.0], z, [0.98, 0.07, 0.98]));
    }
    for x in [-0.2, 0.2] {
        cap.push(p(&cube, &dark, [x, 0.5, 0.0], z, [0.14, 0.1, 0.14]));
    }
    parts[Capacitor.index()] = cap;
    BlockArt { parts, cube, sphere, paint_tex, armor_tex, cache: HashMap::new() }
}
