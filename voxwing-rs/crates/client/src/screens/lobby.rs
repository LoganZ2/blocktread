//! Mode select: what to play (free-for-all or 2v2), with what budget, where (this machine or a
//! server). Starting connects and queues; the match screen takes it from there.

use super::Overlays;
use crate::flight::Session;
use crate::settings::{self, Settings};
use crate::ui::kit::*;
use crate::ui::*;
use crate::{DevOpts, Hosted, LastError, Screen};
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::{EditableText, LetterSpacing};
use std::time::Duration;
use voxwing_client_core::ClientCore;
use voxwing_server::{HubConfig, ServerConfig};
use voxwing_shared::protocol::Mode;

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastLaunch>()
            .add_systems(OnEnter(Screen::Lobby), (spawn_lobby, show_last_error))
            .add_systems(Update, (lobby_actions, lobby_keys, rebuild_server, refresh_lobby).chain().run_if(in_state(Screen::Lobby)));
    }
}

/// Whether the last session was played or watched, so "retry" after an error does the same.
#[derive(Resource, Default)]
struct LastLaunch {
    watch: bool,
}

#[derive(Component)]
struct ServerBody;
#[derive(Component)]
struct LaunchNote;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum LField {
    Name,
    Server,
}
#[derive(Component)]
struct ModeCard(&'static str);
#[derive(Component)]
struct BudgetSeg(String);
#[derive(Component)]
struct WhereSeg(bool);

/// The mode a settings key stands for ("coop" from older settings files plays free-for-all).
pub fn mode_of(key: &str) -> Mode {
    if key == "2v2" { Mode::Teams } else { Mode::Ffa }
}

fn spawn_lobby(mut commands: Commands, font: Res<UiFont>, settings: Res<Settings>) {
    let f = &*font;
    let scope = DespawnOnExit(Screen::Lobby);
    let root = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, Pickable::IGNORE, IgnoreHover, scope)).id();

    // top bar
    let top = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(EDGE), right: px(EDGE), top: px(EDGE), height: px(H_MD), column_gap: px(GAP), align_items: AlignItems::Center, ..default() },
            IgnoreHover,
            ChildOf(root),
        ))
        .id();
    spawn_btn(&mut commands, f, top, Kind::Ghost, "← 主菜单", UiAction::Goto(Screen::Menu), Some("Esc"));
    spawn_title(&mut commands, f, top, "开始游戏", "PLAY");
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(top)));
    commands.spawn((tech(f, "CALLSIGN", 11.0, MUTED), LetterSpacing::Px(3.0), ChildOf(top)));
    let name = spawn_field(&mut commands, f, top, &settings.name, 10.0);
    commands.entity(name).insert(LField::Name);

    let panel_top = EDGE + H_MD + GAP;

    // left: how a match comes together
    let left = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), bottom: px(EDGE), width: px(300), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP_S), ..default() });
    commands.entity(left).insert(SlideIn::new(0.05, -40.0, 0.0));
    let sec = spawn_section(&mut commands, f, left, "流程|HOW IT WORKS");
    for (i, (step, note)) in [("选择模式", "自由混战或 2v2"), ("匹配", "凑不齐人时 AI 补位"), ("选择战机", "能看到其他人选的飞机"), ("开战", "全员确认或倒计时结束")].into_iter().enumerate() {
        let r = commands.spawn((row(12.0), ChildOf(sec))).id();
        commands.spawn((tech(f, format!("{:02}", i + 1), 14.0, ACCENT), Node { width: px(20), ..default() }, ChildOf(r)));
        commands.spawn((text(f, step, 14.0, INK), ChildOf(r)));
        commands.spawn((text(f, note, 12.0, MUTED), Node { margin: UiRect::left(Val::Auto), ..default() }, ChildOf(r)));
    }

    // right: mode, budget, server
    let right = spawn_panel(
        &mut commands,
        Some(root),
        Node { position_type: PositionType::Absolute, right: px(EDGE), top: px(panel_top), bottom: px(EDGE), width: px(440), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() },
    );
    commands.entity(right).insert(SlideIn::new(0.1, 40.0, 0.0));
    let body = spawn_scroll_body(&mut commands, right);
    let modes = spawn_section(&mut commands, f, body, "模式|GAME MODE");
    for (key, title, en, desc) in [
        ("ffa", "自由混战", "FREE-FOR-ALL", "6 个席位各自为战，阵亡 4 秒后重生。先达到击毁上限，或时间结束时击毁最多的人获胜。"),
        ("2v2", "2v2 团队战", "TEAM 2V2", "蓝红两队各 2 人，两队分别从竞技场两侧出发。先达到击毁上限的队伍获胜。"),
    ] {
        let card = spawn_btn(&mut commands, f, modes, Kind::Card, title, UiAction::HostMode(key), None);
        commands.entity(card).insert(ModeCard(key));
        commands.spawn((tech(f, en, 11.0, MUTED), LetterSpacing::Px(3.0), Node { position_type: PositionType::Absolute, right: px(16), top: px(19), ..default() }, Pickable::IGNORE, ChildOf(card)));
        commands.spawn((para(f, desc, 13.0, MUTED), Pickable::IGNORE, ChildOf(card)));
    }
    commands.spawn((para(f, "人不够时空位由 AI 补齐，不用等。", 12.0, MUTED), ChildOf(modes)));
    // only ships within the budget can be picked; AI flies ships that fit too
    let budget = spawn_section(&mut commands, f, body, "造价上限|BUDGET");
    let seg = commands.spawn((row(0.0), ChildOf(budget))).id();
    let tiers = &voxwing_shared::balance::balance().tiers;
    for (key, label) in std::iter::once((String::new(), "不限".to_string())).chain(tiers.iter().map(|t| (t.key.clone(), format!("{} {:.0}", t.name, t.budget)))) {
        let s = spawn_btn(&mut commands, f, seg, Kind::Segment, label, UiAction::HostBudget(key.clone()), None);
        commands.entity(s).insert(BudgetSeg(key));
    }
    let srv = spawn_section(&mut commands, f, body, "服务器|SERVER");
    let seg = commands.spawn((row(0.0), ChildOf(srv))).id();
    for (online, label) in [(false, "本机"), (true, "在线服务器")] {
        let s = spawn_btn(&mut commands, f, seg, Kind::Segment, label, UiAction::Online(online), None);
        commands.entity(s).insert(WhereSeg(online));
    }
    commands.spawn((col(GAP_S), ServerBody, ChildOf(srv)));

    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(right)));
    commands.spawn((text(f, "", 13.0, MUTED), LaunchNote, ChildOf(right)));
    let go = commands.spawn((row(GAP_S), ChildOf(right))).id();
    let lb = spawn_btn(&mut commands, f, go, Kind::Primary, "开始匹配", UiAction::Launch, Some("Enter"));
    commands.entity(lb).insert(Node { flex_grow: 1.0, ..btn_node(Kind::Primary) });
    let wb = spawn_btn(&mut commands, f, go, Kind::Normal, "观战", UiAction::Watch, Some("V"));
    commands.entity(wb).insert((
        Node { height: px(H_LG), padding: UiRect::horizontal(px(18)), ..btn_node(Kind::Normal) },
        Tip("不驾驶，附身到 AI 或其他玩家身上看他们打".into()),
        TipDetail("服务器上有对局时观看它；没有的话开一局全 AI 的自由混战。\n← → 换一架飞机，C 切换座舱视角。".into()),
    ));
}

/// The server section's body: the address and recent servers online, a note on local play.
#[allow(clippy::too_many_arguments)]
fn rebuild_server(
    mut commands: Commands,
    settings: Res<Settings>,
    hosted: Res<Hosted>,
    body: Single<Entity, With<ServerBody>>,
    font: Res<UiFont>,
    fields: Query<(&EditableText, &LField)>,
    mut built: Local<Option<bool>>,
    added: Query<(), Added<ServerBody>>,
) {
    if *built == Some(settings.online) && added.is_empty() {
        return;
    }
    *built = Some(settings.online);
    let f = &*font;
    let b = *body;
    commands.entity(b).despawn_children();
    if !settings.online {
        let ips = lan_ips();
        let port = hosted.0.as_ref().map_or(voxwing_shared::DEFAULT_PORT, |h| h.addr.port());
        let lan = if ips.is_empty() { String::new() } else { format!("\n同一局域网的朋友选「在线服务器」，填 {} 就能匹配到你这里。", ips.iter().map(|ip| format!("{ip}:{port}")).collect::<Vec<_>>().join(" 或 ")) };
        commands.spawn((para(f, format!("在本机运行服务器，马上开始，空位全部由 AI 补齐。{lan}"), 13.0, MUTED), ChildOf(b)));
        return;
    }
    let server_text = fields.iter().find(|(_, k)| **k == LField::Server).map(|(e, _)| e.value().to_string()).unwrap_or_else(|| settings.server.clone());
    let fld = spawn_field(&mut commands, f, b, &server_text, 22.0);
    commands.entity(fld).insert(LField::Server);
    commands.spawn((para(f, "格式 IP:端口，例如 192.168.1.20:4433", 12.0, MUTED), ChildOf(b)));
    for addr in &settings.recent {
        let rb = spawn_btn(&mut commands, f, b, Kind::Normal, addr.clone(), UiAction::RecentServer(addr.clone()), None);
        // text lines up with the address field above (12px inset)
        commands.entity(rb).insert(Node { height: px(H_MD), padding: UiRect::horizontal(px(12)), justify_content: JustifyContent::FlexStart, align_items: AlignItems::Center, ..default() });
    }
}

fn refresh_lobby(settings: Res<Settings>, mut segs: Query<(&mut Selected, Option<&ModeCard>, Option<&BudgetSeg>, Option<&WhereSeg>)>, mut note: Single<&mut Text, With<LaunchNote>>) {
    let mode = mode_of(&settings.host_mode);
    for (mut sel, card, budget, place) in &mut segs {
        let on = match (card, budget, place) {
            (Some(c), _, _) => mode_of(c.0) == mode,
            (_, Some(b), _) => settings.host_budget == b.0,
            (_, _, Some(w)) => settings.online == w.0,
            _ => continue,
        };
        if sel.0 != on {
            sel.0 = on;
        }
    }
    let tier = voxwing_shared::balance::balance().tier(&settings.host_budget).map_or("造价不限".to_string(), |t| format!("{}，造价 ≤ {:.0}", t.name, t.budget));
    let want = format!("{} · {tier} · {}", mode.label(), if settings.online { "在线匹配" } else { "本机，AI 补位" });
    if note.0 != want {
        note.0 = want;
    }
}

#[allow(clippy::too_many_arguments)]
fn lobby_actions(
    mut commands: Commands,
    mut msgs: MessageReader<UiAction>,
    mut settings: ResMut<Settings>,
    mut hosted: ResMut<Hosted>,
    mut fields: Query<(&mut EditableText, &LField)>,
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
            UiAction::Online(on) => settings.online = on,
            UiAction::RecentServer(addr) => {
                for (mut e, k) in &mut fields {
                    if *k == LField::Server {
                        *e = EditableText { visible_width: e.visible_width, allow_newlines: false, ..EditableText::new(&addr) };
                    }
                }
            }
            m @ (UiAction::Launch | UiAction::Watch) => {
                let watch = m == UiAction::Watch;
                last.watch = watch;
                modal.0 = None;
                let read = |k: LField, fields: &Query<(&mut EditableText, &LField)>| fields.iter().find(|(_, f)| **f == k).map(|(e, _)| e.value().to_string().trim().to_string());
                let name: String = read(LField::Name, &fields).unwrap_or_default().chars().take(12).collect();
                settings.name = if name.is_empty() { "飞行员".into() } else { name };
                let addr = if settings.online {
                    let a = read(LField::Server, &fields).unwrap_or_else(|| settings.server.clone());
                    if a.is_empty() {
                        toasts.push("先填写服务器地址", ToastKind::Warn);
                        continue;
                    }
                    settings.server = a.clone();
                    settings.remember_server(&a);
                    a
                } else {
                    if hosted.0.is_none() {
                        match host_local(&dev) {
                            Ok(h) => {
                                info!("local server on {}", h.addr);
                                hosted.0 = Some(h);
                            }
                            Err(e) => {
                                toasts.push(format!("无法在本机启动服务器：{e:#}"), ToastKind::Err);
                                continue;
                            }
                        }
                    }
                    format!("127.0.0.1:{}", hosted.0.as_ref().unwrap().addr.port())
                };
                settings::save_settings(&settings);
                info!("{} {addr}", if watch { "watching" } else { "matchmaking on" });
                let latency = Duration::from_millis(dev.latency_ms);
                let core = if watch { ClientCore::watch(&addr, &settings.name, latency) } else { ClientCore::connect(&addr, &settings.name, latency) };
                let tier = (!settings.host_budget.is_empty()).then(|| settings.host_budget.clone());
                let want = (!watch).then(|| (mode_of(&settings.host_mode), tier));
                commands.insert_resource(Session::new(core, addr, want));
                focus.clear();
                next.set(Screen::Match);
            }
            _ => {}
        }
    }
}

/// The server for local play: no waiting in the queue, so AI fills the match at once. It listens
/// on the LAN too (friends can queue on it), on the usual port if that is free.
fn host_local(dev: &DevOpts) -> anyhow::Result<voxwing_server::ServerHandle> {
    let host = if dev.local_only { "127.0.0.1" } else { "0.0.0.0" };
    let hub = HubConfig { queue_wait: 0.0, ..HubConfig::default() };
    let at = |port: u16| ServerConfig { bind: format!("{host}:{port}").parse().unwrap(), hub: hub.clone(), verbose: false };
    match voxwing_server::start(at(dev.port.unwrap_or(voxwing_shared::DEFAULT_PORT))) {
        Ok(h) => Ok(h),
        // the port is taken (another server on this machine): any free one does for local play
        Err(_) if dev.port.is_none() => voxwing_server::start(at(0)),
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
fn lobby_keys(
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    modal: Res<ModalReq>,
    focus: Res<InputFocus>,
    editing: Query<(), With<EditableText>>,
    mut w: MessageWriter<UiAction>,
    dev: Res<DevOpts>,
    time: Res<Time>,
    mut auto: Local<bool>,
) {
    if !*auto && time.elapsed_secs() > 1.5 {
        *auto = true;
        if let Some(l) = &dev.auto_launch {
            let (what, join) = l.strip_suffix("-join").map_or((l.as_str(), false), |w| (w, true));
            w.write(UiAction::Online(join));
            match what {
                "watch" => {
                    w.write(UiAction::Watch);
                }
                m => {
                    w.write(UiAction::HostMode(if m == "2v2" { "2v2" } else { "ffa" }));
                    w.write(UiAction::Launch);
                }
            }
            return;
        }
        if dev.start.as_deref() == Some("join") {
            w.write(UiAction::Online(true));
        }
    }
    if overlays.any(&modal) {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        w.write(UiAction::Goto(Screen::Menu));
    }
    if keys.just_pressed(KeyCode::Enter) && (focus.get().is_none() || typing(&focus, &editing)) {
        w.write(UiAction::Launch);
    }
    if keys.just_pressed(KeyCode::KeyV) && focus.get().is_none() {
        w.write(UiAction::Watch);
    }
}

/// "115 · 中型": a ship's cost and the cheapest tier it fits.
pub fn cost_label(cost: f32) -> String {
    match voxwing_shared::balance::balance().tier_for(cost) {
        Some(t) => format!("{cost:.0} · {}", t.name),
        None => format!("{cost:.0} · 超重型"),
    }
}

/// When a session ended with an error, explain it and offer a retry.
fn show_last_error(mut last: ResMut<LastError>, mut modal: ResMut<ModalReq>, launch: Res<LastLaunch>) {
    if let Some((title, body)) = last.0.take() {
        let retry = if launch.watch { UiAction::Watch } else { UiAction::Launch };
        modal.show(title, body, vec![("返回", UiAction::CloseModal, Kind::Normal), ("重试", retry, Kind::Accent)], None);
    }
}

/// Private IPv4 addresses of this machine (skips loopback and the 198.18/15 range that proxy and
/// VPN tunnels use).
pub fn lan_ips() -> Vec<std::net::Ipv4Addr> {
    let mut out: Vec<std::net::Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v) if v.is_private() => Some(v),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}
