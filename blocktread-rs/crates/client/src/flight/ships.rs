//! Turns the client core's ship state and events into entities, effects and sounds.

use super::fx::{self, Fx, Kind};
use super::hud::{HitKind, Hud};
use super::{Session, env, ground};
use crate::Screen;
use crate::art::{BlockArt, Finish, dmg_level};
use crate::audio::{Sfx, Sounds, falloff};
use bevy::prelude::*;
use std::collections::HashMap;
use blocktread_client_core::{CoreEvent, CoreShip};
use blocktread_shared::blocks::{BlockType, WeaponSfx};
use blocktread_shared::drive::{steer_angle, wheel_drop, wheel_ray};

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
    /// Each wheel's pivot (and its radius): it rides the suspension, steers and turns.
    wheels: Vec<(usize, Entity, f32)>,
    /// Each gun's pivot (it turns to where the gun points), and its mount: the face it sits on
    /// and the mount's entities. Those are also among the gun block's own entities, so whatever
    /// takes the gun away (shot off, broken off with a piece) takes its mount too.
    guns: Vec<(usize, Entity)>,
    mounts: HashMap<usize, (IVec3, Vec<Entity>)>,
    /// How far the wheels have rolled (rad).
    roll: f32,
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
    // thrusters' plumes (they only burn while boosting)
    let engines = s.body.stats.engines.iter().map(|&i| nozzle(s, i)).collect();
    let mut vis = ShipVis { root, blocks: vec![Vec::new(); n], paint: vec![None; n], levels: vec![3; n], engines, bubble: None, flash: 0.0, wheels: Vec::new(), guns: Vec::new(), mounts: HashMap::new(), roll: 0.0 };
    for i in 0..n {
        if !s.body.alive[i] {
            continue;
        }
        let b = s.body.design.blocks[i];
        let level = dmg_level(s.body.hp_frac(i));
        // wheels and guns hang on pivots that move them; hover pads stay put
        let wheel = b.ty.drive().filter(|d| !d.hover).map(|d| d.radius);
        let (ents, paint) = if wheel.is_some() || b.ty.weapon().is_some() {
            let pivot = commands.spawn((Transform::from_translation(s.body.local(i)), Visibility::default(), ChildOf(root))).id();
            let (mut ents, paint) = art.spawn_block(commands, mats, pivot, &b, Vec3::ZERO, level);
            match wheel {
                Some(r) => vis.wheels.push((i, pivot, r)),
                None => {
                    vis.guns.push((i, pivot));
                    // the mount stays put on the vehicle; only the head turns
                    if let Some(m) = s.body.mounts.iter().find(|m| m.block == i) {
                        let mount = art.spawn_mount(commands, mats, root, &b, s.body.local(i), level, m.face);
                        ents.extend(&mount);
                        vis.mounts.insert(i, (m.face, mount));
                    }
                }
            }
            (ents, paint)
        } else {
            art.spawn_block(commands, mats, root, &b, s.body.local(i), level)
        };
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
    old_ground: Query<Entity, With<ground::Ground>>,
) {
    let now = time.elapsed_secs_f64();
    let ear = cam.translation();
    let events: Vec<CoreEvent> = session.core.events.drain(..).collect();
    let me = session.core.my_ship;
    for ev in events {
        match ev {
            CoreEvent::Welcome => {
                ground::spawn_terrain(&mut commands, &mut meshes, &mut mats, &session.core.terrain, old_ground.iter());
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
                if killer.is_some() && killer == me && Some(id) != me {
                    hud.hit(id, HitKind::Kill);
                    sounds.play(&mut commands, now, Sfx::ConfirmKill, 1.0, 0.0);
                }
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
                    // being hit is heard up close, not from where the chase camera hangs
                    sounds.play(&mut commands, now, Sfx::Hit, 1.0, 0.06);
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
                    v.mounts.remove(&i);
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
                        hud.feed("结构断裂：一部分车体脱落了", true);
                    }
                }
                // a gun whose holding block went: its mount moves to the face that holds it now
                for m in &s.body.mounts {
                    if let Some((face, ents)) = v.mounts.get_mut(&m.block)
                        && *face != m.face
                    {
                        v.blocks[m.block].retain(|e| !ents.contains(e));
                        for e in ents.drain(..) {
                            commands.entity(e).despawn();
                        }
                        let b = s.body.design.blocks[m.block];
                        *ents = art.spawn_mount(&mut commands, &mut mats, v.root, &b, s.body.local(m.block), v.levels[m.block], m.face);
                        v.blocks[m.block].extend(ents.iter().copied());
                        *face = m.face;
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
            CoreEvent::HitConfirm { target, shield } => {
                hud.hit(target, if shield { HitKind::Shield } else { HitKind::Hull });
                let k = if shield { Sfx::ConfirmShield } else { Sfx::ConfirmHull };
                sounds.play(&mut commands, now, k, 1.0, 0.06);
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
        // wheels: hang each where its spring puts it, turn the front ones (only your own vehicle's
        // steering is known) and roll them all with the speed (`roll` is distance; each wheel
        // turns by it over its own radius)
        let fwd_speed = s.vel.dot(s.rot * Vec3::NEG_Z);
        v.roll += fwd_speed * dt;
        let steer = if Some(*id) == me { core.pred.as_ref().and_then(|p| p.last_cmd()).map_or(0.0, |c| steer_angle(c.steer_f(), fwd_speed)) } else { 0.0 };
        let up = s.rot * Vec3::Y;
        for &(i, pivot, r) in &v.wheels {
            let local = s.body.local(i);
            let dist = wheel_ray(&core.terrain, s.pos + s.rot * local, up).map(|(d, _)| d);
            if let Ok(mut t) = tf.get_mut(pivot) {
                t.translation = local - Vec3::Y * wheel_drop(dist, r);
                let turn = if local.z < -0.5 { Quat::from_rotation_y(-steer) } else { Quat::IDENTITY };
                t.rotation = turn * Quat::from_rotation_x(-v.roll / r);
            }
        }
        // guns: turned to where they point (your own as predicted, others' as their aim turns them)
        let aims = if Some(*id) == me { core.pred.as_ref().map(|p| &p.ws.aim) } else { Some(&s.ws.aim) };
        for &(i, pivot) in &v.guns {
            if let (Some(&(yaw, pitch)), Ok(mut t)) = (aims.and_then(|a| a.get(i)), tf.get_mut(pivot)) {
                t.rotation = Quat::from_rotation_y(-yaw) * Quat::from_rotation_x(pitch);
            }
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
