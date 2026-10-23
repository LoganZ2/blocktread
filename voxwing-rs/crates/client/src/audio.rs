//! Procedural sound effects: synthesised into in-memory WAV files at startup.

use bevy::audio::Volume;
use bevy::prelude::*;
use std::sync::Arc;
use voxwing_shared::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sfx {
    Laser,
    Cannon,
    Plasma,
    Missile,
    Rail,
    Flak,
    Hit,
    Shield,
    Pop,
    Boom,
    Crash,
    Lock,
    Warn,
    Place,
    Erase,
    Deny,
    Wave,
    UiHover,
    UiClick,
}

#[derive(Resource)]
pub struct Sounds {
    map: std::collections::HashMap<Sfx, Handle<AudioSource>>,
    pub hum: Handle<AudioSource>,
    pub enabled: bool,
    pub volume: f32,
    last: std::collections::HashMap<Sfx, f64>,
}

const SR: u32 = 22050;

fn wav(samples: &[f32]) -> Arc<[u8]> {
    let n = samples.len() as u32;
    let mut b = Vec::with_capacity(44 + n as usize * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n * 2).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&SR.to_le_bytes());
    b.extend_from_slice(&(SR * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(n * 2).to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32000.0) as i16).to_le_bytes());
    }
    b.into()
}

/// Exponential frequency sweep with an exponential decay envelope.
fn tone(dur: f32, f0: f32, f1: f32, vol: f32, shape: fn(f32) -> f32) -> Vec<f32> {
    let n = (dur * SR as f32) as usize;
    let mut ph = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let f = f0 * (f1 / f0).powf(t);
            ph += f / SR as f32;
            let env = (1.0 - t).powf(2.0) * (t * 200.0).min(1.0);
            shape(ph.fract()) * env * vol
        })
        .collect()
}

fn square(p: f32) -> f32 {
    if p < 0.5 { 1.0 } else { -1.0 }
}
fn sine(p: f32) -> f32 {
    (p * std::f32::consts::TAU).sin()
}
fn saw(p: f32) -> f32 {
    p * 2.0 - 1.0
}
fn tri(p: f32) -> f32 {
    1.0 - 4.0 * (p - 0.5).abs()
}

/// Filtered noise burst; `cutoff` sweeps from c0 to c1 (one-pole low-pass).
fn noise(dur: f32, c0: f32, c1: f32, vol: f32, seed: u64) -> Vec<f32> {
    let n = (dur * SR as f32) as usize;
    let mut rng = Rng::new(seed);
    let mut y = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let c = c0 * (c1 / c0).powf(t);
            let a = (std::f32::consts::TAU * c / SR as f32).min(1.0);
            y += (rng.f32() * 2.0 - 1.0 - y) * a;
            let env = (1.0 - t).powf(1.6) * (t * 300.0).min(1.0);
            y * env * vol * 2.0
        })
        .collect()
}

fn mix(a: Vec<f32>, b: Vec<f32>) -> Vec<f32> {
    let n = a.len().max(b.len());
    (0..n).map(|i| a.get(i).copied().unwrap_or(0.0) + b.get(i).copied().unwrap_or(0.0)).collect()
}

pub fn build_sounds(mut commands: Commands, mut assets: ResMut<Assets<AudioSource>>, settings: Res<crate::settings::Settings>) {
    let mut map = std::collections::HashMap::new();
    let mut add = |k: Sfx, s: Vec<f32>| {
        map.insert(k, assets.add(AudioSource { bytes: wav(&s) }));
    };
    add(Sfx::Laser, tone(0.11, 1400.0, 220.0, 0.22, square));
    add(Sfx::Cannon, mix(noise(0.07, 2400.0, 600.0, 0.5, 1), tone(0.06, 160.0, 60.0, 0.35, tri)));
    add(Sfx::Plasma, mix(tone(0.35, 520.0, 70.0, 0.25, saw), tone(0.3, 900.0, 120.0, 0.3, sine)));
    add(Sfx::Missile, noise(0.6, 300.0, 2200.0, 0.5, 2));
    // a sharp electric crack with a falling whine
    add(Sfx::Rail, mix(noise(0.12, 7000.0, 900.0, 0.45, 9), tone(0.32, 2600.0, 260.0, 0.18, saw)));
    // one heavy thump, wider and duller than the cannon
    add(Sfx::Flak, mix(noise(0.16, 1600.0, 250.0, 0.75, 10), tone(0.12, 120.0, 45.0, 0.45, tri)));
    add(Sfx::Hit, tone(0.05, 380.0, 140.0, 0.4, tri));
    add(Sfx::Shield, tone(0.18, 1500.0, 700.0, 0.3, sine));
    add(Sfx::Pop, noise(0.35, 1800.0, 200.0, 0.8, 3));
    add(Sfx::Boom, mix(noise(1.4, 1200.0, 60.0, 1.0, 4), tone(1.0, 90.0, 25.0, 0.8, sine)));
    add(Sfx::Crash, noise(0.3, 900.0, 120.0, 0.9, 5));
    add(Sfx::Lock, mix(tone(0.07, 1760.0, 1760.0, 0.15, square), tone(0.16, 2200.0, 2200.0, 0.1, square)));
    add(Sfx::Warn, tone(0.12, 880.0, 880.0, 0.14, square));
    add(Sfx::Place, tone(0.07, 420.0, 640.0, 0.3, tri));
    add(Sfx::Erase, tone(0.08, 500.0, 220.0, 0.3, tri));
    add(Sfx::Deny, tone(0.12, 180.0, 140.0, 0.15, square));
    add(Sfx::Wave, mix(tone(0.5, 220.0, 440.0, 0.18, saw), tone(0.5, 330.0, 660.0, 0.14, saw)));
    add(Sfx::UiHover, tone(0.025, 2400.0, 2600.0, 0.06, sine));
    add(Sfx::UiClick, mix(tone(0.04, 900.0, 1500.0, 0.12, tri), noise(0.02, 4000.0, 2000.0, 0.05, 6)));
    // a loopable engine hum: two detuned saws, low-passed
    let n = SR as usize;
    let mut y = 0.0f32;
    let hum: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let s = saw((t * 46.0).fract()) + saw((t * 47.0).fract());
            y += (s - y) * 0.05;
            y * 0.35
        })
        .collect();
    let hum = assets.add(AudioSource { bytes: wav(&hum) });
    commands.insert_resource(Sounds { map, hum, enabled: settings.sound, volume: settings.volume, last: Default::default() });
}

impl Sounds {
    /// Play a one-shot. `gap` (seconds) rate-limits rapid repeats of the same sound.
    pub fn play(&mut self, commands: &mut Commands, now: f64, k: Sfx, volume: f32, gap: f64) {
        let volume = volume * self.volume;
        if !self.enabled || volume < 0.01 {
            return;
        }
        if let Some(&t) = self.last.get(&k)
            && now - t < gap
        {
            return;
        }
        self.last.insert(k, now);
        if let Some(h) = self.map.get(&k) {
            commands.spawn((AudioPlayer(h.clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume.min(1.0)))));
        }
    }
}

/// Distance attenuation relative to the listener.
pub fn falloff(listener: Vec3, at: Vec3) -> f32 {
    1.0 / (1.0 + listener.distance(at) / 110.0)
}
