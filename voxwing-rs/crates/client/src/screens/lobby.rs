//! Launch prep: check the ship, choose to host a room (co-op or free-for-all) or join one, launch.

use super::Overlays;
use crate::hangar::Hangar;
use crate::settings::{self, Settings};
use crate::ui::kit::*;
use crate::ui::*;
use crate::{DevOpts, Hosted, LastError, Screen};
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::{EditableText, LetterSpacing};
use std::time::Duration;
use voxwing_client_core::ClientCore;
use voxwing_shared::protocol::Mode;

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LobbyTab>()
            .init_resource::<LastLaunch>()
            .add_systems(OnEnter(Screen::Lobby), (spawn_lobby, show_last_error))
            .add_systems(Update, (lobby_actions, lobby_keys, rebuild_tab, refresh_lobby).chain().run_if(in_state(Screen::Lobby)));
    }
}

/// 0 = create a room, 1 = join a room.
#[derive(Resource, Default, PartialEq)]
pub struct LobbyTab(pub u8);

/// Whether the last session was flown or watched, so "retry" after an error does the same.
#[derive(Resource, Default)]
struct LastLaunch {
    watch: bool,
}

#[derive(Component)]
struct TabBody;
#[derive(Component)]
struct LaunchBtn;
#[derive(Component)]
struct LaunchNote;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum LField {
    Name,
    Server,
}
#[derive(Component)]
struct ModeCard(&'static str);

fn spawn_lobby(mut commands: Commands, font: Res<UiFont>, hangar: Res<Hangar>, settings: Res<Settings>, mut tab: ResMut<LobbyTab>) {
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
    spawn_title(&mut commands, f, top, "出击准备", "LAUNCH PREP");
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(top)));
    commands.spawn((tech(f, "CALLSIGN", 11.0, MUTED), LetterSpacing::Px(3.0), ChildOf(top)));
    let name = spawn_field(&mut commands, f, top, &settings.name, 10.0);
    commands.entity(name).insert(LField::Name);

    let panel_top = EDGE + H_MD + GAP;

    // left: your ship
    let a = &hangar.analysis;
    let s = &a.stats;
    let left = spawn_panel(&mut commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), top: px(panel_top), width: px(344), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(left).insert(SlideIn::new(0.05, -40.0, 0.0));
    let sec = spawn_section(&mut commands, f, left, "你的战机|YOUR SHIP");
    let r = commands.spawn((Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, margin: UiRect::bottom(px(4)), ..default() }, ChildOf(sec))).id();
    commands.spawn((heading(f, hangar.design.name.clone(), 22.0, INK), ChildOf(r)));
    if a.ok() {
        commands.spawn((chip(f, "可以出击", OK), ChildOf(r)));
    } else {
        commands.spawn((chip(f, format!("{} 项待修改", a.errors), WARN), ChildOf(r)));
    }
    for (label, frac, val, c) in [
        ("速度", s.max_speed / 125.0, format!("{:.0} m/s", s.max_speed), ACCENT),
        ("转向", s.pitch_rate / 2.0, format!("{:.0} 度/秒", s.pitch_rate.to_degrees()), ACCENT),
        ("火力", s.dps / 200.0, format!("{:.0} DPS", s.dps), SIGNAL),
        ("持续", s.dps_sustained / 200.0, format!("{:.0} DPS", s.dps_sustained), SIGNAL),
        ("耐久", s.hp / 3500.0, format!("{:.0}", s.hp), OK),
        ("护盾", s.shield_cap / 200.0, if s.shield_cap > 0.0 { format!("{:.0}", s.shield_cap) } else { "无".into() }, Color::srgb(0.4, 0.62, 1.0)),
        ("能量", s.energy_cap / 300.0, format!("{:.0}", s.energy_cap), WARN),
    ] {
        let (gauge, v) = spawn_stat(&mut commands, f, sec, label, c);
        commands.entity(gauge).insert(Plate::gauge(c, 24.0, frac.clamp(0.0, 1.0)));
        commands.entity(v).insert(Text::new(val));
    }
    if let Some(e) = a.first_error() {
        commands.spawn((para(f, format!("需要先修改：{}", e.msg), 13.0, WARN), ChildOf(left)));
    }
    let br = commands.spawn((row(GAP_S), ChildOf(left))).id();
    spawn_btn(&mut commands, f, br, Kind::Normal, "在机库里修改", UiAction::Goto(Screen::Hangar), None);
    spawn_btn(&mut commands, f, br, Kind::Ghost, "换一架…", UiAction::OpenLoad, None);

    // right: room
    let right = spawn_panel(
        &mut commands,
        Some(root),
        Node { position_type: PositionType::Absolute, right: px(EDGE), top: px(panel_top), bottom: px(EDGE), width: px(440), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() },
    );
    commands.entity(right).insert(SlideIn::new(0.1, 40.0, 0.0));
    let seg = commands.spawn((row(0.0), ChildOf(right))).id();
    spawn_btn(&mut commands, f, seg, Kind::Segment, "创建房间", UiAction::LobbyTab(0), None);
    spawn_btn(&mut commands, f, seg, Kind::Segment, "加入房间", UiAction::LobbyTab(1), None);
    commands.spawn((Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() }, TabBody, ChildOf(right)));
    commands.spawn((text(f, "", 13.0, MUTED), LaunchNote, ChildOf(right)));
    let go = commands.spawn((row(GAP_S), ChildOf(right))).id();
    let lb = spawn_btn(&mut commands, f, go, Kind::Primary, "出击", UiAction::Launch, Some("Enter"));
    commands.entity(lb).insert((LaunchBtn, Node { flex_grow: 1.0, ..btn_node(Kind::Primary) }));
    // spectating needs no flyable ship, so this stays enabled when the launch button isn't
    let wb = spawn_btn(&mut commands, f, go, Kind::Normal, "观战", UiAction::Watch, Some("V"));
    commands.entity(wb).insert((
        Node { height: px(H_LG), padding: UiRect::horizontal(px(18)), ..btn_node(Kind::Normal) },
        Tip("不驾驶，附身到 AI 或其他玩家身上看他们打".into()),
        TipDetail("本机开房观战时用自由混战，至少 2 架 AI 互相打。\n← → 换一架飞机，C 切换座舱视角。".into()),
    ));
    tab.set_changed();
}

/// Rebuild the tab body when the tab changes (or on entering the screen).
#[allow(clippy::too_many_arguments)]
fn rebuild_tab(mut commands: Commands, tab: Res<LobbyTab>, body: Single<Entity, With<TabBody>>, font: Res<UiFont>, settings: Res<Settings>, hosted: Res<Hosted>, fields: Query<(&EditableText, &LField)>) {
    if !tab.is_changed() {
        return;
    }
    let f = &*font;
    let b = *body;
    commands.entity(b).despawn_children();
    let server_text = fields.iter().find(|(_, k)| **k == LField::Server).map(|(e, _)| e.value().to_string()).unwrap_or_else(|| settings.server.clone());
    if tab.0 == 0 {
        let modes = spawn_section(&mut commands, f, b, "房间模式|GAME MODE");
        if let Some(h) = &hosted.0 {
            commands.spawn((para(f, format!("你的房间正在运行（{}，端口 {}）。出击会直接加入它；要换模式，先关闭房间。", h.mode.label(), h.addr.port()), 14.0, INK), ChildOf(modes)));
            let r = commands.spawn((row(0.0), ChildOf(modes))).id();
            spawn_btn(&mut commands, f, r, Kind::Danger, "关闭房间", UiAction::StopHosting, None);
        } else {
            for (key, title, en, desc) in [
                ("coop", "合作波次", "CO-OP WAVES", "和朋友并肩作战。AI 编队一波比一波强，每清完一波全员修复，阵亡的队友会在下一波重生。"),
                ("ffa", "自由混战", "FREE-FOR-ALL", "所有人互相攻击，AI 补位。阵亡 4 秒后在随机位置重生，按击毁数计分。"),
            ] {
                let card = spawn_btn(&mut commands, f, modes, Kind::Card, title, UiAction::HostMode(key), None);
                commands.entity(card).insert(ModeCard(key));
                commands.spawn((tech(f, en, 11.0, MUTED), LetterSpacing::Px(3.0), Node { position_type: PositionType::Absolute, right: px(16), top: px(19), ..default() }, Pickable::IGNORE, ChildOf(card)));
                commands.spawn((para(f, desc, 13.0, MUTED), Pickable::IGNORE, ChildOf(card)));
            }
            let bots = spawn_section(&mut commands, f, b, "AI 数量|BOTS · 自由混战");
            spawn_slider(&mut commands, f, bots, "AI 战机", settings.bots as f32, 0.0, 8.0, SetKey::Bots, |v| format!("{:.0} 架", v.round()));
        }
        let ips = lan_ips();
        let lan = if ips.is_empty() { "没有检测到局域网地址。".to_string() } else { format!("朋友在「加入房间」里填：{}", ips.iter().map(|ip| format!("{ip}:{}", voxwing_shared::DEFAULT_PORT)).collect::<Vec<_>>().join(" 或 ")) };
        let net = spawn_section(&mut commands, f, b, "局域网|LAN");
        commands.spawn((para(f, lan, 13.0, MUTED), ChildOf(net)));
    } else {
        let srv = spawn_section(&mut commands, f, b, "服务器地址|SERVER");
        let fld = spawn_field(&mut commands, f, srv, &server_text, 22.0);
        commands.entity(fld).insert(LField::Server);
        commands.spawn((para(f, "格式 IP:端口，例如 192.168.1.20:4433\n服务器需要放行 UDP 端口", 13.0, MUTED), ChildOf(srv)));
        let rec = spawn_section(&mut commands, f, b, "最近加入|RECENT");
        if settings.recent.is_empty() {
            commands.spawn((text(f, "还没有记录。", 13.0, MUTED), ChildOf(rec)));
        }
        for addr in &settings.recent {
            let rb = spawn_btn(&mut commands, f, rec, Kind::Normal, addr.clone(), UiAction::RecentServer(addr.clone()), None);
            // text lines up with the address field above (12px inset)
            commands.entity(rb).insert(Node { height: px(H_MD), padding: UiRect::horizontal(px(12)), justify_content: JustifyContent::FlexStart, align_items: AlignItems::Center, ..default() });
        }
    }
}

fn refresh_lobby(
    mut commands: Commands,
    tab: Res<LobbyTab>,
    settings: Res<Settings>,
    hangar: Res<Hangar>,
    mut segs: Query<(&ActionOf, &mut Selected, Option<&ModeCard>)>,
    launch: Single<Entity, With<LaunchBtn>>,
    mut note: Single<&mut Text, With<LaunchNote>>,
) {
    for (a, mut sel, card) in &mut segs {
        let on = match (&a.0, card) {
            (UiAction::LobbyTab(t), _) => *t == tab.0,
            (_, Some(c)) => settings.host_mode == c.0,
            _ => continue,
        };
        if sel.0 != on {
            sel.0 = on;
        }
    }
    let ok = hangar.analysis.ok();
    set_enabled(&mut commands, *launch, ok);
    let want = if !ok {
        "机体没有通过起飞检查，先去机库修改。"
    } else if tab.0 == 0 {
        "在本机开房间并加入。单人也可以玩。"
    } else {
        "连接到朋友或独立服务器的房间。"
    };
    if note.0 != want {
        note.0 = want.into();
    }
}

#[allow(clippy::too_many_arguments)]
fn lobby_actions(
    mut commands: Commands,
    mut msgs: MessageReader<UiAction>,
    mut tab: ResMut<LobbyTab>,
    mut settings: ResMut<Settings>,
    mut hosted: ResMut<Hosted>,
    hangar: Res<Hangar>,
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
            UiAction::LobbyTab(t) => tab.0 = t,
            UiAction::HostMode(k) => settings.host_mode = k.to_string(),
            UiAction::StopHosting => {
                hosted.0 = None;
                toasts.push("房间已关闭", ToastKind::Info);
                tab.set_changed();
            }
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
                if !watch && !hangar.analysis.ok() {
                    toasts.push("机体没有通过起飞检查", ToastKind::Warn);
                    continue;
                }
                let read = |k: LField, fields: &Query<(&mut EditableText, &LField)>| fields.iter().find(|(_, f)| **f == k).map(|(e, _)| e.value().to_string().trim().to_string());
                let name: String = read(LField::Name, &fields).unwrap_or_default().chars().take(12).collect();
                settings.name = if name.is_empty() { "飞行员".into() } else { name };
                let addr = if tab.0 == 0 {
                    if hosted.0.is_none() {
                        // a co-op room with nobody flying never starts a wave: watch free-for-all
                        let mode = if watch || settings.host_mode == "ffa" { Mode::Ffa } else { Mode::Coop };
                        let bots = if watch { settings.bots.max(2) } else { settings.bots };
                        let host = if dev.local_only { "127.0.0.1" } else { "0.0.0.0" };
                        let port = dev.port.unwrap_or(voxwing_shared::DEFAULT_PORT);
                        let cfg = voxwing_server::ServerConfig { bind: format!("{host}:{port}").parse().unwrap(), mode, bots, verbose: false };
                        match voxwing_server::start(cfg) {
                            Ok(h) => {
                                info!("hosting room on {}", h.addr);
                                hosted.0 = Some(h);
                            }
                            Err(e) => {
                                toasts.push(format!("无法创建房间：{e:#}"), ToastKind::Err);
                                continue;
                            }
                        }
                    }
                    format!("127.0.0.1:{}", hosted.0.as_ref().unwrap().addr.port())
                } else {
                    let a = read(LField::Server, &fields).unwrap_or_else(|| settings.server.clone());
                    if a.is_empty() {
                        toasts.push("先填写服务器地址", ToastKind::Warn);
                        continue;
                    }
                    settings.server = a.clone();
                    settings.remember_server(&a);
                    a
                };
                settings::save_settings(&settings);
                let mut design = hangar.design.clone();
                design.name = design.sanitized_name();
                info!("{} {addr}", if watch { "watching" } else { "connecting to" });
                let latency = Duration::from_millis(dev.latency_ms);
                let core = if watch { ClientCore::watch(&addr, &settings.name, latency) } else { ClientCore::connect(&addr, &settings.name, design, latency) };
                commands.insert_resource(crate::flight::Session::new(core, addr));
                focus.clear();
                next.set(Screen::Flight);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn lobby_keys(keys: Res<ButtonInput<KeyCode>>, overlays: Res<Overlays>, modal: Res<ModalReq>, focus: Res<InputFocus>, editing: Query<(), With<EditableText>>, mut w: MessageWriter<UiAction>, dev: Res<DevOpts>, time: Res<Time>, mut auto: Local<bool>) {
    if !*auto && time.elapsed_secs() > 1.5 {
        *auto = true;
        if let Some(l) = &dev.auto_launch {
            if l.ends_with("join") {
                w.write(UiAction::LobbyTab(1));
            }
            w.write(if l.starts_with("watch") { UiAction::Watch } else { UiAction::Launch });
            return;
        }
        if dev.start.as_deref() == Some("join") {
            w.write(UiAction::LobbyTab(1));
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

/// When a flight ended with an error, explain it and offer a retry.
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
