//! The lobby's ship carousel: your ships stand in a row in 3D, with a "new ship" slot at each end.
//! The arrows beside the ship (or ← →) slide the row to the previous or next one; the one in view
//! is the ship you fly.

use crate::art::BlockArt;
use crate::hangar::Hangar;
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use crate::{MainCamera, Screen};
use bevy::prelude::*;
use voxwing_shared::analysis::analyze;
use voxwing_shared::blocks::Design;
use voxwing_shared::presets::preset;

pub struct CarouselPlugin;

impl Plugin for CarouselPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Carousel>()
            .init_resource::<Slide>()
            .add_systems(OnEnter(Screen::Menu), enter_carousel)
            .add_systems(Update, (carousel_actions, spawn_ships, slide).chain().run_if(in_state(Screen::Menu)));
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

fn enter_carousel(mut commands: Commands, hangar: Res<Hangar>, mut car: ResMut<Carousel>, mut slide: ResMut<Slide>, font: Res<UiFont>, icons: Res<Icons>) {
    car.slot = hangar.fleet.current as i32;
    slide.pos = car.slot as f32;
    slide.built.clear();
    let f = &*font;
    // the arrows, either side of the ship
    let row = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: percent(36), margin: UiRect::top(px(-28)), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            Pickable::IGNORE,
            IgnoreHover,
            // above the "new ship" cards sliding past
            GlobalZIndex(5),
            DespawnOnExit(Screen::Menu),
        ))
        .id();
    for (step, icon) in [(-1i8, icons.left.clone()), (1, icons.right.clone())] {
        if step == 1 {
            commands.spawn((Node { width: px(600), ..default() }, Pickable::IGNORE, ChildOf(row)));
        }
        let b = spawn_btn(&mut commands, f, row, Kind::Icon, "", UiAction::CarouselStep(step), None);
        commands.entity(b).insert((Arrow(step), Node { width: px(56), height: px(56), ..btn_node(Kind::Icon) }));
        commands.spawn((ImageNode { image: icon, color: INK, ..default() }, Node { width: px(26), height: px(26), ..default() }, Pickable::IGNORE, ChildOf(b)));
    }
    // a card in each "new ship" slot, sliding along with the row
    for slot in [-1, hangar.fleet.ships.len() as i32] {
        let holder = commands
            .spawn((
                Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: percent(36), margin: UiRect::top(px(-90)), justify_content: JustifyContent::Center, ..default() },
                Pickable::IGNORE,
                IgnoreHover,
                DespawnOnExit(Screen::Menu),
            ))
            .id();
        let card = spawn_btn(&mut commands, f, holder, Kind::Card, "新建机体", UiAction::NewShipAt(slot < 0), None);
        commands.entity(card).insert((NewCard(slot), UiTransform::default(), Visibility::Hidden, Node { width: px(280), align_items: AlignItems::Center, padding: px(20).all(), row_gap: px(8), ..btn_node(Kind::Card) }));
        // a drawn plus: the fonts' "+" sits off-centre at this size
        let plus = commands.spawn((Node { width: px(44), height: px(44), margin: UiRect::bottom(px(6)), ..default() }, Pickable::IGNORE)).id();
        for (w, h) in [(44.0, 4.0), (4.0, 44.0)] {
            commands.spawn((
                Node { position_type: PositionType::Absolute, left: px((44.0 - w) / 2.0), top: px((44.0 - h) / 2.0), width: px(w), height: px(h), ..default() },
                BackgroundColor(ACCENT),
                Pickable::IGNORE,
                ChildOf(plus),
            ));
        }
        // children so far: accent bar, label
        commands.entity(card).insert_children(1, &[plus]);
        commands.spawn((para(f, "从一个驾驶舱开始，进机库拼装", 13.0, MUTED), Pickable::IGNORE, ChildOf(card)));
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
    mut commands: Commands,
    time: Res<Time>,
    car: Res<Carousel>,
    mut slide: ResMut<Slide>,
    mut ships: Query<(&mut Transform, &mut Visibility, &CarouselShip), Without<MainCamera>>,
    cam: Single<(&mut Transform, &Camera), With<MainCamera>>,
    mut cards: Query<(&mut UiTransform, &mut Visibility, &NewCard), Without<CarouselShip>>,
    arrows: Query<(Entity, &Arrow)>,
    new_arrows: Query<(), Added<Arrow>>,
    mut ends: Local<(i32, i32)>,
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
    // the "new ship" cards move with the row, a screen's worth per slot
    let w = camera.logical_viewport_size().map_or(1440.0, |s| s.x);
    for (mut ut, mut vis, c) in &mut cards {
        let d = c.0 as f32 - pos;
        let want = if d.abs() < 1.0 { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
        ut.translation = Val2::px(d * w * 0.8, 0.0);
    }
    // an arrow is off at its end of the row
    let n = slide.radii.len() as i32 - 2;
    if *ends != (car.slot, n) || !new_arrows.is_empty() {
        *ends = (car.slot, n);
        for (e, a) in &arrows {
            set_enabled(&mut commands, e, if a.0 < 0 { car.slot > -1 } else { car.slot < n });
        }
    }
}
