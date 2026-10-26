//! The lobby's vehicle carousel: your vehicles stand parked in a row that loops — the last one is
//! followed by a "new vehicle" slot, and that by the first again. The arrows beside the vehicle
//! (or ← →) slide to the previous or next one; the one in view is the vehicle you drive.

use crate::art::BlockArt;
use crate::flight::ground;
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
use blocktread_shared::analysis::analyze;
use blocktread_shared::blocks::Design;
use blocktread_shared::presets::preset;

pub struct CarouselPlugin;

impl Plugin for CarouselPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Carousel>()
            .init_resource::<Slide>()
            .add_systems(OnEnter(Screen::Menu), enter_carousel)
            .add_systems(Update, (carousel_actions, spawn_ships, slide, style_glyphs).chain().run_if(in_state(Screen::Menu)));
    }
}

/// The slot in view, counted without wrapping, so a step across the loop's seam is still a
/// one-slot slide. `slot` folds it onto the loop: ships 0..n, then the "new ship" slot n.
#[derive(Resource, Default)]
pub struct Carousel {
    turn: i32,
}

impl Carousel {
    pub fn slot(&self, ships: usize) -> usize {
        self.turn.rem_euclid(ships as i32 + 1) as usize
    }

    /// True when the "new ship" slot is in view.
    pub fn on_new(&self, ships: usize) -> bool {
        self.slot(ships) == ships
    }
}

/// The loop as it moves: where it is (unwrapped, fractional while sliding), and what was spawned.
#[derive(Resource, Default)]
struct Slide {
    pos: f32,
    /// Each ship's name and size, to notice when the fleet changes.
    built: Vec<(String, usize)>,
    /// Bounding radius per slot: the vehicles, then the "new vehicle" slot.
    radii: Vec<f32>,
    /// How high each vehicle stands to sit on its wheels.
    lift: Vec<f32>,
    /// Distance between neighbouring slots.
    gap: f32,
}

/// Offset `d` (in slots) folded onto the loop of `len` slots: the nearer way round.
fn wrap(d: f32, len: f32) -> f32 {
    (d + len * 0.5).rem_euclid(len) - len * 0.5
}

/// Size of the "new ship" slot (a blank ship's).
const NEW_RADIUS: f32 = 4.0;

#[derive(Component)]
struct CarouselShip(i32);

/// Where the vehicle in view sits on screen (share of the camera distance, up): above the name
/// plate, level with the arrows at `ROW` per cent down the window.
const RISE: f32 = 0.06;
const ROW: f32 = 44.0;
#[derive(Component)]
struct NewCard;
/// A bare glyph that works as a button: no surface. Dark against the daytime scene; under the
/// pointer it turns amber and moves a few pixels (an arrow toward where it goes, the plus's
/// corner marks inward) — no fill behind it, no bounce.
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
const ARROW: f32 = 112.0;
const PLUS: f32 = 152.0;
/// A mark at rest, and under the pointer.
const REST: Color = Color::srgba(0.11, 0.118, 0.09, 0.82);
const LIT: Color = ACCENT;

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

/// An arrow beside the ship: a solid chevron with flat-cut ends, nothing behind it.
fn spawn_arrow(commands: &mut Commands, parent: Entity, image: Handle<Image>, step: i8) -> Entity {
    let b = glyph_button(commands, parent, Node { width: px(ARROW), height: px(ARROW), ..default() }, UiAction::CarouselStep(step));
    commands.spawn((
        ImageNode { image, color: REST, ..default() },
        Node { width: percent(100), height: percent(100), ..default() },
        UiTransform::default(),
        Part::Mark { nudge: Vec2::new(step as f32 * 5.0, 0.0) },
        PartOf(b),
        Pickable::IGNORE,
        ChildOf(b),
    ));
    b
}

/// The "new ship" mark: a thin plus in four corner brackets, captioned.
fn spawn_plus(commands: &mut Commands, f: &UiFont, parent: Entity, image: Handle<Image>) -> Entity {
    let b = glyph_button(commands, parent, Node { flex_direction: FlexDirection::Column, align_items: AlignItems::Center, row_gap: px(10), ..default() }, UiAction::CarouselNew);
    let frame = commands.spawn((Node { width: px(PLUS), height: px(PLUS), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() }, Pickable::IGNORE, ChildOf(b))).id();
    commands.spawn((
        ImageNode { image, color: REST, ..default() },
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
                BackgroundColor(REST),
                Pickable::IGNORE,
                ChildOf(corner),
            ));
        }
    }
    commands.spawn((tech(f, "NEW VEHICLE", 13.0, REST), LetterSpacing::Px(4.0), Part::Caption, PartOf(b), Pickable::IGNORE, ChildOf(b)));
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
                    img.color = REST.mix(&LIT, h);
                }
                if let Some(mut tr) = tr {
                    tr.translation = Val2::px(nudge.x * h, nudge.y * h);
                }
            }
            Part::Corner(dir) => {
                if let Some(mut tr) = tr {
                    tr.translation = Val2::px(-dir.x * 8.0 * h, -dir.y * 8.0 * h);
                }
                let c = REST.mix(&LIT, h);
                for &bar in kids.into_iter().flatten() {
                    if let Ok(mut bg) = bars.get_mut(bar) {
                        bg.0 = c;
                    }
                }
            }
            Part::Caption => {
                if let Some(mut t) = txt {
                    t.0 = REST.mix(&LIT, h);
                }
            }
        }
    }
}

fn enter_carousel(mut commands: Commands, hangar: Res<Hangar>, mut car: ResMut<Carousel>, mut slide: ResMut<Slide>, icons: Res<Icons>, font: Res<UiFont>) {
    car.turn = hangar.fleet.current as i32;
    slide.pos = car.turn as f32;
    slide.built.clear();
    // the arrows, either side of the ship
    let row = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: percent(ROW), margin: UiRect::top(px(-ARROW / 2.0)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
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
        spawn_arrow(&mut commands, row, icon, step);
    }
    // the plus in the "new ship" slot, sliding along with the ships
    let holder = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: percent(ROW), margin: UiRect::top(px(-PLUS / 2.0)), justify_content: JustifyContent::Center, ..default() },
            Pickable::IGNORE,
            IgnoreHover,
            DespawnOnExit(Screen::Menu),
        ))
        .id();
    let plus = spawn_plus(&mut commands, &font, holder, icons.plus.clone());
    commands.entity(plus).insert((NewCard, Visibility::Hidden));
}

/// Spawn the loop: every ship of the fleet, again whenever the fleet changes.
fn spawn_ships(mut commands: Commands, hangar: Res<Hangar>, mut slide: ResMut<Slide>, old: Query<Entity, With<CarouselShip>>, mut art: ResMut<BlockArt>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let ships = &hangar.fleet.ships;
    let built: Vec<(String, usize)> = ships.iter().map(|d| (d.name.clone(), d.blocks.len())).collect();
    if built == slide.built && !old.is_empty() {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    let mut radii = Vec::with_capacity(ships.len() + 1);
    slide.lift.clear();
    for (i, d) in ships.iter().enumerate() {
        let a = analyze(d);
        radii.push(a.stats.radius);
        slide.lift.push(ground::park_height(d, a.stats.com));
        let root = commands.spawn((Transform::from_rotation(Quat::from_rotation_y(super::PARKED)), Visibility::Hidden, CarouselShip(i as i32), DespawnOnExit(Screen::Menu))).id();
        art.spawn_design(&mut commands, &mut mats, root, &d.blocks, -a.stats.com);
    }
    radii.push(NEW_RADIUS);
    // far enough apart that only the ship in view is on screen once the loop stops
    slide.gap = 3.0 * radii.iter().copied().fold(0.0, f32::max) + 16.0;
    slide.built = built;
    slide.radii = radii;
}

fn carousel_actions(mut msgs: MessageReader<UiAction>, mut car: ResMut<Carousel>, mut hangar: ResMut<Hangar>, mut next: ResMut<NextState<Screen>>, mut toasts: ResMut<Toasts>) {
    for m in msgs.read() {
        match m {
            UiAction::CarouselStep(d) => {
                car.turn += *d as i32;
                let n = hangar.fleet.ships.len();
                let s = car.slot(n);
                if s < n {
                    hangar.select(s as isize);
                }
            }
            UiAction::CarouselNew => {
                // after the last ship, where the "new ship" slot is
                hangar.add_ship(Design { name: String::new(), blocks: preset("blank").blocks });
                toasts.push(format!("新建了「{}」", hangar.design.name), ToastKind::Ok);
                next.set(Screen::Hangar);
            }
            _ => {}
        }
    }
}

/// Move the loop toward the slot in view, turn the ships, frame the camera on the one in front.
fn slide(
    time: Res<Time>,
    car: Res<Carousel>,
    mut slide: ResMut<Slide>,
    mut ships: Query<(&mut Transform, &mut Visibility, &CarouselShip), Without<MainCamera>>,
    cam: Single<(&mut Transform, &Camera), With<MainCamera>>,
    mut cards: Query<(&mut UiTransform, &mut Visibility), (With<NewCard>, Without<CarouselShip>)>,
) {
    if slide.radii.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    let t = time.elapsed_secs();
    let target = car.turn as f32;
    slide.pos += (target - slide.pos) * (1.0 - (-dt * 9.0).exp());
    if (target - slide.pos).abs() < 0.001 {
        slide.pos = target;
    }
    let pos = slide.pos;
    let len = slide.radii.len();
    for (mut tr, mut vis, s) in &mut ships {
        let d = wrap(s.0 as f32 - pos, len as f32);
        let want = if d.abs() < 1.05 { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        // parked in a row along x, on their wheels
        tr.translation = Vec3::new(d * slide.gap, slide.lift.get(s.0 as usize).copied().unwrap_or(0.0), 0.0);
    }
    // the camera frames the vehicle in front: closer for small ones
    let (mut ct, camera) = cam.into_inner();
    let k = pos.floor();
    let (a, b) = ((k as i32).rem_euclid(len as i32) as usize, (k as i32 + 1).rem_euclid(len as i32) as usize);
    let r = slide.radii[a] + (slide.radii[b] - slide.radii[a]) * (pos - k);
    *ct = super::parked_view(r, t, Vec2::new(0.0, RISE));
    // the plus moves with the ships, a screen's worth per slot
    let w = camera.logical_viewport_size().map_or(1440.0, |s| s.x);
    let d = wrap((len - 1) as f32 - pos, len as f32);
    for (mut ut, mut vis) in &mut cards {
        let want = if d.abs() < 1.0 { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        ut.translation = Val2::px(d * w * 0.8, 0.0);
    }
}
