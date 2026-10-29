//! The login page, between the entry page and the lobby: log in to the server picked (each server
//! keeps its own accounts), sign up on it, or go on as a guest. A saved login goes through by
//! itself and straight on to the lobby; so does a server with no accounts (one inside this
//! client). Laid out like the entry page: a column down the left over the parked vehicle.

use super::Overlays;
use super::entry::{Pings, ping, ping_text, region};
use crate::account::{AccountSystems, Link, Lobby, link_target, spawn_secret_field};
use crate::settings::{Regions, Settings};
use crate::ui::kit::*;
use crate::ui::*;
use crate::{DevOpts, Screen};
use bevy::input_focus::AutoFocus;
use bevy::prelude::*;
use bevy::text::{EditableText, LetterSpacing};
use blocktread_shared::protocol::AuthReq;

pub struct LoginPlugin;

impl Plugin for LoginPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Form>()
            .add_systems(OnEnter(Screen::Login), spawn_login)
            .add_systems(Update, (login_flow, login_actions, login_keys, refresh_login).chain().after(AccountSystems).run_if(in_state(Screen::Login)));
    }
}

/// Which form is up: logging in, or signing up (which asks for the password twice).
#[derive(Resource, Default)]
struct Form {
    register: bool,
}

#[derive(Component)]
struct UserField;
#[derive(Component)]
struct PassField;
#[derive(Component)]
struct ConfirmField;
/// The second password field and its label, shown when signing up.
#[derive(Component)]
struct ConfirmRow;
#[derive(Component)]
struct Status;
#[derive(Component)]
struct Title;
#[derive(Component)]
struct Submit;
#[derive(Component)]
struct Tab(bool);
#[derive(Component)]
struct ServerPing;

const COLUMN: f32 = 560.0;
const INSET: f32 = 56.0;
/// Minimum characters in a password (the server says the same).
const PASS_MIN: usize = 6;

/// A label over a field, the pair kept together.
fn labelled(commands: &mut Commands, f: &UiFont, parent: Entity, label: &str) -> Entity {
    let g = commands.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }, ChildOf(parent))).id();
    commands.spawn((strong(f, label, 13.0, MUTED), ChildOf(g)));
    g
}

#[allow(clippy::too_many_arguments)]
fn spawn_login(mut commands: Commands, font: Res<UiFont>, mut form: ResMut<Form>, dev: Res<DevOpts>, regions: Res<Regions>, settings: Res<Settings>, pings: Option<Res<Pings>>) {
    form.register = false;
    let f = &*font;
    let scope = DespawnOnExit(Screen::Login);
    let key = link_target(&dev, &regions, &settings, pings.as_deref()).map(|k| k.0);
    let saved = key.as_ref().and_then(|k| settings.accounts.get(k)).map(|s| s.user.clone()).unwrap_or_default();
    let server = match &dev.server {
        Some(addr) => addr.clone(),
        None => {
            let r = &regions.0[region(&settings, &regions, pings.as_deref())];
            format!("{} {}", r.name, r.code)
        }
    };

    let root = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: px(0),
                bottom: px(0),
                width: px(COLUMN),
                padding: UiRect { left: px(INSET), right: px(INSET), top: px(64), bottom: px(48) },
                flex_direction: FlexDirection::Column,
                ..default()
            },
            Plate { fill: PANEL, grain: 1.0, edge_w: 0.0, ..default() },
            SlideIn::new(0.0, -40.0, 0.0),
            scope.clone(),
        ))
        .id();
    commands.spawn((Node { position_type: PositionType::Absolute, right: px(0), top: px(0), bottom: px(0), width: px(2), ..default() }, BackgroundColor(ACCENT), Pickable::IGNORE, ChildOf(root)));

    // the title, and the server this account is on
    let kick = commands.spawn((row(12.0), ChildOf(root))).id();
    commands.spawn((Node { width: px(8), height: px(8), ..default() }, BackgroundColor(ACCENT), ChildOf(kick)));
    commands.spawn((tech(f, "ACCOUNT", 13.0, MUTED), LetterSpacing::Px(4.0), ChildOf(kick)));
    commands.spawn((display(f, "登录", 72.0, INK), LetterSpacing::Px(6.0), Title, Node { margin: UiRect::top(px(40)), ..default() }, ChildOf(root)));
    let at = commands.spawn((Node { margin: UiRect::top(px(16)), column_gap: px(12), align_items: AlignItems::Center, ..default() }, ChildOf(root))).id();
    commands.spawn((text(f, "服务器", 14.0, MUTED), ChildOf(at)));
    commands.spawn((strong(f, server, 15.0, INK), ChildOf(at)));
    // the ping, measured for the listed servers (not one given with --server)
    if dev.server.is_none() {
        commands.spawn((tech(f, "", 14.0, MUTED), ServerPing, ChildOf(at)));
    }
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(at)));
    spawn_btn(&mut commands, f, at, Kind::Ghost, "更换", UiAction::Goto(Screen::Entry), None);
    commands.spawn((para(f, "每台服务器的账号各自独立，进度只在这个大区有效。", 13.0, MUTED), Node { margin: UiRect::top(px(8)), ..default() }, ChildOf(root)));
    commands.spawn((Node { flex_grow: 1.0, min_height: px(GAP), ..default() }, ChildOf(root)));

    // the form
    let body = commands.spawn((Node { flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() }, SlideIn::new(0.1, 0.0, 24.0), ChildOf(root))).id();
    let tabs = commands.spawn((row(0.0), ChildOf(body))).id();
    for (label, register) in [("登录", false), ("注册", true)] {
        let t = spawn_btn(&mut commands, f, tabs, Kind::Segment, label, UiAction::AuthTab(register), None);
        commands.entity(t).insert((Selected(!register), Tab(register)));
    }
    let g = labelled(&mut commands, f, body, "用户名");
    let user = spawn_field(&mut commands, f, g, &saved, 20.0);
    commands.entity(user).insert(UserField);
    let g = labelled(&mut commands, f, body, "密码");
    let pass = spawn_secret_field(&mut commands, f, g, 20.0);
    commands.entity(pass).insert(PassField);
    // focus where the typing starts: the name, or the password when the name is remembered
    commands.entity(if saved.is_empty() { user } else { pass }).insert(AutoFocus);
    let g = labelled(&mut commands, f, body, "确认密码");
    commands.entity(g).insert((ConfirmRow, Node { display: Display::None, flex_direction: FlexDirection::Column, row_gap: px(6), ..default() }));
    let confirm = spawn_secret_field(&mut commands, f, g, 20.0);
    commands.entity(confirm).insert(ConfirmField);
    // one line kept for what is going on, so the form doesn't jump when it fills
    commands.spawn((text(f, "", 13.0, MUTED), Status, Node { min_height: px(18), ..default() }, ChildOf(body)));
    let submit = spawn_btn(&mut commands, f, body, Kind::Primary, "登录", UiAction::SubmitAuth, Some("Enter"));
    commands.entity(submit).insert(Submit);

    // or no account at all
    let or = commands.spawn((Node { column_gap: px(12), align_items: AlignItems::Center, ..default() }, ChildOf(body))).id();
    commands.spawn((Node { flex_grow: 1.0, height: px(1), ..default() }, BackgroundColor(LINE), ChildOf(or)));
    commands.spawn((text(f, "或者", 12.0, MUTED), ChildOf(or)));
    commands.spawn((Node { flex_grow: 1.0, height: px(1), ..default() }, BackgroundColor(LINE), ChildOf(or)));
    spawn_btn(&mut commands, f, body, Kind::Normal, "以游客身份进入", UiAction::PlayAsGuest, None);
    commands.spawn((para(f, "游客也能玩，但不记录进度、没有收益，只能用初始部件。", 12.0, MUTED), ChildOf(body)));

    // corner buttons
    let corner = commands
        .spawn((Node { position_type: PositionType::Absolute, right: px(EDGE), top: px(EDGE), column_gap: px(GAP), align_items: AlignItems::Center, ..default() }, IgnoreHover, scope))
        .id();
    spawn_btn(&mut commands, f, corner, Kind::Normal, "返回", UiAction::Goto(Screen::Entry), Some("Esc"));
    spawn_btn(&mut commands, f, corner, Kind::Normal, "设置", UiAction::OpenSettings, Some("S"));
}

/// On to the lobby once logged in, or at once where there are no accounts to have.
fn login_flow(lobby: Res<Lobby>, mut next: ResMut<NextState<Screen>>) {
    if lobby.account().is_some() || lobby.link() == Link::None {
        next.set(Screen::Menu);
    }
}

fn field(q: &Query<&EditableText, impl bevy::ecs::query::QueryFilter>) -> String {
    q.iter().next().map(|e| e.value().to_string()).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn login_actions(
    mut msgs: MessageReader<UiAction>,
    dev: Res<DevOpts>,
    time: Res<Time>,
    mut form: ResMut<Form>,
    mut lobby: ResMut<Lobby>,
    mut next: ResMut<NextState<Screen>>,
    users: Query<&EditableText, With<UserField>>,
    passes: Query<&EditableText, With<PassField>>,
    confirms: Query<&EditableText, With<ConfirmField>>,
) {
    // dev: the sign-up form (screenshots of it)
    if dev.forced("register", time.elapsed_secs()) && !form.register {
        form.register = true;
    }
    for m in msgs.read() {
        match m {
            UiAction::AuthTab(register) => form.register = *register,
            UiAction::PlayAsGuest => next.set(Screen::Menu),
            UiAction::SubmitAuth => {
                if !matches!(lobby.link(), Link::Up { authing: false }) {
                    continue;
                }
                let (user, pass) = (field(&users).trim().to_string(), field(&passes));
                // what the server would turn down anyway, said here
                let wrong = if user.is_empty() {
                    Some("填上用户名".to_string())
                } else if pass.is_empty() {
                    Some("填上密码".to_string())
                } else if form.register && pass.chars().count() < PASS_MIN {
                    Some(format!("密码至少 {PASS_MIN} 位"))
                } else if form.register && pass != field(&confirms) {
                    Some("两次输入的密码不一样".to_string())
                } else {
                    None
                };
                match wrong {
                    Some(why) => lobby.set_auth_error(why),
                    None if form.register => lobby.auth(AuthReq::Register { user, pass }),
                    None => lobby.auth(AuthReq::Login { user, pass }),
                }
            }
            _ => {}
        }
    }
}

fn login_keys(keys: Res<ButtonInput<KeyCode>>, overlays: Res<Overlays>, modal: Res<ModalReq>, mut w: MessageWriter<UiAction>) {
    if overlays.any(&modal) {
        return;
    }
    if keys.just_pressed(KeyCode::Enter) {
        w.write(UiAction::SubmitAuth);
    }
    if keys.just_pressed(KeyCode::Escape) {
        w.write(UiAction::Goto(Screen::Entry));
    }
}

/// The form follows its tab and the connection: the title, the confirm field, the button, the
/// status line, the ping.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh_login(
    mut commands: Commands,
    form: Res<Form>,
    lobby: Res<Lobby>,
    settings: Res<Settings>,
    regions: Res<Regions>,
    pings: Option<Res<Pings>>,
    mut title: Query<&mut Text, (With<Title>, Without<Status>, Without<ServerPing>, Without<BtnText>)>,
    mut status: Query<(&mut Text, &mut TextColor), (With<Status>, Without<Title>, Without<ServerPing>, Without<BtnText>)>,
    mut ping_line: Query<(&mut Text, &mut TextColor), (With<ServerPing>, Without<Title>, Without<Status>, Without<BtnText>)>,
    mut confirm: Query<&mut Node, With<ConfirmRow>>,
    mut tabs: Query<(&Tab, &mut Selected)>,
    submit: Query<(Entity, &Children), With<Submit>>,
    mut labels: Query<&mut Text, (With<BtnText>, Without<Title>, Without<Status>, Without<ServerPing>)>,
    mut enabled: Local<Option<bool>>,
) {
    let register = form.register;
    for mut t in &mut title {
        let s = if register { "注册" } else { "登录" };
        if t.0 != s {
            t.0 = s.into();
        }
    }
    for mut n in &mut confirm {
        let d = if register { Display::Flex } else { Display::None };
        if n.display != d {
            n.display = d;
        }
    }
    for (t, mut sel) in &mut tabs {
        if sel.0 != (t.0 == register) {
            sel.0 = t.0 == register;
        }
    }
    let resuming = settings.accounts.contains_key(lobby.key());
    let (line, color, ready) = match lobby.link() {
        Link::None => (String::new(), MUTED, false),
        Link::Connecting => ("正在连接服务器…".to_string(), MUTED, false),
        Link::Down(why) => (format!("连不上服务器：{why}。几秒后自动重试"), DANGER, false),
        Link::Up { authing: true } if resuming => ("正在自动登录…".to_string(), MUTED, false),
        Link::Up { authing: true } => (if register { "正在注册…" } else { "正在登录…" }.to_string(), MUTED, false),
        Link::Up { authing: false } => match lobby.auth_error() {
            Some(e) => (e.to_string(), DANGER, true),
            None => (String::new(), MUTED, true),
        },
    };
    for (mut t, mut c) in &mut status {
        if t.0 != line {
            t.0 = line.clone();
        }
        c.set_if_neq(TextColor(color));
    }
    for (e, kids) in &submit {
        if *enabled != Some(ready) {
            set_enabled(&mut commands, e, ready);
        }
        for k in kids.iter() {
            if let Ok(mut t) = labels.get_mut(k) {
                let s = if register { "注册并登录" } else { "登录" };
                if t.0 != s {
                    t.0 = s.into();
                }
            }
        }
    }
    *enabled = Some(ready);
    let at = region(&settings, &regions, pings.as_deref());
    let (p, c) = ping_text(ping(pings.as_deref(), at));
    for (mut t, mut col) in &mut ping_line {
        if t.0 != p {
            t.0 = p.clone();
        }
        col.set_if_neq(TextColor(c));
    }
}
