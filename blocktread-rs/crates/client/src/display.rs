//! Display settings: window mode, resolution, vsync and a frame-rate cap, plus the keep-or-revert
//! countdown after a mode or resolution change (so an unsupported mode can't strand the player).
//! The cap itself is kept by `crate::pacing`.

use crate::pacing::{self, Pacing};
use crate::settings::{self, Settings};
use crate::ui::kit::{ToastKind, Toasts};
use crate::DevOpts;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::{Monitor, MonitorSelection, OnMonitor, PresentMode, PrimaryMonitor, PrimaryWindow, VideoMode, VideoModeSelection, WindowMode};

pub struct DisplayPlugin;

impl Plugin for DisplayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DisplayRevert>().add_systems(Update, (apply_display, share_pacing, revert_countdown));
    }
}

/// Display modes as stored in the settings, with their labels.
pub const DISPLAY_MODES: [(&str, &str); 3] = [("windowed", "窗口"), ("borderless", "无边框全屏"), ("fullscreen", "独占全屏")];
/// Frame-rate caps offered when the monitor's refresh rate is unknown (0 = none).
const FPS_CAPS: [u32; 7] = [0, 30, 60, 120, 144, 165, 240];
/// Caps above the refresh rate, offered with vsync off.
const FAST_CAPS: [u32; 4] = [144, 165, 240, 360];

/// Frame-rate caps for this monitor, with labels: whole refreshes per frame (anything in between
/// can't be shown evenly), and with vsync off, rates above the refresh rate too.
pub fn fps_caps(m: Option<&MonitorInfo>, vsync: bool) -> Vec<(u32, String)> {
    let Some(hz) = m.and_then(|m| m.refresh_hz).filter(|hz| *hz >= 30.0) else {
        return FPS_CAPS.iter().map(|&c| (c, if c == 0 { "无限制".into() } else { format!("{c} FPS") })).collect();
    };
    let mut caps = vec![(0, "无限制".to_string())];
    for n in (1..=4).rev() {
        let fps = (hz / n as f32).round() as u32;
        // with vsync, a cap at the refresh rate is the same as none
        if fps >= 24 && !(n == 1 && vsync) {
            caps.push((fps, if n == 1 { format!("{fps} FPS（满）") } else { format!("{fps} FPS（1/{n}）") }));
        }
    }
    if !vsync {
        caps.extend(FAST_CAPS.iter().filter(|&&c| c as f32 > hz * 1.05).map(|&c| (c, format!("{c} FPS"))));
    }
    caps
}
/// Window sizes offered in windowed mode (logical pixels), when they fit on the monitor.
const WINDOW_SIZES: [[u32; 2]; 7] = [[1280, 720], [1366, 768], [1440, 900], [1600, 900], [1680, 1050], [1920, 1080], [2560, 1440]];
/// Seconds before an unconfirmed display change is undone.
pub const REVERT_AFTER: f64 = 15.0;

/// The window when the app starts, built from the saved settings so it opens at the right size
/// and mode instead of jumping after the first frame.
pub fn initial_window(s: &Settings, hidden: bool) -> Window {
    let mode = if hidden { WindowMode::Windowed } else { startup_mode(&s.display) };
    Window {
        title: "方块战车 BLOCKTREAD".into(),
        resolution: (s.window_size[0].max(640), s.window_size[1].max(400)).into(),
        mode,
        present_mode: present_mode(s.vsync),
        visible: !hidden,
        ..default()
    }
}

fn startup_mode(display: &str) -> WindowMode {
    // before the window exists there is no "current" monitor (winit warns and guesses), so it
    // opens on the primary one
    match display {
        "borderless" => WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
        // the exact video mode is picked once the monitor list is known
        "fullscreen" => WindowMode::Fullscreen(MonitorSelection::Primary, VideoModeSelection::Current),
        _ => WindowMode::Windowed,
    }
}

fn present_mode(vsync: bool) -> PresentMode {
    if vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync }
}

// ---------------------------------------------------------------- monitor

/// The monitor the window is on (or the primary one).
#[derive(Clone, Debug)]
pub struct MonitorInfo {
    /// Size in logical pixels (what window sizes are measured in).
    pub logical: Vec2,
    pub physical: UVec2,
    pub refresh_hz: Option<f32>,
    modes: Vec<VideoMode>,
}

#[derive(SystemParam)]
pub struct Monitors<'w, 's> {
    window: Query<'w, 's, Option<&'static OnMonitor>, With<PrimaryWindow>>,
    monitors: Query<'w, 's, (Entity, &'static Monitor, Has<PrimaryMonitor>)>,
}

impl Monitors<'_, '_> {
    pub fn current(&self) -> Option<MonitorInfo> {
        let on = self.window.single().ok().flatten().map(|m| m.0);
        let (_, m, _) = self.monitors.iter().find(|(e, _, _)| Some(*e) == on).or_else(|| self.monitors.iter().find(|(_, _, primary)| *primary)).or_else(|| self.monitors.iter().next())?;
        let scale = m.scale_factor.max(0.5) as f32;
        Some(MonitorInfo {
            logical: Vec2::new(m.physical_width as f32, m.physical_height as f32) / scale,
            physical: UVec2::new(m.physical_width, m.physical_height),
            refresh_hz: m.refresh_rate_millihertz.map(|r| r as f32 / 1000.0),
            modes: m.video_modes.clone(),
        })
    }
}

/// Window sizes that fit on the monitor (leaving room for the title bar and menu bar), plus
/// `current` if the player has a size of their own.
pub fn window_sizes(m: Option<&MonitorInfo>, current: [u32; 2]) -> Vec<[u32; 2]> {
    let mut v: Vec<[u32; 2]> = WINDOW_SIZES.iter().copied().filter(|s| m.is_none_or(|m| s[0] as f32 <= m.logical.x && s[1] as f32 <= m.logical.y - 64.0)).collect();
    if !v.contains(&current) {
        v.push(current);
        v.sort_by_key(|s| (s[0], s[1]));
    }
    v
}

/// The monitor's video-mode resolutions, smallest first (like the window sizes).
pub fn fullscreen_sizes(m: Option<&MonitorInfo>) -> Vec<[u32; 2]> {
    let mut v: Vec<[u32; 2]> = m.map(|m| m.modes.iter().map(|vm| [vm.physical_size.x, vm.physical_size.y]).collect()).unwrap_or_default();
    v.sort_by_key(|s| (s[0], s[1]));
    v.dedup();
    v
}

/// The best video mode at `size`: the highest refresh rate.
fn video_mode(m: &MonitorInfo, size: [u32; 2]) -> Option<VideoMode> {
    m.modes.iter().filter(|vm| vm.physical_size == UVec2::from(size)).max_by_key(|vm| vm.refresh_rate_millihertz).cloned()
}

// ---------------------------------------------------------------- applying

/// Display mode and sizes last pushed to the window, so other settings changes (or the player
/// resizing the window by hand) don't snap it back.
type Applied = (String, [u32; 2], [u32; 2]);

fn apply_display(settings: Res<Settings>, mut window: Single<&mut Window, With<PrimaryWindow>>, monitors: Monitors, dev: Res<DevOpts>, mut applied: Local<Option<Applied>>) {
    if !settings.is_changed() {
        return;
    }
    let present = present_mode(settings.vsync);
    if window.present_mode != present {
        window.present_mode = present;
    }
    let want: Applied = (settings.display.clone(), settings.window_size, settings.fullscreen_size);
    if applied.as_ref() == Some(&want) || dev.offscreen {
        return;
    }
    let monitor = monitors.current();
    let mode = match settings.display.as_str() {
        "borderless" => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
        "fullscreen" => {
            let vm = monitor.as_ref().and_then(|m| video_mode(m, settings.fullscreen_size));
            WindowMode::Fullscreen(MonitorSelection::Current, vm.map_or(VideoModeSelection::Current, VideoModeSelection::Specific))
        }
        _ => WindowMode::Windowed,
    };
    // settings never move the game to another monitor, so which monitor a mode names doesn't
    // count as a change (the window opened on the primary one; changes say "current")
    let differs = match (window.mode, mode) {
        (WindowMode::BorderlessFullscreen(_), WindowMode::BorderlessFullscreen(_)) => false,
        (WindowMode::Fullscreen(_, a), WindowMode::Fullscreen(_, b)) => a != b,
        (a, b) => a != b,
    };
    if differs {
        window.mode = mode;
    }
    if mode == WindowMode::Windowed {
        let [w, h] = settings.window_size;
        window.resolution.set(w as f32, h as f32);
    }
    *applied = Some(want);
}

// ---------------------------------------------------------------- keep or revert

/// A display change waiting for the player to keep it.
#[derive(Resource, Default)]
pub struct DisplayRevert {
    prev: Option<Applied>,
    deadline: f64,
}

impl DisplayRevert {
    /// Remember the current display settings (once per pending change) and restart the clock.
    pub fn begin(&mut self, s: &Settings, now: f64) {
        if self.prev.is_none() {
            self.prev = Some((s.display.clone(), s.window_size, s.fullscreen_size));
        }
        self.deadline = now + REVERT_AFTER;
    }

    pub fn keep(&mut self) {
        self.prev = None;
    }

    /// Put the remembered settings back.
    pub fn revert(&mut self, s: &mut Settings) {
        if let Some((display, window, full)) = self.prev.take() {
            s.display = display;
            s.window_size = window;
            s.fullscreen_size = full;
        }
    }

    /// Seconds left before the change is undone, if one is pending.
    pub fn remaining(&self, now: f64) -> Option<f64> {
        self.prev.as_ref().map(|_| (self.deadline - now).max(0.0))
    }
}

fn revert_countdown(time: Res<Time>, mut rev: ResMut<DisplayRevert>, mut settings: ResMut<Settings>, mut toasts: ResMut<Toasts>, dev: Res<DevOpts>, mut forced: Local<bool>) {
    // `--force revert:secs` starts a countdown with nothing changed, for screenshots of the bar
    if !*forced && dev.forced("revert", time.elapsed_secs()) {
        *forced = true;
        rev.begin(&settings, time.elapsed_secs_f64());
    }
    if rev.remaining(time.elapsed_secs_f64()) == Some(0.0) {
        rev.revert(&mut settings);
        settings::save_settings(&settings);
        toasts.push("没有确认，显示设置已还原", ToastKind::Info);
    }
}

// ---------------------------------------------------------------- frame cap

/// Hand the cap, vsync and the monitor's refresh rate to the pacer (the monitor can only be known
/// once the window is up, so every frame rather than on change).
fn share_pacing(settings: Res<Settings>, monitors: Monitors, pacing: Res<Pacing>) {
    pacing.set(pacing::Config { cap: settings.fps_cap, vsync: settings.vsync, monitor_hz: monitors.current().and_then(|m| m.refresh_hz) });
}
