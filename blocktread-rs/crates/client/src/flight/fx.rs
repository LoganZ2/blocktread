//! Purely visual effects. Nothing here affects the simulation: bolts are cosmetic copies of the
//! server's projectiles, debris and wreckage only drift and fade. Fire and smoke are `fire`'s.

use super::Session;
use super::fire::{self, Fires};
use crate::Screen;
use crate::art::{BlockArt, glow, srgb};
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use std::collections::HashMap;
use blocktread_shared::blocks::BlockType;
use blocktread_shared::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Fire,
    Flash,
    Spark,
    Shield,
}

#[derive(Resource)]
pub struct Fx {
    pub ball: Handle<Mesh>,
    pub cube: Handle<Mesh>,
    mats: HashMap<Kind, Handle<StandardMaterial>>,
    bolt_mats: HashMap<BlockType, Handle<StandardMaterial>>,
    debris_mats: HashMap<u32, Handle<StandardMaterial>>,
    pub count: usize,
    pub rng: Rng,
}

pub const MAX_PARTICLES: usize = 2600;

impl Fx {
    pub fn new(meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>) -> Self {
        let add = |m: &mut Assets<StandardMaterial>, c: LinearRgba| m.add(StandardMaterial { base_color: Color::LinearRgba(c), alpha_mode: AlphaMode::Add, unlit: true, ..default() });
        let mut kinds = HashMap::new();
        kinds.insert(Kind::Fire, add(mats, LinearRgba::rgb(14.0, 5.0, 1.2)));
        kinds.insert(Kind::Flash, add(mats, LinearRgba::rgb(30.0, 22.0, 14.0)));
        kinds.insert(Kind::Spark, add(mats, LinearRgba::rgb(18.0, 12.0, 5.0)));
        kinds.insert(Kind::Shield, add(mats, LinearRgba::rgb(2.0, 6.0, 16.0)));
        let mut bolt_mats = HashMap::new();
        for t in BlockType::ALL.into_iter().filter(|t| t.weapon().is_some()) {
            let w = t.weapon().unwrap();
            let k = if t == BlockType::Missile { 6.0 } else { 40.0 };
            bolt_mats.insert(t, mats.add(StandardMaterial { base_color: Color::LinearRgba(glow(w.color, k)), unlit: true, ..default() }));
        }
        Fx {
            ball: meshes.add(Sphere::new(0.5).mesh().ico(1).unwrap()),
            cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
            mats: kinds,
            bolt_mats,
            debris_mats: HashMap::new(),
            count: 0,
            rng: Rng::new(42),
        }
    }
}

#[derive(Component)]
pub struct Particle {
    vel: Vec3,
    life: f32,
    max: f32,
    s0: f32,
    s1: f32,
    drag: f32,
}

#[derive(Component)]
pub struct Bolt {
    pub vel: Vec3,
    pub life: f32,
    pub owner: u16,
    pub splash: bool,
}

#[derive(Component)]
pub struct Debris {
    vel: Vec3,
    axis: Dir3,
    spin: f32,
    life: f32,
    size: f32,
}

#[derive(Component)]
pub struct Wreck {
    vel: Vec3,
    axis: Dir3,
    spin: f32,
    life: f32,
    blocks: Vec<(Vec3, u32)>,
}

#[allow(clippy::too_many_arguments)]
pub fn particle(commands: &mut Commands, fx: &mut Fx, pos: Vec3, vel: Vec3, life: f32, s0: f32, s1: f32, kind: Kind, drag: f32) {
    if fx.count >= MAX_PARTICLES {
        return;
    }
    fx.count += 1;
    commands.spawn((
        Mesh3d(fx.ball.clone()),
        MeshMaterial3d(fx.mats[&kind].clone()),
        Transform::from_translation(pos).with_scale(Vec3::splat(s0)),
        Particle { vel, life, max: life, s0, s1, drag },
        NotShadowCaster,
        DespawnOnExit(Screen::Flight),
    ));
}

pub fn sparks(commands: &mut Commands, fx: &mut Fx, pos: Vec3, base: Vec3, n: usize, speed: f32, kind: Kind) {
    for _ in 0..n {
        let v = fx.rng.unit() * speed * fx.rng.range(0.3, 1.0) + base;
        let life = fx.rng.range(0.15, 0.4);
        particle(commands, fx, pos, v, life, 0.45, 0.05, kind, 3.0);
    }
}

/// A blast: its fireball rolling up into smoke, and sparks flying out of it.
pub fn explode(commands: &mut Commands, fx: &mut Fx, fires: &mut Fires, pos: Vec3, size: f32, base: Vec3) {
    fire::blast(commands, fires, pos, size, base);
    sparks(commands, fx, pos, base, (8.0 + size * 6.0) as usize, 45.0 * size.sqrt(), Kind::Spark);
}

pub fn debris(commands: &mut Commands, fx: &mut Fx, mats: &mut Assets<StandardMaterial>, pos: Vec3, vel: Vec3, color: u32) {
    let mat = fx.debris_mats.entry(color).or_insert_with(|| mats.add(StandardMaterial { base_color: srgb(color), metallic: 0.4, perceptual_roughness: 0.6, ..default() })).clone();
    let size = fx.rng.range(0.5, 0.85);
    let axis = Dir3::new(fx.rng.unit()).unwrap_or(Dir3::Y);
    commands.spawn((
        Mesh3d(fx.cube.clone()),
        MeshMaterial3d(mat),
        Transform::from_translation(pos).with_scale(Vec3::splat(size)),
        Debris { vel, axis, spin: fx.rng.range(2.0, 9.0), life: fx.rng.range(1.8, 3.4), size },
        DespawnOnExit(Screen::Flight),
    ));
}

/// A piece that broke off: its blocks as plain cubes, drifting away from the ship.
pub fn wreck(commands: &mut Commands, fx: &mut Fx, art: &mut BlockArt, mats: &mut Assets<StandardMaterial>, blocks: Vec<(Vec3, u32)>, rot: Quat, vel: Vec3, away: Vec3) {
    if blocks.is_empty() {
        return;
    }
    let centre = blocks.iter().map(|b| b.0).sum::<Vec3>() / blocks.len() as f32;
    let axis = Dir3::new(fx.rng.unit()).unwrap_or(Dir3::Y);
    let v = vel + away.normalize_or(Vec3::Y) * fx.rng.range(5.0, 11.0) + fx.rng.unit() * 2.0;
    let root = commands
        .spawn((
            Transform::from_translation(centre).with_rotation(rot),
            Visibility::default(),
            Wreck { vel: v, axis, spin: fx.rng.range(0.6, 2.4), life: fx.rng.range(2.5, 5.0), blocks: blocks.clone() },
            DespawnOnExit(Screen::Flight),
        ))
        .id();
    let inv = rot.inverse();
    for (p, c) in blocks {
        let m = art.paint(mats, c, crate::art::Finish::Paint, 1);
        commands.spawn((Mesh3d(fx.cube.clone()), MeshMaterial3d(m), Transform::from_translation(inv * (p - centre)), ChildOf(root)));
    }
}

pub fn bolt(commands: &mut Commands, fx: &Fx, weapon: BlockType, pos: Vec3, vel: Vec3, owner: u16, rel: Vec3) {
    let w = weapon.weapon().unwrap();
    let dir = rel.normalize_or(vel.normalize_or(Vec3::NEG_Z));
    commands.spawn((
        Mesh3d(fx.cube.clone()),
        MeshMaterial3d(fx.bolt_mats[&weapon].clone()),
        Transform::from_translation(pos).looking_to(dir, Vec3::Y).with_scale(Vec3::new(w.width, w.width, w.len)),
        Bolt { vel, life: w.life, owner, splash: w.splash > 0.0 },
        DespawnOnExit(Screen::Flight),
    ));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update_fx(
    mut commands: Commands,
    time: Res<Time>,
    mut fx: ResMut<Fx>,
    mut fires: ResMut<Fires>,
    session: Option<Res<Session>>,
    mut q: ParamSet<(
        Query<(Entity, &mut Transform, &mut Particle)>,
        Query<(Entity, &mut Transform, &mut Bolt)>,
        Query<(Entity, &mut Transform, &mut Debris)>,
        Query<(Entity, &mut Transform, &mut Wreck)>,
    )>,
) {
    let dt = time.delta_secs();
    if session.as_ref().is_some_and(|s| s.paused) {
        return;
    }
    let mut alive = 0;
    for (e, mut t, mut p) in q.p0().iter_mut() {
        p.life -= dt;
        if p.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        alive += 1;
        let k = (-p.drag * dt).exp();
        p.vel *= k;
        t.translation += p.vel * dt;
        let k = 1.0 - p.life / p.max;
        t.scale = Vec3::splat((p.s0 + (p.s1 - p.s0) * k) * (1.0 - k).sqrt());
    }
    fx.count = alive;

    let mut hits: Vec<(Vec3, bool)> = Vec::new();
    if let Some(s) = session.as_ref() {
        let core = &s.core;
        let rock_t = core.rock_time();
        for (e, mut t, mut b) in q.p1().iter_mut() {
            b.life -= dt;
            let a = t.translation;
            let nb = a + b.vel * dt;
            let mut dead = b.life <= 0.0;
            if !dead {
                for ship in core.ships.values() {
                    if ship.id == b.owner || ship.pos.distance(a) > ship.body.radius + b.vel.length() * dt + 2.0 {
                        continue;
                    }
                    if let Some((_, p)) = ship.body.ray(ship.pos, ship.rot, a, nb) {
                        hits.push((p, b.splash));
                        dead = true;
                        break;
                    }
                }
            }
            if !dead && let Some(f) = core.terrain.segment_hit(a, nb) {
                // the ground, a wall, a crate
                hits.push((a.lerp(nb, f), b.splash));
                dead = true;
            }
            if !dead {
                for r in &core.rocks {
                    if r.hit_segment(a, nb, r.rotation(rock_t)).is_some() {
                        hits.push((nb, b.splash));
                        dead = true;
                        break;
                    }
                }
            }
            if dead {
                commands.entity(e).despawn();
            } else {
                t.translation = nb;
            }
        }
    }
    for (p, splash) in hits {
        if splash {
            explode(&mut commands, &mut fx, &mut fires, p, 0.7, Vec3::ZERO);
        } else {
            sparks(&mut commands, &mut fx, p, Vec3::ZERO, 4, 16.0, Kind::Spark);
        }
    }

    for (e, mut t, mut d) in q.p2().iter_mut() {
        d.life -= dt;
        if d.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        // pieces fall, and bounce off the ground losing most of their speed
        d.vel.y -= blocktread_shared::drive::GRAVITY * dt;
        t.translation += d.vel * dt;
        if let Some(s) = session.as_ref() {
            let g = s.core.terrain.height(t.translation.x, t.translation.z) + d.size * 0.5;
            if t.translation.y < g {
                t.translation.y = g;
                d.vel = Vec3::new(d.vel.x * 0.5, d.vel.y.abs() * 0.3, d.vel.z * 0.5);
                d.spin *= 0.5;
            }
        }
        t.rotate(Quat::from_axis_angle(d.axis.as_vec3(), d.spin * dt));
        t.scale = Vec3::splat(d.size * (d.life / 0.6).min(1.0));
    }

    let mut blasts = Vec::new();
    let mut puffs = Vec::new();
    for (e, mut t, mut w) in q.p3().iter_mut() {
        w.life -= dt;
        // a falling piece blows up where it hits the ground
        w.vel.y -= blocktread_shared::drive::GRAVITY * dt;
        t.translation += w.vel * dt;
        if session.as_ref().is_some_and(|s| t.translation.y < s.core.terrain.height(t.translation.x, t.translation.z) + 0.5) {
            w.life = 0.0;
        }
        t.rotate(Quat::from_axis_angle(w.axis.as_vec3(), w.spin * dt));
        puffs.push((t.translation, w.vel));
        if w.life <= 0.0 {
            let (pos, rot) = (t.translation, t.rotation);
            let centre = w.blocks.iter().map(|b| b.0).sum::<Vec3>() / w.blocks.len().max(1) as f32;
            let blocks: Vec<(Vec3, u32)> = w.blocks.iter().take(30).map(|(p, c)| (pos + rot * (*p - centre), *c)).collect();
            blasts.push((pos, w.vel, w.blocks.len(), blocks));
            commands.entity(e).despawn();
        }
    }
    // a falling piece trails burning smoke
    for (p, v) in puffs {
        if fx.rng.f32() < dt * 10.0 {
            let life = fx.rng.range(0.8, 1.2);
            fire::fireball(&mut fires, p, v * 0.2, fx.rng.range(0.8, 1.2), 0.55, life);
        }
    }
    for (pos, vel, n, blocks) in blasts {
        explode(&mut commands, &mut fx, &mut fires, pos, (0.6 + n as f32 / 12.0).min(2.0), vel);
        let _ = blocks;
    }
}
