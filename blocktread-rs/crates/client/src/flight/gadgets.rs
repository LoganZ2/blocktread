//! What the gadgets leave in the world: mines on the ground, their lamps blinking once they are
//! live (red for the other side's, blue for our own), and clouds of smoke that billow up, hang,
//! and thin out before they clear.

use super::Session;
use crate::Screen;
use crate::art::{glow, srgb};
use bevy::prelude::*;
use blocktread_shared::TICK_RATE;
use blocktread_shared::blocks::BlockType;
use blocktread_shared::gadgets::Cloud;
use blocktread_shared::rng::Rng;

#[derive(Resource)]
pub struct GadgetArt {
    puck: Handle<Mesh>,
    rim: Handle<Mesh>,
    lamp: Handle<Mesh>,
    puff: Handle<Mesh>,
    metal: Handle<StandardMaterial>,
    hazard: Handle<StandardMaterial>,
}

impl GadgetArt {
    pub fn new(meshes: &mut Assets<Mesh>, mats: &mut Assets<StandardMaterial>) -> Self {
        GadgetArt {
            puck: meshes.add(Cylinder::new(0.55, 0.18)),
            rim: meshes.add(Torus { minor_radius: 0.05, major_radius: 0.5 }),
            lamp: meshes.add(Sphere::new(0.13).mesh().ico(2).unwrap()),
            puff: meshes.add(Sphere::new(1.0).mesh().ico(4).unwrap()),
            metal: mats.add(StandardMaterial { base_color: srgb(0x2a2d31), metallic: 0.7, perceptual_roughness: 0.4, ..default() }),
            hazard: mats.add(StandardMaterial { base_color: srgb(0xe0b020), metallic: 0.2, perceptual_roughness: 0.5, ..default() }),
        }
    }
}

/// A mine on the ground: its id, and its lamp (lit as it blinks).
#[derive(Component)]
pub struct MineVis {
    id: u16,
    lamp: Handle<StandardMaterial>,
}

/// A cloud of smoke: the cloud it shows, and its two shades of grey (they fade together).
#[derive(Component)]
pub struct SmokeVis {
    cloud: Cloud,
    shades: [Handle<StandardMaterial>; 2],
}

/// One puff of a cloud: where it sits in the cloud once it has billowed out, which way it drifts
/// as it hangs, and how big it is.
#[derive(Component)]
pub struct Puff {
    at: Vec3,
    drift: Vec3,
    size: f32,
}

/// Lamp colours: our side's mines, and the others'.
const OURS: u32 = 0x4aa8ff;
const THEIRS: u32 = 0xff3b30;
/// How long a cloud takes to billow out, and to thin away at the end (s).
const BILLOW: f32 = 0.8;
const THIN: f32 = 1.5;

/// Keep a mine on the ground for every mine down, and blink the live ones' lamps.
pub fn sync_mines(mut commands: Commands, session: Res<Session>, art: Res<GadgetArt>, shown: Query<(Entity, &MineVis)>, mut mats: ResMut<Assets<StandardMaterial>>, time: Res<Time>) {
    let core = &session.core;
    let ours = session.view().map(|v| v.team);
    for (e, m) in &shown {
        if !core.mines.contains_key(&m.id) {
            commands.entity(e).despawn();
        }
    }
    let blink = (time.elapsed_secs() * 1.6).fract() < 0.22;
    for (&id, m) in &core.mines {
        let col = if ours == Some(m.team) { OURS } else { THEIRS };
        let live = core.clock.latest >= m.armed_at;
        let want = glow(col, if !live { 0.6 } else if blink { 14.0 } else { 0.4 });
        if let Some((_, v)) = shown.iter().find(|(_, v)| v.id == id) {
            if mats.get(&v.lamp).is_some_and(|l| l.emissive != want)
                && let Some(mut l) = mats.get_mut(&v.lamp)
            {
                l.emissive = want;
            }
            continue;
        }
        let lamp = mats.add(StandardMaterial { base_color: Color::BLACK, emissive: want, ..default() });
        commands.spawn((
            Transform::from_translation(m.pos),
            Visibility::default(),
            MineVis { id, lamp: lamp.clone() },
            DespawnOnExit(Screen::Flight),
            children![
                (Mesh3d(art.puck.clone()), MeshMaterial3d(art.metal.clone()), Transform::default()),
                (Mesh3d(art.rim.clone()), MeshMaterial3d(art.hazard.clone()), Transform::from_xyz(0.0, 0.08, 0.0)),
                (Mesh3d(art.lamp.clone()), MeshMaterial3d(lamp), Transform::from_xyz(0.0, 0.1, 0.0)),
            ],
        ));
    }
}

/// Keep a cloud of puffs for every cloud of smoke hanging: they billow out from where it went up,
/// drift as it hangs, and thin out over its last moments.
pub fn sync_smoke(
    mut commands: Commands,
    session: Res<Session>,
    art: Res<GadgetArt>,
    shown: Query<(Entity, &SmokeVis, &Children)>,
    mut puffs: Query<(&Puff, &mut Transform)>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    let core = &session.core;
    // where the world is drawn, in server seconds
    let now = core.render_tick() as f32 / TICK_RATE as f32;
    for (e, v, kids) in &shown {
        if !core.clouds.contains(&v.cloud) {
            commands.entity(e).despawn();
            continue;
        }
        let left = v.cloud.until as f32 / TICK_RATE as f32 - now;
        let total = smoke_secs();
        let age = (total - left).max(0.0);
        let out = 1.0 - (1.0 - (age / BILLOW).min(1.0)).powi(3);
        for &k in kids {
            if let Ok((p, mut tf)) = puffs.get_mut(k) {
                tf.translation = p.at * (0.25 + 0.75 * out) + p.drift * age;
                tf.scale = Vec3::splat(p.size * (0.35 + 0.65 * out) * (1.0 + age * 0.02));
            }
        }
        let alpha = (left / THIN).clamp(0.0, 1.0);
        for h in &v.shades {
            if mats.get(h).is_some_and(|m| (m.base_color.alpha() - alpha).abs() > 0.01)
                && let Some(mut m) = mats.get_mut(h)
            {
                m.base_color.set_alpha(alpha);
                m.alpha_mode = if alpha < 1.0 { AlphaMode::Blend } else { AlphaMode::Opaque };
            }
        }
    }
    for c in &core.clouds {
        if shown.iter().any(|(_, v, _)| v.cloud == *c) {
            continue;
        }
        let shade = |c: u32, mats: &mut Assets<StandardMaterial>| mats.add(StandardMaterial { base_color: srgb(c), perceptual_roughness: 1.0, reflectance: 0.05, ..default() });
        let shades = [shade(0xc4c7c9, &mut mats), shade(0x969a9e, &mut mats)];
        let root = commands.spawn((Transform::from_translation(c.pos), Visibility::default(), SmokeVis { cloud: *c, shades: shades.clone() }, DespawnOnExit(Screen::Flight))).id();
        let mut rng = Rng::new((c.pos.x.to_bits() as u64) << 32 ^ c.pos.z.to_bits() as u64 ^ c.until as u64);
        let r = c.radius;
        for i in 0..26 {
            // a heap: wide at the bottom, piled up over where it went up
            let d = rng.unit();
            let at = Vec3::new(d.x * r * 0.62, d.y.abs() * r * 0.45 - 0.5, d.z * r * 0.62) * rng.range(0.4, 1.0);
            let drift = Vec3::new(d.x * 0.25, rng.range(0.05, 0.3), d.z * 0.25);
            let size = rng.range(0.32, 0.5) * r;
            commands.spawn((Mesh3d(art.puff.clone()), MeshMaterial3d(shades[i % 2].clone()), Transform::from_translation(at * 0.25).with_scale(Vec3::splat(size * 0.35)), Puff { at, drift, size }, ChildOf(root)));
        }
    }
}

/// How long a cloud of smoke hangs (s).
fn smoke_secs() -> f32 {
    BlockType::Smoke.gadget().map_or(8.0, |g| g.secs)
}
