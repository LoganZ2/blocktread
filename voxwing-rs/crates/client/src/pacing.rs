//! Frame pacing: the frame-rate cap, done so that frames reach the screen evenly.
//!
//! A display shows a new image only on its refresh beat (every 8.3 ms at 120 Hz). A cap that
//! sleeps on a clock of its own drifts against that beat: some frames stay up for one refresh,
//! others for two or three, and motion stutters while the frame counter reads a steady number.
//! So instead:
//!
//! - a cap below the refresh rate is a whole number of refreshes per frame (at 120 Hz: 60, 40,
//!   30), and with vsync on, a cap at or above the refresh rate is left to vsync;
//! - the wait happens on the render thread, just before the frame takes its swapchain image, and
//!   ends just after the refresh *before* the one the frame is meant for. The frame then has a
//!   whole refresh to render and present in, so it lands on its refresh every time: timer jitter
//!   is a tiny fraction of that slack. The beat comes from a CoreVideo display link on macOS;
//!   elsewhere the monitor's nominal rate gives the period but not the phase;
//! - game time advances in whole frame periods, so motion is exactly as even as what reaches the
//!   screen. (Bevy otherwise stamps each frame with the moment rendering finished, which jitters.)

use crate::DevOpts;
use bevy::prelude::*;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::time::{TimeReceiver, TimeSystems, TimeUpdateStrategy};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct PacingPlugin;

impl Plugin for PacingPlugin {
    fn build(&self, app: &mut App) {
        let pacing = Pacing::default();
        app.insert_resource(pacing.clone()).add_systems(First, steady_time.before(TimeSystems)).add_systems(Last, report);
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .insert_resource(pacing)
                .init_resource::<Pacer>()
                .add_systems(Render, wait_for_slot.after(RenderSystems::ExtractCommands).before(RenderSystems::PrepareViews))
                .add_systems(Render, presented.in_set(RenderSystems::Cleanup));
        }
    }
}

/// What the player asked for; the display settings set it every frame.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Config {
    /// Frames per second to hold to; 0 = no cap.
    pub cap: u32,
    pub vsync: bool,
    /// The monitor's refresh rate as the window system reports it, if known.
    pub monitor_hz: Option<f32>,
}

/// Shared by the main world (which sets the config and reads the stats) and the render world
/// (which does the waiting).
#[derive(Resource, Clone, Default)]
pub struct Pacing(Arc<Shared>);

#[derive(Default)]
struct Shared {
    config: Mutex<Config>,
    stats: Mutex<Stats>,
}

impl Pacing {
    pub fn set(&self, c: Config) {
        *self.0.config.lock().unwrap() = c;
    }

    fn config(&self) -> Config {
        *self.0.config.lock().unwrap()
    }
}

/// The display's refresh beat: the most recent vblank and the time between vblanks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    pub vblank: Instant,
    pub period: Duration,
}

impl Beat {
    /// The first vblank strictly after `t`.
    fn after(self, t: Instant) -> Instant {
        let p = self.period.as_secs_f64();
        let x = if t >= self.vblank { (t - self.vblank).as_secs_f64() } else { -(self.vblank - t).as_secs_f64() };
        let k = (x / p).floor() + 1.0;
        if k >= 0.0 { self.vblank + self.period.mul_f64(k) } else { self.vblank - self.period.mul_f64(-k) }
    }

    /// How many refreshes from `a` to `b`, rounded.
    fn refreshes(self, a: Instant, b: Instant) -> u32 {
        (b.saturating_duration_since(a).as_secs_f64() / self.period.as_secs_f64()).round() as u32
    }
}

/// How frames are paced.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Plan {
    /// No cap, or one that vsync already holds: frames go as fast as presentation allows.
    Free,
    /// Every `n`th refresh, locked to the display's vblanks.
    Refreshes { n: u32, beat: Beat },
    /// A fixed interval: when the display's beat isn't known, or for a cap above the refresh
    /// rate with vsync off, where there is no beat to keep to.
    Interval(Duration),
}

fn refresh_period(c: Config, beat: Option<Beat>) -> Option<Duration> {
    beat.map(|b| b.period).or_else(|| c.monitor_hz.filter(|hz| *hz > 1.0).map(|hz| Duration::from_secs_f64(1.0 / hz as f64)))
}

fn plan(c: Config, beat: Option<Beat>) -> Plan {
    if c.cap == 0 {
        return Plan::Free;
    }
    let frame = Duration::from_secs_f64(1.0 / c.cap as f64);
    let Some(refresh) = refresh_period(c, beat) else { return Plan::Interval(frame) };
    let ratio = frame.as_secs_f64() / refresh.as_secs_f64();
    // above the refresh rate there is no beat to keep to (2% slack for rates like 59.94 Hz)
    if ratio < 0.98 {
        return if c.vsync { Plan::Free } else { Plan::Interval(frame) };
    }
    // the fewest whole refreshes per frame that stay at or under the cap
    let n = (ratio - 0.02).ceil().max(1.0) as u32;
    match (n, beat) {
        (1, _) if c.vsync => Plan::Free,
        (n, Some(beat)) => Plan::Refreshes { n, beat },
        (n, None) => Plan::Interval(refresh * n),
    }
}

/// The step game time advances in: the frame period when frames come at a steady rate.
fn steady_step(c: Config, beat: Option<Beat>) -> Option<Duration> {
    match plan(c, beat) {
        Plan::Refreshes { n, beat } => Some(beat.period * n),
        Plan::Interval(d) => Some(d),
        Plan::Free if c.vsync => refresh_period(c, beat),
        Plan::Free => None,
    }
}

// ---------------------------------------------------------------- render world: the wait

/// A little slack after a vblank before the next frame starts, for the beat's timing error.
const AFTER_VBLANK: Duration = Duration::from_micros(500);

#[derive(Resource, Default)]
struct Pacer {
    /// The vblank the last frame was aimed at (refresh pacing).
    shown: Option<Instant>,
    /// When the next frame may start (interval pacing).
    next: Option<Instant>,
    last_present: Option<Instant>,
    last_slot: Option<Instant>,
}

fn wait_for_slot(pacing: Res<Pacing>, mut p: ResMut<Pacer>) {
    let now = Instant::now();
    match plan(pacing.config(), vblank::beat()) {
        Plan::Free => {
            p.shown = None;
            p.next = None;
        }
        Plan::Refreshes { n, beat } => {
            p.next = None;
            // n refreshes after the last frame's; if that is too close to make, the first
            // refresh with at least half a refresh to spare
            let want = p.shown.map_or_else(|| beat.after(now), |s| s + beat.period * n);
            let show = if want < now + beat.period / 2 { beat.after(now + beat.period / 2) } else { want };
            p.shown = Some(show);
            // start right after the refresh before it: a whole refresh to render and present
            sleep_until(show - beat.period + AFTER_VBLANK);
        }
        Plan::Interval(period) => {
            p.shown = None;
            let start = p.next.unwrap_or(now);
            sleep_until(start);
            // whole frames from the last start, so the rate doesn't drift; after a hitch the
            // schedule restarts instead of rushing to catch up
            let done = Instant::now();
            p.next = Some(if done > start + period { done + period } else { start + period });
        }
    }
}

/// Sleep most of the wait (the OS wakes us up to a millisecond late), spin the rest.
fn sleep_until(t: Instant) {
    let now = Instant::now();
    if t <= now {
        return;
    }
    let wait = t - now;
    if wait > Duration::from_micros(1500) {
        std::thread::sleep(wait - Duration::from_millis(1));
    }
    while Instant::now() < t {
        std::hint::spin_loop();
    }
}

/// After presenting: note when, for the stats.
fn presented(pacing: Res<Pacing>, mut p: ResMut<Pacer>) {
    let now = Instant::now();
    let beat = vblank::beat();
    // the refresh this frame should appear on: the next one, for a frame presented this early
    let slot = beat.map(|b| b.after(now));
    let mut s = pacing.0.stats.lock().unwrap();
    if let Some(last) = p.last_present {
        s.frame(now - last);
    }
    if let (Some(b), Some(a), Some(z)) = (beat, p.last_slot, slot) {
        s.slot(b.refreshes(a, z));
    }
    p.last_present = Some(now);
    p.last_slot = slot;
}

// ---------------------------------------------------------------- main world: steady game time

/// Advance game time in whole frame periods while frames come at a steady rate, nudged towards
/// the real clock so it never drifts. Off the steady rate (a hitch, no cap), real time it is.
fn steady_time(recv: Option<Res<TimeReceiver>>, pacing: Res<Pacing>, mut strategy: ResMut<TimeUpdateStrategy>, mut last: Local<Option<Instant>>) {
    let Some(recv) = recv else { return };
    // the render world's stamp for the last finished frame; taken here so Bevy's time system
    // never mixes its own reading in
    let mut raw = None;
    while let Ok(t) = recv.0.try_recv() {
        raw = Some(t);
    }
    let raw = raw.unwrap_or_else(Instant::now);
    let c = pacing.config();
    let t = match (steady_step(c, vblank::beat()), *last) {
        (Some(step), Some(prev)) if raw > prev => {
            let k = ((raw - prev).as_secs_f64() / step.as_secs_f64()).round();
            let ideal = prev + step.mul_f64(k.max(1.0));
            let off = if raw >= ideal { (raw - ideal).as_secs_f64() } else { -(ideal - raw).as_secs_f64() };
            if k >= 1.0 && off.abs() < step.as_secs_f64() * 0.3 {
                let nudge = off * 0.02;
                if nudge >= 0.0 { ideal + Duration::from_secs_f64(nudge) } else { ideal - Duration::from_secs_f64(-nudge) }
            } else {
                raw
            }
        }
        _ => raw,
    };
    let t = last.map_or(t, |prev| t.max(prev));
    if let Some(prev) = *last {
        pacing.0.stats.lock().unwrap().step(t - prev);
    }
    *last = Some(t);
    *strategy = TimeUpdateStrategy::ManualInstant(t);
}

// ---------------------------------------------------------------- stats (--frame-stats)

#[derive(Default)]
struct Stats {
    frames: Vec<f64>,
    steps: Vec<f64>,
    /// How many refreshes each frame stayed on screen, counted.
    slots: std::collections::BTreeMap<u32, u32>,
}

impl Stats {
    fn frame(&mut self, d: Duration) {
        self.frames.push(d.as_secs_f64() * 1000.0);
    }
    fn step(&mut self, d: Duration) {
        self.steps.push(d.as_secs_f64() * 1000.0);
    }
    fn slot(&mut self, n: u32) {
        *self.slots.entry(n).or_insert(0) += 1;
    }
}

fn spread(v: &[f64]) -> (f64, f64, f64) {
    let n = v.len().max(1) as f64;
    let mean = v.iter().sum::<f64>() / n;
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    (mean, sd, v.iter().copied().fold(0.0, f64::max))
}

/// Every 5 seconds under `--frame-stats`: frame intervals, game-time steps, and how many
/// refreshes frames stayed on screen (uneven counts are what reads as stutter).
fn report(dev: Res<DevOpts>, pacing: Res<Pacing>, time: Res<Time<Real>>, mut next: Local<f32>) {
    if !dev.frame_stats || time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 5.0;
    let c = pacing.config();
    let beat = vblank::beat();
    let mut s = pacing.0.stats.lock().unwrap();
    let (fm, fsd, fmax) = spread(&s.frames);
    let (_, ssd, smax) = spread(&s.steps);
    let total: u32 = s.slots.values().sum();
    let common = s.slots.values().copied().max().unwrap_or(0);
    let slots: Vec<String> = s.slots.iter().map(|(k, v)| format!("{k}×{v}")).collect();
    info!(
        "[pacing] cap {} vsync {} · refresh {} · {} · frame {fm:.2} ms ± {fsd:.2} (max {fmax:.1}) · game step ± {ssd:.2} (max {smax:.1}) · on screen for [{}] refreshes, uneven {:.1}%",
        c.cap,
        c.vsync,
        beat.map_or("?".into(), |b| format!("{:.2} ms", b.period.as_secs_f64() * 1000.0)),
        match plan(c, beat) {
            Plan::Free => "free".to_string(),
            Plan::Refreshes { n, .. } => format!("every {n} refreshes"),
            Plan::Interval(d) => format!("every {:.2} ms", d.as_secs_f64() * 1000.0),
        },
        slots.join(" "),
        if total > 0 { (total - common) as f64 * 100.0 / total as f64 } else { 0.0 },
    );
    *s = Stats::default();
}

// ---------------------------------------------------------------- the refresh beat

#[cfg(target_os = "macos")]
mod vblank {
    //! A CoreVideo display link calls back on every vblank with that vblank's host time; the
    //! latest one and the refresh period are kept in atomics for the pacer to read.

    use super::Beat;
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    #[repr(C)]
    struct SmpteTime {
        subframes: i16,
        subframe_divisor: i16,
        counter: u32,
        kind: u32,
        flags: u32,
        hours: i16,
        minutes: i16,
        seconds: i16,
        frames: i16,
    }

    #[repr(C)]
    struct TimeStamp {
        version: u32,
        video_time_scale: i32,
        video_time: i64,
        host_time: u64,
        rate_scalar: f64,
        video_refresh_period: i64,
        smpte_time: SmpteTime,
        flags: u64,
        reserved: u64,
    }

    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }

    type Link = *mut c_void;
    type Callback = extern "C" fn(Link, *const TimeStamp, *const TimeStamp, u64, *mut u64, *mut c_void) -> i32;

    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        fn CVDisplayLinkCreateWithActiveCGDisplays(link: *mut Link) -> i32;
        fn CVDisplayLinkSetOutputCallback(link: Link, callback: Callback, context: *mut c_void) -> i32;
        fn CVDisplayLinkStart(link: Link) -> i32;
    }

    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    /// Host time (mach ticks) and `Instant` read together, to convert between them.
    struct Clock {
        base: Instant,
        base_ticks: u64,
        numer: u128,
        denom: u128,
    }

    static CLOCK: OnceLock<Clock> = OnceLock::new();
    static STARTED: OnceLock<bool> = OnceLock::new();
    /// The latest vblank in nanoseconds after `Clock::base` (0 = none yet), and the period.
    static LAST_NS: AtomicU64 = AtomicU64::new(0);
    static PERIOD_NS: AtomicU64 = AtomicU64::new(0);

    extern "C" fn on_vblank(_: Link, now: *const TimeStamp, _: *const TimeStamp, _: u64, _: *mut u64, _: *mut c_void) -> i32 {
        let (Some(c), false) = (CLOCK.get(), now.is_null()) else { return 0 };
        // SAFETY: CoreVideo passes a valid timestamp for the duration of the callback
        let ts = unsafe { &*now };
        if ts.host_time < c.base_ticks {
            return 0;
        }
        let ns = ((ts.host_time - c.base_ticks) as u128 * c.numer / c.denom) as u64;
        let prev = LAST_NS.swap(ns.max(1), Ordering::Release);
        // the nominal period of the current mode, else the measured one
        let nominal = if ts.video_time_scale > 0 && ts.video_refresh_period > 0 { (ts.video_refresh_period as u128 * 1_000_000_000 / ts.video_time_scale as u128) as u64 } else { 0 };
        let period = if nominal > 0 { nominal } else if prev > 0 && ns > prev { ns - prev } else { 0 };
        if period > 0 {
            PERIOD_NS.store(period, Ordering::Release);
        }
        0
    }

    fn start() -> bool {
        CLOCK.get_or_init(|| {
            let mut tb = Timebase { numer: 1, denom: 1 };
            // SAFETY: plain out-parameter calls into libSystem
            let ticks = unsafe {
                mach_timebase_info(&mut tb);
                mach_absolute_time()
            };
            Clock { base: Instant::now(), base_ticks: ticks, numer: tb.numer.max(1) as u128, denom: tb.denom.max(1) as u128 }
        });
        let mut link: Link = std::ptr::null_mut();
        // SAFETY: the link is created, given a callback that only touches atomics, and started;
        // it is never released, so it lives (and calls back) for the rest of the process
        unsafe { CVDisplayLinkCreateWithActiveCGDisplays(&mut link) == 0 && !link.is_null() && CVDisplayLinkSetOutputCallback(link, on_vblank, std::ptr::null_mut()) == 0 && CVDisplayLinkStart(link) == 0 }
    }

    pub fn beat() -> Option<Beat> {
        if !*STARTED.get_or_init(start) {
            return None;
        }
        let c = CLOCK.get()?;
        let (last, period) = (LAST_NS.load(Ordering::Acquire), PERIOD_NS.load(Ordering::Acquire));
        if last == 0 || period == 0 {
            return None;
        }
        let beat = Beat { vblank: c.base + Duration::from_nanos(last), period: Duration::from_nanos(period) };
        // a link that stopped ticking (display asleep) says nothing about now
        (Instant::now().saturating_duration_since(beat.vblank) < beat.period * 4).then_some(beat)
    }
}

#[cfg(not(target_os = "macos"))]
mod vblank {
    //! No display link here: the monitor's nominal rate gives the period but not the phase.
    pub fn beat() -> Option<super::Beat> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beat() -> Beat {
        Beat { vblank: Instant::now(), period: Duration::from_micros(8333) }
    }

    #[test]
    fn caps_become_whole_refreshes() {
        let b = beat();
        let c = |cap, vsync| Config { cap, vsync, monitor_hz: Some(120.0) };
        assert_eq!(plan(c(0, true), Some(b)), Plan::Free);
        // at or above the refresh rate, vsync holds the cap by itself
        assert_eq!(plan(c(120, true), Some(b)), Plan::Free);
        assert_eq!(plan(c(144, true), Some(b)), Plan::Free);
        assert_eq!(plan(c(60, true), Some(b)), Plan::Refreshes { n: 2, beat: b });
        assert_eq!(plan(c(40, false), Some(b)), Plan::Refreshes { n: 3, beat: b });
        assert_eq!(plan(c(30, true), Some(b)), Plan::Refreshes { n: 4, beat: b });
        // between divisors: the next whole refresh count that stays under the cap
        assert_eq!(plan(c(100, true), Some(b)), Plan::Refreshes { n: 2, beat: b });
        // vsync off: the refresh rate itself is locked to the beat, above it a plain interval
        assert_eq!(plan(c(120, false), Some(b)), Plan::Refreshes { n: 1, beat: b });
        assert!(matches!(plan(c(240, false), Some(b)), Plan::Interval(_)));
        // no beat: whole refreshes of the monitor's nominal rate
        assert_eq!(plan(c(60, true), None), Plan::Interval(Duration::from_secs_f64(1.0 / 120.0) * 2));
    }

    #[test]
    fn vblanks_line_up() {
        let b = beat();
        assert_eq!(b.after(b.vblank), b.vblank + b.period);
        assert_eq!(b.after(b.vblank + Duration::from_millis(1)), b.vblank + b.period);
        assert_eq!(b.after(b.vblank + b.period * 3 + Duration::from_micros(10)), b.vblank + b.period * 4);
        assert_eq!(b.after(b.vblank - Duration::from_millis(1)), b.vblank);
        assert_eq!(b.after(b.vblank - b.period - Duration::from_millis(1)), b.vblank - b.period);
        assert_eq!(b.refreshes(b.vblank, b.vblank + b.period * 2), 2);
    }
}
