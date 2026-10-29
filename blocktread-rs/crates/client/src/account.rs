//! Accounts in the client. Each server (region) keeps its own accounts, so the account belongs to
//! the server picked on the entry page.
//!
//! From the lobby on, the client keeps a quiet connection to that server (`Lobby`) for the
//! account: logging in (with the saved token, or a name and password), its coins, level and
//! parts, and buying parts. The match connection logs in with the same token so the match pays
//! the account. A server inside this client (`--server local`, the test range) has no accounts.

use crate::screens::Overlays;
use crate::screens::entry::{Pings, region};
use crate::settings::{self, Regions, SavedLogin, Settings};
use crate::ui::kit::*;
use crate::ui::*;
use crate::{DevOpts, Screen};
use bevy::prelude::*;
use bevy::text::EditableText;
use blocktread_client_core::{ClientCore, ConnState, Target};
use blocktread_shared::economy::{Unlocks, economy};
use blocktread_shared::flight::InputCmd;
use blocktread_shared::protocol::{AuthReq, Profile};
use std::time::Duration;

pub struct AccountPlugin;

impl Plugin for AccountPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Lobby>().add_systems(Update, (keep_link, account_actions, render_account, auth_status, mask_secrets).chain().run_if(linked_screen));
    }
}

/// Screens with a server picked: the lobby and on.
fn linked_screen(s: Res<State<Screen>>) -> bool {
    !matches!(s.get(), Screen::Boot | Screen::Entry)
}

/// Seconds between tries to reconnect a dropped account connection.
const RETRY_SECS: f64 = 5.0;

/// The account connection to the server picked.
#[derive(Resource, Default)]
pub struct Lobby {
    core: Option<ClientCore>,
    /// What the connection is for (`link_target`'s key); empty: none.
    key: String,
    /// The saved token was sent: a failure means it is stale.
    resuming: bool,
    retry_at: f64,
    /// What the account panel was last built for: whether it is open, whether the server keeps
    /// accounts, and the account's name.
    built: Option<(bool, bool, Option<String>)>,
}

impl Lobby {
    /// Whether this server keeps accounts (progression rules apply to play on it).
    pub fn ranked(&self) -> bool {
        self.core.is_some()
    }

    pub fn account(&self) -> Option<&(String, Profile)> {
        self.core.as_ref().and_then(|c| c.account.as_ref())
    }

    pub fn profile(&self) -> Profile {
        self.account().map_or_else(Profile::default, |a| a.1)
    }

    pub fn unlocks(&self) -> Unlocks {
        Unlocks(self.profile().unlocks)
    }

    pub fn level(&self) -> u32 {
        economy().level(self.profile().xp).0
    }

    /// Buy a part (the profile follows, or a notice says why not).
    pub fn unlock(&mut self, ty: blocktread_shared::blocks::BlockType) {
        if let Some(c) = &mut self.core {
            c.unlock(ty);
        }
    }

}

/// The server play goes to, and the key its account is saved under; None for a server inside
/// this client (no accounts there).
pub fn link_target(dev: &DevOpts, regions: &Regions, settings: &Settings, pings: Option<&Pings>) -> Option<(String, Target)> {
    match dev.server.as_deref() {
        Some("local") => None,
        Some(addr) => Some((addr.to_string(), addr.into())),
        None => {
            let r = &regions.0[region(settings, regions, pings)];
            Some((r.key.clone(), r.target()))
        }
    }
}

/// The saved login for play on `key`, to log the match connection in with.
pub fn saved_token(settings: &Settings, key: &str) -> Option<String> {
    settings.accounts.get(key).map(|s| s.token.clone())
}

#[allow(clippy::too_many_arguments)]
fn keep_link(mut lobby: ResMut<Lobby>, dev: Res<DevOpts>, regions: Res<Regions>, mut settings: ResMut<Settings>, pings: Option<Res<Pings>>, time: Res<Time>, mut toasts: ResMut<Toasts>) {
    let now = time.elapsed_secs_f64();
    let want = link_target(&dev, &regions, &settings, pings.as_deref());
    let key = want.as_ref().map_or(String::new(), |w| w.0.clone());
    if key != lobby.key {
        // another server: another account
        if let Some(c) = lobby.core.take() {
            c.leave();
        }
        lobby.key = key.clone();
        lobby.retry_at = 0.0;
        lobby.built = None;
    }
    let Some((key, target)) = want else { return };
    if lobby.core.is_none() {
        if now < lobby.retry_at {
            return;
        }
        lobby.core = Some(ClientCore::connect(target, &settings.name, Duration::ZERO));
        lobby.resuming = false;
    }
    let lobby = &mut *lobby;
    let Some(core) = lobby.core.as_mut() else { return };
    core.update(time.delta_secs(), &mut |_, tick| InputCmd { tick, ..default() });
    // the server has no accounts: no rules, nothing to log in to
    if core.auth_error.as_deref() == Some("这台服务器不开放账号") {
        lobby.core = None;
        lobby.retry_at = f64::MAX;
        return;
    }
    match &core.state {
        ConnState::Lobby if !lobby.resuming && core.account.is_none() && !core.authing => {
            lobby.resuming = true;
            if let Some(token) = saved_token(&settings, &key) {
                core.auth(AuthReq::Resume { token });
            }
        }
        ConnState::Closed(_) | ConnState::Rejected(_) => {
            lobby.core = None;
            lobby.retry_at = now + RETRY_SECS;
            return;
        }
        _ => {}
    }
    let Some(core) = lobby.core.as_mut() else { return };
    if let Some(token) = core.new_token.take() {
        let user = core.account.as_ref().map_or(String::new(), |a| a.0.clone());
        toasts.push(format!("已登录：{user}"), ToastKind::Ok);
        settings.accounts.insert(key.clone(), SavedLogin { user, token });
        settings::save_settings(&settings);
    }
    if let Some(text) = core.notice.take() {
        toasts.push(text, ToastKind::Warn);
    }
    if let Some((coins, xp, up)) = core.reward.take() {
        toasts.push(format!("本局收益：+{coins} 金币 · +{xp} 经验"), ToastKind::Ok);
        if let Some(level) = up {
            toasts.push(format!("升到 {level} 级！"), ToastKind::Ok);
        }
    }
    // a token that no longer works is forgotten; other failures show in the account panel
    if lobby.resuming && core.auth_error.is_some() && core.account.is_none() && settings.accounts.contains_key(&key) && !core.authing {
        let why = core.auth_error.clone().unwrap_or_default();
        if why.contains("失效") {
            settings.accounts.remove(&key);
            settings::save_settings(&settings);
            toasts.push(why, ToastKind::Warn);
            core.auth_error = None;
        }
    }
}

#[derive(Component)]
struct AccountRoot;
#[derive(Component)]
struct UserField;
#[derive(Component)]
struct PassField;
#[derive(Component)]
struct AuthStatus;

#[allow(clippy::too_many_arguments)]
fn account_actions(
    mut msgs: MessageReader<UiAction>,
    mut overlays: ResMut<Overlays>,
    mut lobby: ResMut<Lobby>,
    mut settings: ResMut<Settings>,
    users: Query<&EditableText, With<UserField>>,
    passes: Query<&EditableText, With<PassField>>,
    mut modal: ResMut<ModalReq>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    dev: Res<DevOpts>,
    time: Res<Time>,
    mut forced: Local<bool>,
) {
    // dev: open the panel (screenshots of it)
    if !*forced && dev.forced("account", time.elapsed_secs()) {
        *forced = true;
        overlays.account = true;
    }
    // the panel takes these keys: the screen under it doesn't see them (Enter logs in)
    let mut acts: Vec<UiAction> = Vec::new();
    if overlays.account && !modal.open() {
        if keys.clear_just_pressed(KeyCode::Escape) {
            overlays.account = false;
        }
        if keys.clear_just_pressed(KeyCode::Enter) && lobby.account().is_none() {
            acts.push(UiAction::Login);
        }
    }
    acts.extend(msgs.read().cloned());
    for m in acts {
        match m {
            UiAction::OpenAccount => {
                modal.0 = None;
                overlays.account = true;
            }
            UiAction::CloseAccount => overlays.account = false,
            a @ (UiAction::Login | UiAction::Register) => {
                let user = users.iter().next().map(|e| e.value().to_string().trim().to_string()).unwrap_or_default();
                let pass = passes.iter().next().map(|e| e.value().to_string()).unwrap_or_default();
                let Some(core) = &mut lobby.core else { continue };
                if user.is_empty() || pass.is_empty() {
                    core.auth_error = Some("填上用户名和密码".into());
                    continue;
                }
                lobby.resuming = false;
                let core = lobby.core.as_mut().unwrap();
                core.auth(if a == UiAction::Login { AuthReq::Login { user, pass } } else { AuthReq::Register { user, pass } });
            }
            UiAction::Logout => {
                let key = lobby.key.clone();
                if let (Some(core), Some(saved)) = (&mut lobby.core, settings.accounts.remove(&key)) {
                    core.logout(saved.token);
                }
                settings::save_settings(&settings);
            }
            UiAction::Unlock(t) => {
                modal.0 = None;
                lobby.unlock(t);
            }
            _ => {}
        }
    }
}

/// The account panel: log in or sign up as a guest; who you are, your level and coins once in.
fn render_account(mut commands: Commands, overlays: Res<Overlays>, mut lobby: ResMut<Lobby>, roots: Query<Entity, With<AccountRoot>>, font: Res<UiFont>, regions: Res<Regions>, settings: Res<Settings>) {
    let want = (overlays.account, lobby.ranked(), lobby.account().map(|a| a.0.clone()));
    if lobby.built.as_ref() == Some(&want) {
        return;
    }
    lobby.built = Some(want.clone());
    for e in &roots {
        commands.entity(e).despawn();
    }
    let (open, _, user) = want;
    if !open {
        return;
    }
    let f = &*font;
    let (o, p) = spawn_overlay(&mut commands, f, "账号", 440.0);
    commands.entity(o).insert((AccountRoot, GlobalZIndex(60)));
    let server = regions.0.iter().find(|r| r.key == lobby.key).map_or(lobby.key.clone(), |r| format!("{}服务器", r.name));
    if !lobby.ranked() {
        commands.spawn((para(f, "这台服务器不开放账号：在这里玩不记录进度。", 14.0, MUTED), ChildOf(p)));
    } else if let Some(user) = user {
        let pr = lobby.profile();
        let (level, into, span) = economy().level(pr.xp);
        let have = blocktread_shared::blocks::BlockType::ALL.iter().filter(|&&t| economy().price(t).is_some() && Unlocks(pr.unlocks).has(t)).count();
        let total = economy().prices.len();
        commands.spawn((para(f, format!("{server} · 账号只在这个大区有效"), 13.0, MUTED), ChildOf(p)));
        commands.spawn((heading(f, user, 20.0, INK), ChildOf(p)));
        let (gauge, val) = spawn_stat(&mut commands, f, p, &format!("{level} 级"), ACCENT);
        commands.entity(gauge).insert(Plate::gauge(ACCENT, 24.0, into as f32 / span as f32));
        commands.entity(val).insert(Text::new(format!("{into} / {span}")));
        commands.spawn((text(f, format!("金币 {}  ·  已解锁 {have} / {total} 种部件", pr.coins), 15.0, INK), ChildOf(p)));
        let e = economy();
        let next = [(e.teams_level, "2v2"), (e.tier_levels.get("medium").copied().unwrap_or(1), "中型档"), (e.tier_levels.get("heavy").copied().unwrap_or(1), "重型档")]
            .into_iter()
            .filter(|(l, _)| *l > level)
            .map(|(l, what)| format!("{l} 级开放{what}"))
            .collect::<Vec<_>>();
        if !next.is_empty() {
            commands.spawn((para(f, next.join(" · "), 13.0, MUTED), ChildOf(p)));
        }
        let r = commands.spawn((Node { column_gap: px(GAP_S), justify_content: JustifyContent::FlexEnd, margin: UiRect::top(px(GAP_S)), ..default() }, ChildOf(p))).id();
        spawn_btn(&mut commands, f, r, Kind::Danger, "退出登录", UiAction::Logout, None);
        spawn_btn(&mut commands, f, r, Kind::Accent, "完成", UiAction::CloseAccount, Some("Esc"));
        return;
    } else {
        commands.spawn((para(f, format!("{server}的账号：进度只在这个大区有效。不登录也能玩，只是没有收益，也只能用初始部件。"), 13.0, MUTED), ChildOf(p)));
        commands.spawn((strong(f, "用户名", 13.0, MUTED), ChildOf(p)));
        let saved = settings.accounts.get(&lobby.key).map_or(String::new(), |s| s.user.clone());
        let u = spawn_field(&mut commands, f, p, &saved, 16.0);
        commands.entity(u).insert((UserField, bevy::input_focus::AutoFocus));
        commands.spawn((strong(f, "密码", 13.0, MUTED), ChildOf(p)));
        let pw = spawn_secret_field(&mut commands, f, p, 16.0);
        commands.entity(pw).insert(PassField);
        commands.spawn((text(f, "", 13.0, DANGER), AuthStatus, ChildOf(p)));
        let r = commands.spawn((Node { column_gap: px(GAP_S), justify_content: JustifyContent::FlexEnd, margin: UiRect::top(px(GAP_S)), ..default() }, ChildOf(p))).id();
        spawn_btn(&mut commands, f, r, Kind::Ghost, "关闭", UiAction::CloseAccount, Some("Esc"));
        spawn_btn(&mut commands, f, r, Kind::Normal, "注册", UiAction::Register, None);
        spawn_btn(&mut commands, f, r, Kind::Primary, "登录", UiAction::Login, None);
        return;
    }
    let r = commands.spawn((Node { justify_content: JustifyContent::FlexEnd, ..default() }, ChildOf(p))).id();
    spawn_btn(&mut commands, f, r, Kind::Accent, "完成", UiAction::CloseAccount, Some("Esc"));
}

/// The login form's status line: working on it, or why it didn't go through.
fn auth_status(lobby: Res<Lobby>, mut q: Query<(&mut Text, &mut TextColor), With<AuthStatus>>) {
    let Some(core) = &lobby.core else { return };
    for (mut t, mut c) in &mut q {
        let (s, col) = match (&core.auth_error, core.authing, &core.state) {
            (_, true, _) => ("正在验证…".to_string(), MUTED),
            (Some(e), _, _) => (e.clone(), DANGER),
            (None, _, ConnState::Connecting) => ("正在连接服务器…".to_string(), MUTED),
            _ => (String::new(), MUTED),
        };
        if t.0 != s {
            t.0 = s;
        }
        c.0 = col;
    }
}

/// Dots over a secret field, one per character: the field's own text is drawn clear.
#[derive(Component)]
struct Mask(Entity);

/// A text field whose characters show as dots (passwords). Returns the field.
pub fn spawn_secret_field(commands: &mut Commands, f: &UiFont, parent: Entity, chars: f32) -> Entity {
    let wrap = commands.spawn((Node { position_type: PositionType::Relative, flex_direction: FlexDirection::Column, ..default() }, ChildOf(parent))).id();
    let field = spawn_field(commands, f, wrap, "", chars);
    commands.entity(field).insert(TextColor(Color::NONE));
    commands.spawn((text(f, "", 15.0, INK), Node { position_type: PositionType::Absolute, left: px(12), top: px(9), ..default() }, Pickable::IGNORE, Mask(field), ChildOf(wrap)));
    field
}

fn mask_secrets(fields: Query<&EditableText>, mut masks: Query<(&mut Text, &Mask)>) {
    for (mut t, m) in &mut masks {
        let n = fields.get(m.0).map_or(0, |e| e.value().to_string().chars().count());
        let dots = "•".repeat(n);
        if t.0 != dots {
            t.0 = dots;
        }
    }
}
