//! UI components. Bevy's headless widgets (Button, Checkbox, Slider, TextInput) provide the input
//! behaviour; everything visual is a shader-drawn [`Plate`] plus text, animated here.
//!
//! Builders take `&mut Commands` and a parent entity and return the new entity, so screens can
//! compose them freely.

use super::icons::Icons;
use super::*;
use crate::audio::{Sfx, Sounds};
use bevy::input_focus::InputFocus;
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::picking::hover::Hovered;
use bevy::text::{EditableText, LetterSpacing, TextCursorStyle};
use bevy::ui::{Checked, InteractionDisabled, Pressed};
use bevy::ui_widgets::{Activate, Button, Checkbox, ControlOrientation, ScrollArea, Scrollbar, ScrollbarThumb, Slider, SliderRange, SliderThumb, SliderValue, TextInput, TrackClick, ValueChange, observe};
use bevy::window::PrimaryWindow;

pub fn register(app: &mut App) {
    app.init_resource::<ModalReq>()
        .init_resource::<Toasts>()
        .insert_resource(Fade(1.0))
        .add_systems(Startup, spawn_global_layer)
        .add_systems(Update, (animate_buttons, style_toggles, style_sliders, style_choices, style_fields, show_scrollbars, render_modal, render_toasts, tooltips, spin, fade, slide_in))
        .add_observer(click_sound);
}

// ---------------------------------------------------------------- surfaces

/// The standard panel surface: flat, chamfered top-right and bottom-left, accent strip, grain.
pub fn panel_plate() -> Plate {
    Plate { fill: PANEL, edge: LINE, mark: ACCENT, chamfer: 14.0, corners: TR | BL, accent: 28.0, grain: 1.0, ..default() }
}

/// Opaque variant for dialogs and other surfaces that sit over busy backgrounds.
pub fn solid_plate() -> Plate {
    Plate { fill: PANEL_SOLID, ..panel_plate() }
}

/// A flat surface tone, `k` of the way from the panel colour towards `c`. Fills are mixed like
/// this rather than drawn as low-alpha overlays: UI blending happens in linear space, where even
/// a 5 % white overlay reads as a bright grey.
pub fn tone(c: Color, k: f32) -> Color {
    PANEL_SOLID.mix(&c, k)
}

// ---------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Filled accent: the one main action on a screen.
    Primary,
    /// Filled accent at normal size: the confirm button of a dialog.
    Accent,
    Normal,
    Ghost,
    Danger,
    /// Large main-menu entry.
    Menu,
    /// Square part tile showing the part's rendered thumbnail.
    Part,
    /// Large selectable card: title on top, content below (mode, template).
    Card,
    /// Part of a segmented control / tab strip.
    Segment,
    /// Small square icon-like button.
    Icon,
}

#[derive(Component)]
pub struct Btn {
    pub kind: Kind,
    hot: f32,
    was_hover: bool,
}

#[derive(Component)]
struct BtnBar;
#[derive(Component)]
pub struct BtnText;
/// Children that slide right when a menu entry is hovered.
#[derive(Component)]
struct Shift;

/// Width of a main-menu entry; the menu column's rule matches it.
pub const MENU_W: f32 = 400.0;

pub fn btn_node(kind: Kind) -> Node {
    // buttons keep their width in a crowded row: a squeezed label would wrap onto two lines
    let base = Node { height: px(H_MD), padding: UiRect::horizontal(px(16)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, column_gap: px(8), flex_shrink: 0.0, ..default() };
    match kind {
        Kind::Primary => Node { height: px(H_LG), padding: UiRect::horizontal(px(24)), column_gap: px(12), ..base },
        Kind::Normal | Kind::Danger | Kind::Accent => base,
        Kind::Ghost => Node { padding: UiRect::horizontal(px(8)), ..base },
        // the label sits 20px in, so a negative margin lines it up with the column it is in
        // centre-aligned, not baseline: CJK glyphs sit much taller than the Latin caption and
        // shortcut chip, so a shared baseline leaves the small items visibly low
        Kind::Menu => Node { height: px(56), width: px(MENU_W), padding: UiRect::horizontal(px(20)), margin: UiRect::left(px(-20)), justify_content: JustifyContent::FlexStart, align_items: AlignItems::Center, column_gap: px(16), ..base },
        Kind::Part => Node { height: Val::Auto, aspect_ratio: Some(1.0), padding: px(3).all(), ..base },
        Kind::Card => Node { height: Val::Auto, width: percent(100), flex_direction: FlexDirection::Column, align_items: AlignItems::FlexStart, padding: px(16).all(), row_gap: px(6), ..base },
        Kind::Segment => Node { padding: UiRect::horizontal(px(12)), flex_grow: 1.0, flex_basis: px(0), ..base },
        Kind::Icon => Node { width: px(H_MD), padding: px(0).all(), ..base },
    }
}

fn btn_plate(kind: Kind) -> Option<Plate> {
    let p = Plate { mark: ACCENT, chamfer: 7.0, corners: BR, ..default() };
    match kind {
        Kind::Ghost => None,
        Kind::Primary => Some(Plate { chamfer: 14.0, corners: TL | BR, ..p }),
        Kind::Accent => Some(Plate { chamfer: 8.0, corners: TL | BR, ..p }),
        // the accent bar closes the left side, so only the far corner is cut
        Kind::Menu => Some(Plate { chamfer: 12.0, corners: BR, edge_w: 0.0, ..p }),
        Kind::Card => Some(Plate { chamfer: 12.0, corners: TL | BR, ..p }),
        Kind::Segment => Some(Plate { chamfer: 0.0, corners: 0, ..p }),
        _ => Some(p),
    }
}

fn label_font(f: &UiFont, kind: Kind, label: String) -> impl Bundle {
    let (size, weight) = match kind {
        Kind::Primary => (18.0, 800),
        Kind::Menu => (26.0, 800),
        Kind::Card => (17.0, 800),
        Kind::Part | Kind::Icon => (13.0, 600),
        Kind::Accent => (14.0, 700),
        _ => (14.0, 600),
    };
    (Text::new(label), TextFont { font: f.body.clone(), font_size: FontSize::Px(size), weight: bevy::text::FontWeight(weight), ..default() })
}

/// A button. `hint` shows a keyboard shortcut chip on the right.
pub fn spawn_btn(commands: &mut Commands, f: &UiFont, parent: Entity, kind: Kind, label: impl Into<String>, action: UiAction, hint: Option<&str>) -> Entity {
    let a = action.clone();
    let e = commands
        .spawn((
            Button,
            Hovered::default(),
            Btn { kind, hot: 0.0, was_hover: false },
            Selected(false),
            ActionOf(action),
            TabIndex(0),
            btn_node(kind),
            observe(move |_: On<Activate>, mut w: MessageWriter<UiAction>| {
                w.write(a.clone());
            }),
            ChildOf(parent),
        ))
        .id();
    if let Some(p) = btn_plate(kind) {
        commands.entity(e).insert(p);
    }
    let filled = matches!(kind, Kind::Primary | Kind::Accent);
    if matches!(kind, Kind::Menu | Kind::Card | Kind::Normal | Kind::Danger | Kind::Icon) {
        commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(6), bottom: px(6), width: px(0), ..default() }, BackgroundColor(ACCENT), BtnBar, Pickable::IGNORE, ChildOf(e)));
    }
    let color = if filled { ON_ACCENT } else { INK };
    let l = commands.spawn((label_font(f, kind, label.into()), TextColor(color), BtnText, UiTransform::default(), Pickable::IGNORE, ChildOf(e))).id();
    if kind == Kind::Menu {
        commands.entity(l).insert(Shift);
    }
    if kind == Kind::Primary {
        commands.entity(l).insert(LetterSpacing::Px(6.0));
    }
    if let Some(h) = hint {
        let (ink, line) = if filled { (ON_ACCENT.with_alpha(0.65), ON_ACCENT.with_alpha(0.35)) } else { (MUTED, LINE_HI) };
        // the main action keeps its label centred; its shortcut sits at the right edge
        let place = if kind == Kind::Primary {
            Node { position_type: PositionType::Absolute, right: px(16), top: px((H_LG - 18.0) / 2.0), ..default() }
        } else {
            Node { margin: UiRect::left(Val::Auto), ..default() }
        };
        commands.spawn((
            // 2px bottom padding: Latin caps sit ~1px below the centre of a line box
            Node { height: px(18), padding: UiRect { left: px(6), right: px(6), top: px(0), bottom: px(2) }, border: px(1).all(), align_items: AlignItems::Center, ..place },
            BorderColor::all(line),
            Pickable::IGNORE,
            ChildOf(e),
            children![(tech(f, h, 11.0, ink), Pickable::IGNORE)],
        ));
    }
    e
}

/// Main-menu entry: index, title, English caption and shortcut.
pub fn spawn_menu_item(commands: &mut Commands, f: &UiFont, parent: Entity, index: usize, zh: &str, en: &str, action: UiAction, hint: &str) -> Entity {
    let e = spawn_btn(commands, f, parent, Kind::Menu, zh, action, Some(hint));
    let idx = commands.spawn((tech(f, format!("{:02}", index + 1), 14.0, ACCENT), Node { width: px(20), ..default() }, UiTransform::default(), Shift, Pickable::IGNORE)).id();
    let cap = commands.spawn((tech(f, en, 12.0, MUTED), LetterSpacing::Px(3.0), UiTransform::default(), Shift, Pickable::IGNORE)).id();
    // children so far: bar, label, hint
    commands.entity(e).insert_children(1, &[idx]);
    commands.entity(e).insert_children(3, &[cap]);
    e
}

/// Part tile: the part's rendered thumbnail with its shortcut in the corner. The name and details
/// come from its [`Tip`] / [`TipDetail`].
pub fn spawn_part_tile(commands: &mut Commands, f: &UiFont, parent: Entity, image: Handle<Image>, key: &str, action: UiAction) -> Entity {
    let a = action.clone();
    let e = commands
        .spawn((
            Button,
            Hovered::default(),
            Btn { kind: Kind::Part, hot: 0.0, was_hover: false },
            Selected(false),
            ActionOf(action),
            TabIndex(0),
            btn_node(Kind::Part),
            btn_plate(Kind::Part).unwrap_or_default(),
            observe(move |_: On<Activate>, mut w: MessageWriter<UiAction>| {
                w.write(a.clone());
            }),
            ChildOf(parent),
        ))
        .id();
    commands.spawn((ImageNode::new(image), Node { width: percent(100), height: percent(100), ..default() }, Pickable::IGNORE, ChildOf(e)));
    commands.spawn((tech(f, key, 10.0, MUTED), Node { position_type: PositionType::Absolute, right: px(6), top: px(4), ..default() }, Pickable::IGNORE, ChildOf(e)));
    e
}

#[allow(clippy::type_complexity)]
fn animate_buttons(
    time: Res<Time>,
    focus: Res<InputFocus>,
    mut sounds: ResMut<Sounds>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut Btn, &Hovered, Has<Pressed>, &Selected, Has<InteractionDisabled>, Option<&mut Plate>, &Children)>,
    mut bars: Query<&mut Node, With<BtnBar>>,
    mut labels: Query<&mut TextColor, With<BtnText>>,
    mut shifts: Query<&mut UiTransform, With<Shift>>,
) {
    let dt = time.delta_secs();
    let now = time.elapsed_secs_f64();
    for (e, mut b, hovered, pressed, sel, disabled, plate, children) in &mut q {
        let hover = hovered.get() && !disabled;
        if hover && !b.was_hover {
            sounds.play(&mut commands, now, Sfx::UiHover, 0.25, 0.04);
        }
        b.was_hover = hover;
        let focused = focus.get() == Some(e);
        let filled = matches!(b.kind, Kind::Primary | Kind::Accent);
        // keyboard focus shows as an outline on filled buttons, as a half hover elsewhere
        let target: f32 = if disabled {
            0.0
        } else if pressed {
            1.25
        } else if hover {
            1.0
        } else if focused && !filled {
            0.7
        } else {
            0.0
        };
        b.hot += (target - b.hot) * (dt * 14.0).min(1.0);
        if (target - b.hot).abs() < 0.003 {
            b.hot = target;
        }
        let h = b.hot;
        let h1 = h.min(1.0);
        let press = (h - 1.0).max(0.0) * 4.0;
        let white = |a: f32| Color::srgba(1.0, 1.0, 1.0, a);
        // flat colours only: (fill, edge, label ink, accent strip)
        let (fill, edge, ink, accent) = match b.kind {
            Kind::Primary | Kind::Accent if disabled => (tone(Color::WHITE, 0.08), Color::NONE, MUTED, 0.0),
            // hover flips the accent fill to off-white; pressing dips it back a little
            Kind::Primary | Kind::Accent => (ACCENT.mix(&INK, h1 * 0.9 - press * 0.3), if focused { INK } else { Color::NONE }, ON_ACCENT, 0.0),
            Kind::Ghost => (Color::NONE, Color::NONE, MUTED.mix(&INK, h1), 0.0),
            Kind::Danger => (tone(DANGER, 0.2 + 0.14 * h1 + 0.1 * press), DANGER.with_alpha(0.4 + 0.4 * h1), INK, 0.0),
            Kind::Menu => (tone(Color::WHITE, 0.05 + 0.04 * press).with_alpha(0.85 * h1), Color::NONE, MUTED.mix(&INK, (0.4 + h).min(1.0)), 0.0),
            Kind::Segment | Kind::Part | Kind::Card | Kind::Normal | Kind::Icon if sel.0 => (
                tone(ACCENT, 0.16 + 0.06 * h1),
                ACCENT,
                INK,
                match b.kind {
                    Kind::Part => 18.0,
                    Kind::Card => 40.0,
                    _ => 0.0,
                },
            ),
            _ => (tone(Color::WHITE, 0.06 + 0.05 * h1 + 0.05 * press), if focused { ACCENT } else { white(0.08 + 0.22 * h1) }, INK, 0.0),
        };
        if let Some(mut p) = plate {
            let dim = if disabled && !matches!(b.kind, Kind::Primary | Kind::Accent) { 0.5 } else { 1.0 };
            let next = Plate { fill: fill.with_alpha(fill.alpha() * dim), edge: edge.with_alpha(edge.alpha() * dim), accent, ..*p };
            p.set_if_neq(next);
        }
        for c in children.iter() {
            if let Ok(mut n) = bars.get_mut(c) {
                let w = px(if b.kind == Kind::Menu { 4.0 } else { 3.0 } * h1);
                if n.width != w {
                    n.width = w;
                }
            }
            if let Ok(mut tc) = labels.get_mut(c) {
                tc.set_if_neq(TextColor(if disabled { ink.with_alpha(0.4) } else { ink }));
            }
            if let Ok(mut ut) = shifts.get_mut(c) {
                let to = Val2::px(10.0 * h1, 0.0);
                if ut.translation != to {
                    ut.translation = to;
                }
            }
        }
    }
}

fn click_sound(_: On<Activate>, mut sounds: ResMut<Sounds>, mut commands: Commands, time: Res<Time>) {
    sounds.play(&mut commands, time.elapsed_secs_f64(), Sfx::UiClick, 0.45, 0.03);
}

/// Disable or enable a button (disabled buttons ignore clicks and render dimmed).
pub fn set_enabled(commands: &mut Commands, e: Entity, on: bool) {
    if on {
        commands.entity(e).remove::<InteractionDisabled>();
    } else {
        commands.entity(e).insert(InteractionDisabled);
    }
}

// ---------------------------------------------------------------- panels and headers

/// A panel. Returns the panel entity; put content under it. `node` controls size and layout.
pub fn spawn_panel(commands: &mut Commands, parent: Option<Entity>, node: Node) -> Entity {
    let scrolls = node.overflow.y == OverflowAxis::Scroll;
    let e = commands.spawn((node, panel_plate())).id();
    if scrolls {
        commands.entity(e).insert(ScrollArea);
    }
    if let Some(p) = parent {
        commands.entity(e).insert(ChildOf(p));
    }
    e
}

/// A column that shrinks to fit its parent and scrolls (wheel or trackpad) when its content
/// doesn't fit, with a thin bar at the right that only shows while there is something to scroll.
/// Returns the column to fill.
pub fn spawn_scroll_body(commands: &mut Commands, parent: Entity) -> Entity {
    let frame = commands.spawn((Node { min_height: px(0), flex_shrink: 1.0, column_gap: px(GAP_S), ..default() }, ChildOf(parent))).id();
    let body = commands
        .spawn((Node { flex_grow: 1.0, flex_direction: FlexDirection::Column, row_gap: px(GAP), overflow: Overflow::scroll_y(), ..default() }, ScrollArea, ChildOf(frame)))
        .id();
    let bar = commands
        .spawn((
            Node { width: px(3), display: Display::None, ..default() },
            BackgroundColor(LINE),
            Scrollbar { target: body, orientation: ControlOrientation::Vertical, min_thumb_length: 32.0 },
            ChildOf(frame),
        ))
        .id();
    commands.spawn((ScrollbarThumb::default(), BackgroundColor(LINE_HI), ChildOf(bar)));
    body
}

/// Show a scrollbar only while its column overflows.
fn show_scrollbars(mut bars: Query<(&Scrollbar, &mut Node)>, areas: Query<&ComputedNode>) {
    for (bar, mut node) in &mut bars {
        let Ok(c) = areas.get(bar.target) else { continue };
        let want = if c.content_size.y > c.size.y + 1.0 { Display::Flex } else { Display::None };
        if node.display != want {
            node.display = want;
        }
    }
}

/// Section header: accent bar, caption, hairline to the right. `"中文|ENGLISH"` adds a Latin
/// caption after the Chinese one.
pub fn spawn_header(commands: &mut Commands, f: &UiFont, parent: Entity, label: &str) -> Entity {
    let (zh, en) = label.split_once('|').unwrap_or((label, ""));
    let h = commands.spawn((Node { height: px(16), column_gap: px(8), align_items: AlignItems::Center, ..default() }, ChildOf(parent))).id();
    commands.spawn((Node { width: px(3), height: px(12), ..default() }, BackgroundColor(ACCENT), ChildOf(h)));
    commands.spawn((strong(f, zh, 13.0, INK.with_alpha(0.85)), LetterSpacing::Px(1.0), ChildOf(h)));
    if !en.is_empty() {
        commands.spawn((tech(f, en, 11.0, MUTED), LetterSpacing::Px(2.5), ChildOf(h)));
    }
    commands.spawn((Node { flex_grow: 1.0, height: px(1), ..default() }, BackgroundColor(LINE), ChildOf(h)));
    h
}

/// A titled section: header, then a column for its content (returned). Panels stack sections
/// `GAP` apart; items inside a section sit `GAP_S` apart.
pub fn spawn_section(commands: &mut Commands, f: &UiFont, parent: Entity, label: &str) -> Entity {
    let s = commands.spawn((col(GAP_S), ChildOf(parent))).id();
    spawn_header(commands, f, s, label);
    s
}

/// Screen title: heading with a Latin caption and an accent slash, `H_MD` tall like the
/// controls next to it.
pub fn spawn_title(commands: &mut Commands, f: &UiFont, parent: Entity, zh: &str, en: &str) -> Entity {
    let t = commands.spawn((Node { height: px(H_MD), align_items: AlignItems::Center, column_gap: px(12), flex_shrink: 0.0, ..default() }, ChildOf(parent))).id();
    commands.spawn((Node { width: px(3), height: px(22), ..default() }, BackgroundColor(ACCENT), UiTransform { rotation: Rot2::degrees(14.0), ..default() }, ChildOf(t)));
    commands.spawn((heading(f, zh, 24.0, INK), ChildOf(t)));
    commands.spawn((tech(f, en, 12.0, MUTED), LetterSpacing::Px(4.0), ChildOf(t)));
    t
}

/// Small coloured label.
pub fn chip(f: &UiFont, s: impl Into<String>, color: Color) -> impl Bundle {
    (
        Node { height: px(22), padding: UiRect::horizontal(px(8)), align_items: AlignItems::Center, ..default() },
        Plate { fill: tone(color, 0.16), edge: color.with_alpha(0.5), chamfer: 6.0, corners: BR, ..default() },
        children![strong(f, s, 12.0, color)],
    )
}

pub fn row(gap: f32) -> Node {
    Node { column_gap: px(gap), align_items: AlignItems::Center, ..default() }
}

pub fn col(gap: f32) -> Node {
    Node { flex_direction: FlexDirection::Column, row_gap: px(gap), ..default() }
}

// ---------------------------------------------------------------- toggle

#[derive(Component)]
struct ToggleVis {
    track: Entity,
    knob: Entity,
}

pub fn spawn_toggle(commands: &mut Commands, f: &UiFont, parent: Entity, label: &str, on: bool, key: SetKey) -> Entity {
    let e = commands
        .spawn((
            Checkbox,
            Hovered::default(),
            TabIndex(0),
            Node { height: px(H_MD), justify_content: JustifyContent::SpaceBetween, align_items: AlignItems::Center, column_gap: px(16), ..default() },
            observe(move |ev: On<ValueChange<bool>>, mut commands: Commands, mut w: MessageWriter<UiAction>| {
                if ev.value {
                    commands.entity(ev.source).insert(Checked);
                } else {
                    commands.entity(ev.source).remove::<Checked>();
                }
                w.write(UiAction::Set(key, if ev.value { 1.0 } else { 0.0 }));
            }),
            ChildOf(parent),
        ))
        .id();
    commands.spawn((text(f, label, 14.0, INK), Pickable::IGNORE, ChildOf(e)));
    let track = commands.spawn((Node { width: px(40), height: px(20), ..default() }, Plate { chamfer: 6.0, corners: TL | BR, ..default() }, Pickable::IGNORE, ChildOf(e))).id();
    let knob = commands.spawn((Node { position_type: PositionType::Absolute, width: px(14), height: px(14), top: px(3), left: px(3), ..default() }, Plate { chamfer: 4.0, corners: TL | BR, edge_w: 0.0, ..default() }, Pickable::IGNORE, ChildOf(track))).id();
    commands.entity(e).insert(ToggleVis { track, knob });
    if on {
        commands.entity(e).insert(Checked);
    }
    e
}

fn style_toggles(q: Query<(Has<Checked>, &Hovered, &ToggleVis)>, mut plates: Query<(&mut Node, &mut Plate)>) {
    for (on, hov, vis) in &q {
        let Ok((_, mut tp)) = plates.get_mut(vis.track) else { continue };
        let next = Plate {
            fill: if on { ACCENT } else { Color::srgba(0.0, 0.0, 0.0, 0.3) },
            edge: if on { ACCENT } else { LINE_HI.with_alpha(if hov.get() { 1.0 } else { 0.6 }) },
            ..*tp
        };
        tp.set_if_neq(next);
        if let Ok((mut n, mut kp)) = plates.get_mut(vis.knob) {
            let left = px(if on { 23.0 } else { 3.0 });
            if n.left != left {
                n.left = left;
            }
            kp.set_if_neq(Plate { fill: if on { ON_ACCENT } else { MUTED }, ..*kp });
        }
    }
}

// ---------------------------------------------------------------- slider

#[derive(Component)]
struct SliderVis {
    fill: Entity,
    knob: Entity,
    readout: Entity,
    fmt: fn(f32) -> String,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_slider(commands: &mut Commands, f: &UiFont, parent: Entity, label: &str, value: f32, min: f32, max: f32, key: SetKey, fmt: fn(f32) -> String) -> Entity {
    let r = commands.spawn((Node { height: px(H_MD), ..row(16.0) }, ChildOf(parent))).id();
    commands.spawn((text(f, label, 14.0, INK), Node { flex_grow: 1.0, ..default() }, ChildOf(r)));
    let s = commands
        .spawn((
            Slider { track_click: TrackClick::Snap, ..default() },
            SliderValue(value),
            SliderRange::new(min, max),
            Hovered::default(),
            TabIndex(0),
            Node { width: px(200), height: px(22), ..default() },
            observe(move |ev: On<ValueChange<f32>>, mut commands: Commands, mut w: MessageWriter<UiAction>| {
                commands.entity(ev.source).insert(SliderValue(ev.value));
                w.write(UiAction::Set(key, ev.value));
            }),
            ChildOf(r),
        ))
        .id();
    commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(10), height: px(2), ..default() }, BackgroundColor(LINE_HI.with_alpha(0.35)), Pickable::IGNORE, ChildOf(s)));
    let fill = commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), width: percent(50), top: px(9), height: px(4), ..default() }, BackgroundColor(ACCENT), Pickable::IGNORE, ChildOf(s))).id();
    let lane = commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), right: px(12), top: px(0), bottom: px(0), ..default() }, Pickable::IGNORE, ChildOf(s))).id();
    let knob = commands
        .spawn((SliderThumb, Node { position_type: PositionType::Absolute, width: px(12), height: px(22), left: percent(50), ..default() }, Plate { fill: INK, chamfer: 4.0, corners: TL | BR, edge_w: 0.0, ..default() }, ChildOf(lane)))
        .id();
    let readout = commands.spawn((tech(f, fmt(value), 14.0, INK), Node { width: px(56), justify_content: JustifyContent::FlexEnd, ..default() }, TextLayout::justify(Justify::Right), ChildOf(r))).id();
    commands.entity(s).insert(SliderVis { fill, knob, readout, fmt });
    s
}

fn style_sliders(q: Query<(&SliderValue, &SliderRange, &SliderVis, &Hovered), Or<(Changed<SliderValue>, Changed<Hovered>, Added<SliderVis>)>>, mut nodes: Query<&mut Node>, mut plates: Query<&mut Plate>, mut texts: Query<&mut Text>) {
    for (v, r, vis, hov) in &q {
        let frac = ((v.0 - r.start()) / r.span().max(1e-6)).clamp(0.0, 1.0);
        if let Ok(mut n) = nodes.get_mut(vis.fill) {
            n.width = percent(frac * 100.0);
        }
        if let Ok(mut n) = nodes.get_mut(vis.knob) {
            n.left = percent(frac * 100.0);
        }
        if let Ok(mut p) = plates.get_mut(vis.knob) {
            p.set_if_neq(Plate { fill: if hov.get() { ACCENT } else { INK }, ..*p });
        }
        if let Ok(mut t) = texts.get_mut(vis.readout) {
            t.0 = (vis.fmt)(v.0);
        }
    }
}

// ---------------------------------------------------------------- choice

/// A "< value >" selector over a fixed list of options; each option carries the action sent when
/// it is chosen. It stops at both ends (that arrow is then disabled).
#[derive(Component)]
pub struct Choice {
    options: Vec<(String, UiAction)>,
    index: usize,
    enabled: bool,
    value: Entity,
    arrows: [Entity; 2],
}

/// A square button showing one of the vector icons (the ‹ › of a carousel, say).
pub fn spawn_icon_btn(commands: &mut Commands, f: &UiFont, parent: Entity, image: Handle<Image>, action: UiAction) -> Entity {
    let b = spawn_btn(commands, f, parent, Kind::Icon, "", action, None);
    commands.spawn((ImageNode { image, color: INK, ..default() }, Node { width: px(16), height: px(16), ..default() }, Pickable::IGNORE, ChildOf(b)));
    b
}

/// A settings row: label on the left, selector on the right. `enabled: false` shows the value
/// greyed out with both arrows off (for a setting the current mode decides).
#[allow(clippy::too_many_arguments)]
pub fn spawn_choice(commands: &mut Commands, f: &UiFont, icons: &Icons, parent: Entity, label: &str, options: Vec<(String, UiAction)>, index: usize, enabled: bool) -> Entity {
    let r = commands.spawn((Node { height: px(H_MD), ..row(16.0) }, ChildOf(parent))).id();
    commands.spawn((text(f, label, 14.0, INK), Node { flex_grow: 1.0, ..default() }, ChildOf(r)));
    let ctl = commands
        .spawn((
            Node { width: px(232), height: px(H_MD), align_items: AlignItems::Center, ..default() },
            Plate { fill: Color::srgba(0.0, 0.0, 0.0, 0.4), edge: LINE_HI, chamfer: 8.0, corners: BR, ..default() },
            ChildOf(r),
        ))
        .id();
    let arrow = |commands: &mut Commands, image: Handle<Image>, dir: i32| {
        let b = commands
            .spawn((
                Button,
                Hovered::default(),
                Btn { kind: Kind::Ghost, hot: 0.0, was_hover: false },
                Selected(false),
                TabIndex(0),
                Node { width: px(34), height: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
                observe(move |_: On<Activate>, mut q: Query<&mut Choice>, mut w: MessageWriter<UiAction>| {
                    let Ok(mut c) = q.get_mut(ctl) else { return };
                    let i = c.index as i32 + dir;
                    if c.enabled && i >= 0 && (i as usize) < c.options.len() {
                        c.index = i as usize;
                        w.write(c.options[c.index].1.clone());
                    }
                }),
                ChildOf(ctl),
            ))
            .id();
        commands.spawn((ImageNode { image, color: MUTED, ..default() }, Node { width: px(16), height: px(16), ..default() }, Pickable::IGNORE, ChildOf(b)));
        b
    };
    let prev = arrow(commands, icons.left.clone(), -1);
    let value = commands.spawn((strong(f, "", 14.0, INK), Node { flex_grow: 1.0, justify_content: JustifyContent::Center, ..default() }, TextLayout::justify(Justify::Center), Pickable::IGNORE, ChildOf(ctl))).id();
    let next = arrow(commands, icons.right.clone(), 1);
    commands.entity(ctl).insert(Choice { options, index, enabled, value, arrows: [prev, next] });
    ctl
}

fn style_choices(mut commands: Commands, q: Query<&Choice, Changed<Choice>>, mut texts: Query<(&mut Text, &mut TextColor)>, kids: Query<&Children>, mut images: Query<&mut ImageNode>) {
    for c in &q {
        if let Ok((mut t, mut tc)) = texts.get_mut(c.value) {
            t.0 = c.options.get(c.index).map_or(String::new(), |o| o.0.clone());
            tc.0 = if c.enabled { INK } else { MUTED };
        }
        for (k, arrow) in c.arrows.into_iter().enumerate() {
            let at_end = if k == 0 { c.index == 0 } else { c.index + 1 >= c.options.len() };
            let on = c.enabled && !at_end;
            set_enabled(&mut commands, arrow, on);
            for child in kids.iter_descendants(arrow) {
                if let Ok(mut img) = images.get_mut(child) {
                    img.color = if on { INK } else { MUTED.with_alpha(0.35) };
                }
            }
        }
    }
}

// ---------------------------------------------------------------- text field

#[derive(Component)]
struct FieldVis;

pub fn spawn_field(commands: &mut Commands, f: &UiFont, parent: Entity, value: &str, chars: f32) -> Entity {
    commands
        .spawn((
            // 9 + 18 (15px text line) + 9 = H_MD
            Node { padding: UiRect::axes(px(12), px(9)), min_width: px(chars * 15.0), ..default() },
            Plate { fill: Color::srgba(0.0, 0.0, 0.0, 0.4), edge: LINE_HI, mark: ACCENT, chamfer: 8.0, corners: BR, ..default() },
            TextInput,
            EditableText { visible_width: Some(chars), allow_newlines: false, ..EditableText::new(value) },
            TextLayout::no_wrap(),
            tf(f, 15.0),
            TextColor(INK),
            TextCursorStyle::default(),
            TabIndex(0),
            FieldVis,
            ChildOf(parent),
        ))
        .id()
}

fn style_fields(focus: Res<InputFocus>, mut q: Query<(Entity, &mut Plate), With<FieldVis>>) {
    for (e, mut p) in &mut q {
        let on = focus.get() == Some(e);
        let next = Plate { edge: if on { ACCENT } else { LINE_HI }, accent: if on { 24.0 } else { 0.0 }, ..*p };
        p.set_if_neq(next);
    }
}

/// True when a text field has keyboard focus (screens skip their hotkeys then).
pub fn typing(focus: &InputFocus, fields: &Query<(), With<EditableText>>) -> bool {
    focus.get().is_some_and(|e| fields.contains(e))
}

// ---------------------------------------------------------------- stat gauge

/// Labelled segmented gauge. Returns (gauge, value-text) entities; set the gauge's
/// [`Plate::frac`] to change the fill.
pub fn spawn_stat(commands: &mut Commands, f: &UiFont, parent: Entity, label: &str, color: Color) -> (Entity, Entity) {
    let r = commands.spawn((Node { height: px(20), ..row(12.0) }, ChildOf(parent))).id();
    commands.spawn((text(f, label, 13.0, MUTED), Node { width: px(36), ..default() }, ChildOf(r)));
    let gauge = commands.spawn((Node { flex_grow: 1.0, height: px(8), ..default() }, Plate::gauge(color, 24.0, 0.0), ChildOf(r))).id();
    let val = commands.spawn((tech(f, "", 14.0, INK), Node { width: px(88), justify_content: JustifyContent::FlexEnd, ..default() }, TextLayout::justify(Justify::Right), ChildOf(r))).id();
    (gauge, val)
}

// ---------------------------------------------------------------- overlays: modal, toasts, tooltip, fade, spinner

/// Dimmed full-screen layer with a centred panel. Returns (overlay, panel).
pub fn spawn_overlay(commands: &mut Commands, f: &UiFont, title: &str, width: f32) -> (Entity, Entity) {
    let o = commands
        .spawn((
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.005, 0.01, 0.72)),
            GlobalZIndex(50),
        ))
        .id();
    let p = commands
        .spawn((
            Node { width: px(width), max_height: percent(86), padding: px(24).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), overflow: Overflow::scroll_y(), ..default() },
            solid_plate(),
            ScrollArea,
            SlideIn::new(0.0, 0.0, 18.0),
            ChildOf(o),
        ))
        .id();
    commands.spawn((heading(f, title, 22.0, INK), ChildOf(p)));
    (o, p)
}

pub struct ModalSpec {
    pub title: String,
    pub body: String,
    pub buttons: Vec<(String, UiAction, Kind)>,
    /// Action for Esc / clicking outside; None just closes.
    pub cancel: Option<UiAction>,
}

#[derive(Resource, Default)]
pub struct ModalReq(pub Option<ModalSpec>);

impl ModalReq {
    pub fn show(&mut self, title: impl Into<String>, body: impl Into<String>, buttons: Vec<(&str, UiAction, Kind)>, cancel: Option<UiAction>) {
        self.0 = Some(ModalSpec { title: title.into(), body: body.into(), buttons: buttons.into_iter().map(|(a, b, c)| (a.to_string(), b, c)).collect(), cancel });
    }
    pub fn open(&self) -> bool {
        self.0.is_some()
    }
}

#[derive(Component)]
struct ModalRoot;

fn render_modal(mut commands: Commands, req: Res<ModalReq>, roots: Query<Entity, With<ModalRoot>>, font: Option<Res<UiFont>>, keys: Res<ButtonInput<KeyCode>>, mut w: MessageWriter<UiAction>) {
    if req.is_changed() {
        for e in &roots {
            commands.entity(e).despawn();
        }
        if let (Some(spec), Some(f)) = (&req.0, font.as_ref()) {
            let (o, p) = spawn_overlay(&mut commands, f, &spec.title, 480.0);
            commands.entity(o).insert((ModalRoot, GlobalZIndex(80)));
            commands.spawn((para(f, spec.body.clone(), 14.0, INK.with_alpha(0.9)), ChildOf(p)));
            let r = commands.spawn((Node { column_gap: px(GAP_S), justify_content: JustifyContent::FlexEnd, margin: UiRect::top(px(GAP_S)), ..default() }, ChildOf(p))).id();
            for (label, act, kind) in &spec.buttons {
                spawn_btn(&mut commands, f, r, *kind, label.clone(), act.clone(), None);
            }
        }
    }
    if req.0.is_some() && keys.just_pressed(KeyCode::Escape) {
        w.write(req.0.as_ref().and_then(|s| s.cancel.clone()).unwrap_or(UiAction::CloseModal));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Ok,
    Warn,
    Err,
}

#[derive(Resource, Default)]
pub struct Toasts {
    items: Vec<(String, ToastKind, f32)>,
    dirty: bool,
}

impl Toasts {
    pub fn push(&mut self, s: impl Into<String>, k: ToastKind) {
        self.items.push((s.into(), k, 3.2));
        if self.items.len() > 4 {
            self.items.remove(0);
        }
        self.dirty = true;
    }
}

#[derive(Component)]
struct ToastLayer;
#[derive(Component)]
struct TipLayer;
#[derive(Component)]
struct FadeLayer;

/// Always-on-top layer for toasts, tooltips and screen fades.
fn spawn_global_layer(mut commands: Commands) {
    commands.spawn((
        Node { position_type: PositionType::Absolute, bottom: px(64), left: px(0), right: px(0), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(6), ..default() },
        GlobalZIndex(90),
        Pickable::IGNORE,
        ToastLayer,
    ));
    commands.spawn((
        Node { position_type: PositionType::Absolute, padding: UiRect::axes(px(12), px(8)), max_width: px(300), flex_direction: FlexDirection::Column, row_gap: px(4), ..default() },
        Plate { chamfer: 8.0, corners: BR, grain: 0.0, accent: 18.0, ..solid_plate() },
        Visibility::Hidden,
        GlobalZIndex(95),
        Pickable::IGNORE,
        TipLayer,
    ));
    commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, BackgroundColor(Color::BLACK), GlobalZIndex(100), Pickable::IGNORE, FadeLayer));
}

fn render_toasts(mut commands: Commands, time: Res<Time>, mut toasts: ResMut<Toasts>, layer: Single<Entity, With<ToastLayer>>, font: Option<Res<UiFont>>) {
    let dt = time.delta_secs();
    let before = toasts.items.len();
    for t in toasts.items.iter_mut() {
        t.2 -= dt;
    }
    toasts.items.retain(|t| t.2 > 0.0);
    if toasts.items.len() != before {
        toasts.dirty = true;
    }
    let Some(f) = font else { return };
    if !toasts.dirty {
        return;
    }
    toasts.dirty = false;
    commands.entity(*layer).despawn_children();
    for (s, k, _) in &toasts.items {
        let c = match k {
            ToastKind::Info => ACCENT,
            ToastKind::Ok => OK,
            ToastKind::Warn => WARN,
            ToastKind::Err => DANGER,
        };
        commands.spawn((
            Node { column_gap: px(10), align_items: AlignItems::Center, padding: UiRect { left: px(0), right: px(16), top: px(9), bottom: px(9) }, ..default() },
            Plate { edge: c.with_alpha(0.45), chamfer: 8.0, corners: TL | BR, grain: 0.0, accent: 0.0, ..solid_plate() },
            Pickable::IGNORE,
            ChildOf(*layer),
            children![(Node { width: px(3), height: px(18), ..default() }, BackgroundColor(c)), text(&f, s.clone(), 14.0, INK)],
        ));
    }
}

/// Hover help text for any UI element.
#[derive(Component)]
pub struct Tip(pub String);

/// More about the element, shown in its tooltip while Shift is held.
#[derive(Component)]
pub struct TipDetail(pub String);

#[allow(clippy::too_many_arguments)]
fn tooltips(
    mut commands: Commands,
    tips: Query<(&Tip, Option<&TipDetail>, &Hovered)>,
    layer: Single<(Entity, &mut Node, &mut Visibility, &ComputedNode), With<TipLayer>>,
    window: Single<&Window, With<PrimaryWindow>>,
    keys: Res<ButtonInput<KeyCode>>,
    font: Option<Res<UiFont>>,
    dev: Res<crate::DevOpts>,
    time: Res<Time>,
    mut current: Local<String>,
) {
    let (e, mut node, mut vis, computed) = layer.into_inner();
    // dev: show the first detailed tooltip as if hovered with Shift held (offscreen runs have no cursor)
    let forced = dev.forced("tip", time.elapsed_secs());
    let shift = forced || keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let hovered = tips.iter().find(|(_, d, h)| h.get() || (forced && d.is_some())).map(|(t, d, _)| (t.0.clone(), d.map(|d| d.0.clone())));
    let cursor = if forced { Some(Vec2::new(160.0, 200.0)) } else { window.cursor_position() };
    match (hovered, cursor, font) {
        (Some((tip, detail)), Some(c), Some(f)) => {
            // with details the box gets a definite width, so the wrapped text sets its height;
            // a bare name just fits its text
            let width = if shift && detail.is_some() { px(300) } else { Val::Auto };
            if node.width != width {
                node.width = width;
            }
            let key = format!("{tip}\u{1}{}\u{1}{shift}", detail.as_deref().unwrap_or(""));
            if *current != key {
                *current = key;
                commands.entity(e).despawn_children();
                if detail.is_some() {
                    commands.spawn((strong(&f, tip, 13.0, INK), ChildOf(e)));
                } else {
                    commands.spawn((text(&f, tip, 13.0, INK), ChildOf(e)));
                }
                match detail {
                    Some(d) if shift => {
                        commands.spawn((para(&f, d, 12.0, INK.with_alpha(0.8)), ChildOf(e)));
                    }
                    Some(_) => {
                        commands.spawn((text(&f, "按住 Shift 查看详情", 11.0, MUTED), ChildOf(e)));
                    }
                    None => {}
                }
            }
            // place by the box's actual size (last frame's layout): below-right of the cursor,
            // flipped above or left when it would leave the window
            let size = computed.size() * computed.inverse_scale_factor;
            let x = (c.x + 16.0).min(window.width() - size.x - 8.0).max(8.0);
            let y = if c.y + 18.0 + size.y > window.height() - 8.0 { (c.y - 12.0 - size.y).max(8.0) } else { c.y + 18.0 };
            let (left, top) = (px(x), px(y));
            if node.left != left || node.top != top {
                node.left = left;
                node.top = top;
            }
            vis.set_if_neq(Visibility::Inherited);
        }
        _ => {
            vis.set_if_neq(Visibility::Hidden);
            current.clear();
        }
    }
}

#[derive(Resource)]
pub struct Fade(pub f32);

fn fade(time: Res<Time>, mut f: ResMut<Fade>, layer: Single<&mut BackgroundColor, With<FadeLayer>>, mut last: Local<Option<crate::Screen>>, state: Res<State<crate::Screen>>) {
    if *last != Some(state.get().clone()) {
        *last = Some(state.get().clone());
        f.0 = f.0.max(0.85);
    }
    f.0 = (f.0 - time.delta_secs() * 2.2).max(0.0);
    layer.into_inner().set_if_neq(BackgroundColor(Color::srgba(0.0, 0.0, 0.0, f.0 * f.0)));
}

/// Entrance motion: the node starts offset by `from` and eases into place after `delay` seconds.
#[derive(Component)]
pub struct SlideIn {
    delay: f32,
    from: Vec2,
    t: f32,
}

impl SlideIn {
    pub fn new(delay: f32, dx: f32, dy: f32) -> Self {
        SlideIn { delay, from: Vec2::new(dx, dy), t: 0.0 }
    }
}

fn slide_in(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut SlideIn, &mut UiTransform)>) {
    for (e, mut s, mut ut) in &mut q {
        s.t += time.delta_secs().min(1.0 / 30.0);
        let k = ((s.t - s.delay) / 0.5).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - k).powi(4);
        let off = s.from * (1.0 - ease);
        ut.translation = Val2::px(off.x, off.y);
        if k >= 1.0 {
            commands.entity(e).remove::<SlideIn>();
        }
    }
}

/// Continuous rotation (loading spinners).
#[derive(Component)]
pub struct Spin(pub f32);

fn spin(time: Res<Time>, mut q: Query<(&Spin, &mut UiTransform)>) {
    for (s, mut t) in &mut q {
        t.rotation = Rot2::radians(t.rotation.as_radians() + s.0 * time.delta_secs());
    }
}

pub fn spinner() -> impl Bundle {
    (Node { width: px(28), height: px(28), border: UiRect { top: px(3), right: px(3), ..default() }, border_radius: BorderRadius::MAX, ..default() }, BorderColor::all(ACCENT), UiTransform::default(), Spin(5.0))
}
