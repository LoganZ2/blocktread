//! Screen flow: entry → lobby (main screen, matchmaking) → vehicle pick → battle → results →
//! lobby, with the garage and settings reachable from the lobby. This module holds what several
//! screens share: the stage vehicles park on and the showcase, navigation and design-picking
//! actions, the settings overlay and the FPS readout.

pub mod carousel;
pub mod entry;
pub mod matching;
pub mod menu;

use crate::art::BlockArt;
use crate::audio::Sounds;
use crate::display::{DISPLAY_MODES, DisplayRevert, Monitors, fps_caps, fullscreen_sizes, window_sizes};
use crate::flight::ground::{self, DaySky, Stage};
use crate::hangar::Hangar;
use crate::settings::{self, Settings};
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use crate::{MainCamera, Screen};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use blocktread_shared::analysis::analyze;
use blocktread_shared::blocks::Design;
use blocktread_shared::presets::preset;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((entry::EntryPlugin, menu::MenuPlugin, carousel::CarouselPlugin, matching::MatchPlugin, FrameTimeDiagnosticsPlugin::default()))
            .init_resource::<Overlays>()
            .init_resource::<Showcase>()
            // the lobby's vehicles are the carousel's
            .add_systems(OnEnter(Screen::Menu), |mut c: Commands, a: Res<Stage>, s: Res<DaySky>, cam: Single<(Entity, &mut Projection), With<MainCamera>>| spawn_stage(&mut c, &a, &s, cam, Screen::Menu))
            .add_systems(OnEnter(Screen::Entry), |c: Commands, a: Res<Stage>, s: Res<DaySky>, art: ResMut<BlockArt>, m: ResMut<Assets<StandardMaterial>>, h: Res<Hangar>, cam: Single<(Entity, &mut Projection), With<MainCamera>>| {
                spawn_showcase(c, a, s, art, m, h, cam, Screen::Entry)
            })
            .add_systems(OnEnter(Screen::Match), |c: Commands, a: Res<Stage>, s: Res<DaySky>, art: ResMut<BlockArt>, m: ResMut<Assets<StandardMaterial>>, h: Res<Hangar>, cam: Single<(Entity, &mut Projection), With<MainCamera>>| {
                spawn_showcase(c, a, s, art, m, h, cam, Screen::Match)
            })
            .add_systems(Update, showcase_camera.run_if(|s: Res<State<Screen>>| matches!(s.get(), Screen::Match | Screen::Entry)))
            .add_systems(Update, (global_actions, refresh_showcase, render_settings, display_banner, apply_settings, fps_text));
    }
}

/// Which shared overlays are open.
#[derive(Resource, Default)]
pub struct Overlays {
    pub settings: bool,
}

impl Overlays {
    /// True when something modal covers the screen and screens should ignore their hotkeys.
    pub fn any(&self, modal: &ModalReq) -> bool {
        self.settings || modal.open()
    }
}

// ---------------------------------------------------------------- showcase

/// The vehicle on show, with its bounding radius.
#[derive(Component)]
struct ShowcaseShip(f32);

/// Which way parked vehicles face: three quarters towards the camera.
pub const PARKED: f32 = -0.65;

/// A design to show instead of the garage's current one (another player's pick).
#[derive(Resource, Default)]
pub struct Showcase(pub Option<Design>);

#[allow(clippy::too_many_arguments)]
fn spawn_showcase(
    mut commands: Commands,
    stage: Res<Stage>,
    sky: Res<DaySky>,
    mut art: ResMut<BlockArt>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    hangar: Res<Hangar>,
    cam: Single<(Entity, &mut Projection), With<MainCamera>>,
    screen: Screen,
) {
    spawn_stage(&mut commands, &stage, &sky, cam, screen.clone());
    spawn_showcase_ship(&mut commands, &mut art, &mut mats, &hangar.design, screen);
}

/// The world vehicles are shown in: daylight on the stage's level patch, the camera's lens.
fn spawn_stage(commands: &mut Commands, stage: &Stage, sky: &DaySky, cam: Single<(Entity, &mut Projection), With<MainCamera>>, screen: Screen) {
    let (ce, mut proj) = cam.into_inner();
    if let Projection::Perspective(p) = &mut *proj {
        p.fov = 38f32.to_radians();
    }
    ground::spawn_stage(commands, stage, sky, ce, screen, 0.0);
}

/// A design parked at the origin, on its wheels.
fn spawn_showcase_ship(commands: &mut Commands, art: &mut BlockArt, mats: &mut Assets<StandardMaterial>, design: &Design, screen: Screen) {
    let a = analyze(design);
    let com = a.stats.com;
    let at = Transform::from_xyz(0.0, ground::park_height(design, com), 0.0).with_rotation(Quat::from_rotation_y(PARKED));
    let root = commands.spawn((at, Visibility::default(), ShowcaseShip(a.stats.radius), DespawnOnExit(screen))).id();
    art.spawn_design(commands, mats, root, &design.blocks, -com);
}

/// Where the camera stands to look at a parked vehicle of bounding radius `r`, `t` seconds in: a
/// little above it, three quarters on, drifting slowly to and fro. `shift` moves the vehicle in
/// the picture, as a share of the distance: x positive to the right of centre, y positive up.
pub fn parked_view(r: f32, t: f32, shift: Vec2) -> Transform {
    let dist = r * 3.2 + 9.0;
    let aim = Vec3::new(0.0, r * 0.2, 0.0);
    let az = 0.35 * (t * 0.07).sin();
    // low, so the ridges behind show over the vehicle
    let eye = aim + Quat::from_rotation_y(az) * Vec3::new(0.0, dist * 0.2, dist);
    let view = Transform::from_translation(eye).looking_at(aim, Vec3::Y);
    // slide the camera across its own view: the vehicle moves the other way on screen
    view.with_translation(eye - view.right() * dist * shift.x - view.up() * dist * shift.y)
}

/// Flipping through ships (or looking at another player's pick) swaps the one on display.
#[allow(clippy::too_many_arguments)]
fn refresh_showcase(
    mut commands: Commands,
    hangar: Res<Hangar>,
    over: Res<Showcase>,
    state: Res<State<Screen>>,
    old: Query<Entity, With<ShowcaseShip>>,
    mut art: ResMut<BlockArt>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    if !(hangar.is_changed() || over.is_changed()) || old.is_empty() {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    spawn_showcase_ship(&mut commands, &mut art, &mut mats, over.0.as_ref().unwrap_or(&hangar.design), state.get().clone());
}

/// The camera on the parked vehicle: on the entry page it sits right of the page's column.
fn showcase_camera(time: Res<Time>, state: Res<State<Screen>>, ship: Query<&ShowcaseShip>, mut cam: Single<&mut Transform, With<MainCamera>>) {
    let Ok(s) = ship.single() else { return };
    let shift = if *state.get() == Screen::Entry { Vec2::new(0.3, 0.0) } else { Vec2::ZERO };
    **cam = parked_view(s.0, time.elapsed_secs(), shift);
}

// ---------------------------------------------------------------- shared actions

#[allow(clippy::too_many_arguments)]
fn global_actions(
    mut msgs: MessageReader<UiAction>,
    mut next: ResMut<NextState<Screen>>,
    mut modal: ResMut<ModalReq>,
    mut overlays: ResMut<Overlays>,
    mut hangar: ResMut<Hangar>,
    mut toasts: ResMut<Toasts>,
    mut settings: ResMut<Settings>,
    mut exit: MessageWriter<AppExit>,
    mut revert: ResMut<DisplayRevert>,
    time: Res<Time>,
) {
    let now = time.elapsed_secs_f64();
    for m in msgs.read() {
        match m.clone() {
            UiAction::Goto(s) => next.set(s),
            UiAction::Quit => modal.show("退出游戏？", "正在运行的房间会一起关闭。", vec![("取消", UiAction::CloseModal, Kind::Normal), ("退出", UiAction::QuitConfirmed, Kind::Danger)], None),
            UiAction::QuitConfirmed => {
                exit.write(AppExit::Success);
            }
            UiAction::CloseModal => modal.0 = None,
            UiAction::OpenSettings => overlays.settings = true,
            UiAction::CloseSettings => {
                // closing the dialog keeps a pending display change: the player could see to do it
                overlays.settings = false;
                revert.keep();
                settings::save_settings(&settings);
            }
            UiAction::DisplayMode(mode) if settings.display != mode => {
                revert.begin(&settings, now);
                settings.display = mode.into();
            }
            UiAction::WindowSize(w, h) if settings.window_size != [w, h] => {
                revert.begin(&settings, now);
                settings.window_size = [w, h];
            }
            UiAction::FullscreenSize(w, h) if settings.fullscreen_size != [w, h] => {
                revert.begin(&settings, now);
                settings.fullscreen_size = [w, h];
            }
            UiAction::FpsCap(cap) => settings.fps_cap = cap,
            UiAction::KeepDisplay => {
                revert.keep();
                settings::save_settings(&settings);
            }
            UiAction::RevertDisplay => {
                revert.revert(&mut settings);
                settings::save_settings(&settings);
            }
            // the ship carousel, wherever it shows (hangar, menu, ship pick)
            UiAction::ShipPrev => hangar.step(-1),
            UiAction::ShipNext => hangar.step(1),
            UiAction::NewShip => {
                hangar.add_ship(Design { name: String::new(), blocks: preset("blank").blocks });
                toasts.push(format!("新建了「{}」", hangar.design.name), ToastKind::Ok);
            }
            UiAction::DeleteShip => {
                let n = hangar.design.name.clone();
                modal.show(format!("删除「{n}」？"), "这辆战车会从车库里移除，无法撤销。", vec![("取消", UiAction::CloseModal, Kind::Normal), ("删除", UiAction::DeleteShipConfirmed, Kind::Danger)], None);
            }
            UiAction::DeleteShipConfirmed => {
                modal.0 = None;
                let n = hangar.design.name.clone();
                hangar.delete_current();
                toasts.push(format!("已删除「{n}」"), ToastKind::Info);
            }
            UiAction::Set(k, v) => {
                let on = v > 0.5;
                match k {
                    SetKey::Sound => settings.sound = on,
                    SetKey::Volume => settings.volume = v,
                    SetKey::Music => settings.music = v,
                    SetKey::Sensitivity => settings.sensitivity = v,
                    SetKey::InvertY => settings.invert_y = on,
                    SetKey::Bloom => settings.bloom = on,
                    SetKey::Vsync => settings.vsync = on,
                    SetKey::ShowFps => settings.show_fps = on,
                    SetKey::ShowHelp => settings.show_help = on,
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- settings overlay

#[derive(Component)]
struct SettingsRoot;

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}
fn mult(v: f32) -> String {
    format!("{v:.2}×")
}

/// The display settings a settings dialog was built for: it is rebuilt when they change (the
/// resolution choices depend on the mode, and a revert must show the restored values).
type BuiltFor = (String, [u32; 2], [u32; 2], bool);

#[allow(clippy::too_many_arguments)]
fn render_settings(
    mut commands: Commands,
    mut overlays: ResMut<Overlays>,
    roots: Query<Entity, With<SettingsRoot>>,
    bodies: Query<&ScrollPosition, With<SettingsBody>>,
    font: Res<UiFont>,
    icons: Res<Icons>,
    settings: Res<Settings>,
    keys: Res<ButtonInput<KeyCode>>,
    modal: Res<ModalReq>,
    monitors: Monitors,
    mut revert: ResMut<DisplayRevert>,
    mut built: Local<Option<BuiltFor>>,
) {
    if overlays.settings && !modal.open() && keys.just_pressed(KeyCode::Escape) {
        overlays.settings = false;
        revert.keep();
        settings::save_settings(&settings);
    }
    let open = overlays.settings;
    let exists = !roots.is_empty();
    // vsync too: the frame-rate caps on offer depend on it
    let now: BuiltFor = (settings.display.clone(), settings.window_size, settings.fullscreen_size, settings.vsync);
    let stale = open && exists && built.as_ref() != Some(&now);
    if open == exists && !stale {
        return;
    }
    // a rebuild keeps the scroll position, so changing a display option doesn't jump to the top
    let scrolled = bodies.iter().next().map_or(Vec2::ZERO, |s| s.0);
    for e in &roots {
        commands.entity(e).despawn();
    }
    if !open {
        *built = None;
        return;
    }
    *built = Some(now);
    let f = &*font;
    let (o, p) = spawn_overlay(&mut commands, f, "设置", 520.0);
    commands.entity(o).insert((SettingsRoot, GlobalZIndex(60)));
    if stale {
        // a rebuild in place: no entrance motion
        commands.entity(p).remove::<SlideIn>();
    }
    // the sections scroll when the window is too short for them; the revert bar and the done
    // button stay put below
    let body = spawn_scroll_body(&mut commands, p);
    commands.entity(body).insert((SettingsBody, ScrollPosition(if stale { scrolled } else { Vec2::ZERO })));
    // each section: header, then 36px rows with the label left and the control right
    let audio = spawn_section(&mut commands, f, body, "声音|AUDIO");
    spawn_toggle(&mut commands, f, audio, "音效", settings.sound, SetKey::Sound);
    spawn_slider(&mut commands, f, audio, "音量", settings.volume, 0.0, 1.0, SetKey::Volume, pct);
    spawn_slider(&mut commands, f, audio, "背景音乐", settings.music, 0.0, 1.0, SetKey::Music, pct);
    let controls = spawn_section(&mut commands, f, body, "操控|CONTROLS");
    spawn_slider(&mut commands, f, controls, "鼠标灵敏度", settings.sensitivity, 0.5, 2.0, SetKey::Sensitivity, mult);
    spawn_toggle(&mut commands, f, controls, "鼠标 Y 轴反转（推鼠标往上 = 镜头向下）", settings.invert_y, SetKey::InvertY);
    spawn_toggle(&mut commands, f, controls, "开战时显示操作说明", settings.show_help, SetKey::ShowHelp);

    let display = spawn_section(&mut commands, f, body, "画面|DISPLAY");
    let monitor = monitors.current();
    let modes = DISPLAY_MODES.iter().map(|&(k, label)| (label.to_string(), UiAction::DisplayMode(k))).collect();
    let mode_index = DISPLAY_MODES.iter().position(|(k, _)| *k == settings.display).unwrap_or(0);
    spawn_choice(&mut commands, f, &icons, display, "显示模式", modes, mode_index, true);
    let size_label = |s: [u32; 2]| format!("{} × {}", s[0], s[1]);
    match settings.display.as_str() {
        "windowed" => {
            let sizes = window_sizes(monitor.as_ref(), settings.window_size);
            let i = sizes.iter().position(|s| *s == settings.window_size).unwrap_or(0);
            let opts = sizes.iter().map(|&s| (size_label(s), UiAction::WindowSize(s[0], s[1]))).collect();
            spawn_choice(&mut commands, f, &icons, display, "窗口大小", opts, i, true);
        }
        "fullscreen" if !fullscreen_sizes(monitor.as_ref()).is_empty() => {
            let sizes = fullscreen_sizes(monitor.as_ref());
            // [0, 0] means "the monitor's own mode": show its native size
            let current = if settings.fullscreen_size == [0, 0] { monitor.as_ref().map_or([0, 0], |m| [m.physical.x, m.physical.y]) } else { settings.fullscreen_size };
            let i = sizes.iter().position(|s| *s == current).unwrap_or(0);
            let opts = sizes.iter().map(|&s| (size_label(s), UiAction::FullscreenSize(s[0], s[1]))).collect();
            spawn_choice(&mut commands, f, &icons, display, "分辨率", opts, i, true);
        }
        _ => {
            let native = monitor.as_ref().map_or("跟随显示器".to_string(), |m| format!("跟随显示器 {}", size_label([m.physical.x, m.physical.y])));
            spawn_choice(&mut commands, f, &icons, display, "分辨率", vec![(native, UiAction::CloseModal)], 0, false);
        }
    }
    let hz = monitor.as_ref().and_then(|m| m.refresh_hz).map(|hz| format!("（显示器 {hz:.0} Hz）")).unwrap_or_default();
    spawn_toggle(&mut commands, f, display, &format!("垂直同步{hz}"), settings.vsync, SetKey::Vsync);
    let caps = fps_caps(monitor.as_ref(), settings.vsync);
    // a saved cap this monitor doesn't offer shows as the nearest one
    let cap_index = caps.iter().enumerate().min_by_key(|(_, (c, _))| c.abs_diff(settings.fps_cap)).map_or(0, |(i, _)| i);
    let caps = caps.into_iter().map(|(c, label)| (label, UiAction::FpsCap(c))).collect();
    spawn_choice(&mut commands, f, &icons, display, "帧率上限", caps, cap_index, true);
    spawn_toggle(&mut commands, f, display, "泛光（发光效果）", settings.bloom, SetKey::Bloom);
    spawn_toggle(&mut commands, f, display, "显示帧率", settings.show_fps, SetKey::ShowFps);

    // keep-or-revert bar, shown while a display change waits for confirmation
    let bar = commands
        .spawn((
            Node { display: Display::None, padding: UiRect::axes(px(14), px(10)), column_gap: px(GAP_S), align_items: AlignItems::Center, ..default() },
            Plate { fill: tone(WARN, 0.14), edge: WARN.with_alpha(0.5), ..default() },
            DisplayBanner,
            ChildOf(p),
        ))
        .id();
    commands.spawn((text(f, "", 13.0, INK), BannerText, Node { flex_grow: 1.0, ..default() }, ChildOf(bar)));
    spawn_btn(&mut commands, f, bar, Kind::Normal, "还原", UiAction::RevertDisplay, None);
    spawn_btn(&mut commands, f, bar, Kind::Accent, "保留", UiAction::KeepDisplay, None);

    let foot = commands.spawn((Node { justify_content: JustifyContent::FlexEnd, ..default() }, ChildOf(p))).id();
    spawn_btn(&mut commands, f, foot, Kind::Accent, "完成", UiAction::CloseSettings, None);
}

#[derive(Component)]
struct SettingsBody;
#[derive(Component)]
struct DisplayBanner;
#[derive(Component)]
struct BannerText;

/// Show the keep-or-revert bar with its countdown while a display change is pending.
fn display_banner(revert: Res<DisplayRevert>, time: Res<Time>, mut bars: Query<&mut Node, With<DisplayBanner>>, mut texts: Query<&mut Text, With<BannerText>>) {
    let left = revert.remaining(time.elapsed_secs_f64());
    for mut n in &mut bars {
        let want = if left.is_some() { Display::Flex } else { Display::None };
        if n.display != want {
            n.display = want;
        }
    }
    if let Some(left) = left {
        let msg = format!("显示设置已更改，{} 秒后自动还原", left.ceil() as u32);
        for mut t in &mut texts {
            if t.0 != msg {
                t.0 = msg.clone();
            }
        }
    }
}

fn apply_settings(
    settings: Res<Settings>,
    dev: Res<crate::DevOpts>,
    mut sounds: ResMut<Sounds>,
    mut commands: Commands,
    cam: Single<(Entity, Has<Bloom>), With<MainCamera>>,
) {
    if !settings.is_changed() {
        return;
    }
    sounds.enabled = settings.sound && !dev.mute;
    sounds.volume = settings.volume;
    let (ce, has_bloom) = *cam;
    if settings.bloom && !has_bloom {
        commands.entity(ce).insert(Bloom::NATURAL);
    } else if !settings.bloom && has_bloom {
        commands.entity(ce).remove::<Bloom>();
    }
}

#[derive(Component)]
struct FpsText;

fn fps_text(mut commands: Commands, settings: Res<Settings>, diag: Res<DiagnosticsStore>, q: Query<(Entity, &mut Text), With<FpsText>>, font: Res<UiFont>) {
    let fps = diag.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed());
    match (settings.show_fps, q.single_inner()) {
        (true, Ok((_, mut t))) => t.0 = format!("{:.0} FPS", fps.unwrap_or(0.0)),
        (true, Err(_)) => {
            commands.spawn((tech(&font, "", 12.0, MUTED), Node { position_type: PositionType::Absolute, right: px(10), bottom: px(4), ..default() }, GlobalZIndex(96), Pickable::IGNORE, FpsText));
        }
        (false, Ok((e, _))) => commands.entity(e).despawn(),
        _ => {}
    }
}
