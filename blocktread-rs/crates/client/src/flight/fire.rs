//! Fire: the flames the arena's vents breathe, their grates heating up before and cooling after,
//! and the fireballs of explosions rolling up into smoke.
//!
//! One shader (`fire.wesl`) draws all of it, marching through each flame and fireball as a
//! glowing volume. This side keeps the fireballs moving and rebuilds the one mesh that carries
//! every shape each frame: a box apiece (a plate for a grate), its vertices holding the shape's
//! numbers. Light goes with the fire: a vent lights the floor and whoever is near while it
//! burns, and every blast flashes.

use super::Session;
use crate::{DevOpts, MainCamera, Screen};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, RenderPipelineDescriptor, SpecializedMeshPipelineError, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;
use blocktread_shared::arena::{Rect, VENT_CYCLE, VENT_HIGH, VENT_ON, VENT_WARN};
use blocktread_shared::rng::Rng;

pub struct FirePlugin;

impl Plugin for FirePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "fire.wesl");
        app.add_plugins(MaterialPlugin::<FireMaterial>::default())
            .insert_resource(Fires { balls: Vec::new(), rng: Rng::new(0xf12e), mesh: None, noise: None })
            .add_systems(Startup, bake_noise)
            .add_systems(OnEnter(Screen::Flight), spawn_fire);
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct FireMaterial {
    #[texture(0, dimension = "3d")]
    #[sampler(1)]
    noise: Handle<Image>,
}

impl Material for FireMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://blocktread_client/flight/fire.wesl".into()
    }
    // glow over the scene, smoke over it darkening
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    // both sides: from inside a shape only its far faces are left to march from
    fn specialize(_: &MaterialPipeline, descriptor: &mut RenderPipelineDescriptor, _: &MeshVertexBufferLayoutRef, _: MaterialPipelineKey<Self>) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

// ---------------------------------------------------------------- tuning

/// How far a vent's box reaches past its grate on every side (`PAD` in the shader).
const PAD: f32 = 0.5;
/// How high a vent's flames reach at full burn; the damage reaches `VENT_HIGH`, the tips past it.
const FLAME_HIGH: f32 = VENT_HIGH * 1.5;
/// Where a vent's grate glows, just over the floor and under its bars.
pub const GRATE_Y: f32 = 0.016;
/// The light each of a burning vent's lamps throws (lm; three along it, low over the grate), and
/// the colour of fire. Far more than flames that size would give: in full sun anything less
/// doesn't show on the floor.
const VENT_LUMENS: f32 = 1.4e7;
const FIRE_LIGHT: Color = Color::srgb(1.0, 0.52, 0.2);
/// Fireballs in the air at most; a new one past this is dropped.
const MAX_BALLS: usize = 320;
/// The noise volume: texels along a side, and lattice cells across it (it tiles).
const NOISE_N: usize = 64;
const NOISE_CELLS: usize = 8;

// ---------------------------------------------------------------- state

/// A fireball: where it is and how it drifts, how big it rolls out, how old it is (below zero
/// until it goes up) and how long it lasts, how hot it starts (1 for the heart of a blast), and
/// the seed that shapes its billows.
struct Ball {
    pos: Vec3,
    vel: Vec3,
    size: f32,
    age: f32,
    life: f32,
    heat: f32,
    seed: f32,
}

#[derive(Resource)]
pub struct Fires {
    balls: Vec<Ball>,
    rng: Rng,
    mesh: Option<Handle<Mesh>>,
    noise: Option<Handle<Image>>,
}

/// The light a vent throws while it burns, by the vent's index in the arena.
#[derive(Component)]
pub struct VentLight(pub usize);

/// A blast's flash of light, dying away over its life.
#[derive(Component)]
pub struct Flash {
    age: f32,
    life: f32,
    peak: f32,
}

/// Vent `k`'s lights over its grate `r`, three along it, dark until it burns.
pub fn vent_lights(commands: &mut Commands, k: usize, r: Rect) {
    let (a, b) = if r.x1 - r.x0 > r.z1 - r.z0 { (Vec3::new(r.x0, 0.8, r.centre().z), Vec3::new(r.x1, 0.8, r.centre().z)) } else { (Vec3::new(r.centre().x, 0.8, r.z0), Vec3::new(r.centre().x, 0.8, r.z1)) };
    for f in [1.0 / 6.0, 0.5, 5.0 / 6.0] {
        commands.spawn((PointLight { color: FIRE_LIGHT, intensity: 0.0, range: 20.0, radius: 1.5, ..default() }, Transform::from_translation(a.lerp(b, f)), VentLight(k), DespawnOnExit(Screen::Flight)));
    }
}

/// Four independent fields of gradient noise in one volume that tiles, a field to a channel,
/// each stretched to fill 0..1.
fn bake_noise(mut fires: ResMut<Fires>, mut images: ResMut<Assets<Image>>) {
    let (n, c) = (NOISE_N, NOISE_CELLS);
    let mut data = vec![0u8; n * n * n * 4];
    let mut vals = vec![0f32; n * n * n];
    for ch in 0..4 {
        let grads: Vec<Vec3> = (0..c * c * c).map(|_| fires.rng.unit()).collect();
        let grad = |x: usize, y: usize, z: usize| grads[x % c + (y % c) * c + (z % c) * c * c];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let p = Vec3::new(x as f32, y as f32, z as f32) * (c as f32 / n as f32);
                    let i = p.floor();
                    let f = p - i;
                    let w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
                    let mut v = 0.0;
                    for k in 0..8 {
                        let o = UVec3::new(k & 1, (k >> 1) & 1, (k >> 2) & 1);
                        let along = Vec3::select(o.cmpeq(UVec3::ONE), w, Vec3::ONE - w);
                        let g = grad(i.x as usize + o.x as usize, i.y as usize + o.y as usize, i.z as usize + o.z as usize);
                        v += along.x * along.y * along.z * g.dot(f - o.as_vec3());
                    }
                    vals[x + y * n + z * n * n] = v;
                }
            }
        }
        let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        for (i, v) in vals.iter().enumerate() {
            data[i * 4 + ch] = ((v - lo) / (hi - lo) * 255.0).round() as u8;
        }
    }
    let size = Extent3d { width: n as u32, height: n as u32, depth_or_array_layers: n as u32 };
    let mut img = Image::new(size, TextureDimension::D3, data, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    fires.noise = Some(images.add(img));
}

fn spawn_fire(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<FireMaterial>>, mut fires: ResMut<Fires>) {
    fires.balls.clear();
    let Some(noise) = fires.noise.clone() else { return };
    let mesh = meshes.add(Shapes::default().mesh());
    fires.mesh = Some(mesh.clone());
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mats.add(FireMaterial { noise })),
        Transform::IDENTITY,
        Visibility::default(),
        // world-space vertices that change every frame: the cached bounds would be wrong
        NoFrustumCulling,
        DespawnOnExit(Screen::Flight),
    ));
}

// ---------------------------------------------------------------- what makes fire

/// A fireball going up at `pos`, drifting with `vel`, rolling out to `size` across its middle
/// and lasting `life` seconds; `heat` 1 is white-hot.
pub fn fireball(fires: &mut Fires, pos: Vec3, vel: Vec3, size: f32, heat: f32, life: f32) {
    if fires.balls.len() < MAX_BALLS {
        let seed = fires.rng.f32() * 9.0;
        fires.balls.push(Ball { pos, vel, size, age: 0.0, life, heat, seed });
    }
}

/// A blast of `size` (1 for a missile's): a white-hot heart with smaller balls thrown out of it
/// a moment after, all rolling up into smoke, and a flash of light.
pub fn blast(commands: &mut Commands, fires: &mut Fires, pos: Vec3, size: f32, base: Vec3) {
    let life = 1.5 + 0.35 * size;
    fireball(fires, pos, base * 0.5, 2.0 * size, 1.0, life);
    let more = (3.0 + size * 2.0).min(9.0) as usize;
    for _ in 0..more {
        let d = fires.rng.unit();
        let vel = base * 0.5 + d * size * fires.rng.range(3.0, 7.0);
        let (s, heat, l, late) = (size * fires.rng.range(0.9, 1.4), fires.rng.range(0.7, 0.95), life * fires.rng.range(0.7, 1.0), fires.rng.range(0.0, 0.12));
        if fires.balls.len() < MAX_BALLS {
            let seed = fires.rng.f32() * 9.0;
            fires.balls.push(Ball { pos: pos + d * size * 0.8, vel, size: s, age: -late, life: l, heat, seed });
        }
    }
    commands.spawn((
        PointLight { color: FIRE_LIGHT, intensity: 0.0, range: 14.0 + 10.0 * size, radius: size, ..default() },
        Transform::from_translation(pos + Vec3::Y * size),
        Flash { age: 0.0, life: 0.45, peak: 3.0e7 * size },
        DespawnOnExit(Screen::Flight),
    ));
}

/// How hard a vent burns (0..1, a little over as it catches) and how hot its grate is (0..1),
/// `into` seconds into its cycle: the grate warms up, the flames catch with a whoosh, burn, and
/// die down as the damage stops; the grate then cools.
fn vent_state(into: f32) -> (f32, f32) {
    let catch = smoothstep(0.0, 0.16, into) * (1.0 + 0.25 * (-((into - 0.18) / 0.1).powi(2)).exp());
    let burn = if into < VENT_ON + 0.05 { catch * (1.0 - smoothstep(VENT_ON - 0.3, VENT_ON + 0.05, into)) } else { 0.0 };
    let heat = if into < VENT_ON {
        1.0
    } else if into > VENT_CYCLE - VENT_WARN {
        (into - (VENT_CYCLE - VENT_WARN)) / VENT_WARN * 0.7
    } else {
        (-(into - VENT_ON) * 1.6).exp()
    };
    (burn, heat)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Advance the fireballs and the flashes, light the vents, and rebuild the mesh: the grates,
/// then the flames over them, then the fireballs from the farthest in.
#[allow(clippy::too_many_arguments)]
pub fn update_fire(
    mut commands: Commands,
    session: Res<Session>,
    time: Res<Time>,
    cam: Single<&Transform, With<MainCamera>>,
    mut fires: ResMut<Fires>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut vent_lights: Query<(&VentLight, &mut PointLight), Without<Flash>>,
    mut flashes: Query<(Entity, &mut Flash, &mut PointLight)>,
    dev: Res<DevOpts>,
    mut boom: Local<f32>,
) {
    let dt = time.delta_secs();
    if session.paused {
        return;
    }
    let fires = &mut *fires;
    // dev: blasts going off before the vent close-up, big and small in turn
    if dev.forced("boom", time.elapsed_secs()) {
        *boom -= dt;
        if *boom <= 0.0 {
            *boom = 2.2;
            let (at, size) = if fires.rng.f32() < 0.5 { (Vec3::new(-3.0, 0.5, 27.0), 1.6) } else { (Vec3::new(4.0, 1.0, 28.0), 0.7) };
            blast(&mut commands, fires, at, size, Vec3::ZERO);
        }
    }
    let mut shapes = Shapes::default();

    if let Some(arena) = &session.core.terrain.arena {
        let t = session.core.arena_time();
        let states: Vec<(f32, f32)> = arena.vents.iter().map(|v| vent_state(v.into(t))).collect();
        for (k, v) in arena.vents.iter().enumerate() {
            shapes.plate(v.area, GRATE_Y, [1.0, states[k].1], [0.0, k as f32 * 1.7]);
        }
        for (k, v) in arena.vents.iter().enumerate() {
            let burn = states[k].0;
            if burn > 0.0 {
                let r = v.area;
                let high = FLAME_HIGH * (0.35 + 0.65 * burn.min(1.0));
                shapes.cuboid(Vec3::new(r.x0 - PAD, 0.0, r.z0 - PAD), Vec3::new(r.x1 + PAD, high, r.z1 + PAD), [r.x0, r.z0, r.x1, r.z1], [0.0, burn], [high, k as f32 * 1.7]);
            }
        }
        let now = time.elapsed_secs();
        for (l, mut light) in &mut vent_lights {
            let (burn, heat) = states.get(l.0).copied().unwrap_or_default();
            let flicker = 0.85 + 0.1 * (now * 23.0 + l.0 as f32).sin() + 0.05 * (now * 41.0).sin();
            light.intensity = burn * VENT_LUMENS * flicker + heat * 2.0e5;
        }
    }

    for (e, mut f, mut light) in &mut flashes {
        f.age += dt;
        if f.age >= f.life {
            commands.entity(e).despawn();
            continue;
        }
        light.intensity = f.peak * (1.0 - f.age / f.life).powi(2);
    }

    // the balls roll out fast, then slow; as the fire in them burns out the smoke rises
    let slow = (-2.5 * dt).exp();
    for b in fires.balls.iter_mut() {
        b.age += dt;
        if b.age < 0.0 {
            continue;
        }
        let k = b.age / b.life;
        b.vel = b.vel * slow + Vec3::Y * 3.0 * smoothstep(0.05, 0.5, k) * dt;
        b.pos += b.vel * dt;
    }
    fires.balls.retain(|b| b.age < b.life);
    let eye = cam.translation;
    let mut order: Vec<(f32, usize)> = fires.balls.iter().enumerate().filter(|(_, b)| b.age >= 0.0).map(|(i, b)| (b.pos.distance_squared(eye), i)).collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, i) in order {
        let b = &fires.balls[i];
        let k = b.age / b.life;
        let out = 1.0 - (1.0 - (b.age / 0.3).min(1.0)).powi(3);
        let r = b.size * (0.3 + 0.7 * out) * (1.0 + 0.35 * k);
        // on the ground it rolls up off it rather than sinking into it
        let ground = session.core.terrain.height(b.pos.x, b.pos.z);
        let c = b.pos.with_y(b.pos.y.max(ground + r * 0.45));
        shapes.cuboid(c - Vec3::splat(r), c + Vec3::splat(r), [c.x, c.y, c.z, r], [2.0, k], [b.heat, b.seed]);
    }

    if let Some(h) = &fires.mesh
        && let Some(mut mesh) = meshes.get_mut(h)
    {
        *mesh = shapes.mesh();
    }
}

// ---------------------------------------------------------------- the mesh

/// The mesh being put together: every vertex carries its shape's numbers (see `fire.wesl`).
#[derive(Default)]
struct Shapes {
    pos: Vec<[f32; 3]>,
    col: Vec<[f32; 4]>,
    uv: Vec<[f32; 2]>,
    uv_b: Vec<[f32; 2]>,
    idx: Vec<u32>,
}

impl Shapes {
    fn vertex(&mut self, p: Vec3, col: [f32; 4], uv: [f32; 2], uv_b: [f32; 2]) {
        self.pos.push(p.to_array());
        self.col.push(col);
        self.uv.push(uv);
        self.uv_b.push(uv_b);
    }

    /// A box from `lo` to `hi`, its faces wound counter-clockwise seen from outside.
    fn cuboid(&mut self, lo: Vec3, hi: Vec3, col: [f32; 4], uv: [f32; 2], uv_b: [f32; 2]) {
        let base = self.pos.len() as u32;
        for i in 0..8u32 {
            let at = Vec3::select(BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0), hi, lo);
            self.vertex(at, col, uv, uv_b);
        }
        for [a, b, c, d] in [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]] {
            self.idx.extend([a, b, c, a, c, d].map(|v| base + v));
        }
    }

    /// A grate's plate over `r` at height `y`, facing up.
    fn plate(&mut self, r: Rect, y: f32, uv: [f32; 2], uv_b: [f32; 2]) {
        let base = self.pos.len() as u32;
        let col = [r.x0, r.z0, r.x1, r.z1];
        for (x, z) in [(r.x0, r.z0), (r.x0, r.z1), (r.x1, r.z1), (r.x1, r.z0)] {
            self.vertex(Vec3::new(x, y, z), col, uv, uv_b);
        }
        self.idx.extend([0, 1, 2, 0, 2, 3].map(|v| base + v));
    }

    fn mesh(mut self) -> Mesh {
        if self.idx.is_empty() {
            // a zero-area triangle keeps the mesh valid while nothing burns
            for _ in 0..3 {
                self.vertex(Vec3::ZERO, [0.0; 4], [1.0, 0.0], [0.0; 2]);
            }
            self.idx = vec![0, 1, 2];
        }
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.col)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.uv_b)
            .with_inserted_indices(Indices::U32(self.idx))
    }
}
