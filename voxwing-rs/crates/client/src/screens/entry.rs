//! The entry page, first after starting the game: pick the server to play on, each with its live
//! ping, then 开始游戏 into the lobby. The lobby plays on that server; its 更换 comes back here.

use super::Overlays;
use crate::settings::{self, Settings};
use crate::ui::kit::*;
use crate::ui::*;
use crate::{MainCamera, REGIONS, Region, Screen};
use bevy::prelude::*;
use bevy::text::LetterSpacing;
use std::time::Duration;
use voxwing_client_core::ping::{Ping, Pinger};

pub struct EntryPlugin;

impl Plugin for EntryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Chosen>()
            // pings run on this page and in the lobby, not in the hangar or a match
            .add_systems(OnEnter(Screen::Entry), (start_pings, spawn_entry))
            .add_systems(OnEnter(Screen::Menu), start_pings)
            .add_systems(OnExit(Screen::Menu), |mut c: Commands| c.remove_resource::<Pings>())
            .add_systems(Update, (entry_actions, entry_keys, refresh_cards, drift).chain().run_if(in_state(Screen::Entry)))
            .add_systems(Update, ping_labels.run_if(|s: Res<State<Screen>>| matches!(s.get(), Screen::Entry | Screen::Menu)));
    }
}

/// Measures every server's ping while the entry page or the lobby is open.
#[derive(Resource)]
pub struct Pings(pub Pinger);

fn start_pings(mut commands: Commands, pings: Option<Res<Pings>>) {
    if pings.is_none() {
        commands.insert_resource(Pings(Pinger::start(REGIONS.iter().map(|r| r.addr.to_string()).collect(), Duration::from_secs(3))));
    }
}

/// The ping of `REGIONS[i]`, if it is being measured.
pub fn ping(pings: Option<&Pings>, i: usize) -> Ping {
    pings.map_or(Ping::Pending, |p| p.0.get(i))
}

/// The server to play on: the one picked, else the one answering fastest, else the first.
pub fn region(settings: &Settings, pings: Option<&Pings>) -> usize {
    if let Some(i) = REGIONS.iter().position(|r| r.key == settings.region) {
        return i;
    }
    fastest(pings).unwrap_or(0)
}

fn fastest(pings: Option<&Pings>) -> Option<usize> {
    (0..REGIONS.len())
        .filter_map(|i| match ping(pings, i) {
            Ping::Ms(ms) => Some((i, ms)),
            _ => None,
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A ping as shown, and its colour: green is fine for a dogfight, red is laggy.
pub fn ping_text(p: Ping) -> (String, Color) {
    match p {
        Ping::Pending => ("…".into(), MUTED),
        Ping::Unreachable => ("超时".into(), DANGER),
        Ping::Ms(ms) if ms < 90.0 => (format!("{ms:.0} ms"), OK),
        Ping::Ms(ms) if ms < 180.0 => (format!("{ms:.0} ms"), WARN),
        Ping::Ms(ms) => (format!("{ms:.0} ms"), DANGER),
    }
}

/// Text that shows `REGIONS[i]`'s ping, kept current.
#[derive(Component)]
pub struct RegionPing(pub usize);

fn ping_labels(pings: Option<Res<Pings>>, mut labels: Query<(&mut Text, &mut TextColor, &RegionPing)>) {
    for (mut t, mut c, r) in &mut labels {
        let (s, color) = ping_text(ping(pings.as_deref(), r.0));
        if t.0 != s {
            t.0 = s;
        }
        if c.0 != color {
            c.0 = color;
        }
    }
}

/// The card picked on this page; None follows the saved choice, or the fastest server.
#[derive(Resource, Default)]
struct Chosen(Option<usize>);

#[derive(Component)]
struct ServerCard(usize);

fn spawn_entry(mut commands: Commands, font: Res<UiFont>, mut chosen: ResMut<Chosen>) {
    chosen.0 = None;
    let f = &*font;
    let scope = DespawnOnExit(Screen::Entry);
    let root = commands
        .spawn((
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, justify_content: JustifyContent::Center, row_gap: px(48), ..default() },
            Pickable::IGNORE,
            IgnoreHover,
            scope.clone(),
        ))
        .id();

    // title
    let head = commands.spawn((Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(6), ..default() }, SlideIn::new(0.0, 0.0, -20.0), ChildOf(root))).id();
    let kick = commands.spawn((row(12.0), ChildOf(head))).id();
    commands.spawn((Node { width: px(24), height: px(2), ..default() }, BackgroundColor(ACCENT), ChildOf(kick)));
    commands.spawn((tech(f, "VOXWING", 14.0, ACCENT), LetterSpacing::Px(12.0), ChildOf(kick)));
    commands.spawn((Node { width: px(24), height: px(2), ..default() }, BackgroundColor(ACCENT), ChildOf(kick)));
    commands.spawn((display(f, "体素星翼", 112.0, INK), LetterSpacing::Px(4.0), ChildOf(head)));
    commands.spawn((text(f, "用方块拼出你的战机，在小行星带里和朋友空战", 16.0, MUTED), ChildOf(head)));

    // servers, then start
    let body = commands.spawn((Node { width: px(640), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() }, SlideIn::new(0.1, 0.0, 24.0), ChildOf(root))).id();
    spawn_header(&mut commands, f, body, "选择服务器|SERVER");
    let cards = commands.spawn((Node { column_gap: px(GAP), ..default() }, ChildOf(body))).id();
    for (i, r) in REGIONS.iter().enumerate() {
        let card = spawn_btn(&mut commands, f, cards, Kind::Card, r.name, UiAction::Region(r.key), None);
        commands.entity(card).insert((ServerCard(i), Node { flex_grow: 1.0, flex_basis: px(0), padding: px(20).all(), row_gap: px(10), ..btn_node(Kind::Card) }));
        commands.spawn((tech(f, r.code, 12.0, MUTED), LetterSpacing::Px(4.0), Node { position_type: PositionType::Absolute, right: px(20), top: px(24), ..default() }, Pickable::IGNORE, ChildOf(card)));
        commands.spawn((tech(f, "…", 34.0, MUTED), RegionPing(i), Pickable::IGNORE, ChildOf(card)));
        commands.spawn((tech(f, r.addr, 11.0, MUTED), LetterSpacing::Px(1.0), Pickable::IGNORE, ChildOf(card)));
    }
    commands.spawn((para(f, "延迟每 3 秒刷新一次。选离你近、延迟低的服务器，同一个服务器上的玩家会被匹配到一起。", 12.0, MUTED), ChildOf(body)));
    let go = spawn_btn(&mut commands, f, body, Kind::Primary, "开始游戏", UiAction::StartGame, Some("Enter"));
    commands.entity(go).insert(bevy::input_focus::AutoFocus);

    // corner buttons
    let corner = commands
        .spawn((
            Node { position_type: PositionType::Absolute, right: px(EDGE), top: px(EDGE), column_gap: px(GAP), align_items: AlignItems::Center, ..default() },
            IgnoreHover,
            scope.clone(),
        ))
        .id();
    spawn_btn(&mut commands, f, corner, Kind::Normal, "设置", UiAction::OpenSettings, Some("S"));
    spawn_btn(&mut commands, f, corner, Kind::Ghost, "退出", UiAction::Quit, Some("Esc"));
}

/// The card shown as picked: the click on this page, else the saved choice, else the fastest.
fn current(chosen: &Chosen, settings: &Settings, pings: Option<&Pings>) -> usize {
    chosen.0.unwrap_or_else(|| region(settings, pings))
}

fn refresh_cards(chosen: Res<Chosen>, settings: Res<Settings>, pings: Option<Res<Pings>>, mut cards: Query<(&ServerCard, &mut Selected)>) {
    let at = current(&chosen, &settings, pings.as_deref());
    for (c, mut sel) in &mut cards {
        if sel.0 != (c.0 == at) {
            sel.0 = c.0 == at;
        }
    }
}

fn entry_actions(mut msgs: MessageReader<UiAction>, mut chosen: ResMut<Chosen>, mut settings: ResMut<Settings>, pings: Option<Res<Pings>>, mut next: ResMut<NextState<Screen>>) {
    for m in msgs.read() {
        match m {
            UiAction::Region(k) => chosen.0 = REGIONS.iter().position(|r| r.key == *k),
            UiAction::StartGame => {
                let at = current(&chosen, &settings, pings.as_deref());
                let r: &Region = &REGIONS[at];
                settings.region = r.key.to_string();
                settings::save_settings(&settings);
                next.set(Screen::Menu);
            }
            _ => {}
        }
    }
}

fn entry_keys(keys: Res<ButtonInput<KeyCode>>, overlays: Res<Overlays>, modal: Res<ModalReq>, mut chosen: ResMut<Chosen>, settings: Res<Settings>, pings: Option<Res<Pings>>, mut w: MessageWriter<UiAction>) {
    if overlays.any(&modal) {
        return;
    }
    let step = keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::KeyD]) as i32 - keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::KeyA]) as i32;
    if step != 0 {
        let n = REGIONS.len() as i32;
        let at = current(&chosen, &settings, pings.as_deref()) as i32;
        chosen.0 = Some((at + step).rem_euclid(n) as usize);
    }
    if keys.just_pressed(KeyCode::Enter) {
        w.write(UiAction::StartGame);
    }
    if keys.just_pressed(KeyCode::KeyS) {
        w.write(UiAction::OpenSettings);
    }
    if keys.just_pressed(KeyCode::Escape) {
        w.write(UiAction::Quit);
    }
}

/// A slow drift through the asteroid field behind the page.
fn drift(time: Res<Time>, mut cam: Single<&mut Transform, With<MainCamera>>) {
    let t = time.elapsed_secs() * 0.02;
    let eye = Vec3::new(t.sin() * 90.0, 18.0, t.cos() * 90.0);
    **cam = Transform::from_translation(eye).looking_at(Vec3::new(0.0, 6.0, 0.0), Vec3::Y);
}
