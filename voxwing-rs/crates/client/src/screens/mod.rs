//! Screen flow: main menu → launch prep (lobby) → flight, with the hangar and settings reachable
//! from anywhere sensible. This module holds what several screens share: the 3D ship showcase,
//! navigation and design-picking actions, the settings overlay and the FPS readout.

pub mod lobby;
pub mod menu;

use crate::art::BlockArt;
use crate::audio::Sounds;
use crate::display::{DISPLAY_MODES, DisplayRevert, Monitors, fps_caps, fullscreen_sizes, window_sizes};
use crate::flight::env::{self, EnvAssets, SkyImage};
use crate::hangar::Hangar;
use crate::settings::{self, Settings};
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use crate::{MainCamera, Screen};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::light::Skybox;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use voxwing_shared::analysis::analyze;
use voxwing_shared::presets::{PRESETS, preset};

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((menu::MenuPlugin, lobby::LobbyPlugin, FrameTimeDiagnosticsPlugin::default()))
            .init_resource::<Overlays>()
            .add_systems(OnEnter(Screen::Menu), |c: Commands, a: Res<EnvAssets>, s: Res<SkyImage>, art: ResMut<BlockArt>, m: ResMut<Assets<StandardMaterial>>, h: Res<Hangar>, cam: Single<(Entity, &mut Projection), With<MainCamera>>| {
                spawn_showcase(c, a, s, art, m, h, cam, Screen::Menu)
            })
            .add_systems(OnEnter(Screen::Lobby), |c: Commands, a: Res<EnvAssets>, s: Res<SkyImage>, art: ResMut<BlockArt>, m: ResMut<Assets<StandardMaterial>>, h: Res<Hangar>, cam: Single<(Entity, &mut Projection), With<MainCamera>>| {
                spawn_showcase(c, a, s, art, m, h, cam, Screen::Lobby)
            })
            .add_systems(Update, showcase_turn.run_if(|s: Res<State<Screen>>| matches!(s.get(), Screen::Menu | Screen::Lobby)))
            .add_systems(Update, (global_actions, render_picker, render_settings, display_banner, apply_settings, fps_text));
    }
}

/// Which shared overlays are open.
#[derive(Resource, Default)]
pub struct Overlays {
    pub picker: bool,
    pub settings: bool,
    picker_dirty: bool,
}

impl Overlays {
    /// True when something modal covers the screen and screens should ignore their hotkeys.
    pub fn any(&self, modal: &ModalReq) -> bool {
        self.picker || self.settings || modal.open()
    }
}

// ---------------------------------------------------------------- showcase

#[derive(Component)]
struct ShowcaseShip(f32);

#[allow(clippy::too_many_arguments)]
fn spawn_showcase(
    mut commands: Commands,
    env_assets: Res<EnvAssets>,
    sky: Res<SkyImage>,
    mut art: ResMut<BlockArt>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    hangar: Res<Hangar>,
    cam: Single<(Entity, &mut Projection), With<MainCamera>>,
    screen: Screen,
) {
    let (ce, mut proj) = cam.into_inner();
    commands.entity(ce).insert(Skybox { image: Some(sky.0.clone()), brightness: 1200.0, ..default() });
    if let Projection::Perspective(p) = &mut *proj {
        p.fov = 38f32.to_radians();
    }
    env::spawn_env(&mut commands, &env_assets, screen.clone(), Vec3::ZERO);
    let a = analyze(&hangar.design);
    let com = a.stats.com;
    let root = commands.spawn((Transform::default(), Visibility::default(), ShowcaseShip(a.stats.radius), DespawnOnExit(screen))).id();
    for b in &hangar.design.blocks {
        art.spawn_block(&mut commands, &mut mats, root, b, b.pos().as_vec3() - com, 3);
    }
}

fn showcase_turn(time: Res<Time>, state: Res<State<Screen>>, mut ship: Query<(&mut Transform, &ShowcaseShip), Without<MainCamera>>, mut cam: Single<&mut Transform, With<MainCamera>>) {
    let t = time.elapsed_secs();
    let Ok((mut st, s)) = ship.single_mut() else { return };
    st.rotation = Quat::from_rotation_y(t * 0.22 + 2.4) * Quat::from_rotation_x(0.08 * (t * 0.5).sin());
    st.translation = Vec3::Y * (t * 0.8).sin() * 0.25;
    let dist = s.0 * 3.6 + 18.0;
    let target = match state.get() {
        Screen::Menu => Vec3::new(-s.0 * 1.1 - 6.0, -1.5, 0.0),
        _ => Vec3::new(s.0 * 0.15, 0.6, 0.0),
    };
    let sway = Vec3::new((t * 0.1).sin() * 2.0, 0.0, 0.0);
    **cam = Transform::from_translation(target + Vec3::new(0.0, dist * 0.22, dist) + sway).looking_at(target, Vec3::Y);
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
            UiAction::Goto(s) => {
                overlays.picker = false;
                next.set(s);
            }
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
            UiAction::OpenLoad => {
                overlays.picker = !overlays.picker;
                overlays.picker_dirty = true;
            }
            UiAction::Preset(k) => {
                let d = preset(k);
                toasts.push(format!("已换上「{}」", d.name), ToastKind::Ok);
                hangar.load_design(d);
                overlays.picker = false;
            }
            UiAction::LoadSaved(n) => {
                if let Some(d) = settings::saved_designs().remove(&n) {
                    hangar.load_design(d);
                    toasts.push(format!("已换上「{n}」"), ToastKind::Ok);
                }
                overlays.picker = false;
            }
            UiAction::DeleteSaved(n) => {
                settings::delete_design(&n);
                toasts.push(format!("已删除「{n}」"), ToastKind::Info);
                overlays.picker_dirty = true;
            }
            UiAction::Set(k, v) => {
                let on = v > 0.5;
                match k {
                    SetKey::Sound => settings.sound = on,
                    SetKey::Volume => settings.volume = v,
                    SetKey::Sensitivity => settings.sensitivity = v,
                    SetKey::InvertY => settings.invert_y = on,
                    SetKey::Bloom => settings.bloom = on,
                    SetKey::Vsync => settings.vsync = on,
                    SetKey::ShowFps => settings.show_fps = on,
                    SetKey::ShowHelp => settings.show_help = on,
                    SetKey::Bots => settings.bots = v.round() as u32,
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- design picker

#[derive(Component)]
struct PickerRoot;

fn render_picker(mut commands: Commands, mut overlays: ResMut<Overlays>, roots: Query<Entity, With<PickerRoot>>, font: Res<UiFont>, keys: Res<ButtonInput<KeyCode>>, hangar: Res<Hangar>) {
    if overlays.picker && keys.just_pressed(KeyCode::Escape) {
        overlays.picker = false;
    }
    if !overlays.is_changed() && !overlays.picker_dirty {
        return;
    }
    let open = overlays.picker;
    overlays.picker_dirty = false;
    let exists = !roots.is_empty();
    if exists && open && !overlays.is_changed() {
        // only content refresh requested
    }
    for e in &roots {
        commands.entity(e).despawn();
    }
    if !open {
        return;
    }
    let f = &*font;
    let (o, p) = spawn_overlay(&mut commands, f, "选择机体", 640.0);
    commands.entity(o).insert(PickerRoot);
    commands.spawn((text(f, format!("当前：{}（会被替换，可在机库里撤销）", hangar.design.name), 13.0, MUTED), ChildOf(p)));
    let templates = spawn_section(&mut commands, f, p, "模板|TEMPLATES");
    let grid = commands.spawn((Node { display: Display::Grid, grid_template_columns: RepeatedGridTrack::flex(2, 1.0), column_gap: px(GAP_S), row_gap: px(GAP_S), ..default() }, ChildOf(templates))).id();
    for pr in PRESETS {
        let s = analyze(&preset(pr.key)).stats;
        let card = spawn_btn(&mut commands, f, grid, Kind::Card, pr.name, UiAction::Preset(pr.key), None);
        commands.spawn((text(f, pr.desc, 12.0, MUTED), Pickable::IGNORE, ChildOf(card)));
        commands.spawn((
            tech(f, format!("{:.0} m/s  ·  {:.0} 度/秒  ·  {:.0} DPS", s.max_speed, s.pitch_rate.to_degrees(), s.dps), 12.0, INK.with_alpha(0.8)),
            Pickable::IGNORE,
            ChildOf(card),
        ));
    }
    let mine = spawn_section(&mut commands, f, p, "我的设计|SAVED");
    let saved = settings::saved_designs();
    if saved.is_empty() {
        commands.spawn((text(f, "还没有保存的设计。在机库里点「保存」就会出现在这里。", 13.0, MUTED), ChildOf(mine)));
    }
    for (name, d) in saved {
        let a = analyze(&d);
        let r = commands.spawn((row(GAP_S), ChildOf(mine))).id();
        let b = spawn_btn(&mut commands, f, r, Kind::Normal, name.clone(), UiAction::LoadSaved(name.clone()), None);
        commands.entity(b).insert(Node { height: px(H_MD), flex_grow: 1.0, padding: UiRect::horizontal(px(16)), justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, ..default() });
        let status = if a.ok() { format!("{} 块 · 可以出击", d.blocks.len()) } else { format!("{} 块 · {} 项检查未通过", d.blocks.len(), a.errors) };
        commands.spawn((text(f, status, 12.0, if a.ok() { OK } else { WARN }), Pickable::IGNORE, ChildOf(b)));
        spawn_btn(&mut commands, f, r, Kind::Danger, "删除", UiAction::DeleteSaved(name), None);
    }
    let foot = commands.spawn((Node { justify_content: JustifyContent::FlexEnd, ..default() }, ChildOf(p))).id();
    spawn_btn(&mut commands, f, foot, Kind::Normal, "关闭", UiAction::OpenLoad, Some("Esc"));
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
    let controls = spawn_section(&mut commands, f, body, "操控|CONTROLS");
    spawn_slider(&mut commands, f, controls, "鼠标灵敏度", settings.sensitivity, 0.5, 2.0, SetKey::Sensitivity, mult);
    spawn_toggle(&mut commands, f, controls, "鼠标 Y 轴反转（推鼠标往上 = 机头向下）", settings.invert_y, SetKey::InvertY);
    spawn_toggle(&mut commands, f, controls, "出击时显示操作说明", settings.show_help, SetKey::ShowHelp);

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
            Plate { fill: tone(WARN, 0.14), edge: WARN.with_alpha(0.5), chamfer: 8.0, corners: BR, ..default() },
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
    mut sounds: ResMut<Sounds>,
    mut commands: Commands,
    cam: Single<(Entity, Has<Bloom>), With<MainCamera>>,
) {
    if !settings.is_changed() {
        return;
    }
    sounds.enabled = settings.sound;
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
