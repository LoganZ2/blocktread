//! The arena's built pieces, drawn from the very solids the physics tests: cast concrete walls
//! with a hazard band along their tops, concrete cover, steel ramps with chevrons up them,
//! crates, the hill with a band of light round its top, and the floor: concrete slabs, painted
//! lines, and hazard stripes round the pits. And its traps: saw blades running in their slots,
//! and the grates of the vents that breathe fire on the server's clock (the fire itself is
//! `fire`'s). And its pick-up pads: a ring on the floor in the pick-up's colour and its sign
//! turning over it, gone while it is taken.

use super::fire;
use super::ground::Ground;
use super::hud::team_color;
use super::Session;
use crate::Screen;
use crate::art::{glow, srgb, with_mips};
use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use blocktread_shared::arena::{Arena, CAGE_HALF, PAD_R, PIT_DEPTH, PadKind, Rect, SAW_HALF_T, SAW_R};
use blocktread_shared::rng::hash01;
use blocktread_shared::solid::{Shape, Solid, Style};
use blocktread_shared::terrain::{CELL, Terrain};

/// The hill's band of light: its material, recoloured for whoever holds the hill.
#[derive(Resource)]
pub struct HillLight(pub Handle<StandardMaterial>);

/// An arena saw's blade, by its index in the arena's saws.
#[derive(Component)]
pub struct SawBlade(usize);

/// A pick-up pad's ring: its index in the arena's pads, and the ring's material.
#[derive(Component)]
pub struct PadRing(usize, Handle<StandardMaterial>);

/// The sign turning over a pick-up pad, by the pad's index.
#[derive(Component)]
pub struct PadSign(usize);

/// A pick-up's colour: repair green, energy blue, overdrive red.
pub fn pad_color(kind: PadKind) -> u32 {
    match kind {
        PadKind::Repair => 0x5cc46e,
        PadKind::Energy => 0x4f9cf0,
        PadKind::Overdrive => 0xe5483c,
    }
}

/// The hill's band of light when nobody holds it.
const HILL_WHITE: LinearRgba = LinearRgba::rgb(2.4, 2.25, 1.95);

/// How brightly a ready pad's ring and sign glow (any brighter and they wash out to white).
const PAD_GLOW: f32 = 0.7;

/// How high a pad's sign turns.
pub const SIGN_UP: f32 = 2.2;

/// A slab of the concrete texture is this many metres across (two 4 m slabs each way).
const TILE: f32 = 8.0;

fn linear(c: u32) -> Vec3 {
    let l = srgb(c).to_linear();
    Vec3::new(l.red, l.green, l.blue)
}

/// A mesh being put together: flat-shaded faces, each with its own colour, mapped onto the
/// concrete texture by where they are in the world.
#[derive(Default)]
struct Build {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    col: Vec<[f32; 4]>,
    uv: Vec<[f32; 2]>,
    idx: Vec<u32>,
}

impl Build {
    /// A flat polygon (3 or 4 corners, in order round it) facing `out`, whichever way round the
    /// corners were given.
    fn face(&mut self, corners: &[Vec3], out: Vec3, c: Vec3) {
        let n = out.normalize();
        let mut cs = corners.to_vec();
        if (cs[1] - cs[0]).cross(cs[2] - cs[0]).dot(n) < 0.0 {
            cs.reverse();
        }
        // texture axes across the face: the floor's map on flat faces, along and up on walls
        let (u, v) = if n.y.abs() > 0.7 {
            (Vec3::X, Vec3::Z)
        } else {
            let along = Vec3::Y.cross(n).normalize();
            (along, Vec3::Y)
        };
        let base = self.pos.len() as u32;
        for p in &cs {
            self.pos.push(p.to_array());
            self.nrm.push(n.to_array());
            self.col.push([c.x, c.y, c.z, 1.0]);
            self.uv.push([p.dot(u) / TILE, -p.dot(v) / TILE]);
        }
        for k in 1..cs.len() as u32 - 1 {
            self.idx.extend_from_slice(&[base, base + k, base + k + 1]);
        }
    }

    /// A box round `centre`, `half` out each way, turned by `rot`. Its underside is left out:
    /// nothing in the arena is ever seen from below.
    fn cuboid(&mut self, centre: Vec3, half: Vec3, rot: Quat, c: Vec3) {
        let at = |x: f32, y: f32, z: f32| centre + rot * (half * Vec3::new(x, y, z));
        for (axis, s) in [(Vec3::X, 1.0), (Vec3::X, -1.0), (Vec3::Y, 1.0), (Vec3::Z, 1.0), (Vec3::Z, -1.0)] {
            let n = axis * s;
            // the two axes across this face
            let (a, b) = if axis == Vec3::Y { (Vec3::X, Vec3::Z) } else if axis == Vec3::X { (Vec3::Y, Vec3::Z) } else { (Vec3::X, Vec3::Y) };
            let corner = |i: f32, j: f32| {
                let p = n + a * i + b * j;
                at(p.x, p.y, p.z)
            };
            self.face(&[corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)], rot * n, c);
        }
    }

    fn mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.nrm)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.col)
            .with_inserted_indices(Indices::U32(self.idx))
    }
}

/// Smooth noise that wraps every `period` cells, on a grid `cells` px apart.
fn wrapping_noise(x: f32, y: f32, cells: f32, period: u32, salt: u32) -> f32 {
    let (fx, fy) = (x / cells, y / cells);
    let (i, j) = (fx.floor() as i64, fy.floor() as i64);
    let (u, v) = (fx - i as f32, fy - j as f32);
    let (u, v) = (u * u * (3.0 - 2.0 * u), v * v * (3.0 - 2.0 * v));
    let at = |a: i64, b: i64| hash01(a.rem_euclid(period as i64) as u32 + salt * 7919, b.rem_euclid(period as i64) as u32);
    let top = at(i, j) + (at(i + 1, j) - at(i, j)) * u;
    let bottom = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * u;
    top + (bottom - top) * v
}

/// Cast concrete, 2×2 slabs of 4 m with dark joints between them, each slab a shade of its own,
/// mottled and speckled. Wraps on both axes.
fn concrete_texture(images: &mut Assets<Image>) -> Handle<Image> {
    let s = 512usize;
    let slab = s / 2;
    let mut px = vec![0u8; s * s * 4];
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f32, y as f32);
            let mottle = wrapping_noise(fx, fy, 64.0, 8, 1) * 0.6 + wrapping_noise(fx, fy, 16.0, 32, 2) * 0.3 + wrapping_noise(fx, fy, 4.0, 128, 3) * 0.1;
            let speck = hash01(x as u32, y as u32 + 913);
            let shade = 0.94 + hash01((x / slab) as u32, (y / slab) as u32 + 31) * 0.1;
            let mut v = (0.8 + (mottle - 0.5) * 0.16) * shade;
            if speck > 0.985 {
                v *= 0.78;
            } else if speck < 0.01 {
                v *= 1.08;
            }
            // the joints: three pixels either side of every slab edge
            let (ex, ey) = (x % slab, y % slab);
            let edge = ex.min(slab - 1 - ex).min(ey).min(slab - 1 - ey);
            if edge < 2 {
                v *= 0.55;
            } else if edge < 3 {
                v *= 0.8;
            }
            let b = (v.clamp(0.0, 1.0) * 255.0) as u8;
            px[(y * s + x) * 4..(y * s + x) * 4 + 4].copy_from_slice(&[b, b, b, 255]);
        }
    }
    let mut img = with_mips(px, s);
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        anisotropy_clamp: 8,
        ..ImageSamplerDescriptor::linear()
    });
    images.add(img)
}

const CONCRETE: u32 = 0xb4b0a6;
const COVER: u32 = 0x9d998f;
const FLOOR: u32 = 0xa8a499;
const PLATFORM_SIDE: u32 = 0x857f75;
const STEEL: u32 = 0x5d646c;
const YELLOW: u32 = 0xe2b62a;
const BLACK: u32 = 0x26262a;
const PAINT: u32 = 0xe9e6dd;
const CRATES: [u32; 3] = [0xb4682a, 0x4e6b7c, 0x6b7a3c];

/// Alternating yellow and black along the line `a`→`b`, `w` wide towards `side`, in blocks of
/// `step`, lifted `lift` off the surface facing `n`.
#[allow(clippy::too_many_arguments)]
fn stripes(b: &mut Build, a: Vec3, to: Vec3, side: Vec3, w: f32, step: f32, n: Vec3, lift: f32) {
    let len = a.distance(to);
    let dir = (to - a) / len;
    let count = (len / step).round().max(1.0) as usize;
    let step = len / count as f32;
    for k in 0..count {
        let p = a + dir * step * k as f32 + n * lift;
        let c = if k % 2 == 0 { YELLOW } else { BLACK };
        b.face(&[p, p + dir * step, p + dir * step + side * w, p + side * w], n, linear(c));
    }
}

/// A painted line on the floor from `a` to `b`, `w` wide.
fn line(b: &mut Build, a: Vec3, to: Vec3, w: f32, c: u32) {
    let across = (to - a).cross(Vec3::Y).normalize() * w * 0.5;
    let lift = Vec3::Y * 0.015;
    b.face(&[a - across + lift, to - across + lift, to + across + lift, a + across + lift], Vec3::Y, linear(c));
}

/// The outline of a rectangle on the floor.
fn outline(b: &mut Build, r: Rect, w: f32, c: u32) {
    let (p00, p10, p11, p01) = (Vec3::new(r.x0, 0.0, r.z0), Vec3::new(r.x1, 0.0, r.z0), Vec3::new(r.x1, 0.0, r.z1), Vec3::new(r.x0, 0.0, r.z1));
    let h = w * 0.5;
    line(b, p00 - Vec3::X * h, p10 + Vec3::X * h, w, c);
    line(b, p01 - Vec3::X * h, p11 + Vec3::X * h, w, c);
    line(b, p00, p01, w, c);
    line(b, p10, p11, w, c);
}

/// A chevron pointing up a ramp: two bars meeting at the top, on the slope.
fn chevron(b: &mut Build, origin: Vec3, across: Vec3, up: Vec3, n: Vec3, at: f32, c: Vec3) {
    let (arm, rise, thick) = (1.4, 1.1, 0.4);
    let p = |u: f32, v: f32| origin + across * u + up * (at + v) + n * 0.02;
    for sx in [-1.0, 1.0] {
        b.face(&[p(0.0, rise), p(sx * arm, 0.0), p(sx * arm, thick), p(0.0, rise + thick)], n, c);
    }
}

fn ramp(b: &mut Build, s: &Solid) {
    let rot = s.rotation();
    let (hx, hz, h) = (s.half.x, s.half.z, 2.0 * s.half.y);
    let at = |x: f32, y: f32, z: f32| s.pos + rot * Vec3::new(x, y, z);
    let (a, bb, c, d, e, f) = (at(-hx, 0.0, -hz), at(hx, 0.0, -hz), at(-hx, 0.0, hz), at(hx, 0.0, hz), at(-hx, h, hz), at(hx, h, hz));
    let steel = linear(STEEL);
    let slope_n = rot * Vec3::new(0.0, 2.0 * hz, -h).normalize();
    b.face(&[a, bb, f, e], slope_n, steel);
    b.face(&[c, d, f, e], rot * Vec3::Z, steel * 0.85);
    b.face(&[a, c, e], rot * Vec3::NEG_X, steel * 0.8);
    b.face(&[bb, d, f], rot * Vec3::X, steel * 0.8);
    // chevrons up the middle, and a hazard edge along each side
    let up = (e - a).normalize();
    let across = rot * Vec3::X;
    let low = (a + bb) * 0.5;
    let run = a.distance(e);
    // in a column up the middle, or several across a wide one
    let n = (run / 3.2).floor().max(1.0) as usize;
    let cols = (hx / 4.0).round().max(1.0) as usize;
    for c in 0..cols {
        let u = (c as f32 + 0.5) / cols as f32 * 2.0 * hx - hx;
        for k in 0..n {
            chevron(b, low + across * u, across, up, slope_n, run * (k as f32 + 0.5) / n as f32 - 0.7, linear(YELLOW));
        }
    }
    for side in [-1.0f32, 1.0] {
        let x0 = a.lerp(bb, 0.5 + side * 0.5);
        let x1 = e.lerp(f, 0.5 + side * 0.5);
        stripes(b, x0, x1, -across * side, 0.35, 0.8, slope_n, 0.02);
    }
}

fn crate_box(b: &mut Build, s: &Solid, k: usize) {
    let rot = s.rotation();
    let base = linear(CRATES[(hash01(k as u32, 77) * 3.0) as usize % 3]);
    b.cuboid(s.pos, s.half, rot, base);
    // a frame round every edge, standing a little proud of the faces
    let (t, h) = (0.16, s.half + Vec3::splat(0.03));
    let frame = base * 0.55;
    for (axis, other) in [(0usize, [1usize, 2]), (1, [0, 2]), (2, [0, 1])] {
        for sa in [-1.0, 1.0] {
            for sb in [-1.0, 1.0] {
                let mut c = Vec3::ZERO;
                c[other[0]] = sa * (h[other[0]] - t);
                c[other[1]] = sb * (h[other[1]] - t);
                let mut half = Vec3::splat(t);
                half[axis] = h[axis];
                b.cuboid(s.pos + rot * c, half, rot, frame);
            }
        }
    }
}

/// The arena's pieces and floor markings (replacing any from an earlier welcome).
pub fn spawn_arena(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>, images: &mut Assets<Image>, ground: &Terrain) {
    let Some(arena) = &ground.arena else { return };
    let tex = concrete_texture(images);
    let concrete = mats.add(StandardMaterial { base_color: Color::WHITE, base_color_texture: Some(tex), perceptual_roughness: 0.92, reflectance: 0.25, ..default() });
    let painted = mats.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.8, metallic: 0.1, reflectance: 0.2, ..default() });
    let light = mats.add(StandardMaterial { base_color: srgb(0xf4efe2), emissive: HILL_WHITE, perceptual_roughness: 0.4, ..default() });
    commands.insert_resource(HillLight(light.clone()));

    let (mut cast, mut paint, mut glow) = (Build::default(), Build::default(), Build::default());
    floor(&mut cast, ground, arena);
    for (k, s) in ground.solids.iter().enumerate() {
        let rot = s.rotation();
        match (s.style, s.shape) {
            (Style::Wall, _) => {
                cast.cuboid(s.pos, s.half, rot, linear(CONCRETE));
                // a hazard band along the top of the inside face
                let top = s.pos.y + s.half.y;
                let inward = -s.pos.with_y(0.0).normalize();
                let along = Vec3::Y.cross(inward);
                let face = s.pos.with_y(0.0) + inward * (s.half.x.min(s.half.z) + 0.01);
                let reach = CAGE_HALF + 1.0;
                stripes(&mut paint, face + Vec3::Y * (top - 0.7) - along * reach, face + Vec3::Y * (top - 0.7) + along * reach, Vec3::Y, 0.55, 2.0, inward, 0.0);
            }
            (Style::Cover, _) => {
                cast.cuboid(s.pos, s.half, rot, linear(COVER));
                // a dark cap
                let cap = Vec3::new(s.half.x + 0.02, 0.08, s.half.z + 0.02);
                paint.cuboid(s.pos + Vec3::Y * (s.half.y - 0.06), cap, rot, linear(BLACK) * 1.6);
            }
            (Style::Crate, _) => crate_box(&mut paint, s, k),
            (Style::Platform, _) => {
                cast.cuboid(s.pos, s.half, rot, linear(PLATFORM_SIDE));
                // the top a lighter slab, and a band of light round it a little in from the edge
                // (its sides are all ramps)
                let top = s.pos.y + s.half.y;
                cast.face(
                    &[Vec3::new(-s.half.x, top + 0.01, -s.half.z), Vec3::new(s.half.x, top + 0.01, -s.half.z), Vec3::new(s.half.x, top + 0.01, s.half.z), Vec3::new(-s.half.x, top + 0.01, s.half.z)].map(|p| p + s.pos.with_y(0.0)),
                    Vec3::Y,
                    linear(FLOOR) * 1.05,
                );
                let (x, z, w) = (s.half.x - 0.8, s.half.z - 0.8, 0.4);
                let at = |dx: f32, dz: f32| s.pos.with_y(top + 0.01) + rot * Vec3::new(dx, 0.0, dz);
                for (a, to) in [(at(-x - w / 2.0, -z), at(x + w / 2.0, -z)), (at(-x - w / 2.0, z), at(x + w / 2.0, z)), (at(-x, -z), at(-x, z)), (at(x, -z), at(x, z))] {
                    line(&mut glow, a, to, w, 0xffffff);
                }
            }
            // the pit's own walls are drawn with the floor
            (Style::PitWall, _) => {}
            (Style::Ramp, Shape::Ramp) => ramp(&mut paint, s),
            (Style::Ramp, Shape::Block) => {
                // a kicker's deck: steel, its edge over the pit striped
                paint.cuboid(s.pos, s.half, rot, linear(STEEL) * 0.9);
                let top = s.pos.y + s.half.y + 0.02;
                let pit = if arena.in_pit(s.pos.x, s.pos.z + s.half.z + 0.5) { 1.0 } else { -1.0 };
                let edge = s.pos.with_y(top) + Vec3::Z * pit * s.half.z;
                stripes(&mut paint, edge - Vec3::X * s.half.x, edge + Vec3::X * s.half.x, Vec3::NEG_Z * pit, 0.5, 0.8, Vec3::Y, 0.0);
            }
        }
    }
    traps(commands, meshes, mats, &mut paint, arena);
    pads(commands, meshes, mats, &mut paint, arena);
    for (b, mat) in [(cast, concrete), (paint, painted), (glow, light)] {
        commands.spawn((Mesh3d(meshes.add(b.mesh())), MeshMaterial3d(mat), Transform::IDENTITY, Ground, DespawnOnExit(Screen::Flight)));
    }
}

/// The saws' slots (striped either side) and their blades, the vents' grates, and the light each
/// vent throws while it burns.
fn traps(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>, b: &mut Build, arena: &Arena) {
    let steel = mats.add(StandardMaterial { base_color: srgb(0xc4c9cf), metallic: 0.9, perceptual_roughness: 0.25, ..default() });
    let hub = mats.add(StandardMaterial { base_color: srgb(0x2a2d33), metallic: 0.7, perceptual_roughness: 0.4, ..default() });
    let disc = meshes.add(Cylinder::new(SAW_R - 0.08, 2.0 * SAW_HALF_T));
    let boss = meshes.add(Cylinder::new(0.32, 2.0 * SAW_HALF_T + 0.16));
    let tooth = meshes.add(Cuboid::new(2.0 * SAW_HALF_T + 0.02, 0.22, 0.26));
    for (k, saw) in arena.saws.iter().enumerate() {
        let run = saw.reach + SAW_R + 0.4;
        let (a, z) = (saw.mid - saw.dir * run, saw.mid + saw.dir * run);
        let across = saw.dir.cross(Vec3::Y);
        let lift = Vec3::Y * 0.014;
        b.face(&[a - across * 0.25 + lift, z - across * 0.25 + lift, z + across * 0.25 + lift, a + across * 0.25 + lift], Vec3::Y, linear(BLACK) * 0.5);
        for side in [-1.0, 1.0] {
            stripes(b, a + across * side * 0.25, z + across * side * 0.25, across * side, 0.45, 0.9, Vec3::Y, 0.016);
        }
        // the blade turns about its axle, along x here (see `update_traps`)
        let blade = commands.spawn((Transform::from_translation(saw.at(0.0)), Visibility::default(), SawBlade(k), DespawnOnExit(Screen::Flight))).id();
        let along_x = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        commands.spawn((Mesh3d(disc.clone()), MeshMaterial3d(steel.clone()), Transform::from_rotation(along_x), ChildOf(blade)));
        commands.spawn((Mesh3d(boss.clone()), MeshMaterial3d(hub.clone()), Transform::from_rotation(along_x), ChildOf(blade)));
        for t in 0..24 {
            let ang = t as f32 * std::f32::consts::TAU / 24.0;
            let at = Vec3::new(0.0, ang.cos(), ang.sin()) * (SAW_R - 0.06);
            commands.spawn((Mesh3d(tooth.clone()), MeshMaterial3d(steel.clone()), Transform::from_translation(at).with_rotation(Quat::from_rotation_x(ang + 0.6)), ChildOf(blade)));
        }
    }
    for (k, v) in arena.vents.iter().enumerate() {
        let r = v.area;
        // the grate: a dark frame, and bars across over the glowing plate
        let lift = fire::GRATE_Y;
        let corners = |r: Rect, y: f32| [Vec3::new(r.x0, y, r.z0), Vec3::new(r.x1, y, r.z0), Vec3::new(r.x1, y, r.z1), Vec3::new(r.x0, y, r.z1)];
        b.face(&corners(Rect { x0: r.x0 - 0.4, x1: r.x1 + 0.4, z0: r.z0 - 0.4, z1: r.z1 + 0.4 }, 0.012), Vec3::Y, linear(BLACK) * 0.8);
        let long_x = r.x1 - r.x0 > r.z1 - r.z0;
        let n = 12;
        for i in 0..=n {
            let f = i as f32 / n as f32;
            let bar = if long_x {
                let x = r.x0 + (r.x1 - r.x0) * f;
                Rect { x0: x - 0.07, x1: x + 0.07, z0: r.z0, z1: r.z1 }
            } else {
                let z = r.z0 + (r.z1 - r.z0) * f;
                Rect { x0: r.x0, x1: r.x1, z0: z - 0.07, z1: z + 0.07 }
            };
            b.face(&corners(bar, lift + 0.01), Vec3::Y, linear(STEEL) * 0.7);
        }
        // the hot plate under the bars, and the flames over it, are the fire's to draw
        fire::vent_lights(commands, k, r);
    }
}

/// Each pad: a dark plate on the floor, a ring round it (its own mesh, so that it can go dark
/// while the pad is taken) and the sign over it: a cross for repair, a bolt for energy, two
/// chevrons for overdrive.
fn pads(commands: &mut Commands, meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>, b: &mut Build, arena: &Arena) {
    let seg = 32;
    let ring_at = |r0: f32, r1: f32, y: f32| {
        let mut m = Build::default();
        for k in 0..seg {
            let (a0, a1) = (k as f32 / seg as f32 * std::f32::consts::TAU, (k + 1) as f32 / seg as f32 * std::f32::consts::TAU);
            let at = |a: f32, r: f32| Vec3::new(a.cos() * r, y, a.sin() * r);
            m.face(&[at(a0, r0), at(a1, r0), at(a1, r1), at(a0, r1)], Vec3::Y, Vec3::ONE);
        }
        m.mesh()
    };
    let ring = meshes.add(ring_at(PAD_R - 0.45, PAD_R, 0.03));
    let mut bar = |x: f32, y: f32| meshes.add(Cuboid::new(x, y, 0.32));
    let (long, short) = (bar(1.5, 0.44), bar(0.44, 1.5));
    let (slant, step) = (bar(0.38, 0.95), bar(0.75, 0.3));
    let arm = bar(0.9, 0.3);
    for (k, pad) in arena.pads.iter().enumerate() {
        // the plate, a little wider than the ring, over the floor's lines
        let mid = pad.at + Vec3::Y * 0.022;
        let rim: Vec<Vec3> = (0..seg).map(|i| mid + Vec3::new((i as f32 / seg as f32 * std::f32::consts::TAU).cos(), 0.0, (i as f32 / seg as f32 * std::f32::consts::TAU).sin()) * (PAD_R + 0.25)).collect();
        for i in 0..seg {
            b.face(&[mid, rim[i], rim[(i + 1) % seg]], Vec3::Y, linear(BLACK) * 0.8);
        }
        let col = pad_color(pad.kind);
        let mat = mats.add(StandardMaterial { base_color: srgb(col), emissive: glow(col, PAD_GLOW), perceptual_roughness: 0.85, reflectance: 0.15, ..default() });
        commands.spawn((Mesh3d(ring.clone()), MeshMaterial3d(mat.clone()), Transform::from_translation(pad.at), PadRing(k, mat.clone()), DespawnOnExit(Screen::Flight)));
        let sign = commands.spawn((Transform::from_translation(pad.at + Vec3::Y * SIGN_UP), Visibility::default(), PadSign(k), DespawnOnExit(Screen::Flight))).id();
        let parts: Vec<(Handle<Mesh>, Vec3, f32)> = match pad.kind {
            PadKind::Repair => vec![(long.clone(), Vec3::ZERO, 0.0), (short.clone(), Vec3::ZERO, 0.0)],
            PadKind::Energy => vec![(slant.clone(), Vec3::new(0.16, 0.42, 0.0), -0.45), (step.clone(), Vec3::ZERO, 0.0), (slant.clone(), Vec3::new(-0.16, -0.42, 0.0), -0.45)],
            PadKind::Overdrive => [0.32f32, -0.22]
                .iter()
                .flat_map(|&y| [(arm.clone(), Vec3::new(-0.29, y, 0.0), 0.75), (arm.clone(), Vec3::new(0.29, y, 0.0), -0.75)])
                .collect(),
        };
        for (mesh, at, tilt) in parts {
            commands.spawn((Mesh3d(mesh), MeshMaterial3d(mat.clone()), Transform::from_translation(at).with_rotation(Quat::from_rotation_z(tilt)), ChildOf(sign)));
        }
    }
}

/// The pads as the server last said: a taken one's ring dark and its sign gone; a ready one's
/// sign turning and bobbing.
pub fn update_pads(session: Res<Session>, rings: Query<&PadRing>, mut signs: Query<(&PadSign, &mut Transform, &mut Visibility)>, mut mats: ResMut<Assets<StandardMaterial>>, time: Res<Time>) {
    let core = &session.core;
    let Some(arena) = &core.terrain.arena else { return };
    let t = time.elapsed_secs();
    for r in &rings {
        let Some(pad) = arena.pads.get(r.0) else { continue };
        let ready = core.pad_left(r.0) <= 0.0;
        let col = pad_color(pad.kind);
        let (base, lit) = if ready { (srgb(col), glow(col, PAD_GLOW)) } else { (srgb(0x2c2d30), LinearRgba::BLACK) };
        if mats.get(&r.1).is_some_and(|m| m.emissive != lit)
            && let Some(mut m) = mats.get_mut(&r.1)
        {
            m.base_color = base;
            m.emissive = lit;
        }
    }
    for (sign, mut tf, mut vis) in &mut signs {
        let Some(pad) = arena.pads.get(sign.0) else { continue };
        let ready = core.pad_left(sign.0) <= 0.0;
        vis.set_if_neq(if ready { Visibility::Inherited } else { Visibility::Hidden });
        let phase = sign.0 as f32 * 1.3;
        tf.translation = pad.at + Vec3::Y * (SIGN_UP + 0.18 * (t * 2.2 + phase).sin());
        tf.rotation = Quat::from_rotation_y(t * 1.6 + phase);
    }
}

/// The hill's band of light in the colour of the side alone on it, throbbing amber while both
/// sides are, its own warm white while nobody is (and always, outside 2v2).
pub fn update_hill_light(session: Res<Session>, light: Option<Res<HillLight>>, mut mats: ResMut<Assets<StandardMaterial>>, time: Res<Time>) {
    let (Some(light), Some(hill)) = (light, session.core.hill) else { return };
    let lit = match (hill.holder, hill.contested) {
        (_, true) => glow(0xf0a830, 1.4 + 1.2 * (time.elapsed_secs() * 5.0).sin().abs()),
        (Some(team), _) => team_color(team).to_linear() * 2.6,
        (None, false) => HILL_WHITE,
    };
    if mats.get(&light.0).is_some_and(|m| m.emissive != lit)
        && let Some(mut m) = mats.get_mut(&light.0)
    {
        m.emissive = lit;
    }
}

/// The saws on the clock the own vehicle is predicted at, so that it meets a blade where it sees
/// one: they run along their slots spinning. (The vents' fire is `fire::update_fire`'s.)
pub fn update_traps(session: Res<Session>, mut blades: Query<(&SawBlade, &mut Transform)>) {
    let Some(arena) = &session.core.terrain.arena else { return };
    let t = session.core.arena_time();
    for (b, mut tf) in &mut blades {
        let saw = &arena.saws[b.0];
        tf.translation = saw.at(t);
        let spin = (t * 14.0).rem_euclid(std::f64::consts::TAU) as f32;
        tf.rotation = Quat::from_rotation_arc(Vec3::X, saw.dir.cross(Vec3::Y)) * Quat::from_rotation_x(-spin);
    }
}

/// The concrete floor inside the walls (every cell of the ground there that is level), lines
/// painted on it, and hazard stripes round the pits.
fn floor(b: &mut Build, ground: &Terrain, arena: &Arena) {
    let floor = linear(FLOOR);
    let cells = (2.0 * CAGE_HALF / CELL) as i32;
    for j in 0..cells {
        for i in 0..cells {
            let (x0, z0) = (-CAGE_HALF + i as f32 * CELL, -CAGE_HALF + j as f32 * CELL);
            if is_floor(ground, x0, z0) {
                let (x1, z1) = (x0 + CELL, z0 + CELL);
                b.face(&[Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z0), Vec3::new(x1, 0.0, z1), Vec3::new(x0, 0.0, z1)], Vec3::Y, floor);
            }
        }
    }
    // round the hill and its ramps, and a box at every spawn
    outline(b, Rect::around(0.0, 0.0, 27.0, 27.0), 0.35, PAINT);
    for s in &arena.spawns {
        outline(b, Rect::around(s.x, s.z, 6.0, 6.0), 0.3, PAINT);
    }
    // the middle line between the 2v2 sides, broken where the hill is
    for k in [-1.0, 1.0] {
        line(b, Vec3::new(0.0, 0.0, k * 27.0), Vec3::new(0.0, 0.0, k * 40.0), 0.35, PAINT);
        line(b, Vec3::new(0.0, 0.0, k * 72.0), Vec3::new(0.0, 0.0, k * (CAGE_HALF - 2.0)), 0.35, PAINT);
    }
    // the pits: straight concrete sides, darkening down to the floor
    for p in &arena.pits {
        let (top, bottom) = (linear(0x6a665f), linear(0x2a2927));
        let d = -PIT_DEPTH;
        let corners = [Vec3::new(p.x0, 0.0, p.z0), Vec3::new(p.x1, 0.0, p.z0), Vec3::new(p.x1, 0.0, p.z1), Vec3::new(p.x0, 0.0, p.z1)];
        let centre = p.centre();
        for k in 0..4 {
            let (a, c) = (corners[k], corners[(k + 1) % 4]);
            let inward = (centre - (a + c) * 0.5).with_y(0.0).normalize();
            // in two bands, lighter at the top
            let mid = d * 0.4;
            b.face(&[a, c, c.with_y(mid), a.with_y(mid)], inward, top);
            b.face(&[a.with_y(mid), c.with_y(mid), c.with_y(d), a.with_y(d)], inward, bottom * 1.4);
        }
        b.face(&corners.map(|c| c.with_y(d)), Vec3::Y, bottom);
    }
    for p in &arena.pits {
        let w = 1.2;
        let lift = 0.016;
        stripes(b, Vec3::new(p.x0, 0.0, p.z0), Vec3::new(p.x1, 0.0, p.z0), Vec3::NEG_Z, w, 1.0, Vec3::Y, lift);
        stripes(b, Vec3::new(p.x0, 0.0, p.z1), Vec3::new(p.x1, 0.0, p.z1), Vec3::Z, w, 1.0, Vec3::Y, lift);
        stripes(b, Vec3::new(p.x0, 0.0, p.z0 - w), Vec3::new(p.x0, 0.0, p.z1 + w), Vec3::NEG_X, w, 1.0, Vec3::Y, lift);
        stripes(b, Vec3::new(p.x1, 0.0, p.z0 - w), Vec3::new(p.x1, 0.0, p.z1 + w), Vec3::X, w, 1.0, Vec3::Y, lift);
    }
}

/// Whether the ground cell with its low corner at (x, z) is drawn by the arena, not the terrain:
/// level floor, or a pit (drawn with straight sides).
pub fn is_arena_cell(ground: &Terrain, x: f32, z: f32) -> bool {
    is_floor(ground, x, z) || ground.arena.as_ref().is_some_and(|a| a.in_pit(x + CELL * 0.5, z + CELL * 0.5))
}

/// Whether the ground cell with its low corner at (x, z) is level arena floor.
fn is_floor(ground: &Terrain, x: f32, z: f32) -> bool {
    let inside = x >= -CAGE_HALF - 1e-3 && z >= -CAGE_HALF - 1e-3 && x + CELL <= CAGE_HALF + 1e-3 && z + CELL <= CAGE_HALF + 1e-3;
    inside && [(0.0, 0.0), (CELL, 0.0), (0.0, CELL), (CELL, CELL)].iter().all(|(dx, dz)| ground.height(x + dx, z + dz).abs() < 1e-4)
}
