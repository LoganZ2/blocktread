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
use voxwing_server::ServerHandle;

#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub enum Screen {
    #[default]
    Boot,
    Menu,
    Lobby,
    Hangar,
    Flight,
}

#[derive(Component)]
pub struct MainCamera;

/// The room this client is hosting, if any. Kept alive across flights so friends stay connected.
#[derive(Resource, Default)]
pub struct Hosted(pub Option<ServerHandle>);

/// (title, message) shown in launch prep after a flight ends unexpectedly (rejected, disconnected).
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
    pub auto_launch: Option<String>,
    pub autopilot: bool,
    pub latency_ms: u64,
    /// Host rooms on 127.0.0.1 only (no LAN access).
    pub local_only: bool,
    /// Start on this screen: lobby, join, error, hangar, settings or picker.
    pub start: Option<String>,
    /// Render into an image instead of the window (screenshots work with the display locked).
    pub offscreen: bool,
    /// Show a flight overlay (pause or board) from the given second on, for screenshots.
    pub force: Option<(String, f32)>,
    /// UDP port for rooms hosted from this client (default 4433).
    pub port: Option<u16>,
    /// Log frame pacing stats every 5 seconds.
    pub frame_stats: bool,
}

impl DevOpts {
    pub fn forced(&self, what: &str, now: f32) -> bool {
        self.force.as_ref().is_some_and(|(w, t)| w == what && now >= *t)
    }
}

const USAGE: &str = "用法: voxwing-client [--name 名字] [--server 地址:端口]
开发用: [--launch host|join|watch|watch-join] [--local] [--autopilot] [--latency 毫秒] [--screen lobby|join|error|hangar|settings|picker]
       [--screenshot 路径 --after 秒 [--count N --every 秒] [--exit]] [--offscreen] [--force pause|board|side|tip|revert:秒] [--frame-stats] [--port 端口]";

fn main() {
    let mut s = settings::load_settings();
    let mut dev = DevOpts { shot_after: 8.0, ..default() };
    let mut a = std::env::args().skip(1);
    while let Some(k) = a.next() {
        match k.as_str() {
            "--name" => s.name = a.next().unwrap_or_default(),
            "--server" => s.server = a.next().unwrap_or_default(),
            "--launch" => dev.auto_launch = a.next(),
            "--autopilot" => dev.autopilot = true,
            "--latency" => dev.latency_ms = a.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--screenshot" => dev.shot = a.next(),
            "--after" => dev.shot_after = a.next().and_then(|v| v.parse().ok()).unwrap_or(8.0),
            "--exit" => dev.exit_after_shot = true,
            "--local" => dev.local_only = true,
            "--screen" => dev.start = a.next(),
            "--offscreen" => dev.offscreen = true,
            "--frame-stats" => dev.frame_stats = true,
            "--port" => dev.port = a.next().and_then(|v| v.parse().ok()),
            "--force" => dev.force = a.next().and_then(|v| v.split_once(':').and_then(|(w, t)| Some((w.to_string(), t.parse().ok()?)))),
            "--count" => dev.shot_count = a.next().and_then(|v| v.parse().ok()).unwrap_or(1),
            "--every" => dev.shot_every = a.next().and_then(|v| v.parse().ok()).unwrap_or(5.0),
            "--mode" => s.host_mode = a.next().unwrap_or_else(|| "coop".into()),
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
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            // opens at the saved size and mode; offscreen test runs render into an image, so their
            // window stays hidden and windowed
            primary_window: Some(display::initial_window(&s, dev.offscreen)),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.024, 0.04, 0.07)))
        .insert_resource(s)
        .insert_resource(dev)
        .init_resource::<Hosted>()
        .init_resource::<LastError>()
        .init_state::<Screen>()
        .add_systems(Startup, (ui::load_fonts, setup_camera, setup_art, audio::build_sounds))
        .add_systems(Update, boot.run_if(in_state(Screen::Boot)))
        .add_systems(Update, dev_shot)
        .add_plugins((ui::UiPlugin, hangar::HangarPlugin, flight::FlightPlugin, screens::ScreensPlugin, thumbs::ThumbsPlugin, display::DisplayPlugin, pacing::PacingPlugin))
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
        _ if dev.auto_launch.is_some() => Screen::Lobby,
        Some("lobby" | "join" | "error") => Screen::Lobby,
        Some("hangar") => Screen::Hangar,
        _ => Screen::Menu,
    });
    overlays.settings = dev.start.as_deref() == Some("settings");
    overlays.picker = dev.start.as_deref() == Some("picker");
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
