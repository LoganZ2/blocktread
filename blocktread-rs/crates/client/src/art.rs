//! How blocks look: per-type part meshes, procedural panel textures, and a material cache keyed by
//! paint colour and damage level.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2 as HP;
use blocktread_shared::blocks::{Block, BlockType};
use blocktread_shared::grid::Occupancy;
use blocktread_shared::rng::Rng;

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
    /// A weapon's mount, for a weapon held from below.
    mount: Vec<Part>,
    strut: StrutArt,
    pub cube: Handle<Mesh>,
    pub sphere: Handle<Mesh>,
    paint_tex: Handle<Image>,
    armor_tex: Handle<Image>,
    cache: HashMap<(u32, Finish, u8), Handle<StandardMaterial>>,
}

/// A wheel's tyre, tyre shoulder, rim, rim lip and hub.
type WheelMeshes = (Handle<Mesh>, Handle<Mesh>, Handle<Mesh>, Handle<Mesh>, Handle<Mesh>);

/// What a wheel's suspension is made of: a coil-over strut standing beside the wheel, bolted to
/// the block that holds it, and a hub carrier and a rod that ride up and down with the
/// wheel, the rod sliding in the strut.
struct StrutArt {
    cube: Handle<Mesh>,
    body: Handle<Mesh>,
    cap: Handle<Mesh>,
    coil: Handle<Mesh>,
    rod: Handle<Mesh>,
    dark: PartMat,
    steel: PartMat,
}

/// How far from a wheel's centre (along x, towards the block holding it) its strut stands: the
/// axis the wheel steers about.
pub const KINGPIN: f32 = 0.46;

/// What holds a wheel: the face of its cell towards the block it hangs from (zero when only
/// wheels and hover pads touch it), and the side of the wheel its strut stands on (±1 along x).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hold {
    pub face: IVec3,
    pub side: f32,
}

/// What holds the wheel at `at`: the block beside it, inwards first (wheels sit on a deck's
/// sides), else the one above it, ahead or behind. The strut stands on the side it hangs from, or
/// on the inner side when it hangs from above, ahead or behind.
pub fn wheel_hold(blocks: &[Block], occ: &Occupancy, at: IVec3) -> Hold {
    let inward = if at.x > 0 { IVec3::NEG_X } else { IVec3::X };
    let holds = |d: IVec3| occ.get(at + d).is_some_and(|j| blocks[j].ty.drive().is_none());
    let face = [inward, -inward, IVec3::Y, IVec3::NEG_Z, IVec3::Z].into_iter().find(|&d| holds(d)).unwrap_or(IVec3::ZERO);
    Hold { face, side: if face.x != 0 { face.x as f32 } else { inward.x as f32 } }
}

/// A wheel's tyre width.
fn tyre_width(t: BlockType) -> f32 {
    match t {
        BlockType::SmallWheel => 0.56,
        BlockType::BigWheel => 0.8,
        _ => 0.7,
    }
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

impl BlockArt {
    /// A weapon's mount at `local`, its plate on `face` (towards the block holding the weapon).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_mount(&mut self, commands: &mut Commands, mats: &mut Assets<StandardMaterial>, parent: Entity, b: &Block, local: Vec3, level: u8, face: IVec3) -> Vec<Entity> {
        let turn = Transform::from_rotation(Quat::from_rotation_arc(Vec3::NEG_Y, face.as_vec3()));
        let parts = self.mount.clone();
        parts
            .iter()
            .map(|part| {
                let mat = self.material_for(mats, part, b, level);
                commands.spawn((Mesh3d(part.mesh.clone()), MeshMaterial3d(mat), Transform::from_translation(local) * turn * part.tf, ChildOf(parent))).id()
            })
            .collect()
    }

    /// A wheel's suspension: the strut, on the vehicle under `fixed` at the wheel's cell centre,
    /// and what rides with the wheel under `riding` at its kingpin (at the hub's height). Nothing
    /// when nothing holds the wheel.
    pub fn spawn_strut(&mut self, commands: &mut Commands, mats: &mut Assets<StandardMaterial>, b: &Block, hold: Hold, fixed: (Entity, Vec3), riding: (Entity, Vec3)) -> Vec<Entity> {
        if hold.face == IVec3::ZERO {
            return Vec::new();
        }
        let (on, rides) = self.strut_parts(hold, tyre_width(b.ty));
        let mut out = Vec::new();
        for (parts, (parent, at)) in [(on, fixed), (rides, riding)] {
            for part in &parts {
                let mat = self.material_for(mats, part, b, 3);
                out.push(commands.spawn((Mesh3d(part.mesh.clone()), MeshMaterial3d(mat), Transform::from_translation(at) * part.tf, ChildOf(parent))).id());
            }
        }
        out
    }

    /// The strut's parts for a wheel `w` wide: those on the vehicle (about the wheel's cell
    /// centre) and those that ride with the wheel (about its kingpin).
    fn strut_parts(&self, hold: Hold, w: f32) -> (Vec<Part>, Vec<Part>) {
        let s = &self.strut;
        let k = KINGPIN;
        // built for a strut on +x, then mirrored onto the wheel's side
        let p = |mesh: &Handle<Mesh>, mat: &PartMat, pos: [f32; 3], scale: [f32; 3]| Part {
            mesh: mesh.clone(),
            mat: mat.clone(),
            tf: Transform::from_xyz(pos[0] * hold.side, pos[1], pos[2]).with_scale(Vec3::from(scale)),
        };
        let one = [1.0; 3];
        // the strut, its top and its coil; it reaches down to the knuckle with the wheel at rest
        let mut on = vec![p(&s.body, &s.dark, [k, 0.22, 0.0], one), p(&s.cap, &s.dark, [k, 0.46, 0.0], one)];
        for y in [0.07, 0.12, 0.17, 0.22, 0.27, 0.32, 0.37] {
            on.push(p(&s.coil, &s.steel, [k, y, 0.0], one));
        }
        // bolted to the block that holds it: a plate on the face beside it, a flange under the
        // block above, or an arm out to the block ahead or behind. Each stops just short of the
        // block's face (a face coplanar with it would flicker).
        match hold.face {
            f if f.x != 0 => on.push(p(&s.cube, &s.dark, [0.492, 0.2, 0.0], [0.012, 0.56, 0.4])),
            IVec3::Y => on.push(p(&s.cube, &s.dark, [k, 0.485, 0.0], [0.24, 0.02, 0.24])),
            f => {
                let dz = f.z as f32;
                on.push(p(&s.cube, &s.dark, [k, 0.42, dz * 0.23], [0.07, 0.08, 0.46]));
                on.push(p(&s.cube, &s.dark, [0.42, 0.3, dz * 0.485], [0.16, 0.3, 0.02]));
            }
        }
        // the hub carrier, filling the gap from the tyre to the strut, and the rod up into it
        let inner = w / 2.0 + 0.01;
        let rides = vec![p(&s.cube, &s.dark, [(inner + k) / 2.0 + 0.005 - k, 0.0, 0.0], [k - inner + 0.01, 0.34, 0.3]), p(&s.rod, &s.steel, [0.0, 0.28, 0.0], one)];
        (on, rides)
    }

    /// A whole design that doesn't move: every block, every weapon on its mount and every wheel
    /// on its strut. `offset` moves the grid (e.g. minus the centre of mass).
    pub fn spawn_design(&mut self, commands: &mut Commands, mats: &mut Assets<StandardMaterial>, parent: Entity, blocks: &[Block], offset: Vec3) {
        let occ = blocktread_shared::analysis::occupancy(blocks, &vec![true; blocks.len()]);
        for b in blocks {
            let at = b.pos().as_vec3() + offset;
            self.spawn_block(commands, mats, parent, b, at, 3);
            if b.ty.weapon().is_some() {
                self.spawn_mount(commands, mats, parent, b, at, 3, blocktread_shared::weapons::attach_face(&occ, b.pos()));
            }
            if b.ty.drive().is_some_and(|d| !d.hover) {
                let hold = wheel_hold(blocks, &occ, b.pos());
                self.spawn_strut(commands, mats, b, hold, (parent, at), (parent, at + Vec3::X * hold.side * KINGPIN));
            }
        }
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
    let ball = m(Sphere::new(0.24).mesh().ico(2).unwrap());
    let dome = m(Sphere::new(0.2).mesh().ico(2).unwrap());
    let ring = m(Torus { minor_radius: 0.07, major_radius: 0.3 }.into());
    let ring_s = m(Torus { minor_radius: 0.045, major_radius: 0.31 }.into());
    let ring_l = m(Torus { minor_radius: 0.045, major_radius: 0.43 }.into());
    let sphere = m(Sphere::new(1.0).mesh().ico(2).unwrap());
    let nozzle_s = m(ConicalFrustum { radius_top: 0.28, radius_bottom: 0.22, height: 0.24 }.into());
    let disc_s = m(Circle::new(0.21).into());
    let flak_barrel = m(Cylinder::new(0.19, 0.62).into());
    // a tyre for each wheel size, as wide as it is sturdy
    // wheels, by radius and width: the tyre, its rounded shoulders, the rim and its lip, the hub
    let wheel_r = |t: BlockType| t.drive().map_or(0.7, |d| d.radius);
    let (rs, rw, rb) = (wheel_r(BlockType::SmallWheel), wheel_r(BlockType::Wheel), wheel_r(BlockType::BigWheel));
    let (ws, ww, wb) = (tyre_width(BlockType::SmallWheel), tyre_width(BlockType::Wheel), tyre_width(BlockType::BigWheel));
    // rims and hubs in proportion to the wheel; the hub reaches out past the tyre into the strut
    let (rim_s, rim_w, rim_b) = (rs * 0.6, rw * 0.57, rb * 0.53);
    let (hub_s, hub_w, hub_b) = (rs * 0.2, rw * 0.17, rb * 0.17);
    let small = (m(Cylinder::new(rs - 0.02, ws).into()), m(Torus { minor_radius: 0.05, major_radius: rs - 0.07 }.into()), m(Cylinder::new(rim_s, ws + 0.02).into()), m(Torus { minor_radius: 0.03, major_radius: rim_s }.into()), m(Cylinder::new(hub_s, ws + 0.08).into()));
    let road = (m(Cylinder::new(rw - 0.04, ww).into()), m(Torus { minor_radius: 0.08, major_radius: rw - 0.1 }.into()), m(Cylinder::new(rim_w, ww + 0.02).into()), m(Torus { minor_radius: 0.035, major_radius: rim_w }.into()), m(Cylinder::new(hub_w, ww + 0.1).into()));
    let big = (m(Cylinder::new(rb - 0.11, wb).into()), m(Torus { minor_radius: 0.1, major_radius: rb - 0.17 }.into()), m(Cylinder::new(rim_b, wb + 0.02).into()), m(Torus { minor_radius: 0.045, major_radius: rim_b }.into()), m(Cylinder::new(hub_b, wb + 0.12).into()));
    let pad_glow = m(Circle::new(0.42).into());
    let tube_s = m(Cylinder::new(0.1, 0.12).into());
    let cone_s = m(Cone { radius: 0.08, height: 0.2 }.into());
    let stem = m(Cylinder::new(0.1, 0.32).into());
    let collar = m(Torus { minor_radius: 0.04, major_radius: 0.2 }.into());
    // a wheel's strut: its body and top, the coil round it, the rod sliding in it
    let strut_body = m(Cylinder::new(0.055, 0.52).into());
    let strut_cap = m(Cylinder::new(0.08, 0.06).into());
    let strut_coil = m(Torus { minor_radius: 0.017, major_radius: 0.07 }.into());
    let strut_rod = m(Cylinder::new(0.028, 0.36).into());

    let mut fixed = |c: StandardMaterial| PartMat::Fixed(mats.add(c));
    let dark = fixed(StandardMaterial { base_color: srgb(0x262b33), metallic: 0.8, perceptual_roughness: 0.35, ..default() });
    let glass = fixed(StandardMaterial { base_color: srgb(0x0b2238), emissive: glow(0x2a7bff, 2.0), metallic: 0.9, perceptual_roughness: 0.1, ..default() });
    let rubber = fixed(StandardMaterial { base_color: srgb(0x1d1f23), perceptual_roughness: 0.92, ..default() });
    let rim = fixed(StandardMaterial { base_color: srgb(0x7c838c), metallic: 0.6, perceptual_roughness: 0.4, ..default() });
    let rim_dark = fixed(StandardMaterial { base_color: srgb(0x4a4f45), metallic: 0.4, perceptual_roughness: 0.55, ..default() });
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
    // weapons: a head small enough to turn inside its cell (it sits on a mount, see `mount`), its
    // barrels out of the front
    parts[Laser.index()] = vec![
        p(&cube, &paint, z, z, [0.5, 0.42, 0.62]),
        p(&barrel, &dark, [0.0, 0.0, -0.8], [HP, 0.0, 0.0], one),
        p(&cube, &g_red, [0.0, 0.0, -1.31], z, [0.13, 0.13, 0.1]),
    ];
    parts[Cannon.index()] = vec![
        p(&cube, &paint, z, z, [0.56, 0.44, 0.62]),
        p(&gun, &dark, [0.14, 0.0, -0.75], [HP, 0.0, 0.0], one),
        p(&gun, &dark, [-0.14, 0.0, -0.75], [HP, 0.0, 0.0], one),
        p(&cube, &dark, [0.0, 0.0, -0.33], z, [0.5, 0.2, 0.08]),
    ];
    parts[Plasma.index()] = vec![
        p(&cube, &paint, z, z, [0.6, 0.6, 0.6]),
        p(&ring, &dark, [0.0, 0.0, -0.34], [HP, 0.0, 0.0], [0.8, 0.8, 0.8]),
        p(&ball, &g_green, [0.0, 0.0, -0.32], z, [0.8, 0.8, 0.8]),
    ];
    let mut missile = vec![p(&cube, &paint, z, z, [0.6, 0.6, 0.62])];
    for (a, b) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
        missile.push(p(&tube_s, &dark, [0.15 * a, 0.15 * b, -0.36], [HP, 0.0, 0.0], one));
        missile.push(p(&cone_s, &red, [0.15 * a, 0.15 * b, -0.48], [-HP, 0.0, 0.0], one));
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
    let mut rail = vec![p(&cube, &paint, z, z, [0.46, 0.4, 0.62]), p(&cube, &g_cyan, [0.0, 0.0, -0.8], z, [0.05, 0.05, 1.0]), p(&cube, &dark, [0.0, 0.0, -1.32], z, [0.36, 0.22, 0.08])];
    for x in [-0.11, 0.11] {
        rail.push(p(&cube, &dark, [x, 0.0, -0.8], z, [0.07, 0.14, 1.0]));
    }
    parts[Railgun.index()] = rail;
    parts[Flak.index()] = vec![
        p(&cube, &paint, z, z, [0.56, 0.48, 0.62]),
        p(&flak_barrel, &dark, [0.0, 0.0, -0.62], [HP, 0.0, 0.0], one),
        p(&cube, &dark, [0.0, 0.0, -0.96], z, [0.46, 0.16, 0.12]),
    ];
    let mut cap = vec![p(&cube, &paint, z, z, [0.92, 1.0, 0.92])];
    for y in [-0.28, 0.0, 0.28] {
        cap.push(p(&cube, &g_cyan, [0.0, y, 0.0], z, [0.98, 0.07, 0.98]));
    }
    for x in [-0.2, 0.2] {
        cap.push(p(&cube, &dark, [x, 0.5, 0.0], z, [0.14, 0.1, 0.14]));
    }
    parts[Capacitor.index()] = cap;
    // wheels turn on an axle along x. A part at angle `a` round it, `r` out, `x` along it: a cube
    // there stands radially (its y) once turned by `a` about x.
    let round = |x: f32, r: f32, a: f32| [x, r * a.cos(), r * a.sin()];
    let tau = std::f32::consts::TAU;
    // the tyre, its shoulders, the rim and its lips, the hub: alike on every wheel
    let body = |set: &WheelMeshes, w: f32, inset: f32, rim_mat: &PartMat| {
        let (tyre, shoulder, wheel_rim, lip, hub) = set;
        vec![
            p(tyre, &rubber, z, [0.0, 0.0, HP], one),
            p(shoulder, &rubber, [w / 2.0 - inset, 0.0, 0.0], [0.0, 0.0, HP], one),
            p(shoulder, &rubber, [-w / 2.0 + inset, 0.0, 0.0], [0.0, 0.0, HP], one),
            p(wheel_rim, rim_mat, z, [0.0, 0.0, HP], one),
            p(lip, &rim, [w / 2.0 + 0.01, 0.0, 0.0], [0.0, 0.0, HP], one),
            p(lip, &rim, [-w / 2.0 - 0.01, 0.0, 0.0], [0.0, 0.0, HP], one),
            p(hub, &dark, z, [0.0, 0.0, HP], one),
        ]
    };
    // spokes on both faces, from the hub out to the rim
    let spokes = |n: usize, w: f32, len: f32, mid: f32| {
        let mut v = Vec::new();
        for k in 0..n {
            let a = k as f32 * tau / n as f32;
            for side in [-1.0, 1.0] {
                v.push(p(&cube, &dark, round(side * (w / 2.0 + 0.015), mid, a), [a, 0.0, 0.0], [0.04, len, 0.07]));
            }
        }
        v
    };
    // 小轮: a slick tyre, three spokes
    let mut v = body(&small, ws, 0.04, &rim);
    v.extend(spokes(3, ws + 0.02, rim_s - hub_s, (rim_s + hub_s) / 2.0));
    parts[SmallWheel.index()] = v;
    // 车轮: shallow ribs round the tread, five spokes
    let mut v = body(&road, ww, 0.06, &rim);
    for k in 0..22 {
        let a = k as f32 * tau / 22.0;
        v.push(p(&cube, &rubber, round(0.0, rw - 0.02, a), [a, 0.0, 0.0], [ww - 0.08, 0.05, 0.1]));
    }
    v.extend(spokes(5, ww + 0.02, rim_w - hub_w, (rim_w + hub_w) / 2.0));
    parts[Wheel.index()] = v;
    // 越野轮: big knobby lugs, staggered side to side, a deep dark rim with a bead ring and bolts
    let mut v = body(&big, wb, 0.07, &rim_dark);
    for k in 0..18 {
        let a = k as f32 * tau / 18.0;
        let x = if k % 2 == 0 { 0.17 } else { -0.17 };
        v.push(p(&cube, &rubber, round(x, rb - 0.06, a), [a, 0.0, 0.0], [0.46, 0.14, 0.24]));
    }
    for k in 0..8 {
        let a = k as f32 * tau / 8.0 + 0.2;
        for side in [-1.0, 1.0] {
            v.push(p(&cube, &rim, round(side * (wb / 2.0 + 0.025), rim_b * 0.84, a), [a, 0.0, 0.0], [0.05, 0.07, 0.07]));
        }
    }
    parts[BigWheel.index()] = v;
    // a hover pad: a squat housing over a skirt, its underside glowing
    parts[Hover.index()] = vec![
        p(&cube, &paint, [0.0, 0.12, 0.0], z, [0.92, 0.6, 0.92]),
        p(&cube, &dark, [0.0, -0.3, 0.0], z, [1.0, 0.24, 1.0]),
        p(&pad_glow, &g_cyan, [0.0, -0.43, 0.0], [HP, 0.0, 0.0], one),
    ];
    // a weapon's mount, for a weapon standing on its face below (`spawn_mount` turns it onto
    // whichever face holds the weapon): a plate flush with the face, a stem up into the head
    let mount = vec![
        p(&cube, &dark, [0.0, -0.45, 0.0], z, [0.8, 0.1, 0.8]),
        p(&collar, &dark, [0.0, -0.38, 0.0], z, one),
        p(&stem, &dark, [0.0, -0.26, 0.0], z, one),
    ];
    let strut = StrutArt { cube: cube.clone(), body: strut_body, cap: strut_cap, coil: strut_coil, rod: strut_rod, dark: dark.clone(), steel: rim.clone() };
    BlockArt { parts, mount, strut, cube, sphere, paint_tex, armor_tex, cache: HashMap::new() }
}
