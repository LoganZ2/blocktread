//! Accounts in the client. Each server (region) keeps its own accounts, so the account belongs to
//! the server picked on the entry page.
//!
//! Past the entry page the client keeps a quiet connection to that server (`Lobby`) for the
//! account: the login page logs in on it (with the saved token, or a name and password), the
//! lobby shows its level and coins, the garage buys parts on it. The match connection logs in
//! with the same token so the match pays the account. A server inside this client (`--server
//! local`, the test range) has no accounts.

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
        app.init_resource::<Lobby>().add_systems(Update, (keep_link, account_actions, render_account, mask_secrets).chain().in_set(AccountSystems).run_if(linked_screen));
    }
}

/// The account connection's systems: screens that read `Lobby` run after them, so they see this
/// frame's state of it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct AccountSystems;

/// Screens with a server picked: the login page and on.
fn linked_screen(s: Res<State<Screen>>) -> bool {
    !matches!(s.get(), Screen::Boot | Screen::Entry)
}

/// Seconds between tries to reconnect a dropped account connection.
const RETRY_SECS: f64 = 5.0;

/// Where the account connection stands, for the login page to show.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    /// No accounts to have: a server inside this client, or one that doesn't keep them.
    None,
    Connecting,
    /// The connection failed (it is tried again shortly), and why.
    Down(String),
    /// Connected; a login is on its way if `authing`.
    Up { authing: bool },
}

/// The account connection to the server picked.
#[derive(Resource, Default)]
pub struct Lobby {
    core: Option<ClientCore>,
    /// What the connection is for (`link_target`'s key); empty: none.
    key: String,
    /// The saved token was sent (or there was none to send): a failure now means it is stale.
    resuming: bool,
    retry_at: f64,
    /// The server answered that it keeps no accounts.
    no_accounts: bool,
    /// Why the last connection failed.
    down: Option<String>,
    /// What the account panel was last built for: whether it is open, and the account.
    built: Option<(bool, Option<(String, Profile)>)>,
}

impl Lobby {
    /// Whether this server keeps accounts (progression rules apply to play on it).
    pub fn ranked(&self) -> bool {
        self.core.is_some()
    }

    pub fn link(&self) -> Link {
        match &self.core {
            _ if self.no_accounts || self.key.is_empty() => Link::None,
            None => self.down.clone().map_or(Link::Connecting, Link::Down),
            Some(c) => match &c.state {
                ConnState::Connecting => Link::Connecting,
                _ => Link::Up { authing: c.authing || !self.resuming },
            },
        }
    }

    /// The server's key (`link_target`), empty when there is none.
    pub fn key(&self) -> &str {
        &self.key
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

    /// Log in or sign up (`account` or `auth_error` follows).
    pub fn auth(&mut self, req: AuthReq) {
        if let Some(c) = &mut self.core {
            self.resuming = true;
            c.auth(req);
        }
    }

    /// Why the last login failed, if it did.
    pub fn auth_error(&self) -> Option<&str> {
        self.core.as_ref().and_then(|c| c.auth_error.as_deref())
    }

    pub fn set_auth_error(&mut self, why: impl Into<String>) {
        if let Some(c) = &mut self.core {
            c.auth_error = Some(why.into());
        }
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
        lobby.no_accounts = false;
        lobby.down = None;
        lobby.built = None;
    }
    let Some((key, target)) = want else { return };
    if lobby.no_accounts {
        return;
    }
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
        lobby.no_accounts = true;
        return;
    }
    match &core.state {
        ConnState::Lobby => {
            lobby.down = None;
            // in: log back in with the saved token, if there is one
            if !lobby.resuming && core.account.is_none() && !core.authing {
                lobby.resuming = true;
                if let Some(token) = saved_token(&settings, &key) {
                    core.auth(AuthReq::Resume { token });
                }
            }
        }
        ConnState::Closed(why) | ConnState::Rejected(why) => {
            lobby.down = Some(why.clone());
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
    // a token that no longer works is forgotten (the login page says so)
    if core.auth_error.as_deref().is_some_and(|e| e.contains("失效")) && settings.accounts.contains_key(&key) {
        settings.accounts.remove(&key);
        settings::save_settings(&settings);
    }
}

#[derive(Component)]
struct AccountRoot;

#[allow(clippy::too_many_arguments)]
fn account_actions(
    mut msgs: MessageReader<UiAction>,
    mut overlays: ResMut<Overlays>,
    mut lobby: ResMut<Lobby>,
    mut settings: ResMut<Settings>,
    mut modal: ResMut<ModalReq>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut next: ResMut<NextState<Screen>>,
    dev: Res<DevOpts>,
    time: Res<Time>,
    mut forced: Local<bool>,
) {
    // dev: open the panel (screenshots of it)
    if !*forced && dev.forced("account", time.elapsed_secs()) && lobby.account().is_some() {
        *forced = true;
        overlays.account = true;
    }
    // the panel takes Esc: the screen under it doesn't see it
    if overlays.account && !modal.open() && keys.clear_just_pressed(KeyCode::Escape) {
        overlays.account = false;
    }
    for m in msgs.read() {
        match m.clone() {
            UiAction::OpenAccount if lobby.account().is_some() => {
                modal.0 = None;
                overlays.account = true;
            }
            UiAction::CloseAccount => overlays.account = false,
            // logged out: back to the login page, to log in as someone else or go on as a guest
            UiAction::Logout => {
                let key = lobby.key.clone();
                if let (Some(core), Some(saved)) = (&mut lobby.core, settings.accounts.remove(&key)) {
                    core.logout(saved.token);
                }
                settings::save_settings(&settings);
                overlays.account = false;
                next.set(Screen::Login);
            }
            UiAction::Unlock(t) => {
                modal.0 = None;
                lobby.unlock(t);
            }
            _ => {}
        }
    }
}

/// The account panel, for a logged-in player: who, where, the level and how far into it, the
/// coins and parts, and what the next levels open.
fn render_account(mut commands: Commands, overlays: Res<Overlays>, mut lobby: ResMut<Lobby>, roots: Query<Entity, With<AccountRoot>>, font: Res<UiFont>, regions: Res<Regions>) {
    let want = (overlays.account, lobby.account().cloned());
    if lobby.built.as_ref() == Some(&want) {
        return;
    }
    lobby.built = Some(want.clone());
    for e in &roots {
        commands.entity(e).despawn();
    }
    let (true, Some((user, pr))) = want else { return };
    let f = &*font;
    let e = economy();
    let (o, p) = spawn_overlay(&mut commands, f, "账号", 480.0);
    commands.entity(o).insert((AccountRoot, GlobalZIndex(60)));
    let server = regions.0.iter().find(|r| r.key == lobby.key).map_or(lobby.key.clone(), |r| format!("{} {}", r.name, r.code));

    // who, and the server the account belongs to
    let who = commands.spawn((Node { column_gap: px(GAP_S), align_items: AlignItems::Center, ..default() }, ChildOf(p))).id();
    commands.spawn((heading(f, user, 24.0, INK), ChildOf(who)));
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(who)));
    commands.spawn((chip(f, server, MUTED), ChildOf(who)));
    commands.spawn((para(f, "账号属于这台服务器，进度只在这个大区有效。", 12.0, MUTED), ChildOf(p)));

    // the level, and the way to the next
    let (level, into, span) = e.level(pr.xp);
    let lv = spawn_section(&mut commands, f, p, "等级|LEVEL");
    let line = commands.spawn((Node { align_items: AlignItems::Center, ..default() }, ChildOf(lv))).id();
    commands.spawn((strong(f, format!("{level} 级"), 16.0, ACCENT), ChildOf(line)));
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(line)));
    commands.spawn((tech(f, format!("{into} / {span} 经验"), 13.0, MUTED), ChildOf(line)));
    commands.spawn((Node { height: px(8), ..default() }, Plate::gauge(ACCENT, 24.0, into as f32 / span as f32), ChildOf(lv)));

    // coins and parts, side by side
    let have = blocktread_shared::blocks::BlockType::ALL.iter().filter(|&&t| e.price(t).is_some() && Unlocks(pr.unlocks).has(t)).count();
    let tiles = commands.spawn((Node { column_gap: px(GAP_S), ..default() }, ChildOf(p))).id();
    for (label, value) in [("金币", pr.coins.to_string()), ("已解锁部件", format!("{have} / {}", e.prices.len()))] {
        let t = commands
            .spawn((Node { flex_grow: 1.0, flex_basis: px(0), flex_direction: FlexDirection::Column, row_gap: px(4), padding: px(12).all(), ..default() }, BackgroundColor(tone(Color::WHITE, 0.04)), ChildOf(tiles)))
            .id();
        commands.spawn((text(f, label, 12.0, MUTED), ChildOf(t)));
        commands.spawn((tech(f, value, 24.0, INK), ChildOf(t)));
    }

    // what the next levels open
    let next: Vec<(u32, &str)> = [(e.teams_level, "开放 2v2 占点战"), (e.tier_levels.get("medium").copied().unwrap_or(1), "开放中型档"), (e.tier_levels.get("heavy").copied().unwrap_or(1), "开放重型档")]
        .into_iter()
        .filter(|(l, _)| *l > level)
        .collect();
    if !next.is_empty() {
        let up = spawn_section(&mut commands, f, p, "升级解锁|NEXT");
        for (l, what) in next {
            let r = commands.spawn((Node { column_gap: px(GAP_S), align_items: AlignItems::Center, ..default() }, ChildOf(up))).id();
            commands.spawn((chip(f, format!("{l} 级"), MUTED), ChildOf(r)));
            commands.spawn((text(f, what, 13.0, INK), ChildOf(r)));
        }
    }

    let r = commands.spawn((Node { justify_content: JustifyContent::SpaceBetween, margin: UiRect::top(px(GAP_S)), ..default() }, ChildOf(p))).id();
    spawn_btn(&mut commands, f, r, Kind::Danger, "退出登录", UiAction::Logout, None);
    spawn_btn(&mut commands, f, r, Kind::Accent, "完成", UiAction::CloseAccount, Some("Esc"));
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
