//! The lobby, the game's main screen: your vehicles in a carousel (carousel.rs), the mode, budget
//! and server to play on (each server with its live ping), and matchmaking. The queue runs right
//! here; when the server finds a match the vehicle pick takes over.

use super::Overlays;
use crate::account::{Lobby, link_target, saved_token};
use super::carousel::Carousel;
use crate::flight::Session;
use crate::hangar::Hangar;
use crate::settings::{self, Settings};
use crate::ui::kit::*;
use crate::ui::*;
use crate::settings::Regions;
use crate::{DevOpts, Hosted, LastError, Screen};
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::{EditableText, LetterSpacing};
use std::time::Duration;
use super::entry::{Pings, RegionPing, region};
use super::matching::straight_in;
use blocktread_client_core::{ClientCore, ConnState, Phase, Target};
use blocktread_server::{HubConfig, ServerConfig};
use blocktread_shared::economy::economy;
use blocktread_shared::protocol::{AuthReq, Mode};

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastLaunch>()
            .add_systems(OnEnter(Screen::Menu), spawn_menu)
            .add_systems(Update, (menu_actions, menu_keys, menu_flow, refresh_ship, refresh_play, queue_line, show_last_error, account_bar).chain().run_if(in_state(Screen::Menu)));
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
/// The callsign label and field: a guest's name (hidden once logged in, the account's name is used).
#[derive(Component)]
struct CallsignBox;
/// The top bar's account corner: log in, or who you are, your level and coins.
#[derive(Component)]
struct AccountBox;
/// The mode a settings key stands for (anything unknown plays free-for-all).
pub fn mode_of(key: &str) -> Mode {
    match key {
        "2v2" => Mode::Teams,
        "solo" => Mode::Solo,
        _ => Mode::Ffa,
    }
}

/// "115 · 中型": a vehicle's cost and the cheapest tier it fits.
pub fn cost_label(cost: f32) -> String {
    match blocktread_shared::balance::balance().tier_for(cost) {
        Some(t) => format!("{cost:.0} · {}", t.name),
        None => format!("{cost:.0} · 超重型"),
    }
}

/// Where to connect: `--server` if given, else the server picked on the entry page.
fn server_addr(dev: &DevOpts, regions: &Regions, region: usize) -> String {
    server_target(dev, regions, region).addr
}

/// The server to play on: `--server`'s if given (no pin), else the region's.
fn server_target(dev: &DevOpts, regions: &Regions, region: usize) -> Target {
    match &dev.server {
        Some(addr) => addr.into(),
        None => regions.0[region].target(),
    }
}

fn spawn_menu(mut commands: Commands, font: Res<UiFont>, settings: Res<Settings>) {
    let f = &*font;
    let scope = DespawnOnExit(Screen::Menu);
    let root = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, Pickable::IGNORE, IgnoreHover, scope)).id();

    // top bar: the wordmark and your callsign left, settings and exit right
    let top = spawn_top_bar(&mut commands, root);
    commands.entity(top).insert(SlideIn::new(0.0, 0.0, -16.0));
    spawn_brand(&mut commands, f, top, 30.0);
    commands.spawn((Node { width: px(1), height: px(28), margin: UiRect::horizontal(px(GAP_S)), ..default() }, BackgroundColor(LINE_HI), ChildOf(top)));
    let callsign = commands.spawn((Node { column_gap: px(GAP_S), align_items: AlignItems::Center, ..default() }, CallsignBox, ChildOf(top))).id();
    commands.spawn((strong(f, "呼号", 13.0, MUTED), ChildOf(callsign)));
    commands.spawn((tech(f, "CALLSIGN", 12.0, MUTED), LetterSpacing::Px(3.0), ChildOf(callsign)));
    let name = spawn_field(&mut commands, f, callsign, &settings.name, 10.0);
    commands.entity(name).insert(NameField);
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(top)));
    commands.spawn((Node { column_gap: px(GAP_S), align_items: AlignItems::Center, ..default() }, AccountBox, ChildOf(top)));
    spawn_btn(&mut commands, f, top, Kind::Normal, "设置", UiAction::OpenSettings, Some("S"));
    spawn_btn(&mut commands, f, top, Kind::Normal, "退出", UiAction::Quit, Some("Esc"));

    // bottom left: what to play, and the button that finds a match
    let play = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), bottom: px(EDGE), width: px(380), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(play).insert((PlayBox, SlideIn::new(0.1, -40.0, 0.0)));

    // bottom centre: the name plate of the vehicle in view
    commands.spawn((
        Node { position_type: PositionType::Absolute, left: px(0), right: px(0), bottom: px(EDGE), justify_content: JustifyContent::Center, ..default() },
        Pickable::IGNORE,
        IgnoreHover,
        ChildOf(root),
        children![(Node { column_gap: px(GAP_S), align_items: AlignItems::Center, padding: UiRect::axes(px(PAD), px(12)), ..default() }, panel_plate(), ShipBar, SlideIn::new(0.15, 0.0, 24.0))],
    ));

    // bottom right: the vehicle's numbers
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

    // a "new vehicle" slot: what it would make
    if car.on_new(hangar.fleet.ships.len()) {
        commands.spawn((display(f, "新建战车", 34.0, INK), LetterSpacing::Px(3.0), ChildOf(mid)));
        commands.spawn((text(f, format!("车库里有 {} 辆 · 点中间的加号新建一辆", hangar.fleet.ships.len()), 13.0, MUTED), ChildOf(mid)));
        let sec = spawn_section(&mut commands, f, card, "新建战车|NEW VEHICLE");
        commands.spawn((para(f, "空白战车只有一个驾驶室。新建后进入车库，装上车轮、发动机和武器，通过出车检查就能出战。", 13.0, MUTED), ChildOf(sec)));
        return;
    }
    commands.spawn((display(f, hangar.design.name.clone(), 34.0, INK), LetterSpacing::Px(3.0), ChildOf(mid)));
    let line = commands.spawn((row(GAP_S), ChildOf(mid))).id();
    if a.ok() {
        commands.spawn((chip(f, "可以出战", OK), ChildOf(line)));
    } else {
        commands.spawn((chip(f, format!("{} 项待修改", a.errors), WARN), ChildOf(line)));
    }
    commands.spawn((tech(f, cost_label(s.cost), 13.0, MUTED), ChildOf(line)));
    commands.spawn((tech(f, format!("{} / {}", hangar.fleet.current + 1, hangar.fleet.ships.len()), 13.0, MUTED), ChildOf(line)));

    let sec = spawn_section(&mut commands, f, card, "当前战车|VEHICLE");
    for (label, frac, val, c) in vehicle_stats(s, &settings.host_budget) {
        let (gauge, v) = spawn_stat(&mut commands, f, sec, label, c);
        commands.entity(gauge).insert(Plate::gauge(c, 24.0, frac.clamp(0.0, 1.0)));
        commands.entity(v).insert(Text::new(val));
    }
    if let Some(e) = a.first_error() {
        commands.spawn((para(f, format!("需要先修改：{}", e.msg), 13.0, WARN), ChildOf(card)));
    }
    let br = commands.spawn((row(GAP_S), ChildOf(card))).id();
    spawn_btn(&mut commands, f, br, Kind::Normal, "编辑战车", UiAction::Goto(Screen::Hangar), Some("H"));
}

/// A vehicle's numbers for the stat gauges: label, gauge fill, value, gauge colour. The cost
/// gauge is against the budget picked (the heavy tier's when there is none).
pub fn vehicle_stats(s: &blocktread_shared::analysis::Stats, budget: &str) -> [(&'static str, f32, String, Color); 6] {
    let cap = blocktread_shared::balance::balance().tier(budget).map_or(350.0, |t| t.budget);
    [
        ("造价", s.cost / cap, cost_label(s.cost), INK.with_alpha(0.7)),
        ("极速", s.max_speed / 47.0, format!("{:.0} km/h", s.max_speed * 3.6), ACCENT),
        ("加速", s.accel / 16.0, format!("{:.1} m/s²", s.accel), ACCENT),
        ("火力", s.dps / 250.0, format!("{:.0} DPS", s.dps), SIGNAL),
        ("耐久", s.hp / 5000.0, format!("{:.0}", s.hp), OK),
        ("护盾", s.shield_cap / 200.0, if s.shield_cap > 0.0 { format!("{:.0}", s.shield_cap) } else { "无".into() }, Color::srgb(0.4, 0.62, 1.0)),
    ]
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
    pings: Option<Res<Pings>>,
    regions: Res<Regions>,
    dev: Res<DevOpts>,
    font: Res<UiFont>,
    panel: Single<Entity, With<PlayBox>>,
    mut built: Local<Option<(String, String, usize, Queue, Option<String>)>>,
    added: Query<(), Added<PlayBox>>,
) {
    let q = queue_state(session.as_deref());
    let mode = mode_of(&settings.host_mode);
    let solo = mode == Mode::Solo;
    // only 2v2 is matched by budget
    let direct = straight_in(mode);
    let tier = blocktread_shared::balance::balance().tier(&settings.host_budget).filter(|_| !direct);
    // the vehicle can be changed in the 2v2 pick, but say so if this one won't do; the room and
    // the test range have no pick: they take the vehicle in view, which has to pass its checks
    let cost = hangar.analysis.stats.cost;
    let blocked = if direct { hangar.analysis.first_error().map(|e| format!("「{}」还没通过出车检查：{}", hangar.design.name, e.msg)) } else { None };
    let warn = blocked.clone().or_else(|| tier.filter(|t| cost > t.budget + 1e-3).map(|t| format!("「{}」超出{}上限 {:.0}，匹配到之后要换一辆。", hangar.design.name, t.name, t.budget)));
    let here = region(&settings, &regions, pings.as_deref());
    let want = (settings.host_mode.clone(), settings.host_budget.clone(), here, q, warn.clone());
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
    for (key, label) in [("ffa", Mode::Ffa.label()), ("2v2", Mode::Teams.label()), ("solo", Mode::Solo.label())] {
        let b = spawn_btn(&mut commands, f, seg, Kind::Segment, label, UiAction::HostMode(key), None);
        commands.entity(b).insert(Selected(mode_of(key) == mode));
        set_enabled(&mut commands, b, !busy);
    }
    let desc = match mode {
        Mode::Teams => "蓝红两队各 2 人，抢竞技场中央的高台：只有一方在台上时每秒得 1 分，击毁对手得 5 分，先到 100 分的队获胜。",
        Mode::Solo => "只有你自己：没有对手，不限时间，每次都是同一张沙漠地图。在本机运行，不连服务器。",
        _ => "服务器上的混战房间，在封闭竞技场里打，随时进出，开着当前这辆车直接进场。场上有 2 人以上才开始计时，每局按击毁数排名。",
    };
    commands.spawn((para(f, desc, 12.0, MUTED), ChildOf(modes)));

    if !direct {
        let budget = spawn_section(&mut commands, f, p, "造价上限|BUDGET");
        let seg = commands.spawn((row(0.0), ChildOf(budget))).id();
        let tiers = &blocktread_shared::balance::balance().tiers;
        for (key, label) in std::iter::once((String::new(), "不限".to_string())).chain(tiers.iter().map(|t| (t.key.clone(), format!("{} {:.0}", t.name, t.budget)))) {
            let b = spawn_btn(&mut commands, f, seg, Kind::Segment, label, UiAction::HostBudget(key.clone()), None);
            commands.entity(b).insert(Selected(settings.host_budget == key));
            set_enabled(&mut commands, b, !busy);
        }
    }

    if busy {
        let r = commands.spawn((row(GAP_S), ChildOf(p))).id();
        commands.spawn((spinner(), ChildOf(r)));
        commands.spawn((text(f, "", 13.0, INK), QueueLine, ChildOf(r)));
    } else if let Some(w) = warn {
        commands.spawn((para(f, w, 12.0, WARN), ChildOf(p)));
    }
    let go = commands.spawn((row(GAP_S), ChildOf(p))).id();
    let label = match (busy, mode) {
        (true, Mode::Teams) => "取消匹配",
        (true, _) => "取消",
        (false, Mode::Solo) => "进入试车场",
        (false, Mode::Ffa) => "进入混战",
        (false, _) => "开始匹配",
    };
    let lb = spawn_btn(&mut commands, f, go, Kind::Primary, label, UiAction::Launch, Some("Enter"));
    commands.entity(lb).insert(Node { flex_grow: 1.0, ..btn_node(Kind::Primary) });
    set_enabled(&mut commands, lb, busy || blocked.is_none());
    let wb = spawn_btn(&mut commands, f, go, Kind::Normal, "观战", UiAction::Watch, Some("V"));
    commands.entity(wb).insert((
        Node { height: px(H_LG), padding: UiRect::horizontal(px(16)), ..btn_node(Kind::Normal) },
        Tip("不驾驶，附身到其他玩家身上看他们打".into()),
        TipDetail("混战房间里有人时看混战，没人时看正在打的 2v2；都没有的话在空房间里等人进来。\n← → 换一辆车，C 切换驾驶室视角。".into()),
    ));
    set_enabled(&mut commands, wb, !busy);
    // the server this lobby plays on, its ping, and the way back to pick another
    let line = commands.spawn((row(GAP_S), ChildOf(p))).id();
    if solo {
        commands.spawn((tech(f, "LOCAL", 11.0, MUTED), LetterSpacing::Px(1.5), ChildOf(line)));
        commands.spawn((text(f, "本机运行，不连服务器", 13.0, INK), ChildOf(line)));
    } else if dev.server.is_some() {
        commands.spawn((tech(f, format!("SERVER  {}", server_addr(&dev, &regions, here)), 11.0, MUTED), LetterSpacing::Px(1.5), ChildOf(line)));
    } else {
        let r = &regions.0[here];
        commands.spawn((tech(f, "SERVER", 11.0, MUTED), LetterSpacing::Px(1.5), ChildOf(line)));
        commands.spawn((text(f, format!("{} {}", r.name, r.code), 13.0, INK), ChildOf(line)));
        commands.spawn((tech(f, "…", 13.0, MUTED), RegionPing(here), ChildOf(line)));
        commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(line)));
        let change = spawn_btn(&mut commands, f, line, Kind::Ghost, "更换", UiAction::Goto(Screen::Entry), None);
        set_enabled(&mut commands, change, !busy);
    }
}

/// The queue's progress: time waited, players waiting, when AI fills in.
fn queue_line(session: Option<Res<Session>>, settings: Res<Settings>, mut line: Query<&mut Text, With<QueueLine>>) {
    let Some(s) = session else { return };
    let want = match &s.core.phase {
        _ if mode_of(&settings.host_mode) == Mode::Solo => "正在启动试车场…".to_string(),
        _ if mode_of(&settings.host_mode) == Mode::Ffa => format!("正在进入自由混战 {}…", s.addr),
        Phase::Queued { waited, players, .. } => {
            let t = format!("{}:{:02}", *waited as u32 / 60, *waited as u32 % 60);
            format!("正在匹配 {t} · 队列中 {players} 人")
        }
        _ => format!("正在连接 {}…", s.addr),
    };
    for mut l in &mut line {
        if l.0 != want {
            l.0 = want.clone();
        }
    }
}

/// The session on this screen: queue once connected, and go to the vehicle pick when a match is found.
fn menu_flow(mut commands: Commands, session: Option<ResMut<Session>>, mut next: ResMut<NextState<Screen>>, mut toasts: ResMut<Toasts>) {
    let Some(mut s) = session else { return };
    // the server turned the queue down (a level or tier not reached): say why, and stop waiting
    if let Some(text) = s.core.notice.take() {
        toasts.push(text, ToastKind::Warn);
        if matches!(s.core.phase, Phase::Queued { .. }) {
            s.core.leave();
            commands.remove_resource::<Session>();
            return;
        }
    }
    // queue once logged in: the server holds a guest to a newcomer's rules
    if s.core.state != ConnState::Lobby || s.core.authing {
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
    hangar: Res<Hangar>,
    session: Option<Res<Session>>,
    pings: Option<Res<Pings>>,
    regions: Res<Regions>,
    fields: Query<&EditableText, With<NameField>>,
    mut toasts: ResMut<Toasts>,
    mut next: ResMut<NextState<Screen>>,
    dev: Res<DevOpts>,
    mut focus: ResMut<InputFocus>,
    mut modal: ResMut<ModalReq>,
    mut last: ResMut<LastLaunch>,
    lobby: Res<Lobby>,
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
                // straight in means with the vehicle in view: it has to pass its checks (Enter
                // gets here even while the button is off)
                if !watch
                    && straight_in(mode_of(&settings.host_mode))
                    && let Some(e) = hangar.analysis.first_error()
                {
                    toasts.push(format!("「{}」还没通过出车检查：{}", hangar.design.name, e.msg), ToastKind::Warn);
                    continue;
                }
                // what the server would turn down, said here first (the test range has no rules)
                if !watch
                    && lobby.ranked()
                    && let Some(why) = progression_block(&lobby, mode_of(&settings.host_mode), &settings.host_budget, &hangar)
                {
                    toasts.push(why, ToastKind::Warn);
                    continue;
                }
                last.watch = watch;
                modal.0 = None;
                let name: String = fields.iter().next().map(|e| e.value().to_string().trim().to_string()).unwrap_or_default().chars().take(12).collect();
                settings.name = if name.is_empty() { "车长".into() } else { name };
                settings::save_settings(&settings);
                let solo = !watch && mode_of(&settings.host_mode) == Mode::Solo;
                if solo && hosted.0.is_none() {
                    match host_solo() {
                        Ok(h) => hosted.0 = Some(h),
                        Err(e) => {
                            toasts.push(format!("无法在本机启动试车场：{e:#}"), ToastKind::Err);
                            continue;
                        }
                    }
                }
                let target: Target = match dev.server.as_deref() {
                    _ if solo => format!("127.0.0.1:{}", hosted.0.as_ref().unwrap().addr.port()).into(),
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
                        format!("127.0.0.1:{}", hosted.0.as_ref().unwrap().addr.port()).into()
                    }
                    _ => server_target(&dev, &regions, region(&settings, &regions, pings.as_deref())),
                };
                let addr = target.addr.clone();
                info!("{} {addr}", if watch { "watching" } else { "matchmaking on" });
                let latency = Duration::from_millis(dev.latency_ms);
                let mut core = if watch { ClientCore::watch(target, &settings.name, latency) } else { ClientCore::connect(target, &settings.name, latency) };
                // the match connection logs in too, so the match pays the account
                if !watch
                    && !solo
                    && let Some((key, _)) = link_target(&dev, &regions, &settings, pings.as_deref())
                    && let Some(token) = saved_token(&settings, &key)
                {
                    core.auth(AuthReq::Resume { token });
                }
                let mode = mode_of(&settings.host_mode);
                let tier = (!settings.host_budget.is_empty() && !straight_in(mode)).then(|| settings.host_budget.clone());
                let want = (!watch).then_some((mode, tier));
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

/// Why the server would turn this down for the player's level and parts (None: it wouldn't).
fn progression_block(lobby: &Lobby, mode: Mode, budget: &str, hangar: &Hangar) -> Option<String> {
    let (e, level) = (economy(), lobby.level());
    match mode {
        Mode::Teams if level < e.teams_level => Some(format!("2v2 {} 级开放，你现在 {level} 级。先在自由混战里升级吧", e.teams_level)),
        Mode::Teams if !e.tier_open((!budget.is_empty()).then_some(budget), level) => Some(format!("这个档位要 {} 级，你现在 {level} 级", e.tier_levels.get(budget).copied().unwrap_or(1))),
        Mode::Ffa => {
            let locked = e.locked(&hangar.design, lobby.unlocks());
            if !locked.is_empty() {
                let names: Vec<&str> = locked.iter().map(|t| t.info().name.as_str()).collect();
                return Some(format!("「{}」用了还没解锁的部件：{}。可以先在试车场里试", hangar.design.name, names.join("、")));
            }
            match e.budget_at(level) {
                Some(b) if hangar.analysis.stats.cost > b + 1e-3 => Some(format!("你现在 {level} 级，混战里最多开造价 {b:.0} 的车")),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The top bar's account corner and the callsign field, kept to who is logged in.
fn account_bar(mut commands: Commands, lobby: Res<Lobby>, font: Res<UiFont>, boxes: Single<Entity, With<AccountBox>>, mut callsign: Single<&mut Node, With<CallsignBox>>, mut shown: Local<Option<(bool, Option<(String, u32, u32)>)>>) {
    let now = (lobby.ranked(), lobby.account().map(|(u, p)| (u.clone(), lobby.level(), p.coins)));
    if shown.as_ref() == Some(&now) {
        return;
    }
    *shown = Some(now.clone());
    let f = &*font;
    let b = *boxes;
    commands.entity(b).despawn_children();
    callsign.display = if now.1.is_some() { Display::None } else { Display::Flex };
    match now {
        (false, _) => {}
        (true, None) => {
            spawn_btn(&mut commands, f, b, Kind::Accent, "登录 / 注册", UiAction::OpenAccount, Some("L"));
        }
        (true, Some((user, level, coins))) => {
            commands.spawn((strong(f, user, 15.0, INK), ChildOf(b)));
            commands.spawn((chip(f, format!("{level} 级"), ACCENT), ChildOf(b)));
            commands.spawn((tech(f, format!("金币 {coins}"), 14.0, INK), ChildOf(b)));
            spawn_btn(&mut commands, f, b, Kind::Normal, "账号", UiAction::OpenAccount, Some("L"));
        }
    }
}

/// The test range's server: inside this client, reachable from this machine only, on any free port.
fn host_solo() -> anyhow::Result<blocktread_server::ServerHandle> {
    let hub = HubConfig { queue_wait: 0.0, ..HubConfig::default() };
    blocktread_server::start(ServerConfig { bind: "127.0.0.1:0".parse().unwrap(), hub, verbose: false, status: None, data: None })
}

/// A server inside this client (`--server local`): no queue wait, AI fills the match at once.
fn host_local(dev: &DevOpts) -> anyhow::Result<blocktread_server::ServerHandle> {
    let host = if dev.local_only { "127.0.0.1" } else { "0.0.0.0" };
    let hub = HubConfig { queue_wait: 0.0, ..HubConfig::default() };
    let at = |port: u16| ServerConfig { bind: format!("{host}:{port}").parse().unwrap(), hub: hub.clone(), verbose: false, status: None, data: None };
    match blocktread_server::start(at(dev.port.unwrap_or(blocktread_shared::DEFAULT_PORT))) {
        Ok(h) => Ok(h),
        // the port is taken (another server on this machine): any free one will do
        Err(_) if dev.port.is_none() => blocktread_server::start(at(0)),
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
                w.write(UiAction::HostMode(match m {
                    "2v2" => "2v2",
                    "solo" => "solo",
                    _ => "ffa",
                }));
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
        w.write(if session.is_some() { UiAction::Launch } else { UiAction::Goto(Screen::Entry) });
    }
    if typing_now {
        return;
    }
    for (k, a) in [
        (KeyCode::KeyH, UiAction::Goto(Screen::Hangar)),
        (KeyCode::KeyS, UiAction::OpenSettings),
        (KeyCode::KeyL, UiAction::OpenAccount),
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
