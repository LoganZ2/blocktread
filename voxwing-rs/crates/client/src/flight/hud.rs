//! Flight HUD. Static instrument panels are built from the UI kit; markers (brackets, lead pip,
//! radar dots, arrows) are pooled nodes repositioned every frame. Overlays (connecting, pause,
//! death, scoreboard) come and go with the session state.

use super::Session;
use crate::Screen;
use crate::settings::Settings;
use crate::ui::kit::*;
use crate::ui::*;
use bevy::prelude::*;
use bevy::text::LetterSpacing;
use bevy::window::PrimaryWindow;
use voxwing_client_core::ConnState;
use voxwing_shared::blocks::BlockType;
use voxwing_shared::flight::InputCmd;
use voxwing_shared::protocol::{Mode, ScoreRow};
use voxwing_shared::weapons::intercept;

pub const LOCK_T: f32 = 0.9;
const BRACKETS: usize = 16;
const DOTS: usize = 48;
const FEED: usize = 5;
const WARNS: usize = 3;
const SHIELD: Color = Color::srgb(0.4, 0.62, 1.0);
/// The off-screen arrow: half its base, and its height.
const ARROW_W: f32 = 11.0;
const ARROW_H: f32 = 18.0;

/// What the last of your rounds did, for the marker around the crosshair.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum HitKind {
    #[default]
    Hull,
    Shield,
    Kill,
}

#[derive(Resource, Default)]
pub struct Hud {
    ann: (String, String, f32),
    feed: Vec<(String, bool, f32)>,
    pub dmg_flash: f32,
    pub shield_flash: f32,
    pub silo_dirty: bool,
    help_t: f32,
    show_help: bool,
    /// The hit marker: what it shows and how much of it is left (1 → 0).
    hit: (HitKind, f32),
    /// The ship your last round landed on, and how much its bracket still flashes.
    hit_on: (u16, f32),
}

impl Hud {
    pub fn announce(&mut self, t: &str, s: &str) {
        self.ann = (t.to_string(), s.to_string(), 3.2);
    }
    pub fn feed(&mut self, t: &str, bad: bool) {
        self.feed.insert(0, (t.to_string(), bad, 4.5));
        self.feed.truncate(FEED);
    }
    /// One of your rounds landed on `target`, or finished it off.
    pub fn hit(&mut self, target: u16, kind: HitKind) {
        // the hits that land right after a kill don't cut its marker short
        if kind == HitKind::Kill || self.hit.0 != HitKind::Kill || self.hit.1 <= 0.0 {
            self.hit = (kind, 1.0);
        }
        self.hit_on = (target, 1.0);
    }
}

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum El {
    Mode,
    Score,
    Info,
    Feed(usize),
    Ann,
    AnnTitle,
    AnnSub,
    Fill(u8),
    BarText(u8),
    Speed,
    Throttle,
    Weapons,
    Warn(usize),
    Radar,
    Dot(usize),
    Cross,
    /// Four ticks around the crosshair when your rounds land; `HitTick(0..4)` are the ticks.
    HitMark,
    HitTick(u8),
    Stick,
    Bracket(usize),
    BracketText(usize),
    BracketHp(usize),
    Lead,
    Lock,
    Arrow(usize),
    VigRed,
    VigBlue,
    Silo,
    Help,
    Instruments,
    /// The ship's own gauges (hull, speed, weapons): hidden while a spectator has no ship to read.
    Gauges,
    /// Spectator banner: who is being watched, their score line.
    Watch,
    WatchName,
    WatchStat,
}

fn abs(left: f32, top: f32) -> Node {
    Node { position_type: PositionType::Absolute, left: px(left), top: px(top), ..default() }
}

/// Screen-edge glow for hits (red) and shield hits (teal): clear centre, tinted edges.
fn vignette(c: Color, a: f32) -> BackgroundGradient {
    BackgroundGradient::from(RadialGradient::new(
        UiPosition::CENTER,
        RadialGradientShape::FarthestCorner,
        vec![ColorStop::new(Color::NONE, percent(45)), ColorStop::new(c.with_alpha(a * 0.4), percent(75)), ColorStop::new(c.with_alpha(a), percent(100))],
    ))
}

/// Fade the hit vignettes; only rebuilds a gradient when its strength visibly changes.
pub fn update_vignettes(hud: Res<Hud>, mut q: Query<(&El, &mut BackgroundGradient)>, mut last: Local<[f32; 2]>) {
    for (el, mut g) in &mut q {
        let (i, c, a) = match el {
            El::VigRed => (0, DANGER, hud.dmg_flash * 0.6),
            El::VigBlue => (1, ACCENT, hud.shield_flash * 0.45),
            _ => continue,
        };
        let a = if a < 0.01 { 0.0 } else { a };
        if (a - last[i]).abs() > 0.004 || (a == 0.0 && last[i] != 0.0) {
            last[i] = a;
            *g = vignette(c, a);
        }
    }
}

/// The HUD's top node.
#[derive(Component)]
pub struct HudRoot;

/// Dev: `--force nohud` hides the whole HUD, for clean screenshots of the scene.
pub fn dev_hide_hud(dev: Res<crate::DevOpts>, time: Res<Time>, mut q: Query<&mut Visibility, With<HudRoot>>) {
    if dev.forced("nohud", time.elapsed_secs()) {
        for mut v in &mut q {
            *v = Visibility::Hidden;
        }
    }
}

pub fn spawn_hud(mut commands: Commands, font: Res<UiFont>, mut hud: ResMut<Hud>, settings: Res<Settings>) {
    *hud = Hud { silo_dirty: true, help_t: 12.0, show_help: settings.show_help, ..default() };
    let f = &*font;
    let root = commands.spawn((Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, ..default() }, Pickable::IGNORE, IgnoreHover, HudRoot, DespawnOnExit(Screen::Flight))).id();
    for (el, c) in [(El::VigRed, DANGER), (El::VigBlue, ACCENT)] {
        commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, vignette(c, 0.0), Pickable::IGNORE, el, ChildOf(root)));
    }

    // mission panel, top left
    let tl = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), top: px(EDGE), padding: px(16).all(), flex_direction: FlexDirection::Column, row_gap: px(4), min_width: px(256), ..default() });
    commands.entity(tl).insert((Pickable::IGNORE, El::Instruments));
    commands.spawn((heading(f, "", 18.0, INK), El::Mode, Pickable::IGNORE, ChildOf(tl)));
    commands.spawn((tech(f, "", 16.0, SIGNAL), El::Score, Pickable::IGNORE, ChildOf(tl)));
    commands.spawn((text(f, "", 12.0, MUTED), El::Info, Pickable::IGNORE, ChildOf(tl)));

    // kill feed, top right
    let tr = commands.spawn((Node { position_type: PositionType::Absolute, right: px(EDGE), top: px(EDGE), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, row_gap: px(4), ..default() }, Pickable::IGNORE, ChildOf(root))).id();
    for i in 0..FEED {
        commands.spawn((
            Node { padding: UiRect::axes(px(10), px(4)), border: UiRect::right(px(3)), ..default() },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
            El::Feed(i),
            Pickable::IGNORE,
            ChildOf(tr),
            children![(text(f, "", 14.0, INK), Pickable::IGNORE)],
        ));
    }

    // announcement: title between two accent rules
    let ann = commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: percent(18), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(6), ..default() }, El::Ann, Pickable::IGNORE, ChildOf(root))).id();
    let tr2 = commands.spawn((row(18.0), Pickable::IGNORE, ChildOf(ann))).id();
    commands.spawn((Node { width: px(70), height: px(2), ..default() }, BackgroundColor(ACCENT), Pickable::IGNORE, ChildOf(tr2)));
    commands.spawn((display(f, "", 64.0, INK), LetterSpacing::Px(6.0), El::AnnTitle, Pickable::IGNORE, ChildOf(tr2)));
    commands.spawn((Node { width: px(70), height: px(2), ..default() }, BackgroundColor(ACCENT), Pickable::IGNORE, ChildOf(tr2)));
    commands.spawn((text(f, "", 16.0, ACCENT), El::AnnSub, Pickable::IGNORE, ChildOf(ann)));

    // hull instruments, bottom left
    let bl = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), bottom: px(EDGE), padding: px(16).all(), column_gap: px(16), align_items: AlignItems::FlexEnd, ..default() });
    commands.entity(bl).insert((Pickable::IGNORE, El::Gauges));
    commands.spawn((Node { width: px(140), height: px(140), ..default() }, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.25)), El::Silo, Pickable::IGNORE, ChildOf(bl)));
    let bars = commands.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(9), width: px(210), ..default() }, Pickable::IGNORE, ChildOf(bl))).id();
    for (i, (label, c)) in [("车体", OK), ("护盾", SHIELD), ("能量", WARN)].into_iter().enumerate() {
        let r = commands.spawn((col(3.0), Pickable::IGNORE, ChildOf(bars))).id();
        let head = commands.spawn((Node { justify_content: JustifyContent::SpaceBetween, ..default() }, Pickable::IGNORE, ChildOf(r))).id();
        commands.spawn((text(f, label, 12.0, MUTED), Pickable::IGNORE, ChildOf(head)));
        commands.spawn((tech(f, "", 13.0, INK), El::BarText(i as u8), Pickable::IGNORE, ChildOf(head)));
        commands.spawn((Node { height: px(8), ..default() }, Plate::gauge(c, 30.0, 1.0), El::Fill(i as u8), Pickable::IGNORE, ChildOf(r)));
    }

    // flight instruments, bottom right
    let br = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, right: px(EDGE), bottom: px(EDGE), padding: px(16).all(), column_gap: px(16), align_items: AlignItems::FlexEnd, ..default() });
    commands.entity(br).insert((Pickable::IGNORE, El::Gauges));
    let c = commands.spawn((Node { flex_direction: FlexDirection::Column, align_items: AlignItems::FlexEnd, row_gap: px(6), min_width: px(150), ..default() }, Pickable::IGNORE, ChildOf(br))).id();
    commands.spawn((tech(f, "SPD  km/h", 12.0, MUTED), LetterSpacing::Px(3.0), Pickable::IGNORE, ChildOf(c)));
    commands.spawn((tech(f, "0", 48.0, ACCENT), El::Speed, Pickable::IGNORE, ChildOf(c)));
    commands.spawn((text(f, "", 12.0, MUTED), El::Weapons, TextLayout::justify(Justify::Right), Pickable::IGNORE, ChildOf(c)));
    let thr = commands.spawn((Node { width: px(10), height: px(118), border: px(1).all(), flex_direction: FlexDirection::Column, justify_content: JustifyContent::FlexEnd, ..default() }, BorderColor::all(LINE_HI), Pickable::IGNORE, ChildOf(br))).id();
    commands.spawn((Node { width: percent(100), height: percent(70), ..default() }, BackgroundColor(ACCENT), El::Throttle, Pickable::IGNORE, ChildOf(thr)));

    // warnings above the radar
    let wc = commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(140.0 + EDGE + GAP), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(4), ..default() }, Pickable::IGNORE, ChildOf(root))).id();
    for i in 0..WARNS {
        commands.spawn((
            Node { padding: UiRect::axes(px(12), px(3)), border: UiRect::left(px(3)), ..default() },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::NONE),
            El::Warn(i),
            Pickable::IGNORE,
            ChildOf(wc),
            children![(text(f, "", 14.0, WARN), Pickable::IGNORE)],
        ));
    }

    // radar
    let radar = commands
        .spawn((Node { position_type: PositionType::Absolute, width: px(140), height: px(140), border: px(1).all(), border_radius: BorderRadius::MAX, ..default() }, BackgroundColor(Color::srgba(0.01, 0.03, 0.05, 0.6)), BorderColor::all(ACCENT.with_alpha(0.35)), El::Radar, Pickable::IGNORE, ChildOf(root)))
        .id();
    commands.spawn((Node { position_type: PositionType::Absolute, left: px(69), top: px(4), width: px(1), height: px(132), ..default() }, BackgroundColor(LINE), Pickable::IGNORE, ChildOf(radar)));
    commands.spawn((Node { position_type: PositionType::Absolute, top: px(69), left: px(4), height: px(1), width: px(132), ..default() }, BackgroundColor(LINE), Pickable::IGNORE, ChildOf(radar)));
    commands.spawn((Node { position_type: PositionType::Absolute, left: px(35), top: px(35), width: px(70), height: px(70), border: px(1).all(), border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(LINE), Pickable::IGNORE, ChildOf(radar)));
    commands.spawn((Node { position_type: PositionType::Absolute, left: px(66), top: px(64), width: px(8), height: px(10), ..default() }, BackgroundColor(ACCENT), Pickable::IGNORE, ChildOf(radar)));
    for i in 0..DOTS {
        commands.spawn((Node { position_type: PositionType::Absolute, width: px(6), height: px(6), ..default() }, BackgroundColor(DANGER), Visibility::Hidden, El::Dot(i), Pickable::IGNORE, ChildOf(radar)));
    }

    // reticles and markers
    let put = |commands: &mut Commands, n: Node, el: El| commands.spawn((n, el, Pickable::IGNORE, ChildOf(root))).id();
    let cross = put(&mut commands, Node { width: px(32), height: px(32), border: px(2).all(), border_radius: BorderRadius::MAX, ..abs(0.0, 0.0) }, El::Cross);
    commands.entity(cross).insert(BorderColor::all(ACCENT));
    commands.spawn((Node { position_type: PositionType::Absolute, left: px(13), top: px(13), width: px(2), height: px(2), ..default() }, BackgroundColor(ACCENT), Pickable::IGNORE, ChildOf(cross)));
    // hit marker: a tick at each edge of a box turned 45°, so they form an X around the crosshair
    let mark = put(&mut commands, Node { width: px(0), height: px(0), ..abs(0.0, 0.0) }, El::HitMark);
    commands.entity(mark).insert((Visibility::Hidden, UiTransform { rotation: Rot2::degrees(45.0), ..default() }));
    for i in 0..4u8 {
        let n = match i {
            0 => Node { left: percent(50), top: px(0), ..abs(0.0, 0.0) },
            1 => Node { left: percent(50), bottom: px(0), top: Val::Auto, ..abs(0.0, 0.0) },
            2 => Node { top: percent(50), left: px(0), ..abs(0.0, 0.0) },
            _ => Node { top: percent(50), right: px(0), left: Val::Auto, ..abs(0.0, 0.0) },
        };
        commands.spawn((n, BackgroundColor(INK), El::HitTick(i), Pickable::IGNORE, ChildOf(mark)));
    }
    let stick = put(&mut commands, Node { width: px(12), height: px(12), border: px(1).all(), border_radius: BorderRadius::MAX, ..abs(0.0, 0.0) }, El::Stick);
    commands.entity(stick).insert(BorderColor::all(ACCENT.with_alpha(0.6)));
    let lead = put(&mut commands, Node { width: px(12), height: px(12), border: px(2).all(), ..abs(0.0, 0.0) }, El::Lead);
    commands.entity(lead).insert((BorderColor::all(SIGNAL), Visibility::Hidden, UiTransform { rotation: Rot2::degrees(45.0), ..default() }));
    let lock = put(&mut commands, Node { width: px(40), height: px(40), border: px(2).all(), ..abs(0.0, 0.0) }, El::Lock);
    commands.entity(lock).insert((BorderColor::all(SIGNAL), Visibility::Hidden, UiTransform { rotation: Rot2::degrees(45.0), ..default() }));
    for i in 0..BRACKETS {
        let b = put(&mut commands, Node { width: px(30), height: px(30), ..abs(0.0, 0.0) }, El::Bracket(i));
        commands.entity(b).insert(Visibility::Hidden);
        // four corner ticks instead of a full box
        for (l, t, border) in [
            (true, true, UiRect { left: px(2), top: px(2), ..default() }),
            (false, true, UiRect { right: px(2), top: px(2), ..default() }),
            (true, false, UiRect { left: px(2), bottom: px(2), ..default() }),
            (false, false, UiRect { right: px(2), bottom: px(2), ..default() }),
        ] {
            commands.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(30),
                    height: percent(30),
                    left: if l { px(0) } else { Val::Auto },
                    right: if l { Val::Auto } else { px(0) },
                    top: if t { px(0) } else { Val::Auto },
                    bottom: if t { Val::Auto } else { px(0) },
                    border,
                    ..default()
                },
                BorderColor::all(DANGER),
                Pickable::IGNORE,
                ChildOf(b),
            ));
        }
        commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(-8), height: px(3), ..default() }, BackgroundColor(DANGER), El::BracketHp(i), Pickable::IGNORE, ChildOf(b)));
        commands.spawn((text(f, "", 12.0, DANGER), Node { position_type: PositionType::Absolute, top: percent(100), margin: UiRect::top(px(10)), ..default() }, TextLayout::no_wrap(), El::BracketText(i), Pickable::IGNORE, ChildOf(b)));
        let a = put(&mut commands, Node { width: px(0), height: px(0), border: UiRect { left: px(ARROW_W), right: px(ARROW_W), bottom: px(ARROW_H), ..default() }, ..abs(0.0, 0.0) }, El::Arrow(i));
        commands.entity(a).insert((BorderColor { left: Color::NONE, right: Color::NONE, top: Color::NONE, bottom: DANGER }, Visibility::Hidden, UiTransform::default()));
    }

    // spectator banner, top centre (where the controls help sits for pilots)
    let watch = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: percent(50), top: px(EDGE), margin: UiRect::left(px(-240)), width: px(480), padding: UiRect::axes(px(PAD), px(14)), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(6), ..default() });
    commands.entity(watch).insert((El::Watch, Pickable::IGNORE, Visibility::Hidden));
    let line = commands.spawn((row(12.0), Pickable::IGNORE, ChildOf(watch))).id();
    commands.spawn((chip(f, "观战", ACCENT), Pickable::IGNORE, ChildOf(line)));
    commands.spawn((heading(f, "", 20.0, INK), El::WatchName, Pickable::IGNORE, ChildOf(line)));
    commands.spawn((tech(f, "", 14.0, SIGNAL), El::WatchStat, Pickable::IGNORE, ChildOf(watch)));
    commands.spawn((text(f, "← → 换一辆 · C 驾驶室视角 · Tab 记分板 · Esc 菜单", 12.0, MUTED), Pickable::IGNORE, ChildOf(watch)));

    // controls help
    let help = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: percent(50), top: px(EDGE), margin: UiRect::left(px(-250)), width: px(500), padding: px(16).all(), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() });
    commands.entity(help).insert((El::Help, Pickable::IGNORE, solid_plate()));
    spawn_header(&mut commands, f, help, "操作|CONTROLS · H 隐藏");
    for (k, v) in [
        ("W / S", "前进 / 倒车（反着按就是刹车）"),
        ("A / D", "左右转向"),
        ("鼠标", "转动镜头"),
        ("Shift", "加速（消耗能量）"),
        ("左键 / 空格", "开火（炮口朝车头方向）"),
        ("右键 / F", "发射导弹（车头对准目标一会儿即可锁定）"),
        ("R（按住）", "翻车后把车翻回来"),
        ("T · C · Tab", "切换目标 · 切换视角 · 记分板"),
        ("Esc", "暂停菜单"),
    ] {
        commands.spawn((row(12.0), Pickable::IGNORE, ChildOf(help), children![(tech(f, k, 13.0, ACCENT), Node { width: px(100), ..default() }), text(f, v, 13.0, INK)]));
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update_hud(
    mut commands: Commands,
    mut hud: ResMut<Hud>,
    session: Res<Session>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    cam: Single<(&Camera, &GlobalTransform), With<crate::MainCamera>>,
    mut els: Query<(Entity, &El, &mut Node, &mut Visibility, Option<&mut BackgroundColor>, Option<&mut BorderColor>, Option<&mut UiTransform>, Option<&Children>, Option<&mut Plate>)>,
    mut texts: Query<(&mut Text, &mut TextColor)>,
    mut corners: Query<&mut BorderColor, Without<El>>,
    dev: Res<crate::DevOpts>,
) {
    let dt = time.delta_secs();
    if keys.just_pressed(KeyCode::KeyH) {
        hud.show_help = !hud.show_help;
        hud.help_t = if hud.show_help { 1e6 } else { 0.0 };
    }
    hud.help_t -= dt;
    hud.ann.2 -= dt;
    for f in hud.feed.iter_mut() {
        f.2 -= dt;
    }
    hud.feed.retain(|f| f.2 > 0.0);
    hud.dmg_flash *= (-dt * 3.0).exp();
    hud.shield_flash *= (-dt * 3.0).exp();
    let hold = if hud.hit.0 == HitKind::Kill { 0.7 } else { 0.28 };
    hud.hit.1 = (hud.hit.1 - dt / hold).max(0.0);
    hud.hit_on.1 = (hud.hit_on.1 - dt / 0.15).max(0.0);
    // dev: hold a marker up for screenshots
    for (k, kind) in [("hit", HitKind::Hull), ("shieldhit", HitKind::Shield), ("kill", HitKind::Kill)] {
        if dev.forced(k, time.elapsed_secs()) {
            hud.hit = (kind, 0.9);
        }
    }
    let core = &session.core;
    let (camera, cgt) = *cam;
    let (w, h) = (window.width(), window.height());
    let me = core.my();
    // the ship the instruments read: your own, or the one you are spectating
    let view = session.view();
    let in_game = core.state == ConnState::InGame;
    let inv = cgt.affine().inverse();
    let screen = |p: Vec3| -> (Vec2, bool) {
        let v = inv.transform_point3(p);
        let behind = v.z > 0.0;
        match camera.world_to_viewport(cgt, p) {
            Ok(s) if !behind => (s, false),
            _ => (Vec2::new(w / 2.0, h / 2.0) + Vec2::new(v.x, -v.y).normalize_or(Vec2::Y) * 10000.0, true),
        }
    };
    // the crosshair: where rounds fired now are when they reach the aim distance, straight out
    // along the nose and carried along by the ship's own velocity, which they inherit (a sliding
    // turn takes them off a plain boresight mark)
    let aim = view.map(|m| {
        let own = me.is_some_and(|x| x.id == m.id);
        let d = core.pred.as_ref().filter(|_| own).and_then(|p| p.last_cmd()).map_or(InputCmd::default().aim_dist, |c| c.aim_dist).max(30) as f32;
        let t = d / m.body.bolt_speed.max(1.0);
        screen(m.pos + m.rot * Vec3::NEG_Z * d + m.vel * t)
    });
    // the hit marker starts just outside the crosshair's ring (radius 16) and opens out a little as
    // it fades; a kill's is longer, thicker and red
    let kill = hud.hit.0 == HitKind::Kill;
    let (gap, len, thick) = if kill { (19.0 + 8.0 * (1.0 - hud.hit.1), 13.0, 3.0) } else { (19.0 + 4.0 * (1.0 - hud.hit.1), 8.0, 2.0) };
    let mark_c = match hud.hit.0 {
        HitKind::Hull => INK,
        HitKind::Shield => SHIELD,
        HitKind::Kill => DANGER,
    }
    .with_alpha((hud.hit.1 * 1.8).min(1.0));
    let view_id = view.map(|v| v.id);
    let enemies: Vec<_> = core.ships.values().filter(|s| Some(s.id) != view_id && view.is_none_or(|m| m.team != s.team)).collect();
    let friends: Vec<_> = core.ships.values().filter(|s| Some(s.id) != view_id && view.is_some_and(|m| m.team == s.team)).collect();
    let mut silo_entity = None;
    let mut set_text = |kids: Option<&Children>, e: Entity, s: String, c: Option<Color>| {
        let target = if texts.contains(e) { Some(e) } else { kids.and_then(|k| k.first().copied()) };
        if let Some(t) = target
            && let Ok((mut tx, mut tc)) = texts.get_mut(t)
        {
            if tx.0 != s {
                tx.0 = s;
            }
            if let Some(c) = c {
                tc.0 = c;
            }
        }
    };

    for (ent, el, mut node, mut vis, bg, border, ut, kids, plate) in &mut els {
        let show = |v: &mut Visibility, on: bool| {
            let t = if on { Visibility::Inherited } else { Visibility::Hidden };
            if *v != t {
                *v = t;
            }
        };
        match *el {
            El::Instruments => show(&mut vis, in_game),
            El::Gauges => show(&mut vis, in_game && (view.is_some() || !core.watching)),
            El::Mode => {
                let mine = core.scores.iter().find(|r| Some(r.pid) == core.pid);
                let mode = match core.mode {
                    Some(Mode::Coop) => format!("合作波次 · 第 {} 波", core.wave),
                    Some(Mode::Teams) => format!("2v2 团队战{}", mine.map_or(String::new(), |r| format!(" · {}", team_label(r.team)))),
                    Some(Mode::Ffa) => "自由混战".into(),
                    None => "连接中".into(),
                };
                let clock = if core.left > 0.0 { format!(" · 剩余 {}:{:02}", core.left as u32 / 60, core.left as u32 % 60) } else { String::new() };
                set_text(kids, ent, format!("{}{mode}{clock}", if core.watching { "观战 · " } else { "" }), None);
            }
            El::Score => {
                // spectating: the watched ship's line (score rows carry names, not ship ids)
                let row = if core.watching { view.and_then(|v| core.scores.iter().find(|r| r.name == v.name)) } else { core.scores.iter().find(|r| Some(r.pid) == core.pid) };
                let line = match (core.mode, row) {
                    // the race between the teams, then your own part in it
                    (Some(Mode::Teams), Some(r)) => {
                        let kills = |t: u16| core.scores.iter().filter(|o| o.team == t).map(|o| o.kills).sum::<u32>();
                        format!("蓝队 {} : {} 红队  ·  击毁 {}  ·  阵亡 {}", kills(1), kills(2), r.kills, r.deaths)
                    }
                    (_, Some(r)) if core.limit > 0 => format!("击毁 {}/{}  ·  阵亡 {}  ·  {} 分", r.kills, core.limit, r.deaths, r.score),
                    (_, Some(r)) => format!("{} 分  ·  击毁 {}  ·  阵亡 {}", r.score, r.kills, r.deaths),
                    (_, None) => "0 分".into(),
                };
                set_text(kids, ent, line, None);
            }
            El::Watch => show(&mut vis, in_game && core.watching),
            El::WatchName => {
                let name = match (view, session.wreck) {
                    (Some(v), _) => v.name.clone(),
                    (None, Some(_)) => "被击毁".into(),
                    (None, None) => "等待战车出场".into(),
                };
                set_text(kids, ent, name, None);
            }
            El::WatchStat => {
                let line = match (view, session.wreck) {
                    (Some(v), _) => {
                        let hull = v.body.hp_total() / v.body.max_hp * 100.0;
                        let who = if v.owner.is_some() { "玩家" } else { "AI" };
                        match core.scores.iter().find(|r| r.name == v.name) {
                            Some(r) => format!("{who} · 击毁 {} · 阵亡 {} · 车体 {hull:.0}%", r.kills, r.deaths),
                            None => format!("{who} · 车体 {hull:.0}%"),
                        }
                    }
                    (None, Some(_)) => "镜头马上切到击毁它的战车".into(),
                    (None, None) => "场上还没有战车。AI 数量为 0 的服务器只能看玩家".into(),
                };
                set_text(kids, ent, line, None);
            }
            El::Info => {
                let left = if core.mode == Some(Mode::Coop) { format!("剩余敌车 {} · ", core.enemies) } else { format!("在线 {} 人 · ", core.scores.iter().filter(|r| !r.is_bot()).count()) };
                set_text(kids, ent, format!("{left}延迟 {:.0} ms", core.rtt() * 1000.0), None);
            }
            El::Feed(i) => {
                let item = hud.feed.get(i);
                set_text(kids, ent, item.map_or(String::new(), |f| f.0.clone()), None);
                if let Some(mut b) = bg {
                    b.0 = if item.is_some() { PANEL_SOLID.with_alpha(0.75) } else { Color::NONE };
                }
                if let Some(mut b) = border {
                    b.set_all(item.map_or(Color::NONE, |f| if f.1 { DANGER } else { ACCENT }));
                }
            }
            El::Ann => show(&mut vis, hud.ann.2 > 0.0),
            El::AnnTitle | El::AnnSub => {
                let a = (hud.ann.2 / 0.8).clamp(0.0, 1.0);
                let s = if *el == El::AnnTitle { hud.ann.0.clone() } else { hud.ann.1.clone() };
                let c = if *el == El::AnnTitle { INK } else { ACCENT };
                set_text(kids, ent, s, Some(c.with_alpha(a)));
            }
            El::Fill(i) | El::BarText(i) => {
                let Some(m) = view else { continue };
                // your own ship's shield and energy come from prediction; a watched ship's shield
                // from its snapshots, and its energy isn't sent at all
                let p = core.pred.as_ref().filter(|_| me.is_some());
                let (frac, label) = match i {
                    0 => {
                        let f = m.body.hp_total() / m.body.max_hp;
                        (f, format!("{:.0}%", f * 100.0))
                    }
                    1 if m.body.stats.shield_cap > 0.0 => {
                        let s = p.map_or(m.shield_frac * m.body.stats.shield_cap, |p| p.kin.shield);
                        (s / m.body.stats.shield_cap, format!("{s:.0}"))
                    }
                    1 => (0.0, "无".into()),
                    _ => match p {
                        Some(p) => (p.kin.energy / m.body.stats.energy_cap, format!("{:.0}", p.kin.energy)),
                        None => (0.0, "—".into()),
                    },
                };
                if let El::Fill(_) = el {
                    let fr = frac.clamp(0.0, 1.0);
                    if let Some(mut p) = plate
                        && (p.frac - fr).abs() > 0.002
                    {
                        p.frac = fr;
                    }
                } else {
                    set_text(kids, ent, label, None);
                }
            }
            El::Speed => set_text(kids, ent, format!("{:.0}", view.map_or(0.0, |m| m.vel.length() * 3.6)), None),
            El::Throttle => {
                let throttle = if me.is_some() { session.throttle } else { view.map_or(0.0, |v| v.throttle) };
                node.height = percent(throttle * 100.0);
                if let Some(mut b) = bg {
                    b.0 = if view.is_some_and(|m| m.boosting) { SIGNAL } else { ACCENT };
                }
            }
            El::Weapons => {
                let Some(m) = view else { continue };
                let mut counts = std::collections::BTreeMap::new();
                for &i in &m.body.stats.weapons {
                    *counts.entry(m.body.design.blocks[i].ty).or_insert(0) += 1;
                }
                let mut s: Vec<String> = counts.iter().map(|(t, n): (&BlockType, &i32)| format!("{} ×{n}", t.info().name)).collect();
                if s.is_empty() {
                    s.push("武器全毁".into());
                }
                if m.body.has_missile && me.is_some() {
                    s.push(if session.locked { "导弹 · 已锁定，右键发射".into() } else if session.lock > 0.0 { "导弹 · 锁定中…".into() } else { "导弹 · 未锁定".into() });
                }
                set_text(kids, ent, s.join("\n"), None);
            }
            El::Warn(i) => {
                let mut warns: Vec<(String, bool)> = Vec::new();
                if let Some(m) = me {
                    let st = &m.body.stats;
                    if core.missiles.values().any(|ms| core.missile_pos(ms).distance(m.pos) < 300.0 && ms.vel.dot(m.pos - core.missile_pos(ms)) > 0.0) {
                        warns.push(("导弹接近".into(), true));
                    }
                    if m.pos.x.abs().max(m.pos.z.abs()) > voxwing_shared::ARENA_R * 0.9 {
                        warns.push(("到战区边缘了，请掉头".into(), true));
                    }
                    if m.rot.mul_vec3(Vec3::Y).y < 0.3 {
                        warns.push(("翻车了 · 按住 R 翻回来".into(), true));
                    }
                    if m.body.power == 0.0 {
                        warns.push(("发动机全部损毁".into(), true));
                    } else if st.engines.len() < m.body.engines0 {
                        warns.push((format!("发动机受损 {}/{}", st.engines.len(), m.body.engines0), false));
                    }
                }
                let item = warns.get(i);
                let crit = item.is_some_and(|w| w.1);
                let blink = (time.elapsed_secs() * 3.0) as i32 % 2 == 0;
                set_text(kids, ent, item.map_or(String::new(), |w| w.0.clone()), Some(if crit { INK } else { WARN }));
                if let Some(mut b) = bg {
                    b.0 = match item {
                        // opaque so the text stays readable over engine glow and explosions
                        Some(_) if crit => PANEL_SOLID.mix(&DANGER, if blink { 0.55 } else { 0.3 }),
                        Some(_) => PANEL_SOLID.with_alpha(0.8),
                        None => Color::NONE,
                    };
                }
                if let Some(mut b) = border {
                    b.set_all(item.map_or(Color::NONE, |_| if crit { DANGER } else { WARN }));
                }
            }
            El::Radar => {
                node.left = px(w / 2.0 - 70.0);
                node.top = px(h - 140.0 - EDGE);
                show(&mut vis, view.is_some());
            }
            El::Dot(i) => {
                let Some(m) = view else {
                    show(&mut vis, false);
                    continue;
                };
                let inv_rot = m.rot.inverse();
                let all: Vec<(Vec3, Color)> = enemies
                    .iter()
                    .map(|s| (s.pos, if Some(s.id) == session.target { SIGNAL } else { DANGER }))
                    .chain(friends.iter().map(|s| (s.pos, ACCENT)))
                    .chain(core.rocks.iter().filter(|r| r.pos.distance(m.pos) < 900.0 + r.r).map(|r| (r.pos, Color::srgba(0.63, 0.59, 0.55, 0.45))))
                    .collect();
                if let Some((p, c)) = all.get(i) {
                    let l = inv_rot * (*p - m.pos);
                    let mut v = Vec2::new(l.x, l.z) / 900.0 * 66.0;
                    if v.length() > 66.0 {
                        v = v.normalize() * 66.0;
                    }
                    node.left = px(70.0 + v.x - 3.0);
                    node.top = px(70.0 + v.y - 3.0);
                    if let Some(mut b) = bg {
                        b.0 = *c;
                    }
                    show(&mut vis, true);
                } else {
                    show(&mut vis, false);
                }
            }
            El::Cross => match aim {
                Some((s, off)) => {
                    node.left = px(s.x - 16.0);
                    node.top = px(s.y - 16.0);
                    show(&mut vis, !off);
                }
                None => show(&mut vis, false),
            },
            El::HitMark => match aim {
                Some((s, off)) if hud.hit.1 > 0.0 => {
                    let r = gap + len;
                    node.left = px(s.x - r);
                    node.top = px(s.y - r);
                    node.width = px(r * 2.0);
                    node.height = px(r * 2.0);
                    show(&mut vis, !off);
                }
                _ => show(&mut vis, false),
            },
            El::HitTick(i) => {
                // 0, 1 run along the box's vertical axis, 2, 3 along its horizontal one
                let (tw, th) = if i < 2 { (thick, len) } else { (len, thick) };
                node.width = px(tw);
                node.height = px(th);
                node.margin = if i < 2 { UiRect::left(px(-thick / 2.0)) } else { UiRect::top(px(-thick / 2.0)) };
                if let Some(mut b) = bg
                    && b.0 != mark_c
                {
                    b.0 = mark_c;
                }
            }
            El::Stick => match window.cursor_position() {
                // the mouse turns the camera now; the stick marker stays hidden
                Some(c) => {
                    node.left = px(c.x - 6.0);
                    node.top = px(c.y - 6.0);
                    show(&mut vis, false);
                }
                None => show(&mut vis, false),
            },
            El::Bracket(i) | El::BracketText(i) | El::BracketHp(i) => {
                let Some(s) = enemies.get(i) else {
                    if let El::Bracket(_) = el {
                        show(&mut vis, false);
                    }
                    continue;
                };
                let (sp, off) = screen(s.pos);
                let is_t = Some(s.id) == session.target;
                let col = if is_t { SIGNAL } else { DANGER };
                match el {
                    El::Bracket(_) => {
                        let dist = cgt.translation().distance(s.pos).max(1.0);
                        let r = (s.body.radius / dist * (h / 2.0) / (1.15f32 / 2.0).tan()).clamp(10.0, 160.0) + 5.0;
                        node.left = px(sp.x - r);
                        node.top = px(sp.y - r);
                        node.width = px(r * 2.0);
                        node.height = px(r * 2.0);
                        // the corners flash white when one of your rounds lands on it
                        let flash = if hud.hit_on.0 == s.id { hud.hit_on.1 } else { 0.0 };
                        if let Some(k) = kids {
                            for c in k.iter() {
                                if let Ok(mut b) = corners.get_mut(c) {
                                    b.set_all(col.mix(&INK, flash));
                                }
                            }
                        }
                        show(&mut vis, view.is_some() && !off && sp.x > -50.0 && sp.x < w + 50.0 && sp.y > -50.0 && sp.y < h + 50.0);
                    }
                    El::BracketText(_) => {
                        let d = view.map_or(0.0, |m| m.pos.distance(s.pos));
                        set_text(kids, ent, format!("{} · {:.0}m", s.name, d), Some(col));
                    }
                    _ => {
                        node.width = percent(s.body.hp_total() / s.body.max_hp * 100.0);
                        if let Some(mut b) = bg {
                            b.0 = col;
                        }
                    }
                }
            }
            El::Arrow(i) => {
                let Some(s) = enemies.get(i) else {
                    show(&mut vis, false);
                    continue;
                };
                let (sp, off) = screen(s.pos);
                let on_screen = !off && sp.x > 30.0 && sp.x < w - 30.0 && sp.y > 30.0 && sp.y < h - 30.0;
                if on_screen || view.is_none() {
                    show(&mut vis, false);
                    continue;
                }
                let d = (sp - Vec2::new(w / 2.0, h / 2.0)).normalize_or(Vec2::Y);
                let e = Vec2::new(w / 2.0, h / 2.0) + Vec2::new(d.x * (w / 2.0 - 46.0), d.y * (h / 2.0 - 46.0));
                node.left = px(e.x - ARROW_W);
                node.top = px(e.y - ARROW_H / 2.0);
                if let Some(mut t) = ut {
                    t.rotation = Rot2::radians(d.y.atan2(d.x) + std::f32::consts::FRAC_PI_2);
                }
                if let Some(mut b) = border {
                    b.bottom = if Some(s.id) == session.target { SIGNAL } else { DANGER };
                }
                show(&mut vis, true);
            }
            El::Lead | El::Lock => {
                let (Some(m), Some(t)) = (me, session.target.and_then(|id| core.ships.get(&id))) else {
                    show(&mut vis, false);
                    continue;
                };
                if *el == El::Lead {
                    let (s, off) = screen(intercept(m.pos, m.vel, t.pos, t.vel, m.body.bolt_speed));
                    node.left = px(s.x - 6.0);
                    node.top = px(s.y - 6.0);
                    show(&mut vis, !off && m.body.gun_range > 0.0);
                } else {
                    let (s, off) = screen(t.pos);
                    let k = (session.lock / LOCK_T).clamp(0.0, 1.0);
                    let r = 22.0 + 26.0 * (1.0 - k);
                    node.left = px(s.x - r);
                    node.top = px(s.y - r);
                    node.width = px(r * 2.0);
                    node.height = px(r * 2.0);
                    if let Some(mut b) = border {
                        b.set_all(if session.locked { DANGER } else { SIGNAL });
                    }
                    show(&mut vis, !off && m.body.has_missile && session.lock > 0.0);
                }
            }
            El::VigRed | El::VigBlue => {}
            El::Help => show(&mut vis, in_game && !core.watching && hud.show_help && hud.help_t > 0.0 && !session.paused),
            El::Silo => silo_entity = Some(ent),
        }
    }

    // damage silhouette, top-down, coloured by the most damaged block in each column
    if let (Some(silo), true) = (silo_entity, hud.silo_dirty || time.elapsed_secs() % 0.25 < dt) {
        hud.silo_dirty = false;
        commands.entity(silo).despawn_children();
        if let Some(m) = view {
            let blocks = &m.body.design.blocks;
            let (mut lo, mut hi) = (IVec2::splat(i32::MAX), IVec2::splat(i32::MIN));
            for b in blocks.iter() {
                let p = IVec2::new(b.x as i32, b.z as i32);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            let span = (hi - lo + IVec2::ONE).as_vec2();
            let cell = (124.0 / span.x).min(124.0 / span.y).min(13.0);
            let off = (Vec2::splat(140.0) - span * cell) / 2.0;
            let mut cols: std::collections::BTreeMap<(i32, i32), (f32, bool)> = Default::default();
            for (i, b) in blocks.iter().enumerate() {
                let e = cols.entry((b.x as i32, b.z as i32)).or_insert((2.0, false));
                if m.body.alive[i] {
                    e.0 = e.0.min(m.body.hp_frac(i));
                    e.1 |= b.ty == BlockType::Cockpit;
                }
            }
            for ((x, z), (frac, cockpit)) in cols {
                let p = off + (IVec2::new(x, z) - lo).as_vec2() * cell;
                let color = if cockpit {
                    INK
                } else if frac > 1.5 {
                    DANGER.with_alpha(0.25)
                } else {
                    ACCENT.mix(&WARN, ((1.0 - frac) * 1.6).min(1.0)).mix(&DANGER, ((0.5 - frac) * 2.0).clamp(0.0, 1.0))
                };
                commands.spawn((Node { position_type: PositionType::Absolute, left: px(p.x + 1.0), top: px(p.y + 1.0), width: px(cell - 2.0), height: px(cell - 2.0), ..default() }, BackgroundColor(color), Pickable::IGNORE, ChildOf(silo)));
            }
        }
    }
}

// ---------------------------------------------------------------- overlays

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shown {
    None,
    Connecting,
    Paused,
    Dead,
    Board,
}

#[derive(Component)]
pub struct OverlayRoot;

#[allow(clippy::too_many_arguments)]
pub fn flight_overlays(mut commands: Commands, session: Res<Session>, keys: Res<ButtonInput<KeyCode>>, dev: Res<crate::DevOpts>, font: Res<UiFont>, roots: Query<Entity, With<OverlayRoot>>, mut shown: Local<Option<Shown>>, time: Res<Time>, mut board_t: Local<f32>) {
    let core = &session.core;
    let want = if core.state == ConnState::Connecting {
        Shown::Connecting
    } else if session.paused {
        Shown::Paused
    } else if keys.pressed(KeyCode::Tab) || dev.forced("board", time.elapsed_secs()) {
        Shown::Board
    } else if core.state == ConnState::InGame && !core.watching && core.my_ship.is_none() && session.death.is_some() {
        Shown::Dead
    } else {
        Shown::None
    };
    // the scoreboard changes while it is open; refresh it twice a second
    *board_t -= time.delta_secs();
    let refresh = want == Shown::Board && *board_t <= 0.0;
    if *shown == Some(want) && !refresh && !roots.is_empty() == (want != Shown::None) {
        return;
    }
    if refresh {
        *board_t = 0.5;
    }
    *shown = Some(want);
    for e in &roots {
        commands.entity(e).despawn();
    }
    let f = &*font;
    match want {
        Shown::None => {}
        Shown::Connecting => {
            let (o, p) = spawn_overlay(&mut commands, f, "正在连接", 420.0);
            commands.entity(o).insert((OverlayRoot, DespawnOnExit(Screen::Flight)));
            let r = commands.spawn((row(GAP), ChildOf(p))).id();
            commands.spawn((spinner(), ChildOf(r)));
            commands.spawn((col(2.0), ChildOf(r), children![text(f, session.addr.clone(), 16.0, INK), text(f, "QUIC · UDP · 5 秒无响应会自动放弃", 12.0, MUTED)]));
            let foot = commands.spawn((Node { justify_content: JustifyContent::FlexEnd, ..default() }, ChildOf(p))).id();
            spawn_btn(&mut commands, f, foot, Kind::Normal, "取消", UiAction::LeaveFlight, Some("Esc"));
        }
        Shown::Paused => {
            let (o, p) = spawn_overlay(&mut commands, f, "已暂停", 400.0);
            commands.entity(o).insert((OverlayRoot, DespawnOnExit(Screen::Flight)));
            let note = if core.watching { "你在观战，比赛照常进行。" } else { "联机游戏不会真的停下，你的战车还在场上。" };
            commands.spawn((text(f, note, 13.0, MUTED), ChildOf(p)));
            let c = commands.spawn((col(GAP_S), ChildOf(p))).id();
            let resume = spawn_btn(&mut commands, f, c, Kind::Primary, "继续", UiAction::Resume, Some("Esc"));
            commands.entity(resume).insert(bevy::input_focus::AutoFocus);
            spawn_btn(&mut commands, f, c, Kind::Normal, "设置", UiAction::OpenSettings, None);
            spawn_btn(&mut commands, f, c, Kind::Normal, if core.watching { "停止观战" } else { "退出对局" }, UiAction::LeaveFlight, None);
            spawn_btn(&mut commands, f, c, Kind::Danger, "退出游戏", UiAction::Quit, None);
        }
        Shown::Dead => {
            let o = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), bottom: px(200), justify_content: JustifyContent::Center, ..default() }, Pickable::IGNORE, OverlayRoot, DespawnOnExit(Screen::Flight))).id();
            let p = spawn_panel(&mut commands, Some(o), Node { width: px(440), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP_S), ..default() });
            commands.entity(p).insert((Plate { edge: DANGER.with_alpha(0.6), mark: DANGER, ..solid_plate() }, SlideIn::new(0.0, 0.0, 20.0)));
            commands.spawn((heading(f, "战车被击毁", 22.0, DANGER), ChildOf(p)));
            if let Some(k) = &session.killer {
                commands.spawn((text(f, format!("击毁你的是 {k}"), 15.0, INK), ChildOf(p)));
            }
            let rule = match core.mode {
                Some(Mode::Ffa) => "4 秒后在随机位置重生。",
                Some(Mode::Teams) => "4 秒后在己方一侧重生。",
                _ => "本波结束后和队友一起重生；全员阵亡则从第 1 波重新开始。",
            };
            commands.spawn((text(f, rule, 13.0, MUTED), ChildOf(p)));
            commands.spawn((text(f, "按住 Tab 查看记分板 · Esc 暂停菜单", 12.0, MUTED), ChildOf(p)));
        }
        Shown::Board => {
            let (o, p) = spawn_overlay(&mut commands, f, "记分板", 560.0);
            // refreshed twice a second while open: no entrance motion, and keep the game visible
            commands.entity(o).insert((OverlayRoot, DespawnOnExit(Screen::Flight), BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.3))));
            commands.entity(p).remove::<SlideIn>();
            spawn_score_table(&mut commands, f, p, &core.scores, core.mode, core.pid);
        }
    }
}

/// A team's name and colour (2v2: team 1 blue, team 2 red).
pub fn team_label(team: u16) -> &'static str {
    if team == 1 { "蓝队" } else { "红队" }
}

pub fn team_color(team: u16) -> Color {
    if team == 1 { Color::srgb(0.4, 0.62, 1.0) } else { DANGER }
}

/// The scoreboard table (in flight and in the results): one row per pilot, grouped by team in
/// 2v2 with each team's kills in its header.
pub fn spawn_score_table(commands: &mut Commands, f: &UiFont, parent: Entity, rows: &[ScoreRow], mode: Option<Mode>, me: Option<u16>) {
    let grid = commands
        .spawn((Node { display: Display::Grid, grid_template_columns: vec![GridTrack::px(36.0), GridTrack::flex(1.0), GridTrack::px(64.0), GridTrack::px(64.0), GridTrack::px(72.0)], row_gap: px(GAP_S), column_gap: px(12), ..default() }, ChildOf(parent)))
        .id();
    // numeric columns are right-aligned
    let num = || Node { justify_self: JustifySelf::End, ..default() };
    for (i, h) in ["#", "车长", "击毁", "阵亡", "得分"].into_iter().enumerate() {
        let c = commands.spawn((strong(f, h, 12.0, MUTED), ChildOf(grid))).id();
        if i >= 2 {
            commands.entity(c).insert(num());
        }
    }
    let groups: Vec<(Option<u16>, Vec<&ScoreRow>)> = if mode == Some(Mode::Teams) {
        let mut teams: Vec<u16> = rows.iter().map(|r| r.team).collect();
        teams.sort();
        teams.dedup();
        teams.into_iter().map(|t| (Some(t), rows.iter().filter(|r| r.team == t).collect())).collect()
    } else {
        vec![(None, rows.iter().collect())]
    };
    for (team, list) in groups {
        if let Some(t) = team {
            let kills: u32 = list.iter().map(|r| r.kills).sum();
            commands.spawn((Node { grid_column: GridPlacement::span(5), margin: UiRect::top(px(4)), column_gap: px(8), align_items: AlignItems::Center, ..default() }, ChildOf(grid), children![
                (Node { width: px(3), height: px(14), ..default() }, BackgroundColor(team_color(t))),
                strong(f, format!("{}  ·  击毁 {kills}", team_label(t)), 13.0, team_color(t)),
            ]));
        }
        for (i, r) in list.into_iter().enumerate() {
            let mine = Some(r.pid) == me;
            let c = if mine { ACCENT } else if r.is_bot() { MUTED.mix(&INK, 0.45) } else { INK };
            commands.spawn((tech(f, format!("{:02}", i + 1), 15.0, MUTED), ChildOf(grid)));
            commands.spawn((text(f, format!("{}{}", r.name, if r.alive { "" } else { "  · 阵亡中" }), 15.0, c), ChildOf(grid)));
            commands.spawn((tech(f, r.kills.to_string(), 16.0, c), num(), ChildOf(grid)));
            commands.spawn((tech(f, r.deaths.to_string(), 16.0, c), num(), ChildOf(grid)));
            commands.spawn((tech(f, r.score.to_string(), 16.0, c), num(), ChildOf(grid)));
        }
    }
}
