//! The lobby's ship carousel: your ships stand in a row in 3D, with a "new ship" slot at each end.
//! The arrows beside the ship (or ← →) slide the row to the previous or next one; the one in view
//! is the ship you fly.

use crate::art::BlockArt;
use crate::hangar::Hangar;
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use crate::{MainCamera, Screen};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::LetterSpacing;
use bevy::ui::Pressed;
use bevy::ui_widgets::{Activate, Button, observe};
use voxwing_shared::analysis::analyze;
use voxwing_shared::blocks::Design;
use voxwing_shared::presets::preset;

pub struct CarouselPlugin;

impl Plugin for CarouselPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Carousel>()
            .init_resource::<Slide>()
            .add_systems(OnEnter(Screen::Menu), enter_carousel)
            .add_systems(Update, (carousel_actions, spawn_ships, slide, style_glyphs).chain().run_if(in_state(Screen::Menu)));
    }
}

/// The slot in view: -1 and the fleet's length are the "new ship" slots at the ends, the rest are
/// the fleet's ships.
#[derive(Resource, Default)]
pub struct Carousel {
    pub slot: i32,
}

impl Carousel {
    /// The "new ship" slot in view, if one is: true for the left end.
    pub fn new_slot(&self, ships: usize) -> Option<bool> {
        match self.slot {
            -1 => Some(true),
            s if s >= ships as i32 => Some(false),
            _ => None,
        }
    }
}

/// The row as it moves: where it is (a slot, fractional while sliding), and what was spawned.
#[derive(Resource, Default)]
struct Slide {
    pos: f32,
    /// Each ship's name and size, to notice when the fleet changes.
    built: Vec<(String, usize)>,
    /// Bounding radius per slot, both ends included (index = slot + 1).
    radii: Vec<f32>,
    /// Where each slot stands along the row (index = slot + 1).
    xs: Vec<f32>,
}

impl Slide {
    /// Linear in the slot position: the row's offset and the camera's subject size.
    fn at(v: &[f32], pos: f32) -> f32 {
        let i = (pos + 1.0).clamp(0.0, (v.len() - 1) as f32);
        let k = (i.floor() as usize).min(v.len() - 1);
        let next = v[(k + 1).min(v.len() - 1)];
        v[k] + (next - v[k]) * (i - k as f32)
    }
}

/// Size of the "new ship" slots (a blank ship's).
const NEW_RADIUS: f32 = 4.0;

#[derive(Component)]
struct CarouselShip(i32);
#[derive(Component)]
struct Arrow(i8);
#[derive(Component)]
struct NewCard(i32);
/// A bare glyph that works as a button: no surface. Under the pointer it brightens and moves a
/// few pixels (an arrow toward where it goes, the plus's corner marks inward) — no colour fill, no
/// bounce.
#[derive(Component, Default)]
struct Glyph {
    /// 0 at rest, 1 hovered; eased.
    hot: f32,
}

/// A piece of a glyph that reacts to it: the mark itself, a corner bracket, the caption.
#[derive(Component)]
enum Part {
    /// The drawn mark; `nudge` is the direction it shifts under the pointer.
    Mark { nudge: Vec2 },
    /// A corner bracket at (±1, ±1); it closes in under the pointer.
    Corner(Vec2),
    Caption,
}

#[derive(Component)]
struct PartOf(Entity);

/// Glyph sizes (px): the arrows beside the ship, the frame around the plus.
const ARROW: f32 = 128.0;
const PLUS: f32 = 152.0;
/// Rest and hover alpha of a mark.
const DIM: f32 = 0.42;
const LIT: f32 = 0.95;

fn glyph_button(commands: &mut Commands, parent: Entity, node: Node, action: UiAction) -> Entity {
    commands
        .spawn((
            Button,
            Hovered::default(),
            Glyph::default(),
            node,
            UiTransform::default(),
            observe(move |_: On<Activate>, mut w: MessageWriter<UiAction>| {
                w.write(action.clone());
            }),
            ChildOf(parent),
        ))
        .id()
}

/// An arrow beside the ship: a thin chevron, nothing behind it.
fn spawn_arrow(commands: &mut Commands, parent: Entity, image: Handle<Image>, step: i8) -> Entity {
    let b = glyph_button(commands, parent, Node { width: px(ARROW), height: px(ARROW), ..default() }, UiAction::CarouselStep(step));
    commands.spawn((
        ImageNode { image, color: INK.with_alpha(DIM), ..default() },
        Node { width: percent(100), height: percent(100), ..default() },
        UiTransform::default(),
        Part::Mark { nudge: Vec2::new(step as f32 * 6.0, 0.0) },
        PartOf(b),
        Pickable::IGNORE,
        ChildOf(b),
    ));
    b
}

/// The "new ship" mark: a thin plus in four corner brackets, captioned.
fn spawn_plus(commands: &mut Commands, f: &UiFont, parent: Entity, image: Handle<Image>, front: bool) -> Entity {
    let b = glyph_button(commands, parent, Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(10), ..default() }, UiAction::NewShipAt(front));
    let frame = commands.spawn((Node { width: px(PLUS), height: px(PLUS), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }, Pickable::IGNORE, ChildOf(b))).id();
    commands.spawn((
        ImageNode { image, color: INK.with_alpha(DIM), ..default() },
        Node { width: px(PLUS * 0.62), height: px(PLUS * 0.62), ..default() },
        Part::Mark { nudge: Vec2::ZERO },
        PartOf(b),
        Pickable::IGNORE,
        ChildOf(frame),
    ));
    // whole pixels: a 1.5 px line blurs on a 1× screen
    let (arm, w) = (18.0, 2.0);
    for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        let corner = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: if sx < 0.0 { px(0) } else { Val::Auto },
                    right: if sx > 0.0 { px(0) } else { Val::Auto },
                    top: if sy < 0.0 { px(0) } else { Val::Auto },
                    bottom: if sy > 0.0 { px(0) } else { Val::Auto },
                    width: px(arm),
                    height: px(arm),
                    ..default()
                },
                UiTransform::default(),
                Part::Corner(Vec2::new(sx, sy)),
                PartOf(b),
                Pickable::IGNORE,
                ChildOf(frame),
            ))
            .id();
        let edge = |at_start: bool| if at_start { px(0) } else { Val::Auto };
        for (bw, bh) in [(arm, w), (w, arm)] {
            commands.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: edge(sx < 0.0),
                    right: edge(sx > 0.0),
                    top: edge(sy < 0.0),
                    bottom: edge(sy > 0.0),
                    width: px(bw),
                    height: px(bh),
                    ..default()
                },
                BackgroundColor(INK.with_alpha(0.2)),
                Pickable::IGNORE,
                ChildOf(corner),
            ));
        }
    }
    commands.spawn((tech(f, "NEW SHIP", 11.0, MUTED), LetterSpacing::Px(4.0), Part::Caption, PartOf(b), Pickable::IGNORE, ChildOf(b)));
    b
}

/// Ease each glyph toward hovered or not, then pose its parts.
#[allow(clippy::type_complexity)]
fn style_glyphs(
    time: Res<Time>,
    dev: Res<crate::DevOpts>,
    mut glyphs: Query<(&Hovered, Has<Pressed>, &mut Glyph)>,
    mut parts: Query<(&Part, &PartOf, Option<&mut ImageNode>, Option<&mut UiTransform>, Option<&mut TextColor>, Option<&Children>)>,
    mut bars: Query<&mut BackgroundColor>,
) {
    let k = 1.0 - (-time.delta_secs() * 12.0).exp();
    // dev: show every glyph hovered (offscreen screenshots have no pointer)
    let all = dev.forced("hover", time.elapsed_secs());
    for (hover, pressed, mut g) in &mut glyphs {
        // pressed: lit but held back from the full nudge, so the click registers
        let want = if pressed { 0.6 } else if hover.get() || all { 1.0 } else { 0.0 };
        if (g.hot - want).abs() > 1e-3 {
            g.hot += (want - g.hot) * k;
        }
    }
    for (part, of, img, tr, txt, kids) in &mut parts {
        let Ok((_, _, g)) = glyphs.get(of.0) else { continue };
        let h = g.hot;
        match part {
            Part::Mark { nudge } => {
                if let Some(mut img) = img {
                    img.color = INK.with_alpha(DIM + (LIT - DIM) * h);
                }
                if let Some(mut tr) = tr {
                    tr.translation = Val2::px(nudge.x * h, nudge.y * h);
                }
            }
            Part::Corner(dir) => {
                if let Some(mut tr) = tr {
                    tr.translation = Val2::px(-dir.x * 8.0 * h, -dir.y * 8.0 * h);
                }
                let c = INK.with_alpha(0.2).mix(&ACCENT.with_alpha(0.85), h);
                for &bar in kids.into_iter().flatten() {
                    if let Ok(mut bg) = bars.get_mut(bar) {
                        bg.0 = c;
                    }
                }
            }
            Part::Caption => {
                if let Some(mut t) = txt {
                    t.0 = MUTED.mix(&INK, h);
                }
            }
        }
    }
}

fn enter_carousel(mut commands: Commands, hangar: Res<Hangar>, mut car: ResMut<Carousel>, mut slide: ResMut<Slide>, icons: Res<Icons>, font: Res<UiFont>) {
    car.slot = hangar.fleet.current as i32;
    slide.pos = car.slot as f32;
    slide.built.clear();
    // the arrows, either side of the ship
    let row = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: percent(36), margin: UiRect::top(px(-ARROW / 2.0)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            Pickable::IGNORE,
            IgnoreHover,
            // above the "new ship" plus sliding past
            GlobalZIndex(5),
            DespawnOnExit(Screen::Menu),
        ))
        .id();
    for (step, icon) in [(-1i8, icons.arrow_left.clone()), (1, icons.arrow_right.clone())] {
        if step == 1 {
            commands.spawn((Node { width: px(560), ..default() }, Pickable::IGNORE, ChildOf(row)));
        }
        let b = spawn_arrow(&mut commands, row, icon, step);
        commands.entity(b).insert(Arrow(step));
    }
    // a plus in each "new ship" slot, sliding along with the row
    for slot in [-1, hangar.fleet.ships.len() as i32] {
        let holder = commands
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: percent(36), margin: UiRect::top(px(-PLUS / 2.0)), justify_content: JustifyContent::Center, ..default() },
                Pickable::IGNORE,
                IgnoreHover,
                DespawnOnExit(Screen::Menu),
            ))
            .id();
        let plus = spawn_plus(&mut commands, &font, holder, icons.plus.clone(), slot < 0);
        commands.entity(plus).insert((NewCard(slot), Visibility::Hidden));
    }
}

/// Spawn the row: every ship of the fleet, again whenever the fleet changes.
fn spawn_ships(mut commands: Commands, hangar: Res<Hangar>, mut slide: ResMut<Slide>, old: Query<Entity, With<CarouselShip>>, mut art: ResMut<BlockArt>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let ships = &hangar.fleet.ships;
    let built: Vec<(String, usize)> = ships.iter().map(|d| (d.name.clone(), d.blocks.len())).collect();
    if built == slide.built && !old.is_empty() {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    let mut radii = vec![NEW_RADIUS];
    for (i, d) in ships.iter().enumerate() {
        let a = analyze(d);
        radii.push(a.stats.radius);
        let root = commands.spawn((Transform::from_xyz(0.0, 0.0, 0.0), Visibility::Hidden, CarouselShip(i as i32), DespawnOnExit(Screen::Menu))).id();
        for b in &d.blocks {
            art.spawn_block(&mut commands, &mut mats, root, b, b.pos().as_vec3() - a.stats.com, 3);
        }
    }
    radii.push(NEW_RADIUS);
    // far enough apart that only the ship in view is on screen once the row stops
    let mut xs = vec![0.0];
    for w in radii.windows(2) {
        let last = *xs.last().unwrap();
        xs.push(last + 3.0 * w[0].max(w[1]) + 16.0);
    }
    slide.built = built;
    slide.radii = radii;
    slide.xs = xs;
}

fn carousel_actions(mut msgs: MessageReader<UiAction>, mut car: ResMut<Carousel>, mut hangar: ResMut<Hangar>, mut next: ResMut<NextState<Screen>>, mut toasts: ResMut<Toasts>) {
    for m in msgs.read() {
        match m {
            UiAction::CarouselStep(d) => {
                let n = hangar.fleet.ships.len() as i32;
                let to = (car.slot + *d as i32).clamp(-1, n);
                if to != car.slot {
                    car.slot = to;
                    if (0..n).contains(&to) {
                        hangar.select(to as isize);
                    }
                }
            }
            UiAction::NewShipAt(front) => {
                let at = if *front { 0 } else { hangar.fleet.ships.len() };
                hangar.add_ship_at(at, Design { name: String::new(), blocks: preset("blank").blocks });
                toasts.push(format!("新建了「{}」", hangar.design.name), ToastKind::Ok);
                next.set(Screen::Hangar);
            }
            _ => {}
        }
    }
}

/// Move the row toward the slot in view, turn the ships, frame the camera on the one in front.
#[allow(clippy::too_many_arguments)]
fn slide(
    time: Res<Time>,
    car: Res<Carousel>,
    mut slide: ResMut<Slide>,
    mut ships: Query<(&mut Transform, &mut Visibility, &CarouselShip), Without<MainCamera>>,
    cam: Single<(&mut Transform, &Camera), With<MainCamera>>,
    mut cards: Query<(&mut UiTransform, &mut Visibility, &NewCard), (Without<CarouselShip>, Without<Arrow>)>,
    mut arrows: Query<(&mut Visibility, &Arrow), (Without<CarouselShip>, Without<NewCard>)>,
) {
    if slide.xs.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    let t = time.elapsed_secs();
    let target = car.slot as f32;
    slide.pos += (target - slide.pos) * (1.0 - (-dt * 9.0).exp());
    if (target - slide.pos).abs() < 0.001 {
        slide.pos = target;
    }
    let pos = slide.pos;
    let offset = Slide::at(&slide.xs, pos);
    for (mut tr, mut vis, s) in &mut ships {
        let near = (s.0 as f32 - pos).abs() < 1.05;
        let want = if near { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        tr.translation = Vec3::new(slide.xs[(s.0 + 1) as usize] - offset, (t * 0.8).sin() * 0.25, 0.0);
        tr.rotation = Quat::from_rotation_y(t * 0.22 + 2.4) * Quat::from_rotation_x(0.08 * (t * 0.5).sin());
    }
    // the camera frames the ship in front: closer for small ones, a little high above the ship bar
    let (mut ct, camera) = cam.into_inner();
    let r = Slide::at(&slide.radii, pos);
    let dist = r * 3.0 + 11.0;
    let aim = Vec3::new(0.0, -r * 0.3 - 1.0, 0.0);
    let sway = Vec3::new((t * 0.1).sin() * 2.0, 0.0, 0.0);
    *ct = Transform::from_translation(aim + Vec3::new(0.0, dist * 0.22, dist) + sway).looking_at(aim, Vec3::Y);
    // the "new ship" pluses move with the row, a screen's worth per slot
    let w = camera.logical_viewport_size().map_or(1440.0, |s| s.x);
    for (mut ut, mut vis, c) in &mut cards {
        let d = c.0 as f32 - pos;
        let want = if d.abs() < 1.0 { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        ut.translation = Val2::px(d * w * 0.8, 0.0);
    }
    // no arrow past an end of the row
    let n = slide.radii.len() as i32 - 2;
    for (mut vis, a) in &mut arrows {
        let on = if a.0 < 0 { car.slot > -1 } else { car.slot < n };
        let want = if on { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
}
