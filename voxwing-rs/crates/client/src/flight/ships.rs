//! Turns the client core's ship state and events into entities, effects and sounds.

use super::fx::{self, Fx, Kind};
use super::hud::Hud;
use super::{Session, env};
use crate::Screen;
use crate::art::{BlockArt, Finish, dmg_level};
use crate::audio::{Sfx, Sounds, falloff};
use bevy::prelude::*;
use std::collections::HashMap;
use voxwing_client_core::{CoreEvent, CoreShip};
use voxwing_shared::blocks::{BlockType, WeaponSfx};

pub struct ShipVis {
    pub root: Entity,
    blocks: Vec<Vec<Entity>>,
    paint: Vec<Option<Entity>>,
    levels: Vec<u8>,
    /// Ship-local nozzle positions of the surviving engines and thrusters, with each nozzle's size
    /// (the exhaust system emits from them).
    pub engines: Vec<(Vec3, f32)>,
    bubble: Option<(Entity, Handle<StandardMaterial>)>,
    flash: f32,
}

#[derive(Resource, Default)]
pub struct ShipVisuals(pub HashMap<u16, ShipVis>);

#[derive(Resource)]
pub struct FlameMats {
    pub missile: Handle<StandardMaterial>,
}

#[derive(Component)]
pub struct MissileVis(pub u16);

#[allow(clippy::too_many_arguments)]
fn spawn_vis(commands: &mut Commands, art: &mut BlockArt, mats: &mut Assets<StandardMaterial>, s: &CoreShip) -> ShipVis {
    let root = commands.spawn((Transform::from_translation(s.pos).with_rotation(s.rot), Visibility::default(), DespawnOnExit(Screen::Flight))).id();
    let n = s.body.design.blocks.len();
    let engines = s.body.stats.engines.iter().map(|&i| nozzle(s, i)).collect();
    let mut vis = ShipVis { root, blocks: vec![Vec::new(); n], paint: vec![None; n], levels: vec![3; n], engines, bubble: None, flash: 0.0 };
    for i in 0..n {
        if !s.body.alive[i] {
            continue;
        }
        let b = s.body.design.blocks[i];
        let level = dmg_level(s.body.hp_frac(i));
        let (ents, paint) = art.spawn_block(commands, mats, root, &b, s.body.local(i), level);
        vis.blocks[i] = ents;
        vis.paint[i] = paint;
        vis.levels[i] = level;
    }
    if s.body.stats.shield_cap > 0.0 {
        let m = mats.add(StandardMaterial { base_color: Color::LinearRgba(LinearRgba::rgb(0.0, 0.0, 0.0)), alpha_mode: AlphaMode::Add, unlit: true, ..default() });
        let e = commands.spawn((Mesh3d(art.sphere.clone()), MeshMaterial3d(m.clone()), Transform::from_scale(Vec3::splat(s.body.radius * 1.08)), ChildOf(root))).id();
        vis.bubble = Some((e, m));
    }
    vis
}

/// Where an engine's exhaust leaves it, and how big its plume is.
fn nozzle(s: &CoreShip, i: usize) -> (Vec3, f32) {
    if s.body.design.blocks[i].ty == BlockType::Thruster { (s.body.local(i) + Vec3::Z * 0.49, 0.55) } else { (s.body.local(i) + Vec3::Z * 0.58, 1.0) }
}

fn weapon_sfx(t: BlockType) -> Sfx {
    match t.weapon().map(|w| w.sfx) {
        Some(WeaponSfx::Laser) => Sfx::Laser,
        Some(WeaponSfx::Cannon) => Sfx::Cannon,
        Some(WeaponSfx::Plasma) => Sfx::Plasma,
        Some(WeaponSfx::Rail) => Sfx::Rail,
        Some(WeaponSfx::Flak) => Sfx::Flak,
        _ => Sfx::Missile,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn apply_events(
    mut commands: Commands,
    mut session: ResMut<Session>,
    mut vis: ResMut<ShipVisuals>,
    mut art: ResMut<BlockArt>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut fx: ResMut<Fx>,
    mut sounds: ResMut<Sounds>,
    mut hud: ResMut<Hud>,
    time: Res<Time>,
    cam: Single<&GlobalTransform, With<crate::MainCamera>>,
) {
    let now = time.elapsed_secs_f64();
    let ear = cam.translation();
    let events: Vec<CoreEvent> = session.core.events.drain(..).collect();
    let me = session.core.my_ship;
    for ev in events {
        match ev {
            CoreEvent::Welcome => {
                let rocks = session.core.rocks.clone();
                env::spawn_rocks(&mut commands, &mut meshes, &mut mats, &rocks);
            }
            CoreEvent::ShipSpawned(id) | CoreEvent::ShipRepaired(id) => {
                if let Some(old) = vis.0.remove(&id) {
                    commands.entity(old.root).despawn();
                }
                if let Some(s) = session.core.ships.get(&id) {
                    let v = spawn_vis(&mut commands, &mut art, &mut mats, s);
                    vis.0.insert(id, v);
                }
                hud.silo_dirty = true;
            }
            CoreEvent::ShipRemoved(id) => {
                if let Some(v) = vis.0.remove(&id) {
                    commands.entity(v.root).despawn();
                }
            }
            CoreEvent::ShipKilled { id, killer, pos, vel, blocks } => {
                if Some(id) == me {
                    session.killer = killer.and_then(|k| session.core.ships.get(&k)).map(|s| s.name.clone());
                }
                if session.core.watching && session.spectate == Some(id) && session.wreck.is_none() {
                    session.wreck = Some((pos, super::WRECK_LINGER, killer));
                }
                if let Some(v) = vis.0.remove(&id) {
                    commands.entity(v.root).despawn();
                }
                let size = (blocks.len() as f32 / 25.0).clamp(1.2, 3.5);
                fx::explode(&mut commands, &mut fx, pos, size, vel * 0.5);
                for (p, c) in blocks.iter().take(160) {
                    let v = (*p - pos).normalize_or(Vec3::Y) * fx.rng.range(6.0, 22.0) + vel * 0.6 + fx.rng.unit() * 4.0;
                    fx::debris(&mut commands, &mut fx, &mut mats, *p, v, *c);
                }
                sounds.play(&mut commands, now, Sfx::Boom, (falloff(ear, pos) * 2.0).min(1.0), 0.0);
                if Some(id) == me {
                    session.death = Some((pos, 0.0));
                    hud.dmg_flash = 1.0;
                }
            }
            CoreEvent::BlocksDamaged { ship, blocks } => {
                let Some(s) = session.core.ships.get(&ship) else { continue };
                let Some(v) = vis.0.get_mut(&ship) else { continue };
                for i in blocks {
                    let lvl = dmg_level(s.body.hp_frac(i));
                    if lvl != v.levels[i]
                        && let Some(e) = v.paint[i]
                    {
                        let b = s.body.design.blocks[i];
                        let m = art.paint(&mut mats, b.color, Finish::of(b.ty), lvl);
                        commands.entity(e).insert(MeshMaterial3d(m));
                        v.levels[i] = lvl;
                    }
                    let p = s.body.world(s.pos, s.rot, i);
                    fx::sparks(&mut commands, &mut fx, p, s.vel, 5, 18.0, Kind::Spark);
                }
                if Some(ship) == me {
                    hud.dmg_flash = (hud.dmg_flash + 0.25).min(1.0);
                    session.shake = (session.shake + 0.3).min(1.2);
                    hud.silo_dirty = true;
                } else {
                    sounds.play(&mut commands, now, Sfx::Hit, falloff(ear, s.pos) * 0.8, 0.04);
                }
            }
            CoreEvent::BlocksDestroyed { ship, blocks, pieces } => {
                let Some(s) = session.core.ships.get(&ship) else { continue };
                let Some(v) = vis.0.get_mut(&ship) else { continue };
                for &i in &blocks {
                    for e in v.blocks[i].drain(..) {
                        commands.entity(e).despawn();
                    }
                    v.paint[i] = None;
                    let p = s.body.world(s.pos, s.rot, i);
                    let dv = fx.rng.unit() * fx.rng.range(4.0, 12.0) + s.vel;
                    fx::debris(&mut commands, &mut fx, &mut mats, p, dv, s.body.design.blocks[i].color);
                    fx::sparks(&mut commands, &mut fx, p, s.vel, 6, 16.0, Kind::Fire);
                    fx::particle(&mut commands, &mut fx, p, s.vel, 0.35, 1.0, 2.2, Kind::Fire, 0.0);
                }
                for piece in pieces {
                    let mut cubes = Vec::new();
                    for &i in &piece {
                        for e in v.blocks[i].drain(..) {
                            commands.entity(e).despawn();
                        }
                        v.paint[i] = None;
                        cubes.push((s.body.world(s.pos, s.rot, i), s.body.design.blocks[i].color));
                    }
                    let c = cubes.iter().map(|c| c.0).sum::<Vec3>() / cubes.len().max(1) as f32;
                    fx::wreck(&mut commands, &mut fx, &mut art, &mut mats, cubes, s.rot, s.vel, c - s.pos);
                    if Some(ship) == me {
                        hud.feed("结构断裂：一部分机体脱落了", true);
                    }
                }
                // engines may have been lost: keep exhausting only from the survivors
                v.engines.retain(|(at, _)| s.body.stats.engines.iter().any(|&i| nozzle(s, i).0.distance(*at) < 0.01));
                sounds.play(&mut commands, now, Sfx::Pop, falloff(ear, s.pos), 0.03);
                if Some(ship) == me {
                    hud.silo_dirty = true;
                    hud.dmg_flash = (hud.dmg_flash + 0.35).min(1.0);
                    session.shake = (session.shake + 0.5).min(1.2);
                }
            }
            CoreEvent::ShieldHit { ship, pos } => {
                if let Some(v) = vis.0.get_mut(&ship) {
                    v.flash = 1.0;
                }
                let base = session.core.ships.get(&ship).map_or(Vec3::ZERO, |s| s.vel);
                fx::sparks(&mut commands, &mut fx, pos, base, 6, 18.0, Kind::Shield);
                sounds.play(&mut commands, now, Sfx::Shield, falloff(ear, pos), 0.05);
                if Some(ship) == me {
                    hud.shield_flash = (hud.shield_flash + 0.35).min(1.0);
                }
            }
            CoreEvent::Explosion { pos, size } => {
                fx::explode(&mut commands, &mut fx, pos, size, Vec3::ZERO);
                sounds.play(&mut commands, now, Sfx::Pop, falloff(ear, pos), 0.03);
            }
            CoreEvent::Crash { pos } => {
                fx::sparks(&mut commands, &mut fx, pos, Vec3::ZERO, 14, 25.0, Kind::Spark);
                sounds.play(&mut commands, now, Sfx::Crash, falloff(ear, pos), 0.1);
            }
            CoreEvent::Shot { ship, weapon, pos, vel, own } => {
                let (svel, mine) = session.core.ships.get(&ship).map_or((Vec3::ZERO, false), |s| (s.vel, Some(s.id) == me));
                fx::bolt(&mut commands, &fx, weapon, pos, vel, ship, vel - svel);
                fx::particle(&mut commands, &mut fx, pos, svel, 0.06, 0.9, 0.3, Kind::Flash, 0.0);
                let vol = if own || mine { 0.55 } else { falloff(ear, pos) * 0.5 };
                let gap = if own { 0.045 } else { 0.09 };
                sounds.play(&mut commands, now, weapon_sfx(weapon), vol, gap);
            }
            CoreEvent::Announce { title, sub } => {
                info!("announce: {title} · {sub}");
                hud.announce(&title, &sub);
                sounds.play(&mut commands, now, Sfx::Wave, 0.5, 0.5);
            }
            CoreEvent::Feed { text, bad } => {
                info!("feed: {text}");
                hud.feed(&text, bad);
            }
            CoreEvent::YourShip(id) => {
                if id.is_some() {
                    session.death = None;
                    session.cam_init = false;
                    hud.silo_dirty = true;
                }
            }
        }
    }
}

pub fn sync_ships(
    mut commands: Commands,
    session: Res<Session>,
    mut vis: ResMut<ShipVisuals>,
    mut tf: Query<&mut Transform>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut fx: ResMut<Fx>,
    flames: Res<FlameMats>,
    missiles: Query<(Entity, &MissileVis)>,
    art: Res<BlockArt>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    let core = &session.core;
    let me = core.my_ship;
    for (id, v) in vis.0.iter_mut() {
        let Some(s) = core.ships.get(id) else { continue };
        if let Ok(mut t) = tf.get_mut(v.root) {
            t.translation = s.pos;
            t.rotation = s.rot;
        }
        if let Some((_, m)) = &v.bubble {
            v.flash *= (-dt * 4.0).exp();
            let a = if s.shield_frac > 0.01 { v.flash * 0.9 + if Some(*id) == me { 0.0 } else { 0.03 } } else { 0.0 };
            if let Some(mut mat) = mats.get_mut(m) {
                mat.base_color = Color::LinearRgba(LinearRgba::rgb(0.6, 1.4, 3.2) * a);
            }
        }
    }
    // missiles: one entity per server missile, drawn at the interpolated time
    let mut have: HashMap<u16, Entity> = missiles.iter().map(|(e, m)| (m.0, e)).collect();
    for (id, m) in &core.missiles {
        let p = core.missile_pos(m);
        let rot = Transform::IDENTITY.looking_to(m.vel.normalize_or(Vec3::NEG_Z), Vec3::Y).rotation;
        match have.remove(id) {
            Some(e) => {
                if let Ok(mut t) = tf.get_mut(e) {
                    t.translation = p;
                    t.rotation = rot;
                }
            }
            None => {
                commands.spawn((
                    Mesh3d(art.cube.clone()),
                    MeshMaterial3d(flames.missile.clone()),
                    Transform::from_translation(p).with_rotation(rot).with_scale(Vec3::new(0.36, 0.36, 1.6)),
                    MissileVis(*id),
                    DespawnOnExit(Screen::Flight),
                ));
            }
        }
        // a small flare at the motor; its exhaust comes from the exhaust system
        fx::particle(&mut commands, &mut fx, p, -m.vel * 0.1, 0.08, 0.45, 0.2, Kind::Fire, 0.0);
    }
    for (_, e) in have {
        commands.entity(e).despawn();
    }
}

pub fn flame_mats(mats: &mut Assets<StandardMaterial>) -> FlameMats {
    FlameMats { missile: mats.add(StandardMaterial { base_color: Color::LinearRgba(LinearRgba::rgb(8.0, 7.5, 7.0)), unlit: true, ..default() }) }
}
