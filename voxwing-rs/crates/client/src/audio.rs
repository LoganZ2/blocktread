//! Sound: recorded effects and the background music, built into the program.
//!
//! The effects come from Kenney's CC0 sci-fi and interface packs, the jet engine and afterburner
//! from two CC0 recordings on Freesound, and the music is "Cosmic Navigation" by Synth-thetic
//! (CC0); `assets/sounds/SOURCES.md` lists which file became which sound and how it was cut. Most effects have several takes: each play picks one at random and
//! detunes it a little, so a burst of fire doesn't sound like one clip on repeat.

use crate::Screen;
use crate::settings::Settings;
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
    /// The afterburner lighting up.
    Ignite,
    /// Your round landed on a hull, on a shield, or finished a ship off: heard at full volume
    /// however far away the target is.
    ConfirmHull,
    ConfirmShield,
    ConfirmKill,
    Lock,
    Warn,
    Place,
    Erase,
    Deny,
    Wave,
    UiHover,
    UiClick,
}

/// The takes of one effect, as built-in OGG files.
macro_rules! takes {
    ($($f:literal),+) => { &[$(include_bytes!(concat!("../assets/sounds/", $f, ".ogg")) as &[u8]),+] };
}

/// Each effect: its takes, its level (the files are all peak-normalised, so this sets the mix),
/// and how far one play may be detuned from the next (fraction of pitch).
const EFFECTS: &[(Sfx, &[&[u8]], f32, f32)] = &[
    (Sfx::Laser, takes!("laser_0", "laser_1", "laser_2", "laser_3", "laser_4"), 0.3, 0.05),
    (Sfx::Plasma, takes!("plasma_0", "plasma_1", "plasma_2", "plasma_3", "plasma_4"), 0.42, 0.05),
    (Sfx::Rail, takes!("rail_0", "rail_1", "rail_2"), 0.5, 0.04),
    (Sfx::Cannon, takes!("cannon_0", "cannon_1", "cannon_2"), 0.38, 0.07),
    (Sfx::Flak, takes!("flak_0", "flak_1"), 0.55, 0.06),
    (Sfx::Missile, takes!("missile_0", "missile_1"), 0.45, 0.05),
    (Sfx::Hit, takes!("hit_0", "hit_1", "hit_2", "hit_3", "hit_4"), 0.42, 0.08),
    (Sfx::Shield, takes!("shield_0", "shield_1", "shield_2"), 0.38, 0.06),
    (Sfx::Pop, takes!("pop_1", "pop_2", "pop_3"), 0.55, 0.08),
    (Sfx::Boom, takes!("boom_0", "boom_1"), 0.85, 0.05),
    (Sfx::Crash, takes!("crash_1", "crash_3"), 0.65, 0.06),
    (Sfx::Ignite, takes!("ignite"), 0.5, 0.04),
    (Sfx::ConfirmHull, takes!("confirm_hull"), 0.34, 0.04),
    (Sfx::ConfirmShield, takes!("confirm_shield"), 0.26, 0.04),
    (Sfx::ConfirmKill, takes!("confirm_kill"), 0.5, 0.0),
    (Sfx::Lock, takes!("lock"), 0.35, 0.0),
    (Sfx::Warn, takes!("warn"), 0.3, 0.0),
    (Sfx::Deny, takes!("deny"), 0.32, 0.0),
    (Sfx::Place, takes!("place_1", "place_2", "place_3", "place_4"), 0.4, 0.04),
    (Sfx::Erase, takes!("erase"), 0.36, 0.04),
    (Sfx::Wave, takes!("wave"), 0.38, 0.0),
    (Sfx::UiHover, takes!("hover"), 0.15, 0.03),
    (Sfx::UiClick, takes!("click"), 0.25, 0.03),
];

const ENGINE: &[u8] = include_bytes!("../assets/sounds/engine.ogg");
const BURNER: &[u8] = include_bytes!("../assets/sounds/burner.ogg");
const MUSIC: &[u8] = include_bytes!("../assets/sounds/music.ogg");

/// The jet loop's level at full throttle, and the afterburner loop's under boost (both files are
/// peak-normalised).
pub const ENGINE_GAIN: f32 = 0.42;
pub const BURNER_GAIN: f32 = 0.4;
/// The music under flight, relative to the menus.
const MUSIC_IN_FLIGHT: f32 = 0.45;

struct Effect {
    takes: Vec<Handle<AudioSource>>,
    gain: f32,
    detune: f32,
}

#[derive(Resource)]
pub struct Sounds {
    map: std::collections::HashMap<Sfx, Effect>,
    pub hum: Handle<AudioSource>,
    pub burner: Handle<AudioSource>,
    music: Option<Handle<AudioSource>>,
    pub enabled: bool,
    pub volume: f32,
    last: std::collections::HashMap<Sfx, f64>,
    rng: Rng,
}

fn source(bytes: &'static [u8]) -> AudioSource {
    AudioSource { bytes: Arc::from(bytes) }
}

pub fn build_sounds(mut commands: Commands, assets: Option<ResMut<Assets<AudioSource>>>, settings: Res<Settings>, dev: Res<crate::DevOpts>) {
    // muted (test runs): no audio at all
    let Some(mut assets) = assets.filter(|_| !dev.mute) else {
        commands.insert_resource(Sounds { map: Default::default(), hum: Handle::default(), burner: Handle::default(), music: None, enabled: false, volume: 0.0, last: Default::default(), rng: Rng::new(1) });
        return;
    };
    let map = EFFECTS
        .iter()
        .map(|&(k, files, gain, detune)| (k, Effect { takes: files.iter().map(|b| assets.add(source(b))).collect(), gain, detune }))
        .collect();
    let hum = assets.add(source(ENGINE));
    let burner = assets.add(source(BURNER));
    let music = Some(assets.add(source(MUSIC)));
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
    commands.insert_resource(Sounds { map, hum, burner, music, enabled: settings.sound, volume: settings.volume, last: Default::default(), rng: Rng::new(seed) });
}

impl Sounds {
    /// Play a one-shot. `gap` (seconds) rate-limits rapid repeats of the same sound.
    pub fn play(&mut self, commands: &mut Commands, now: f64, k: Sfx, volume: f32, gap: f64) {
        let Some(e) = self.map.get(&k) else { return };
        let volume = volume * e.gain * self.volume;
        if !self.enabled || volume < 0.005 {
            return;
        }
        if let Some(&t) = self.last.get(&k)
            && now - t < gap
        {
            return;
        }
        self.last.insert(k, now);
        let take = e.takes[self.rng.below(e.takes.len())].clone();
        let speed = 1.0 + e.detune * (self.rng.f32() * 2.0 - 1.0);
        commands.spawn((AudioPlayer(take), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume.min(1.0))).with_speed(speed)));
    }
}

/// Distance attenuation relative to the listener.
pub fn falloff(listener: Vec3, at: Vec3) -> f32 {
    1.0 / (1.0 + listener.distance(at) / 110.0)
}

#[derive(Component)]
struct Music;

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, music);
    }
}

/// The background music: one loop for the whole session, full in the menus and lower in flight,
/// eased between the two.
fn music(mut commands: Commands, sounds: Option<Res<Sounds>>, settings: Res<Settings>, screen: Res<State<Screen>>, mut sinks: Query<&mut AudioSink, With<Music>>, playing: Query<(), With<Music>>, time: Res<Time>, mut level: Local<f32>) {
    let Some(track) = sounds.and_then(|s| s.music.clone()) else { return };
    let flight = if *screen.get() == Screen::Flight { MUSIC_IN_FLIGHT } else { 1.0 };
    let want = settings.volume * settings.music * flight;
    if playing.is_empty() {
        if want > 0.0 {
            *level = 0.0;
            commands.spawn((AudioPlayer(track), PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0)), Music));
        }
        return;
    }
    *level += (want - *level) * (1.0 - (-time.delta_secs() * 1.5).exp());
    for mut sink in &mut sinks {
        sink.set_volume(Volume::Linear(*level));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::audio::Decodable;

    /// Every built-in sound decodes, and is about as long as it should be.
    #[test]
    fn sounds_decode() {
        let mut files: Vec<(&str, &[u8])> = vec![("engine", ENGINE), ("burner", BURNER), ("music", MUSIC)];
        for (k, takes, _, _) in EFFECTS {
            assert!(!takes.is_empty(), "{k:?} has no takes");
            files.extend(takes.iter().map(|b| ("effect", *b)));
        }
        for (name, bytes) in files {
            let d = source(bytes).decoder();
            let (rate, ch) = (rodio_rate(&d), rodio_channels(&d));
            let secs = d.count() as f32 / (rate as f32 * ch as f32);
            let (lo, hi) = match name {
                "music" => (150.0, 157.0),
                "engine" => (7.5, 8.5),
                "burner" => (2.3, 2.9),
                _ => (0.005, 3.0),
            };
            assert!((lo..hi).contains(&secs), "{name}: {secs:.2} s");
        }
    }

    fn rodio_rate<S: bevy::audio::Source>(s: &S) -> u32 {
        s.sample_rate().into()
    }
    fn rodio_channels<S: bevy::audio::Source>(s: &S) -> u16 {
        s.channels().into()
    }
}
