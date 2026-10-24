//! The entry page, first after starting the game: pick the server to play on from a dropdown
//! (each server with its live ping), then 开始游戏 into the lobby. The lobby plays on that server;
//! its 更换 comes back here.

use super::Overlays;
use crate::settings::{self, Regions, Settings};
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use crate::{MainCamera, Screen};
use bevy::prelude::*;
use bevy::text::LetterSpacing;
use std::time::Duration;
use voxwing_client_core::ping::{Ping, Pinger};

pub struct EntryPlugin;

impl Plugin for EntryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Chosen>()
            .init_resource::<ListOpen>()
            // pings run on this page and in the lobby, not in the hangar or a match
            .add_systems(OnEnter(Screen::Entry), (start_pings, spawn_entry))
            .add_systems(OnEnter(Screen::Menu), start_pings)
            .add_systems(OnExit(Screen::Menu), |mut c: Commands| c.remove_resource::<Pings>())
            .add_systems(Update, (entry_actions, entry_keys, refresh_select, drift).chain().run_if(in_state(Screen::Entry)))
            .add_systems(Update, ping_labels.run_if(|s: Res<State<Screen>>| matches!(s.get(), Screen::Entry | Screen::Menu)));
    }
}

/// Measures every server's ping while the entry page or the lobby is open.
#[derive(Resource)]
pub struct Pings(pub Pinger);

fn start_pings(mut commands: Commands, pings: Option<Res<Pings>>, regions: Res<Regions>) {
    if pings.is_none() {
        commands.insert_resource(Pings(Pinger::start(regions.0.iter().map(|r| r.addr.clone()).collect(), Duration::from_secs(3))));
    }
}

/// The ping of server `i`, if it is being measured.
pub fn ping(pings: Option<&Pings>, i: usize) -> Ping {
    pings.map_or(Ping::Pending, |p| p.0.get(i))
}

/// The server to play on: the one picked, else the one answering fastest, else the first.
pub fn region(settings: &Settings, regions: &Regions, pings: Option<&Pings>) -> usize {
    if let Some(i) = regions.0.iter().position(|r| r.key == settings.region) {
        return i;
    }
    fastest(regions, pings).unwrap_or(0)
}

fn fastest(regions: &Regions, pings: Option<&Pings>) -> Option<usize> {
    (0..regions.0.len())
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

/// Text that shows server `i`'s ping, kept current.
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

/// The server picked on this page; None follows the saved choice, or the fastest server.
#[derive(Resource, Default)]
struct Chosen(Option<usize>);

/// Whether the dropdown's list is open.
#[derive(Resource, Default)]
struct ListOpen(bool);

/// The dropdown's closed face: the server picked, its ping, the arrow.
#[derive(Component)]
struct SelectName;
#[derive(Component)]
struct SelectPing;
#[derive(Component)]
struct SelectArrow;
/// The list under it, and one line of it per server.
#[derive(Component)]
struct SelectList;
#[derive(Component)]
struct SelectItem(usize);
/// The accent bar on the list line of the server picked.
#[derive(Component)]
struct ItemMark(usize);

/// Width of the server picker and the start button.
const COLUMN: f32 = 480.0;

fn spawn_entry(mut commands: Commands, font: Res<UiFont>, icons: Res<Icons>, regions: Res<Regions>, mut chosen: ResMut<Chosen>, mut open: ResMut<ListOpen>) {
    chosen.0 = None;
    open.0 = false;
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

    // the server dropdown, then start
    let body = commands.spawn((Node { width: px(COLUMN), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() }, SlideIn::new(0.1, 0.0, 24.0), ChildOf(root))).id();
    spawn_header(&mut commands, f, body, "服务器|SERVER");
    // the field and its list share a holder, so the list can hang under the field over the rest
    let holder = commands.spawn((Node { flex_direction: FlexDirection::Column, ..default() }, ChildOf(body))).id();
    let field = spawn_btn(&mut commands, f, holder, Kind::Normal, "", UiAction::ServerList, None);
    commands.entity(field).insert(Node { height: px(H_LG), padding: UiRect::horizontal(px(16)), justify_content: JustifyContent::FlexStart, column_gap: px(12), ..btn_node(Kind::Normal) });
    commands.spawn((text(f, "", 16.0, INK), SelectName, Pickable::IGNORE, ChildOf(field)));
    commands.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE, ChildOf(field)));
    commands.spawn((tech(f, "…", 15.0, MUTED), SelectPing, Pickable::IGNORE, ChildOf(field)));
    commands.spawn((
        ImageNode { image: icons.right.clone(), color: MUTED, ..default() },
        Node { width: px(14), height: px(14), ..default() },
        UiTransform { rotation: Rot2::degrees(90.0), ..default() },
        SelectArrow,
        Pickable::IGNORE,
        ChildOf(field),
    ));
    let list = commands
        .spawn((
            Node { display: Display::None, position_type: PositionType::Absolute, top: px(H_LG + 4.0), left: px(0), right: px(0), flex_direction: FlexDirection::Column, padding: px(4).all(), ..default() },
            solid_plate(),
            GlobalZIndex(20),
            SelectList,
            ChildOf(holder),
        ))
        .id();
    for (i, r) in regions.0.iter().enumerate() {
        let item = spawn_btn(&mut commands, f, list, Kind::Ghost, format!("{} {}", r.name, r.code), UiAction::Region(i), None);
        commands.entity(item).insert((SelectItem(i), Node { height: px(44), padding: UiRect::horizontal(px(12)), justify_content: JustifyContent::FlexStart, column_gap: px(12), ..btn_node(Kind::Ghost) }));
        commands.spawn((
            Node { position_type: PositionType::Absolute, left: px(0), top: px(10), bottom: px(10), width: px(3), ..default() },
            BackgroundColor(ACCENT),
            Visibility::Hidden,
            ItemMark(i),
            Pickable::IGNORE,
            ChildOf(item),
        ));
        commands.spawn((Node { flex_grow: 1.0, ..default() }, Pickable::IGNORE, ChildOf(item)));
        commands.spawn((tech(f, r.addr.clone(), 11.0, MUTED), LetterSpacing::Px(1.0), Pickable::IGNORE, ChildOf(item)));
        commands.spawn((tech(f, "…", 14.0, MUTED), RegionPing(i), Node { width: px(64), justify_content: JustifyContent::FlexEnd, ..default() }, TextLayout::justify(Justify::Right), Pickable::IGNORE, ChildOf(item)));
    }
    commands.spawn((para(f, "选延迟低的服务器。同一服务器上的玩家会匹配到一起。", 12.0, MUTED), ChildOf(body)));
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

/// The server shown as picked: the choice on this page, else the saved one, else the fastest.
fn current(chosen: &Chosen, settings: &Settings, regions: &Regions, pings: Option<&Pings>) -> usize {
    chosen.0.unwrap_or_else(|| region(settings, regions, pings))
}

/// The dropdown: the picked server and its ping on the face, the list shown while open.
#[allow(clippy::type_complexity)]
fn refresh_select(
    chosen: Res<Chosen>,
    open: Res<ListOpen>,
    settings: Res<Settings>,
    regions: Res<Regions>,
    pings: Option<Res<Pings>>,
    mut name: Query<&mut Text, (With<SelectName>, Without<SelectPing>)>,
    mut ping_face: Query<(&mut Text, &mut TextColor), (With<SelectPing>, Without<SelectName>)>,
    mut arrow: Query<&mut UiTransform, With<SelectArrow>>,
    mut list: Query<&mut Node, With<SelectList>>,
    mut items: Query<(&SelectItem, &mut Selected)>,
    mut marks: Query<(&ItemMark, &mut Visibility)>,
) {
    let at = current(&chosen, &settings, &regions, pings.as_deref());
    let r = &regions.0[at];
    let label = format!("{} {}", r.name, r.code);
    for mut t in &mut name {
        if t.0 != label {
            t.0 = label.clone();
        }
    }
    let (s, color) = ping_text(ping(pings.as_deref(), at));
    for (mut t, mut c) in &mut ping_face {
        if t.0 != s {
            t.0 = s.clone();
        }
        if c.0 != color {
            c.0 = color;
        }
    }
    // the arrow points down while closed, up while open
    let turn = Rot2::degrees(if open.0 { -90.0 } else { 90.0 });
    for mut tr in &mut arrow {
        if tr.rotation != turn {
            tr.rotation = turn;
        }
    }
    let shown = if open.0 { Display::Flex } else { Display::None };
    for mut n in &mut list {
        if n.display != shown {
            n.display = shown;
        }
    }
    for (item, mut sel) in &mut items {
        if sel.0 != (item.0 == at) {
            sel.0 = item.0 == at;
        }
    }
    for (m, mut vis) in &mut marks {
        let want = if m.0 == at { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn entry_actions(
    mut msgs: MessageReader<UiAction>,
    mut chosen: ResMut<Chosen>,
    mut open: ResMut<ListOpen>,
    mut settings: ResMut<Settings>,
    regions: Res<Regions>,
    pings: Option<Res<Pings>>,
    mut next: ResMut<NextState<Screen>>,
) {
    for m in msgs.read() {
        match m {
            UiAction::ServerList => open.0 = !open.0,
            UiAction::Region(i) => {
                chosen.0 = Some(*i);
                open.0 = false;
            }
            UiAction::StartGame => {
                let at = current(&chosen, &settings, &regions, pings.as_deref());
                settings.region = regions.0[at].key.clone();
                settings::save_settings(&settings);
                next.set(Screen::Menu);
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn entry_keys(
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    modal: Res<ModalReq>,
    mut chosen: ResMut<Chosen>,
    mut open: ResMut<ListOpen>,
    settings: Res<Settings>,
    regions: Res<Regions>,
    pings: Option<Res<Pings>>,
    mut w: MessageWriter<UiAction>,
    dev: Res<crate::DevOpts>,
    time: Res<Time>,
    mut forced: Local<bool>,
) {
    // dev: open the list (offscreen screenshots have no pointer)
    if !*forced && dev.forced("serverlist", time.elapsed_secs()) {
        *forced = true;
        open.0 = true;
    }
    if overlays.any(&modal) {
        return;
    }
    // ↑ ↓ change the server, open or not
    let step = keys.just_pressed(KeyCode::ArrowDown) as i32 - keys.just_pressed(KeyCode::ArrowUp) as i32;
    if step != 0 {
        let n = regions.0.len() as i32;
        let at = current(&chosen, &settings, &regions, pings.as_deref()) as i32;
        chosen.0 = Some((at + step).rem_euclid(n) as usize);
    }
    if keys.just_pressed(KeyCode::Enter) {
        // Enter on an open list takes the server; on a closed one it starts
        if open.0 {
            open.0 = false;
        } else {
            w.write(UiAction::StartGame);
        }
    }
    if keys.just_pressed(KeyCode::KeyS) {
        w.write(UiAction::OpenSettings);
    }
    if keys.just_pressed(KeyCode::Escape) {
        if open.0 {
            open.0 = false;
        } else {
            w.write(UiAction::Quit);
        }
    }
}

/// A slow drift through the asteroid field behind the page.
fn drift(time: Res<Time>, mut cam: Single<&mut Transform, With<MainCamera>>) {
    let t = time.elapsed_secs() * 0.02;
    let eye = Vec3::new(t.sin() * 90.0, 18.0, t.cos() * 90.0);
    **cam = Transform::from_translation(eye).looking_at(Vec3::new(0.0, 6.0, 0.0), Vec3::Y);
}
