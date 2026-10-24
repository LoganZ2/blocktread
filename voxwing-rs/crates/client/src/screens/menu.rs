//! The lobby, the game's main screen: your ships in a carousel (carousel.rs), the mode and budget to
//! play, and matchmaking. The queue runs right here; when the server finds a match the ship pick
//! takes over.

use super::Overlays;
use super::carousel::Carousel;
use crate::flight::Session;
use crate::hangar::Hangar;
use crate::settings::{self, Settings};
use crate::ui::kit::*;
use crate::ui::*;
use crate::{DevOpts, Hosted, LastError, Screen};
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::{EditableText, LetterSpacing};
use std::time::Duration;
use voxwing_client_core::{ClientCore, ConnState, Phase};
use voxwing_server::{HubConfig, ServerConfig};
use voxwing_shared::protocol::Mode;

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastLaunch>()
            .add_systems(OnEnter(Screen::Menu), spawn_menu)
            .add_systems(Update, (menu_actions, menu_keys, menu_flow, refresh_ship, refresh_play, queue_line, show_last_error).chain().run_if(in_state(Screen::Menu)));
    }
}

/// Whether the last session was played or watched, so "retry" after an error does the same.
#[derive(Resource, Default)]
struct LastLaunch {
    watch: bool,
}

#[derive(Component)]
struct ShipBar;
#[derive(Component)]
struct StatsCard;
#[derive(Component)]
struct PlayBox;
#[derive(Component)]
struct QueueLine;
#[derive(Component)]
struct NameField;

/// The mode a settings key stands for ("coop" from older settings files plays free-for-all).
pub fn mode_of(key: &str) -> Mode {
    if key == "2v2" { Mode::Teams } else { Mode::Ffa }
}

/// "115 · 中型": a ship's cost and the cheapest tier it fits.
pub fn cost_label(cost: f32) -> String {
    match voxwing_shared::balance::balance().tier_for(cost) {
        Some(t) => format!("{cost:.0} · {}", t.name),
        None => format!("{cost:.0} · 超重型"),
    }
}

fn server_addr(dev: &DevOpts) -> String {
    dev.server.clone().unwrap_or_else(|| crate::SERVER.to_string())
}

fn spawn_menu(mut commands: Commands, font: Res<UiFont>, settings: Res<Settings>) {
    let f = &*font;
    let scope = DespawnOnExit(Screen::Menu);
    let root = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, Pickable::IGNORE, IgnoreHover, scope)).id();

    // top bar: name and callsign left, settings and exit right
    let top = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(EDGE), right: px(EDGE), top: px(EDGE), height: px(H_MD), column_gap: px(GAP), align_items: AlignItems::Center, ..default() },
            IgnoreHover,
            SlideIn::new(0.0, 0.0, -16.0),
            ChildOf(root),
        ))
        .id();
    let brand = commands.spawn((row(12.0), ChildOf(top))).id();
    commands.spawn((Node { width: px(3), height: px(22), ..default() }, BackgroundColor(ACCENT), UiTransform { rotation: Rot2::degrees(14.0), ..default() }, ChildOf(brand)));
    commands.spawn((heading(f, "体素星翼", 24.0, INK), ChildOf(brand)));
    commands.spawn((tech(f, "VOXWING", 12.0, ACCENT), LetterSpacing::Px(6.0), ChildOf(brand)));
    commands.spawn((Node { width: px(GAP), ..default() }, ChildOf(top)));
    commands.spawn((tech(f, "CALLSIGN", 11.0, MUTED), LetterSpacing::Px(3.0), ChildOf(top)));
    let name = spawn_field(&mut commands, f, top, &settings.name, 10.0);
    commands.entity(name).insert(NameField);
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(top)));
    spawn_btn(&mut commands, f, top, Kind::Normal, "设置", UiAction::OpenSettings, Some("S"));
    spawn_btn(&mut commands, f, top, Kind::Ghost, "退出", UiAction::Quit, Some("Esc"));

    // bottom left: what to play, and the button that finds a match
    let play = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), bottom: px(EDGE), width: px(380), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(play).insert((PlayBox, SlideIn::new(0.1, -40.0, 0.0)));

    // bottom centre: the name of the ship in view
    commands.spawn((
        Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(EDGE), justify_content: JustifyContent::Center, ..default() },
        Pickable::IGNORE,
        IgnoreHover,
        ChildOf(root),
        children![(Node { column_gap: px(GAP_S), align_items: AlignItems::Center, ..default() }, ShipBar, SlideIn::new(0.15, 0.0, 24.0))],
    ));

    // bottom right: the ship's numbers
    let stats = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, right: px(EDGE), bottom: px(EDGE), width: px(320), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(stats).insert((StatsCard, SlideIn::new(0.2, 40.0, 0.0)));
}

/// The name bar and the numbers card follow the carousel.
#[allow(clippy::too_many_arguments)]
fn refresh_ship(
    mut commands: Commands,
    hangar: Res<Hangar>,
    settings: Res<Settings>,
    car: Res<Carousel>,
    font: Res<UiFont>,
    bar: Single<Entity, With<ShipBar>>,
    card: Single<Entity, With<StatsCard>>,
    added: Query<(), Added<ShipBar>>,
) {
    if !hangar.is_changed() && !settings.is_changed() && !car.is_changed() && added.is_empty() {
        return;
    }
    let f = &*font;
    let a = &hangar.analysis;
    let s = &a.stats;
    let bar = *bar;
    let card = *card;
    commands.entity(bar).despawn_children();
    commands.entity(card).despawn_children();
    let mid = commands.spawn((Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(4), ..default() }, ChildOf(bar))).id();

    // a "new ship" slot: what it would make
    if car.new_slot(hangar.fleet.ships.len()).is_some() {
        commands.spawn((heading(f, "新建机体", 28.0, INK), ChildOf(mid)));
        commands.spawn((text(f, format!("机库里有 {} 架 · 点中间的加号新建一架", hangar.fleet.ships.len()), 13.0, MUTED), ChildOf(mid)));
        let sec = spawn_section(&mut commands, f, card, "新建机体|NEW SHIP");
        commands.spawn((para(f, "空白机体只有一个驾驶舱。新建后进入机库，拼上引擎、机翼和武器，通过起飞检查就能出击。", 13.0, MUTED), ChildOf(sec)));
        return;
    }
    commands.spawn((heading(f, hangar.design.name.clone(), 28.0, INK), ChildOf(mid)));
    let line = commands.spawn((row(GAP_S), ChildOf(mid))).id();
    if a.ok() {
        commands.spawn((chip(f, "可以出击", OK), ChildOf(line)));
    } else {
        commands.spawn((chip(f, format!("{} 项待修改", a.errors), WARN), ChildOf(line)));
    }
    commands.spawn((tech(f, cost_label(s.cost), 13.0, MUTED), ChildOf(line)));
    commands.spawn((tech(f, format!("{} / {}", hangar.fleet.current + 1, hangar.fleet.ships.len()), 13.0, MUTED), ChildOf(line)));

    let sec = spawn_section(&mut commands, f, card, "当前机体|CURRENT SHIP");
    let budget = voxwing_shared::balance::balance().tier(&settings.host_budget).map_or(350.0, |t| t.budget);
    for (label, frac, val, c) in [
        ("造价", s.cost / budget, cost_label(s.cost), INK.with_alpha(0.7)),
        ("速度", s.max_speed / 125.0, format!("{:.0} m/s", s.max_speed), ACCENT),
        ("转向", s.pitch_rate / 2.0, format!("{:.0} 度/秒", s.pitch_rate.to_degrees()), ACCENT),
        ("火力", s.dps / 200.0, format!("{:.0} DPS", s.dps), SIGNAL),
        ("耐久", s.hp / 3500.0, format!("{:.0}", s.hp), OK),
        ("护盾", s.shield_cap / 200.0, if s.shield_cap > 0.0 { format!("{:.0}", s.shield_cap) } else { "无".into() }, Color::srgb(0.4, 0.62, 1.0)),
    ] {
        let (gauge, v) = spawn_stat(&mut commands, f, sec, label, c);
        commands.entity(gauge).insert(Plate::gauge(c, 24.0, frac.clamp(0.0, 1.0)));
        commands.entity(v).insert(Text::new(val));
    }
    if let Some(e) = a.first_error() {
        commands.spawn((para(f, format!("需要先修改：{}", e.msg), 13.0, WARN), ChildOf(card)));
    }
    let br = commands.spawn((row(GAP_S), ChildOf(card))).id();
    spawn_btn(&mut commands, f, br, Kind::Normal, "编辑机体", UiAction::Goto(Screen::Hangar), Some("H"));
}

/// Where matchmaking stands, for the play panel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Queue {
    Idle,
    Connecting,
    Queued,
}

fn queue_state(session: Option<&Session>) -> Queue {
    match session {
        None => Queue::Idle,
        Some(s) if s.core.watching => Queue::Idle,
        Some(s) if matches!(s.core.phase, Phase::Queued { .. }) => Queue::Queued,
        Some(_) => Queue::Connecting,
    }
}

/// The play panel: mode, budget, the queue's progress and the match button. Rebuilt when one of
/// them changes.
#[allow(clippy::too_many_arguments)]
fn refresh_play(
    mut commands: Commands,
    settings: Res<Settings>,
    hangar: Res<Hangar>,
    session: Option<Res<Session>>,
    dev: Res<DevOpts>,
    font: Res<UiFont>,
    panel: Single<Entity, With<PlayBox>>,
    mut built: Local<Option<(String, String, Queue, Option<String>)>>,
    added: Query<(), Added<PlayBox>>,
) {
    let q = queue_state(session.as_deref());
    let mode = mode_of(&settings.host_mode);
    let tier = voxwing_shared::balance::balance().tier(&settings.host_budget);
    // the ship can be changed in the pick, but say so if this one won't do
    let cost = hangar.analysis.stats.cost;
    let warn = tier.filter(|t| cost > t.budget + 1e-3).map(|t| format!("「{}」超出{}上限 {:.0}，匹配到之后要换一架。", hangar.design.name, t.name, t.budget));
    let want = (settings.host_mode.clone(), settings.host_budget.clone(), q, warn.clone());
    if built.as_ref() == Some(&want) && added.is_empty() {
        return;
    }
    *built = Some(want);
    let f = &*font;
    let p = *panel;
    commands.entity(p).despawn_children();
    let busy = q != Queue::Idle;

    let modes = spawn_section(&mut commands, f, p, "模式|MODE");
    let seg = commands.spawn((row(0.0), ChildOf(modes))).id();
    for (key, label) in [("ffa", "自由混战"), ("2v2", "2v2 团队战")] {
        let b = spawn_btn(&mut commands, f, seg, Kind::Segment, label, UiAction::HostMode(key), None);
        commands.entity(b).insert(Selected(mode_of(key) == mode));
        set_enabled(&mut commands, b, !busy);
    }
    let desc = match mode {
        Mode::Teams => "蓝红两队各 2 人，空位由 AI 补齐。",
        _ => "6 人各自为战，空位由 AI 补齐。",
    };
    commands.spawn((para(f, desc, 12.0, MUTED), ChildOf(modes)));

    let budget = spawn_section(&mut commands, f, p, "造价上限|BUDGET");
    let seg = commands.spawn((row(0.0), ChildOf(budget))).id();
    let tiers = &voxwing_shared::balance::balance().tiers;
    for (key, label) in std::iter::once((String::new(), "不限".to_string())).chain(tiers.iter().map(|t| (t.key.clone(), format!("{} {:.0}", t.name, t.budget)))) {
        let b = spawn_btn(&mut commands, f, seg, Kind::Segment, label, UiAction::HostBudget(key.clone()), None);
        commands.entity(b).insert(Selected(settings.host_budget == key));
        set_enabled(&mut commands, b, !busy);
    }

    if busy {
        let r = commands.spawn((row(GAP_S), ChildOf(p))).id();
        commands.spawn((spinner(), ChildOf(r)));
        commands.spawn((text(f, "", 13.0, INK), QueueLine, ChildOf(r)));
    } else if let Some(w) = warn {
        commands.spawn((para(f, w, 12.0, WARN), ChildOf(p)));
    }
    let go = commands.spawn((row(GAP_S), ChildOf(p))).id();
    let lb = spawn_btn(&mut commands, f, go, Kind::Primary, if busy { "取消匹配" } else { "开始匹配" }, UiAction::Launch, Some("Enter"));
    commands.entity(lb).insert(Node { flex_grow: 1.0, ..btn_node(Kind::Primary) });
    let wb = spawn_btn(&mut commands, f, go, Kind::Normal, "观战", UiAction::Watch, Some("V"));
    commands.entity(wb).insert((
        Node { height: px(H_LG), padding: UiRect::horizontal(px(16)), ..btn_node(Kind::Normal) },
        Tip("不驾驶，附身到 AI 或其他玩家身上看他们打".into()),
        TipDetail("服务器上有对局时观看它；没有的话开一局全 AI 的自由混战。\n← → 换一架飞机，C 切换座舱视角。".into()),
    ));
    set_enabled(&mut commands, wb, !busy);
    commands.spawn((tech(f, format!("SERVER  {}", server_addr(&dev)), 11.0, MUTED), LetterSpacing::Px(1.5), ChildOf(p)));
}

/// The queue's progress: time waited, players waiting, when AI fills in.
fn queue_line(session: Option<Res<Session>>, mut line: Query<&mut Text, With<QueueLine>>) {
    let Some(s) = session else { return };
    let want = match &s.core.phase {
        Phase::Queued { waited, players, fill_in, .. } => {
            let t = format!("{}:{:02}", *waited as u32 / 60, *waited as u32 % 60);
            if *fill_in > 0.5 { format!("正在匹配 {t} · 队列中 {players} 人 · {} 秒后 AI 补位", fill_in.ceil() as u32) } else { format!("正在匹配 {t} · 队列中 {players} 人 · 正在组建对局…") }
        }
        _ => format!("正在连接 {}…", s.addr),
    };
    for mut l in &mut line {
        if l.0 != want {
            l.0 = want.clone();
        }
    }
}

/// The session on this screen: queue once connected, and go to the ship pick when a match is found.
fn menu_flow(session: Option<ResMut<Session>>, mut next: ResMut<NextState<Screen>>) {
    let Some(mut s) = session else { return };
    if s.core.state != ConnState::Lobby {
        return;
    }
    match s.core.phase {
        Phase::Idle if !s.core.watching => {
            if let Some((mode, tier)) = s.want.take() {
                s.core.queue(mode, tier);
            }
        }
        Phase::Picking(_) => next.set(Screen::Match),
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn menu_actions(
    mut commands: Commands,
    mut msgs: MessageReader<UiAction>,
    mut settings: ResMut<Settings>,
    mut hosted: ResMut<Hosted>,
    session: Option<Res<Session>>,
    fields: Query<&EditableText, With<NameField>>,
    mut toasts: ResMut<Toasts>,
    mut next: ResMut<NextState<Screen>>,
    dev: Res<DevOpts>,
    mut focus: ResMut<InputFocus>,
    mut modal: ResMut<ModalReq>,
    mut last: ResMut<LastLaunch>,
) {
    for m in msgs.read() {
        match m.clone() {
            UiAction::HostMode(k) => settings.host_mode = k.to_string(),
            UiAction::HostBudget(k) => settings.host_budget = k,
            m @ (UiAction::Launch | UiAction::Watch) => {
                let watch = m == UiAction::Watch;
                if let Some(s) = &session {
                    // pressed again while queued: cancel
                    if !watch {
                        s.core.leave();
                        commands.remove_resource::<Session>();
                    }
                    continue;
                }
                last.watch = watch;
                modal.0 = None;
                let name: String = fields.iter().next().map(|e| e.value().to_string().trim().to_string()).unwrap_or_default().chars().take(12).collect();
                settings.name = if name.is_empty() { "飞行员".into() } else { name };
                settings::save_settings(&settings);
                let addr = match dev.server.as_deref() {
                    Some("local") => {
                        if hosted.0.is_none() {
                            match host_local(&dev) {
                                Ok(h) => hosted.0 = Some(h),
                                Err(e) => {
                                    toasts.push(format!("无法在本机启动服务器：{e:#}"), ToastKind::Err);
                                    continue;
                                }
                            }
                        }
                        format!("127.0.0.1:{}", hosted.0.as_ref().unwrap().addr.port())
                    }
                    _ => server_addr(&dev),
                };
                info!("{} {addr}", if watch { "watching" } else { "matchmaking on" });
                let latency = Duration::from_millis(dev.latency_ms);
                let core = if watch { ClientCore::watch(&addr, &settings.name, latency) } else { ClientCore::connect(&addr, &settings.name, latency) };
                let tier = (!settings.host_budget.is_empty()).then(|| settings.host_budget.clone());
                let want = (!watch).then(|| (mode_of(&settings.host_mode), tier));
                commands.insert_resource(Session::new(core, addr, want));
                focus.clear();
                if watch {
                    next.set(Screen::Match);
                }
            }
            _ => {}
        }
    }
}

/// A server inside this client (`--server local`): no queue wait, AI fills the match at once.
fn host_local(dev: &DevOpts) -> anyhow::Result<voxwing_server::ServerHandle> {
    let host = if dev.local_only { "127.0.0.1" } else { "0.0.0.0" };
    let hub = HubConfig { queue_wait: 0.0, ..HubConfig::default() };
    let at = |port: u16| ServerConfig { bind: format!("{host}:{port}").parse().unwrap(), hub: hub.clone(), verbose: false };
    match voxwing_server::start(at(dev.port.unwrap_or(voxwing_shared::DEFAULT_PORT))) {
        Ok(h) => Ok(h),
        // the port is taken (another server on this machine): any free one will do
        Err(_) if dev.port.is_none() => voxwing_server::start(at(0)),
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
fn menu_keys(
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    modal: Res<ModalReq>,
    focus: Res<InputFocus>,
    editing: Query<(), With<EditableText>>,
    session: Option<Res<Session>>,
    mut w: MessageWriter<UiAction>,
    dev: Res<DevOpts>,
    time: Res<Time>,
    mut auto: Local<bool>,
    mut stepped: Local<f32>,
) {
    // dev: slide the carousel right every 2 seconds (screenshots of the slide)
    let now = time.elapsed_secs();
    if dev.forced("next", now) && now - *stepped > 2.0 {
        *stepped = now;
        w.write(UiAction::CarouselStep(1));
    }
    if !*auto && time.elapsed_secs() > 1.5 {
        *auto = true;
        match dev.auto_launch.as_deref() {
            Some("watch") => {
                w.write(UiAction::Watch);
            }
            Some(m) => {
                w.write(UiAction::HostMode(if m == "2v2" { "2v2" } else { "ffa" }));
                w.write(UiAction::Launch);
            }
            None => {}
        }
    }
    if overlays.any(&modal) {
        return;
    }
    let typing_now = typing(&focus, &editing);
    if keys.just_pressed(KeyCode::Enter) && (focus.get().is_none() || typing_now) {
        w.write(UiAction::Launch);
    }
    if keys.just_pressed(KeyCode::Escape) {
        w.write(if session.is_some() { UiAction::Launch } else { UiAction::Quit });
    }
    if typing_now {
        return;
    }
    for (k, a) in [
        (KeyCode::KeyH, UiAction::Goto(Screen::Hangar)),
        (KeyCode::KeyS, UiAction::OpenSettings),
        (KeyCode::KeyV, UiAction::Watch),
        (KeyCode::ArrowLeft, UiAction::CarouselStep(-1)),
        (KeyCode::Comma, UiAction::CarouselStep(-1)),
        (KeyCode::ArrowRight, UiAction::CarouselStep(1)),
        (KeyCode::Period, UiAction::CarouselStep(1)),
    ] {
        if keys.just_pressed(k) {
            w.write(a);
        }
    }
}

/// When a session ended with an error, explain it and offer a retry.
fn show_last_error(mut last: ResMut<LastError>, mut modal: ResMut<ModalReq>, launch: Res<LastLaunch>) {
    if let Some((title, body)) = last.0.take() {
        let retry = if launch.watch { UiAction::Watch } else { UiAction::Launch };
        modal.show(title, body, vec![("返回", UiAction::CloseModal, Kind::Normal), ("重试", retry, Kind::Accent)], None);
    }
}
