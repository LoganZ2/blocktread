//! Engine exhaust as one continuous stream of glowing gas per nozzle.
//!
//! Every nozzle emits puffs of gas: each leaves along the nozzle axis with the ship's velocity
//! plus the exhaust velocity, then slows, grows, cools from white-hot to the team colour and is
//! eroded into wisps by the shader (`exhaust.wesl`). The flame at the nozzle and the trail behind
//! it are the same gas at different ages, so they always form one stream, through turns and
//! drift alike. Puffs are emitted along the path the nozzle swept during the frame, so the
//! stream stays continuous at any speed. All puffs share one mesh, rebuilt each frame as
//! camera-facing quads.

use super::Session;
use super::ships::ShipVisuals;
use crate::{MainCamera, Screen};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use std::collections::HashMap;
use std::f32::consts::TAU;
use voxwing_shared::rng::Rng;

pub struct ExhaustPlugin;

impl Plugin for ExhaustPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "exhaust.wesl");
        app.add_plugins(MaterialPlugin::<GasMaterial>::default())
            .insert_resource(Exhaust { puffs: Vec::new(), emitters: HashMap::new(), rng: Rng::new(0x5eed), mesh: None })
            .add_systems(OnEnter(Screen::Flight), spawn_cloud);
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct GasMaterial {
    /// Brightness and tint for the whole effect.
    #[uniform(0)]
    tint: Vec4,
}

impl Material for GasMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://voxwing_client/flight/exhaust.wesl".into()
    }
    // additive glow: order-independent, no depth writes
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(_: &MaterialPipeline, descriptor: &mut RenderPipelineDescriptor, _: &MeshVertexBufferLayoutRef, _: MaterialPipelineKey<Self>) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

// ---------------------------------------------------------------- tuning

/// Share of the source's velocity the gas keeps, and how fast it then slows (1/s).
const CARRY: f32 = 0.85;
const DRAG: f32 = 2.2;
/// Brightness of a single puff; many overlap, so this stays low.
const PUFF_GAIN: f32 = 0.11;
const MAX_PUFFS: usize = 24_000;

/// Colours (linear HDR): white-hot fresh gas and the cooled trail.
#[derive(Clone, Copy)]
struct Palette {
    hot: Vec3,
    cool: Vec3,
}

const FRIENDLY: Palette = Palette { hot: Vec3::new(2.0, 2.9, 4.6), cool: Vec3::new(0.05, 0.28, 1.4) };
const HOSTILE: Palette = Palette { hot: Vec3::new(4.6, 2.5, 1.0), cool: Vec3::new(1.4, 0.2, 0.03) };
const MISSILE: Palette = Palette { hot: Vec3::new(4.0, 2.7, 1.3), cool: Vec3::new(0.32, 0.3, 0.34) };

// ---------------------------------------------------------------- state

struct Puff {
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    /// Radius when fresh and when spent.
    size: (f32, f32),
    palette: Palette,
    strength: f32,
    seed: f32,
}

/// A nozzle, identified by its ship visual (a respawn gets a new one) and its local position,
/// or a missile.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Emitter {
    Engine(Entity, IVec3),
    Missile(u16),
}

/// Where an emitter was last frame, so this frame's puffs can be spread along the path between.
#[derive(Clone, Copy)]
struct Nozzle {
    pos: Vec3,
    /// Exhaust direction (out of the nozzle).
    dir: Vec3,
    vel: Vec3,
    /// Fractional puffs carried over to the next frame.
    owed: f32,
}

/// What one emitter does this frame.
struct Emission {
    key: Emitter,
    now: Nozzle,
    palette: Palette,
    /// Puffs per second.
    rate: f32,
    exhaust_speed: f32,
    carry: f32,
    life: f32,
    size: (f32, f32),
    strength: f32,
    /// Radius of the always-on glow at the nozzle.
    throat: f32,
}

#[derive(Resource)]
pub struct Exhaust {
    puffs: Vec<Puff>,
    emitters: HashMap<Emitter, Nozzle>,
    rng: Rng,
    mesh: Option<Handle<Mesh>>,
}

fn spawn_cloud(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<GasMaterial>>, mut ex: ResMut<Exhaust>) {
    ex.puffs.clear();
    ex.emitters.clear();
    let mesh = meshes.add(quads(&[], Quat::IDENTITY));
    ex.mesh = Some(mesh.clone());
    commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(mats.add(GasMaterial { tint: Vec4::ONE })),
        Transform::IDENTITY,
        Visibility::default(),
        // world-space vertices that change every frame: the cached bounds would be wrong
        NoFrustumCulling,
        DespawnOnExit(Screen::Flight),
    ));
}

/// One camera-facing quad to draw: centre, radius, colour, age 0..1, seed (also sets its spin).
type Sprite = (Vec3, f32, Vec3, f32, f32);

fn quads(sprites: &[Sprite], cam_rot: Quat) -> Mesh {
    let (right, up) = (cam_rot * Vec3::X, cam_rot * Vec3::Y);
    let n = sprites.len().max(1);
    let (mut pos, mut uv, mut info, mut col) = (Vec::with_capacity(n * 4), Vec::with_capacity(n * 4), Vec::with_capacity(n * 4), Vec::with_capacity(n * 4));
    let mut idx = Vec::with_capacity(n * 6);
    for &(c, r, color, age, seed) in sprites {
        // each puff turns a little so the noise never repeats from puff to puff
        let (s, k) = (seed * TAU + age * 1.5).sin_cos();
        let (rx, uy) = ((right * k + up * s) * r, (up * k - right * s) * r);
        let base = pos.len() as u32;
        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            pos.push((c + rx * x + uy * y).to_array());
            uv.push([x, y]);
            info.push([age, seed]);
            col.push([color.x, color.y, color.z, 1.0]);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    if idx.is_empty() {
        // a zero-area triangle keeps the mesh valid while there is no gas
        pos = vec![[0.0; 3]; 3];
        uv = vec![[0.0; 2]; 3];
        info = vec![[1.0, 0.0]; 3];
        col = vec![[0.0; 4]; 3];
        idx = vec![0, 1, 2];
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, info)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col)
        .with_inserted_indices(Indices::U32(idx))
}

/// Colour of a puff at age `t` (0..1): a brief white-hot flash at birth (the flame right
/// behind the nozzle), then cooling to the team colour and fading out.
fn puff_color(p: Palette, t: f32, strength: f32) -> Vec3 {
    let flame = 1.0 + 2.5 * (-t * 30.0).exp();
    p.cool.lerp(p.hot, (-t * 14.0).exp()) * strength * flame * (1.0 - t).powf(1.6) * PUFF_GAIN
}

/// Advance the gas, emit new puffs from every nozzle and missile, and rebuild the mesh.
/// Runs after the camera has moved, so the quads face it.
pub fn update_exhaust(session: Res<Session>, vis: Res<ShipVisuals>, time: Res<Time>, cam: Single<&Transform, With<MainCamera>>, mut ex: ResMut<Exhaust>, mut meshes: ResMut<Assets<Mesh>>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let core = &session.core;
    let ex = &mut *ex;

    // the gas already in flight
    let slow = (-DRAG * dt).exp();
    for p in ex.puffs.iter_mut() {
        p.age += dt;
        p.pos += p.vel * dt;
        p.vel *= slow;
    }
    ex.puffs.retain(|p| p.age < p.life);

    // this frame's emitters
    let my_team = core.my().map(|m| m.team);
    let mut emissions = Vec::new();
    for (id, v) in &vis.0 {
        let Some(s) = core.ships.get(id) else { continue };
        let palette = if Some(*id) == core.my_ship || my_team == Some(s.team) { FRIENDLY } else { HOSTILE };
        let boost = if s.boosting { 1.0 } else { 0.0 };
        let dir = s.rot * Vec3::Z;
        // k scales a nozzle's plume: 1 for an engine, smaller for a thruster
        for &(at, k) in &v.engines {
            emissions.push(Emission {
                key: Emitter::Engine(v.root, (at * 100.0).round().as_ivec3()),
                now: Nozzle { pos: s.pos + s.rot * at, dir, vel: s.vel, owed: 0.0 },
                palette,
                rate: (70.0 + 230.0 * s.throttle) * (1.0 + 0.3 * boost) * k,
                exhaust_speed: (40.0 + 40.0 * s.throttle) * (1.0 + 0.4 * boost),
                carry: CARRY,
                life: (0.45 + 0.25 * s.throttle + 0.15 * boost) * (0.5 + 0.5 * k),
                size: (0.2 * (1.0 + 0.15 * boost) * k, 1.05 * k),
                strength: (0.3 + 0.7 * s.throttle) * (1.0 + 0.4 * boost) * k.sqrt(),
                throat: 0.32 * (0.7 + 0.3 * s.throttle) * (1.0 + 0.25 * boost) * k,
            });
        }
    }
    for (id, m) in &core.missiles {
        emissions.push(Emission {
            key: Emitter::Missile(*id),
            now: Nozzle { pos: core.missile_pos(m), dir: -m.vel.normalize_or(Vec3::Z), vel: m.vel, owed: 0.0 },
            palette: MISSILE,
            rate: 160.0,
            exhaust_speed: 25.0,
            carry: 0.35,
            life: 0.7,
            size: (0.18, 1.1),
            strength: 0.9,
            throat: 0.2,
        });
    }

    // emit along the path each nozzle swept since last frame
    let mut sprites: Vec<Sprite> = Vec::with_capacity(ex.puffs.len() + emissions.len() * 8);
    let mut live = HashMap::with_capacity(emissions.len());
    for e in &emissions {
        let mut prev = ex.emitters.get(&e.key).copied().unwrap_or(e.now);
        // a jump (respawn, correction snap) restarts the stream instead of streaking across the map
        if prev.pos.distance(e.now.pos) > 60.0 {
            prev = Nozzle { owed: prev.owed, ..e.now };
        }
        let want = prev.owed + e.rate * dt;
        let n = (want.floor() as usize).min(48);
        if ex.puffs.len() < MAX_PUFFS {
            for i in 0..n {
                // f: when in the frame this puff left (0 = last frame, 1 = now), evenly spaced
                let f = ((i as f32 + 1.0) / n as f32).clamp(0.0, 1.0);
                let at = prev.pos.lerp(e.now.pos, f);
                let dir = prev.dir.lerp(e.now.dir, f).normalize_or(e.now.dir);
                let base = prev.vel.lerp(e.now.vel, f);
                let vel = base * e.carry + dir * e.exhaust_speed + ex.rng.unit() * 2.5;
                let age = (1.0 - f) * dt;
                let life = e.life * ex.rng.range(0.85, 1.15);
                ex.puffs.push(Puff { pos: at + vel * age, vel, age, life, size: e.size, palette: e.palette, strength: e.strength, seed: ex.rng.f32() });
            }
        }
        live.insert(e.key, Nozzle { owed: want - n as f32, ..e.now });
        // the white-hot throat anchors the stream to the nozzle
        sprites.push((e.now.pos + e.now.dir * e.throat * 0.6, e.throat, e.palette.hot * 0.12 * e.strength, 0.0, 0.5));
    }
    ex.emitters = live;

    for p in &ex.puffs {
        let t = (p.age / p.life).min(1.0);
        let r = p.size.0 + (p.size.1 - p.size.0) * t.sqrt();
        sprites.push((p.pos, r, puff_color(p.palette, t, p.strength), t, p.seed));
    }
    if let Some(h) = &ex.mesh
        && let Some(mut mesh) = meshes.get_mut(h)
    {
        *mesh = quads(&sprites, cam.rotation);
    }
}
