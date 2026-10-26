// release builds on Windows open no console window behind the game
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod art;
mod audio;
mod display;
mod pacing;
mod flight;
mod hangar;
mod screens;
mod settings;
mod thumbs;
mod ui;

use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::camera::RenderTarget;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use blocktread_server::ServerHandle;

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub enum Screen {
    #[default]
    Boot,
    /// The entry page: pick a server (with its ping), start.
    Entry,
    /// The lobby: your ship on display, the mode to play, matchmaking.
    Menu,
    Hangar,
    /// Connected to a server between flights: matchmaking, ship pick, results.
    Match,
    Flight,
}

#[derive(Component)]
pub struct MainCamera;


/// The server this client runs under `--server local`, if any. Kept alive across matches.
#[derive(Resource, Default)]
pub struct Hosted(pub Option<ServerHandle>);

/// (title, message) shown in the lobby after a session ends unexpectedly (rejected,
/// disconnected).
#[derive(Resource, Default)]
pub struct LastError(pub Option<(String, String)>);

/// Developer switches for unattended runs (screenshots, autopilot).
#[derive(Resource, Default, Clone)]
pub struct DevOpts {
    pub shot: Option<String>,
    pub shot_after: f32,
    pub exit_after_shot: bool,
    /// Take this many screenshots, `shot_every` seconds apart (files get _0, _1, ... suffixes).
    pub shot_count: u32,
    pub shot_every: f32,
    /// Start matchmaking (ffa, 2v2), the test range (solo) or spectating (watch) on launch.
    pub auto_launch: Option<String>,
    /// Server address instead of the one picked on the entry page; "local" runs one in this
    /// client (no queue wait). Players change servers in ~/.blocktread/servers.json instead.
    pub server: Option<String>,
    /// Fly by AI; also locks in the ship pick on its own.
    pub autopilot: bool,
    pub latency_ms: u64,
    /// `--server local` listens on 127.0.0.1 only (no LAN access).
    pub local_only: bool,
    /// Start on this screen: error, hangar or settings.
    pub start: Option<String>,
    /// Render into an image instead of the window (screenshots work with the display locked).
    pub offscreen: bool,
    /// Show a flight overlay (pause or board) from the given second on, for screenshots.
    pub force: Vec<(String, f32)>,
    /// UDP port for `--server local` (default: 4433, or any free one).
    pub port: Option<u16>,
    /// Log frame pacing stats every 5 seconds.
    pub frame_stats: bool,
    /// No sound at all, not even an audio device (`--mute`, and every `--offscreen` test run).
    pub mute: bool,
}

impl DevOpts {
    pub fn forced(&self, what: &str, now: f32) -> bool {
        self.force.iter().any(|(w, t)| w == what && now >= *t)
    }
}

const USAGE: &str = "用法: blocktread-client [--name 名字] [--server 地址:端口|local]
开发用: [--launch ffa|2v2|solo|watch] [--local] [--autopilot] [--latency 毫秒] [--screen lobby|error|hangar|settings]
       [--screenshot 路径 --after 秒 [--count N --every 秒] [--exit]] [--offscreen] [--force pause|board|side|tip|revert|next|hover|hit|shieldhit|kill|nohud|serverlist:秒] [--frame-stats] [--port 端口] [--mute]";

fn main() {
    let mut s = settings::load_settings();
    let mut dev = DevOpts { shot_after: 8.0, ..default() };
    let mut a = std::env::args().skip(1);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--name" => s.name = a.next().unwrap_or_default(),
            "--server" => dev.server = a.next(),
            "--launch" => dev.auto_launch = a.next(),
            "--autopilot" => dev.autopilot = true,
            "--latency" => dev.latency_ms = a.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--screenshot" => dev.shot = a.next(),
            "--after" => dev.shot_after = a.next().and_then(|v| v.parse().ok()).unwrap_or(8.0),
            "--exit" => dev.exit_after_shot = true,
            "--local" => dev.local_only = true,
            "--screen" => dev.start = a.next(),
            "--offscreen" => {
                // test runs stay silent
                dev.offscreen = true;
                dev.mute = true;
            }
            "--mute" => dev.mute = true,
            "--frame-stats" => dev.frame_stats = true,
            "--port" => dev.port = a.next().and_then(|v| v.parse().ok()),
            "--force" => dev.force.extend(a.next().and_then(|v| v.split_once(':').and_then(|(w, t)| Some((w.to_string(), t.parse().ok()?))))),
            "--count" => dev.shot_count = a.next().and_then(|v| v.parse().ok()).unwrap_or(1),
            "--every" => dev.shot_every = a.next().and_then(|v| v.parse().ok()).unwrap_or(5.0),
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("不认识的参数 {other}\n{USAGE}");
                return;
            }
        }
    }
    let mut plugins = DefaultPlugins
        .set(WindowPlugin {
            // opens at the saved size and mode; offscreen test runs render into an image, so their
            // window stays hidden and windowed
            primary_window: Some(display::initial_window(&s, dev.offscreen)),
            ..default()
        })
        .build();
    if dev.mute {
        plugins = plugins.disable::<bevy::audio::AudioPlugin>();
    }
    App::new()
        .add_plugins(plugins)
        .insert_resource(ClearColor(Color::srgb(0.024, 0.04, 0.07)))
        .insert_resource(s)
        .insert_resource(settings::load_regions())
        .insert_resource(dev)
        .init_resource::<Hosted>()
        .init_resource::<LastError>()
        .init_state::<Screen>()
        .add_systems(Startup, (ui::load_fonts, setup_camera, setup_art, audio::build_sounds))
        .add_systems(Update, boot.run_if(in_state(Screen::Boot)))
        .add_systems(Update, dev_shot)
        .add_systems(Last, exit_watchdog)
        .add_plugins((ui::UiPlugin, hangar::HangarPlugin, flight::FlightPlugin, screens::ScreensPlugin, thumbs::ThumbsPlugin, display::DisplayPlugin, pacing::PacingPlugin, audio::MusicPlugin))
        .run();
}

/// The image the main camera renders into under `--offscreen`.
#[derive(Resource)]
struct Offscreen(Handle<Image>);

fn setup_camera(mut commands: Commands, dev: Res<DevOpts>, settings: Res<settings::Settings>, mut images: ResMut<Assets<Image>>) {
    let mut cam = commands.spawn((
        Camera3d::default(),
        Camera { clear_color: ClearColorConfig::Custom(Color::srgb(0.02, 0.035, 0.06)), ..default() },
        Projection::Perspective(PerspectiveProjection { fov: 42f32.to_radians(), near: 0.1, far: 20000.0, ..default() }),
        Tonemapping::TonyMcMapface,
        Bloom::NATURAL,
        Transform::from_xyz(17.0, 13.0, 22.0).looking_at(Vec3::ZERO, Vec3::Y),
        MainCamera,
    ));
    if dev.offscreen {
        // the saved window size, so layouts can be checked at each resolution the game offers
        let [w, h] = settings.window_size;
        let img = images.add(Image::new_target_texture(w.max(640), h.max(400), TextureFormat::Rgba8UnormSrgb, None));
        cam.insert((RenderTarget::Image(img.clone().into()), IsDefaultUiCamera));
        commands.insert_resource(Offscreen(img));
    }
}

fn setup_art(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(art::build_block_art(&mut meshes, &mut mats, &mut images));
}

fn boot(mut next: ResMut<NextState<Screen>>, dev: Res<DevOpts>, mut overlays: ResMut<screens::Overlays>, mut last: ResMut<LastError>) {
    if dev.start.as_deref() == Some("error") {
        last.0 = Some(("无法连接".into(), "连接超时。请确认地址和端口无误、服务器正在运行，并且防火墙放行了 UDP。".into()));
    }
    next.set(match dev.start.as_deref() {
        Some("hangar") => Screen::Hangar,
        // straight to the lobby for unattended runs
        Some("lobby" | "error") => Screen::Menu,
        _ if dev.auto_launch.is_some() => Screen::Menu,
        _ => Screen::Entry,
    });
    overlays.settings = dev.start.as_deref() == Some("settings");
}

fn dev_shot(mut commands: Commands, dev: Res<DevOpts>, off: Option<Res<Offscreen>>, time: Res<Time>, mut taken: Local<u32>, mut exit: MessageWriter<AppExit>) {
    let Some(path) = dev.shot.clone() else { return };
    let t = time.elapsed_secs();
    let count = dev.shot_count.max(1);
    if *taken < count && t > dev.shot_after + *taken as f32 * dev.shot_every {
        let file = if count > 1 { path.replace(".png", &format!("_{}.png", *taken)) } else { path };
        let shot = match &off {
            Some(o) => Screenshot::image(o.0.clone()),
            None => Screenshot::primary_window(),
        };
        commands.spawn(shot).observe(save_to_disk(file));
        *taken += 1;
    } else if *taken >= count && dev.exit_after_shot && t > dev.shot_after + (count - 1) as f32 * dev.shot_every + 1.5 {
        exit.write(AppExit::Success);
    }
}

/// Quitting can deadlock on macOS: the render thread drops the window, which has to happen on the
/// main thread, while the main thread waits for the render thread to finish. Everything is saved
/// as it changes, so if the app hasn't exited shortly after quitting, end the process.
fn exit_watchdog(mut exits: MessageReader<AppExit>, mut armed: Local<bool>) {
    if exits.read().next().is_some() && !*armed {
        *armed = true;
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            std::process::exit(0);
        });
    }
}
