//! Main menu: title, the four top-level choices, and a card for the ship currently in the hangar.

use super::Overlays;
use crate::Screen;
use crate::hangar::Hangar;
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use bevy::input_focus::{InputFocus, FocusCause};
use bevy::input_focus::AutoFocus;
use bevy::prelude::*;
use bevy::text::LetterSpacing;

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(Screen::Menu), spawn_menu).add_systems(Update, (menu_keys, refresh_ship_card).run_if(in_state(Screen::Menu)));
    }
}

#[derive(Component)]
struct MenuItem(usize);
#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
struct ShipCard;

/// Left edge of the title column.
const COLUMN_X: f32 = 96.0;

fn spawn_menu(mut commands: Commands, font: Res<UiFont>, icons: Res<Icons>, hangar: Res<Hangar>) {
    let f = &*font;
    let scope = DespawnOnExit(Screen::Menu);
    let root = commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, Pickable::IGNORE, IgnoreHover, MenuRoot, scope)).id();

    let left = commands
        .spawn((Node { position_type: PositionType::Absolute, left: px(COLUMN_X), top: px(0), bottom: px(0), flex_direction: FlexDirection::Column, justify_content: JustifyContent::Center, ..default() }, IgnoreHover, ChildOf(root)))
        .id();
    let head = commands.spawn((col(0.0), SlideIn::new(0.0, -36.0, 0.0), ChildOf(left))).id();
    let kick = commands.spawn((row(12.0), ChildOf(head))).id();
    commands.spawn((Node { width: px(24), height: px(2), ..default() }, BackgroundColor(ACCENT), ChildOf(kick)));
    commands.spawn((tech(f, "VOXWING", 14.0, ACCENT), LetterSpacing::Px(12.0), ChildOf(kick)));
    commands.spawn((display(f, "体素星翼", 128.0, INK), LetterSpacing::Px(4.0), Node { margin: UiRect::vertical(px(4)), ..default() }, ChildOf(head)));
    commands.spawn((text(f, "用方块拼出你的战机，在小行星带里和朋友空战", 16.0, MUTED), ChildOf(head)));
    // spans from the column edge to the right edge of the entries' shortcut chips
    let rule = commands.spawn((Node { width: px(MENU_W - 40.0), margin: UiRect { top: px(32), bottom: px(16), ..default() }, ..default() }, ChildOf(head))).id();
    commands.spawn((Node { width: px(48), height: px(2), ..default() }, BackgroundColor(ACCENT), ChildOf(rule)));
    commands.spawn((Node { flex_grow: 1.0, height: px(1), margin: UiRect::top(px(1)), ..default() }, BackgroundColor(LINE_HI), ChildOf(rule)));

    let items: [(&str, &str, UiAction, &str); 4] = [
        ("开始游戏", "PLAY", UiAction::Goto(Screen::Lobby), "Enter"),
        ("机库", "HANGAR", UiAction::Goto(Screen::Hangar), "H"),
        ("设置", "SETTINGS", UiAction::OpenSettings, "S"),
        ("退出", "EXIT", UiAction::Quit, "Esc"),
    ];
    for (i, (zh, en, act, hint)) in items.into_iter().enumerate() {
        let b = spawn_menu_item(&mut commands, f, left, i, zh, en, act, hint);
        commands.entity(b).insert((MenuItem(i), SlideIn::new(0.12 + 0.06 * i as f32, -48.0, 0.0)));
        if i == 0 {
            commands.entity(b).insert(AutoFocus);
        }
    }
    commands.spawn((
        tech(f, "v0.1  ·  BEVY 0.20  ·  QUIC", 11.0, MUTED),
        LetterSpacing::Px(2.0),
        Node { position_type: PositionType::Absolute, left: px(COLUMN_X), bottom: px(EDGE), ..default() },
        ChildOf(root),
    ));

    spawn_ship_card(&mut commands, f, &icons, root, &hangar, true);
}

/// The current ship: name (with ‹ › to flip through your ships), readiness, headline numbers.
fn spawn_ship_card(commands: &mut Commands, f: &UiFont, icons: &Icons, root: Entity, hangar: &Hangar, slide: bool) {
    let a = &hangar.analysis;
    let s = &a.stats;
    let card = spawn_panel(commands, Some(root), Node { position_type: PositionType::Absolute, right: px(2.0 * EDGE), bottom: px(2.0 * EDGE), width: px(344), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), ..default() });
    commands.entity(card).insert(ShipCard);
    if slide {
        commands.entity(card).insert(SlideIn::new(0.25, 40.0, 0.0));
    }
    let sec = spawn_section(commands, f, card, "当前机体|CURRENT SHIP");
    let r = commands.spawn((Node { justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, ..default() }, ChildOf(sec))).id();
    // ‹ name ›: flip through your ships right here
    let pick = commands.spawn((row(GAP_S), ChildOf(r))).id();
    spawn_icon_btn(commands, f, pick, icons.left.clone(), UiAction::ShipPrev);
    commands.spawn((heading(f, hangar.design.name.clone(), 22.0, INK), ChildOf(pick)));
    spawn_icon_btn(commands, f, pick, icons.right.clone(), UiAction::ShipNext);
    if a.ok() {
        commands.spawn((chip(f, "可以出击", OK), ChildOf(r)));
    } else {
        commands.spawn((chip(f, format!("{} 项待修改", a.errors), WARN), ChildOf(r)));
    }
    let grid = commands.spawn((Node { display: Display::Grid, grid_template_columns: RepeatedGridTrack::flex(2, 1.0), row_gap: px(12), column_gap: px(16), ..default() }, ChildOf(card))).id();
    for (label, v, unit) in [
        ("最高速度", format!("{:.0}", s.max_speed), "m/s"),
        ("转向", format!("{:.0}", s.pitch_rate.to_degrees()), "度/秒"),
        ("火力", format!("{:.0}", s.dps), "DPS"),
        ("耐久", format!("{:.0}", s.hp), ""),
    ] {
        let c = commands.spawn((col(2.0), ChildOf(grid))).id();
        commands.spawn((text(f, label, 12.0, MUTED), ChildOf(c)));
        // Bevy's layout has no text baselines, so align line-box bottoms and lift the unit by the
        // difference in descent (both faces put the baseline 0.164em above the line-box bottom)
        let vr = commands.spawn((Node { align_items: AlignItems::FlexEnd, column_gap: px(4), ..default() }, ChildOf(c))).id();
        commands.spawn((tech(f, v, 26.0, INK), ChildOf(vr)));
        if !unit.is_empty() {
            commands.spawn((text(f, unit, 12.0, MUTED), Node { margin: UiRect::bottom(px((26.0 - 12.0) * 0.164)), ..default() }, ChildOf(vr)));
        }
    }
    let br = commands.spawn((row(GAP_S), ChildOf(card))).id();
    spawn_btn(commands, f, br, Kind::Normal, "编辑机体", UiAction::Goto(Screen::Hangar), None);
}

/// Rebuild the card when the ship changes (flipping with ‹ ›).
fn refresh_ship_card(mut commands: Commands, hangar: Res<Hangar>, font: Res<UiFont>, icons: Res<Icons>, cards: Query<Entity, With<ShipCard>>, root: Single<Entity, With<MenuRoot>>) {
    if !hangar.is_changed() || cards.is_empty() {
        return;
    }
    for e in &cards {
        commands.entity(e).despawn();
    }
    spawn_ship_card(&mut commands, &font, &icons, *root, &hangar, false);
}

#[allow(clippy::too_many_arguments)]
fn menu_keys(
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    modal: Res<ModalReq>,
    mut focus: ResMut<InputFocus>,
    items: Query<(Entity, &MenuItem)>,
    mut w: MessageWriter<UiAction>,
) {
    if overlays.any(&modal) {
        return;
    }
    if keys.just_pressed(KeyCode::KeyH) {
        w.write(UiAction::Goto(Screen::Hangar));
    }
    if keys.just_pressed(KeyCode::KeyS) {
        w.write(UiAction::OpenSettings);
    }
    if keys.just_pressed(KeyCode::Escape) {
        w.write(UiAction::Quit);
    }
    let dir = keys.just_pressed(KeyCode::ArrowDown) as i32 - keys.just_pressed(KeyCode::ArrowUp) as i32;
    if dir != 0 {
        let mut list: Vec<(Entity, usize)> = items.iter().map(|(e, m)| (e, m.0)).collect();
        list.sort_by_key(|x| x.1);
        let cur = focus.get().and_then(|e| list.iter().position(|x| x.0 == e)).unwrap_or(0) as i32;
        let n = list.len() as i32;
        let next = ((cur + dir) % n + n) % n;
        focus.set(list[next as usize].0, FocusCause::Navigated);
    }
}
