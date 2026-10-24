//! The hangar: build a ship on a 3D grid with live flight-readiness checks.

use crate::art::{BlockArt, srgb};
use crate::audio::{Sfx, Sounds};
use crate::screens::Overlays;
use crate::settings;
use crate::thumbs::PartThumbs;
use crate::ui::icons::Icons;
use crate::ui::kit::*;
use crate::ui::*;
use crate::{MainCamera, Screen};
use bevy::asset::RenderAssetUsages;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::input_focus::InputFocus;
use bevy::picking::hover::{HoverMap, Hovered};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::EditableText;
use bevy::ui_widgets::{Activate, Button, observe};
use bevy::window::PrimaryWindow;
use voxwing_shared::analysis::{Analysis, Level, MAX_BLOCKS, analyze, place_reason};
use voxwing_shared::blocks::{Block, BlockType, Category, Design};
use voxwing_shared::glam::IVec3;
use voxwing_shared::grid::{GMAX, GMIN, in_grid, raycast};
use voxwing_shared::presets::preset;

pub struct HangarPlugin;

impl Plugin for HangarPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Hangar::load())
            .init_resource::<OrbitCam>()
            .add_systems(OnEnter(Screen::Hangar), enter_hangar)
            .add_systems(OnExit(Screen::Hangar), leave_hangar)
            .add_systems(Update, (orbit_and_pick, edit_keys, hangar_actions, sync_ship_name, rebuild_model, refresh_ui, draw_gizmos).chain().run_if(in_state(Screen::Hangar)))
            .add_systems(Update, keep_analysis);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Place,
    Erase,
    Paint,
}

const SWATCHES: [u32; 12] = [0xe8ecf0, 0xc9d2dc, 0x8d9aa8, 0x4d5662, 0x2a2f38, 0xffae42, 0xff6a3d, 0xe8444f, 0x5fd4ff, 0x3b7bff, 0x57e39a, 0xb07cff];
const KEYS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "="];

#[derive(Resource)]
pub struct Hangar {
    /// All your ships; `design` is the selected one being edited, written back on every change.
    pub fleet: settings::Fleet,
    pub design: Design,
    tool: Tool,
    ty: BlockType,
    color: u32,
    mirror: bool,
    layer: i32,
    undo: Vec<Vec<Block>>,
    redo: Vec<Vec<Block>>,
    pub analysis: Analysis,
    dirty: bool,
    ui_dirty: bool,
    pick: Option<(Option<usize>, IVec3)>,
    drag: Option<(Vec2, MouseButton, bool)>,
    focus: Vec<usize>,
    pending_toasts: Vec<(String, ToastKind)>,
}

impl Hangar {
    fn load() -> Self {
        let fleet = settings::load_fleet();
        let design = fleet.ships[fleet.current].clone();
        let analysis = analyze(&design);
        Hangar {
            fleet,
            design,
            tool: Tool::Place,
            ty: BlockType::Hull,
            color: 0xc9d2dc,
            mirror: true,
            layer: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            analysis,
            dirty: true,
            ui_dirty: true,
            pick: None,
            drag: None,
            focus: Vec::new(),
            pending_toasts: Vec::new(),
        }
    }

    fn toast(&mut self, s: impl Into<String>, k: ToastKind) {
        self.pending_toasts.push((s.into(), k));
    }

    fn push_undo(&mut self) {
        self.undo.push(self.design.blocks.clone());
        if self.undo.len() > 120 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn changed(&mut self) {
        self.analysis = analyze(&self.design);
        self.dirty = true;
        self.ui_dirty = true;
        self.fleet.ships[self.fleet.current] = self.design.clone();
        settings::save_fleet(&self.fleet);
    }

    /// Switch to ship `i` (wrapping around). Undo history belongs to the ship left behind.
    pub fn select(&mut self, i: isize) {
        let n = self.fleet.ships.len() as isize;
        self.fleet.current = i.rem_euclid(n) as usize;
        self.design = self.fleet.ships[self.fleet.current].clone();
        self.undo.clear();
        self.redo.clear();
        self.changed();
    }

    pub fn step(&mut self, by: isize) {
        self.select(self.fleet.current as isize + by);
    }

    /// Add a ship (a blank one: just a cockpit) after the last and switch to it.
    pub fn add_ship(&mut self, mut d: Design) {
        if d.name.trim().is_empty() || self.fleet.ships.iter().any(|s| s.name == d.name) {
            let base = if d.name.trim().is_empty() { "新机体".to_string() } else { d.name.clone() };
            d.name = (1..).map(|k| format!("{base} {k}")).find(|n| !self.fleet.ships.iter().any(|s| &s.name == n)).unwrap();
        }
        self.fleet.ships.push(d);
        self.select(self.fleet.ships.len() as isize - 1);
    }

    pub fn delete_current(&mut self) {
        self.fleet.ships.remove(self.fleet.current);
        if self.fleet.ships.is_empty() {
            self.fleet.ships.push(Design { name: "新机体".into(), blocks: preset("blank").blocks });
        }
        self.select(self.fleet.current.min(self.fleet.ships.len() - 1) as isize);
    }

    /// Rename the selected ship (blank names are ignored).
    fn rename(&mut self, name: &str) {
        let n: String = name.trim().chars().take(14).collect();
        if !n.is_empty() && n != self.design.name {
            self.design.name = n;
            self.fleet.ships[self.fleet.current] = self.design.clone();
            settings::save_fleet(&self.fleet);
        }
    }

    fn mirror_of(&self, c: IVec3) -> Option<IVec3> {
        (self.mirror && c.x != 0).then_some(IVec3::new(-c.x, c.y, c.z))
    }

    fn at(&self, c: IVec3) -> Option<usize> {
        self.design.blocks.iter().position(|b| b.pos() == c)
    }

    fn place(&mut self, c: IVec3) -> Result<(), String> {
        if let Some(why) = place_reason(&self.design.blocks, c, self.ty) {
            return Err(why);
        }
        if self.ty == BlockType::Cockpit && self.design.blocks.iter().any(|b| b.ty == BlockType::Cockpit) {
            return Err("只能有一个驾驶舱。先擦掉旧的再放".into());
        }
        let mut cells = vec![c];
        if self.ty != BlockType::Cockpit
            && let Some(m) = self.mirror_of(c)
        {
            match place_reason(&self.design.blocks, m, self.ty) {
                None => cells.push(m),
                Some(why) => self.toast(format!("镜像那一侧放不下：{why}"), ToastKind::Warn),
            }
        }
        if self.design.blocks.len() + cells.len() > MAX_BLOCKS {
            return Err(format!("最多 {MAX_BLOCKS} 个方块"));
        }
        self.push_undo();
        let color = if self.ty.paintable() { Some(self.color) } else { None };
        for p in cells {
            self.design.blocks.push(Block::new(p.x, p.y, p.z, self.ty, color));
        }
        self.changed();
        Ok(())
    }

    fn erase(&mut self, i: usize) {
        self.push_undo();
        let b = self.design.blocks[i];
        let m = self.mirror_of(b.pos()).and_then(|m| self.at(m)).filter(|&j| self.design.blocks[j].ty == b.ty);
        let mut gone = vec![i];
        gone.extend(m);
        gone.sort_unstable();
        gone.dedup();
        for j in gone.into_iter().rev() {
            self.design.blocks.remove(j);
        }
        self.changed();
    }

    fn paint(&mut self, i: usize) {
        self.push_undo();
        let p = self.design.blocks[i].pos();
        self.design.blocks[i].color = self.color;
        if let Some(j) = self.mirror_of(p).and_then(|m| self.at(m)) {
            self.design.blocks[j].color = self.color;
        }
        self.changed();
    }

    /// Move every block by `d`, unless that would push one off the build grid. The build layer
    /// moves with the ship, so you keep building where you were.
    fn nudge(&mut self, d: IVec3) -> Result<(), String> {
        if self.design.blocks.iter().any(|b| !in_grid(b.pos() + d)) {
            return Err("再移就超出搭建范围了".into());
        }
        self.push_undo();
        for b in &mut self.design.blocks {
            let p = b.pos() + d;
            (b.x, b.y, b.z) = (p.x as i8, p.y as i8, p.z as i8);
        }
        self.layer = (self.layer + d.y).clamp(GMIN.y, GMAX.y);
        self.changed();
        Ok(())
    }

    /// Replace the design (keeps an undo step).
    pub fn load_design(&mut self, d: Design) {
        self.push_undo();
        self.design = d;
        self.changed();
    }
}

/// Analysis must stay current even when the design changes outside the hangar (design picker).
fn keep_analysis(h: Res<Hangar>) {
    let _ = h;
}

#[derive(Resource)]
struct OrbitCam {
    yaw: f32,
    pitch: f32,
    dist: f32,
    target: Vec3,
}

impl Default for OrbitCam {
    fn default() -> Self {
        OrbitCam { yaw: 0.66, pitch: 0.46, dist: 34.0, target: Vec3::new(0.0, -1.0, 0.0) }
    }
}

#[derive(Component)]
struct HangarShip;
#[derive(Component)]
struct Ghost;
#[derive(Component)]
struct CheckList;
#[derive(Component)]
struct CheckSummary;
#[derive(Component)]
struct LayerText;
#[derive(Component)]
struct MetaText;
#[derive(Component)]
struct NameField;
#[derive(Component)]
struct CheckRow(Vec<usize>);
#[derive(Component)]
struct Swatch(u32);
#[derive(Resource)]
struct StatBars(Vec<(Entity, Entity)>);

fn floor_texture(images: &mut Assets<Image>) -> Handle<Image> {
    let n = 1024usize;
    let world = 52.0f32;
    let k = n as f32 / world;
    let w = |v: f32| ((v + world / 2.0) * k) as i32;
    let mut px = vec![0u8; n * n * 4];
    let (fx0, fx1, fz0, fz1) = (w(GMIN.x as f32 - 0.5), w(GMAX.x as f32 + 0.5), w(GMIN.z as f32 - 0.5), w(GMAX.z as f32 + 0.5));
    for y in 0..n as i32 {
        for x in 0..n as i32 {
            let mut c = [9u8, 14, 22];
            let gx = ((x as f32 / k - world / 2.0 + 0.5).rem_euclid(1.0) * k) as i32;
            let gy = ((y as f32 / k - world / 2.0 + 0.5).rem_euclid(1.0) * k) as i32;
            if gx == 0 || gy == 0 {
                c = [16, 30, 40];
            }
            let inner = x >= fx0 && x <= fx1 && y >= fz0 && y <= fz1;
            let on_edge = inner && (x - fx0 < 3 || fx1 - x < 3 || y - fz0 < 3 || fz1 - y < 3);
            if on_edge {
                c = [40, 150, 140];
            }
            // corner brackets of the build pad
            let near = |a: i32, b: i32| (a - b).abs() < 26;
            if !inner && (near(x, fx0) || near(x, fx1)) && (near(y, fz0) || near(y, fz1)) && (x - fx0).abs().min((x - fx1).abs()) < 4 + 22 && ((x - fx0).abs() < 4 || (x - fx1).abs() < 4 || (y - fz0).abs() < 4 || (y - fz1).abs() < 4) {
                c = [62, 230, 208];
            }
            if (x - w(0.0)).abs() <= 1 && inner && (y / 10) % 2 == 0 {
                c = [30, 90, 90];
            }
            // nose-direction chevron in front of the pad
            let ax = (x - w(0.0)) as f32 / k;
            let az = (y - w(GMIN.z as f32 - 3.5)) as f32 / k;
            let chev = |o: f32| az > -1.2 + o && az < -0.5 + o && (ax.abs() - (az - o + 1.2) * 0.9).abs() < 0.35;
            if chev(0.0) || chev(1.0) {
                c = [62, 230, 208];
            }
            let i = (y as usize * n + x as usize) * 4;
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    images.add(Image::new(Extent3d { width: n as u32, height: n as u32, depth_or_array_layers: 1 }, TextureDimension::D2, px, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD))
}

#[allow(clippy::too_many_arguments)]
fn enter_hangar(
    mut commands: Commands,
    font: Res<UiFont>,
    mut hangar: ResMut<Hangar>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    cam: Single<(Entity, &mut Projection), With<MainCamera>>,
    thumbs: Res<PartThumbs>,
    icons: Res<Icons>,
) {
    let (ce, mut proj) = cam.into_inner();
    commands.entity(ce).remove::<bevy::light::Skybox>();
    if let Projection::Perspective(p) = &mut *proj {
        p.fov = 42f32.to_radians();
    }
    commands.insert_resource(GlobalAmbientLight { color: Color::srgb(0.75, 0.84, 0.92), brightness: 380.0, ..default() });
    let scope = DespawnOnExit(Screen::Hangar);
    commands.spawn((DirectionalLight { illuminance: 9000.0, ..default() }, Transform::from_xyz(10.0, 20.0, 12.0).looking_at(Vec3::ZERO, Vec3::Y), scope.clone()));
    commands.spawn((DirectionalLight { illuminance: 2600.0, color: srgb(0x3ee6d0), ..default() }, Transform::from_xyz(-14.0, 6.0, -16.0).looking_at(Vec3::ZERO, Vec3::Y), scope.clone()));
    let tex = floor_texture(&mut images);
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(52.0, 52.0))),
        MeshMaterial3d(mats.add(StandardMaterial { base_color_texture: Some(tex), metallic: 0.4, perceptual_roughness: 0.75, ..default() })),
        Transform::from_xyz(0.0, GMIN.y as f32 - 0.52, 0.0),
        scope.clone(),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.02, 1.02, 1.02))),
        MeshMaterial3d(mats.add(StandardMaterial { base_color: ACCENT.with_alpha(0.25), alpha_mode: AlphaMode::Blend, unlit: true, ..default() })),
        Transform::default(),
        Visibility::Hidden,
        Ghost,
        scope.clone(),
    ));
    spawn_ui(&mut commands, &font, &hangar, &thumbs, &icons);
    hangar.dirty = true;
    hangar.ui_dirty = true;
}

fn leave_hangar(fields: Query<&EditableText, With<NameField>>, mut hangar: ResMut<Hangar>) {
    if let Ok(e) = fields.single() {
        hangar.rename(&e.value().to_string());
    }
}

/// The name field renames the ship as you type; switching ships puts the new name in it.
fn sync_ship_name(mut fields: Query<&mut EditableText, With<NameField>>, mut hangar: ResMut<Hangar>, mut shown: Local<Option<usize>>, mut counter: Query<&mut Text, With<ShipCounter>>) {
    let Ok(mut e) = fields.single_mut() else {
        *shown = None;
        return;
    };
    let at = hangar.fleet.current;
    if *shown != Some(at) {
        *shown = Some(at);
        *e = EditableText { visible_width: e.visible_width, allow_newlines: false, ..EditableText::new(&hangar.design.name) };
    } else {
        let typed = e.value().to_string();
        if typed.trim() != hangar.design.name {
            hangar.rename(&typed);
        }
    }
    let label = format!("{} / {}", at + 1, hangar.fleet.ships.len());
    for mut t in &mut counter {
        if t.0 != label {
            t.0 = label.clone();
        }
    }
}

#[derive(Component)]
struct ShipCounter;

fn spawn_ui(commands: &mut Commands, font: &UiFont, hangar: &Hangar, thumbs: &PartThumbs, icons: &Icons) {
    let f = font;
    let scope = DespawnOnExit(Screen::Hangar);
    let root = commands.spawn((Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, ..default() }, Pickable::IGNORE, IgnoreHover, scope)).id();

    // top bar: one row of H_MD controls, groups split by hairline dividers
    let bar_h = H_MD + 24.0;
    let top = commands
        .spawn((
            Node { position_type: PositionType::Absolute, left: px(0), right: px(0), top: px(0), height: px(bar_h), padding: UiRect::horizontal(px(EDGE)), column_gap: px(GAP_S), align_items: AlignItems::Center, border: UiRect::bottom(px(1)), ..default() },
            BackgroundColor(PANEL_SOLID.with_alpha(0.94)),
            BorderColor::all(LINE),
            ChildOf(root),
        ))
        .id();
    let divider = |commands: &mut Commands| {
        commands.spawn((Node { width: px(1), height: px(20), margin: UiRect::horizontal(px(GAP_S)), ..default() }, BackgroundColor(LINE_HI), ChildOf(top)));
    };
    // a dozen controls share the row: tighter buttons than elsewhere, and the name field gives
    // way first, so the bar still fits a 1280px window
    let bar_btn = |commands: &mut Commands, kind: Kind, label: &str, action: UiAction, hint: Option<&str>| {
        let b = spawn_btn(commands, f, top, kind, label, action, hint);
        commands.entity(b).insert(Node { padding: UiRect::horizontal(px(12)), ..btn_node(kind) });
        b
    };
    spawn_btn(commands, f, top, Kind::Ghost, "← 大厅", UiAction::Goto(Screen::Menu), Some("Esc"));
    spawn_title(commands, f, top, "机库", "HANGAR");
    divider(commands);
    // the ship carousel: ‹ name › 2 / 5, then a new ship and delete
    let prev = spawn_icon_btn(commands, f, top, icons.left.clone(), UiAction::ShipPrev);
    commands.entity(prev).insert(Tip("上一架  ,".into()));
    let nf = spawn_field(commands, f, top, &hangar.design.name, 9.0);
    commands.entity(nf).insert((NameField, Node { padding: UiRect::axes(px(12), px(9)), width: px(150), min_width: px(96), flex_shrink: 1.0, ..default() }));
    let next = spawn_icon_btn(commands, f, top, icons.right.clone(), UiAction::ShipNext);
    commands.entity(next).insert(Tip("下一架  .".into()));
    commands.spawn((tech(f, "", 12.0, MUTED), ShipCounter, Node { width: px(40), justify_content: JustifyContent::Center, ..default() }, TextLayout::justify(Justify::Center), ChildOf(top)));
    let new = bar_btn(commands, Kind::Normal, "+ 新建", UiAction::NewShip, None);
    commands.entity(new).insert(Tip("新建一架空白机体（只有驾驶舱）".into()));
    let del = spawn_btn(commands, f, top, Kind::Icon, "×", UiAction::DeleteShip, None);
    commands.entity(del).insert(Tip("删除这架机体".into()));
    divider(commands);
    let share = bar_btn(commands, Kind::Normal, "复制分享码", UiAction::CopyShare, None);
    commands.entity(share).insert(Tip("复制一段 VW1. 开头的文字，朋友粘贴就能得到同一架飞机（网页版也能用）".into()));
    bar_btn(commands, Kind::Normal, "粘贴分享码", UiAction::PasteShare, None);
    commands.spawn((Node { flex_grow: 1.0, ..default() }, ChildOf(top)));
    let u = bar_btn(commands, Kind::Normal, "撤销", UiAction::Undo, None);
    commands.entity(u).insert(Tip("撤销  Ctrl/⌘+Z".into()));
    let r = bar_btn(commands, Kind::Normal, "重做", UiAction::Redo, None);
    commands.entity(r).insert(Tip("重做  Ctrl/⌘+Shift+Z".into()));
    let m = bar_btn(commands, Kind::Normal, "镜像", UiAction::Mirror, Some("M"));
    commands.entity(m).insert(Tip("开启后左右对称同时放置、擦除和喷漆".into()));
    bar_btn(commands, Kind::Danger, "清空", UiAction::Clear, None);
    divider(commands);
    bar_btn(commands, Kind::Accent, "完成", UiAction::Goto(Screen::Menu), None);

    let panel_top = bar_h + GAP;
    let (left_w, right_w) = (296.0, 344.0);

    // left panel: tools, parts, paint, layer
    let left = spawn_panel(commands, Some(root), Node { position_type: PositionType::Absolute, left: px(EDGE), top: px(panel_top), bottom: px(EDGE), width: px(left_w), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), overflow: Overflow::scroll_y(), ..default() });
    commands.entity(left).insert(SlideIn::new(0.05, -36.0, 0.0));
    let tools = commands.spawn((row(0.0), ChildOf(left))).id();
    for (l, t) in [("放置", 0u8), ("擦除", 1), ("喷漆", 2)] {
        let b = spawn_btn(commands, f, tools, Kind::Segment, l, UiAction::Tool(t), Some(["B", "X", "G"][t as usize]));
        commands.entity(b).insert(Node { height: px(H_MD), flex_grow: 1.0, flex_basis: px(0), padding: UiRect::horizontal(px(10)), column_gap: px(6), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() });
    }
    let four_cols = || Node { display: Display::Grid, grid_template_columns: RepeatedGridTrack::flex(4, 1.0), column_gap: px(GAP_S), row_gap: px(GAP_S), ..default() };
    for cat in Category::ALL {
        let sec = spawn_section(commands, f, left, cat.label());
        let grid = commands.spawn((four_cols(), ChildOf(sec))).id();
        for (i, t) in BlockType::ALL.iter().enumerate() {
            if t.info().cat != cat {
                continue;
            }
            let tile = spawn_part_tile(commands, f, grid, thumbs.0[t.index()].clone(), KEYS.get(i).copied().unwrap_or(""), UiAction::SelectType(*t));
            commands.entity(tile).insert((Tip(t.info().name.to_string()), TipDetail(part_detail(*t))));
        }
    }
    commands.spawn((text(f, "悬停看名称 · 按住 Shift 看详情", 12.0, MUTED), ChildOf(left)));
    let paint = spawn_section(commands, f, left, "涂装|PAINT");
    let sw = commands.spawn((Node { display: Display::Grid, grid_template_columns: RepeatedGridTrack::flex(6, 1.0), column_gap: px(GAP_S), row_gap: px(GAP_S), ..default() }, ChildOf(paint))).id();
    for c in SWATCHES {
        commands.spawn((
            Button,
            Hovered::default(),
            Node { height: px(24), border: px(2).all(), ..default() },
            BackgroundColor(srgb(c)),
            BorderColor::all(LINE),
            Swatch(c),
            observe(move |_: On<Activate>, mut w: MessageWriter<UiAction>| {
                w.write(UiAction::Color(c));
            }),
            ChildOf(sw),
        ));
    }
    let layer = spawn_section(commands, f, left, "搭建层|LAYER");
    let lrow = commands.spawn((row(GAP_S), ChildOf(layer))).id();
    let dn = spawn_btn(commands, f, lrow, Kind::Icon, "−", UiAction::Layer(-1), None);
    commands.entity(dn).insert(Tip("下移一层  [".into()));
    commands.spawn((tech(f, "Y 0", 16.0, INK), LayerText, Node { width: px(44), justify_content: JustifyContent::Center, ..default() }, TextLayout::justify(Justify::Center), ChildOf(lrow)));
    let up = spawn_btn(commands, f, lrow, Kind::Icon, "+", UiAction::Layer(1), None);
    commands.entity(up).insert(Tip("上移一层  ]".into()));
    commands.spawn((text(f, "空处放在这一层", 12.0, MUTED), Node { margin: UiRect::left(px(4)), ..default() }, ChildOf(lrow)));
    let mv = spawn_section(commands, f, left, "整机平移|MOVE SHIP");
    let mrow = commands.spawn((row(GAP_S), ChildOf(mv))).id();
    for (label, d, tip) in [
        ("←", (-1, 0, 0), "整机左移  Shift+←"),
        ("→", (1, 0, 0), "整机右移  Shift+→"),
        ("↑", (0, 1, 0), "整机上移  Shift+↑"),
        ("↓", (0, -1, 0), "整机下移  Shift+↓"),
        ("前", (0, 0, -1), "整机前移  Shift+["),
        ("后", (0, 0, 1), "整机后移  Shift+]"),
    ] {
        let b = spawn_btn(commands, f, mrow, Kind::Icon, label, UiAction::Nudge(d.0, d.1, d.2), None);
        commands.entity(b).insert(Tip(tip.into()));
    }

    // right panel: preflight checks and performance
    let right = spawn_panel(commands, Some(root), Node { position_type: PositionType::Absolute, right: px(EDGE), top: px(panel_top), bottom: px(EDGE), width: px(right_w), padding: px(PAD).all(), flex_direction: FlexDirection::Column, row_gap: px(GAP), overflow: Overflow::scroll_y(), ..default() });
    commands.entity(right).insert(SlideIn::new(0.1, 36.0, 0.0));
    let checks = commands.spawn((col(GAP_S), ChildOf(right))).id();
    let hdr = commands.spawn((Node { height: px(22), column_gap: px(12), align_items: AlignItems::Center, ..default() }, ChildOf(checks))).id();
    let h = spawn_header(commands, f, hdr, "起飞检查|PREFLIGHT");
    commands.entity(h).insert(Node { flex_grow: 1.0, height: px(16), column_gap: px(8), align_items: AlignItems::Center, ..default() });
    commands.spawn((Node::default(), CheckSummary, ChildOf(hdr)));
    commands.spawn((col(GAP_S), CheckList, ChildOf(checks)));
    let perf = spawn_section(commands, f, right, "性能预估|PERFORMANCE");
    let mut bars = Vec::new();
    for (l, c) in [("造价", INK.with_alpha(0.7)), ("速度", ACCENT), ("俯仰", ACCENT), ("偏航", ACCENT), ("翻滚", ACCENT), ("火力", SIGNAL), ("持续", SIGNAL), ("耐久", OK), ("护盾", Color::srgb(0.4, 0.62, 1.0)), ("能量", WARN)] {
        bars.push(spawn_stat(commands, f, perf, l, c));
    }
    commands.insert_resource(StatBars(bars));
    commands.spawn((para(f, "", 12.0, MUTED), MetaText, ChildOf(perf)));

    // mouse hints, centred in the gap between the panels
    let hints = commands
        .spawn((
            Node { position_type: PositionType::Absolute, bottom: px(EDGE), left: px(EDGE + left_w + GAP), right: px(EDGE + right_w + GAP), column_gap: px(GAP), row_gap: px(GAP_S), justify_content: JustifyContent::Center, flex_wrap: FlexWrap::Wrap, ..default() },
            IgnoreHover,
            ChildOf(root),
        ))
        .id();
    for (k, v) in [("左键", "放置"), ("右键", "删除"), ("左键拖动", "旋转"), ("右键拖动", "平移"), ("滚轮", "缩放"), ("Alt+点击", "吸取"), ("Shift+方向键", "平移整机")] {
        commands.spawn((row(6.0), ChildOf(hints), children![(Node { padding: UiRect::axes(px(6), px(1)), border: px(1).all(), ..default() }, BorderColor::all(LINE_HI), children![text(f, k, 11.0, MUTED)]), text(f, v, 12.0, INK.with_alpha(0.7))]));
    }
}

#[allow(clippy::too_many_arguments)]
fn orbit_and_pick(
    mut hangar: ResMut<Hangar>,
    mut orbit: ResMut<OrbitCam>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    hover: Res<HoverMap>,
    nodes: Query<(), (With<Node>, Without<IgnoreHover>)>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut cam: Single<(&Camera, &GlobalTransform, &mut Transform), With<MainCamera>>,
    mut ghost: Single<(&mut Transform, &mut Visibility), (With<Ghost>, Without<MainCamera>)>,
    mut sounds: ResMut<Sounds>,
    mut commands: Commands,
    time: Res<Time>,
    overlays: Res<Overlays>,
    modal: Res<ModalReq>,
) {
    let over_ui = pointer_over_ui(&hover, &nodes) || overlays.any(&modal);
    let cursor = window.cursor_position();
    for b in [MouseButton::Left, MouseButton::Right] {
        if buttons.just_pressed(b) && !over_ui
            && let Some(c) = cursor
        {
            hangar.drag = Some((c, b, false));
        }
    }
    if let Some((start, b, moved)) = hangar.drag {
        let d = motion.delta;
        let moved = moved || cursor.is_some_and(|c| c.distance(start) > 5.0);
        hangar.drag = Some((start, b, moved));
        if moved && buttons.pressed(b) {
            if b == MouseButton::Left {
                orbit.yaw -= d.x * 0.008;
                orbit.pitch = (orbit.pitch + d.y * 0.008).clamp(-0.25, 1.45);
            } else {
                let (_, _, ref t) = *cam;
                let k = orbit.dist * 0.0016;
                orbit.target += (-t.right().as_vec3() * d.x + t.up().as_vec3() * d.y) * k;
                orbit.target = orbit.target.clamp(Vec3::new(-12.0, -8.0, -14.0), Vec3::new(12.0, 8.0, 14.0));
            }
        }
    }
    if !over_ui && scroll.delta.y != 0.0 {
        orbit.dist = (orbit.dist * (1.0 - scroll.delta.y.clamp(-3.0, 3.0) * 0.08)).clamp(8.0, 80.0);
    }
    let (cp, sp) = (orbit.pitch.cos(), orbit.pitch.sin());
    let eye = orbit.target + Vec3::new(cp * orbit.yaw.sin(), sp, cp * orbit.yaw.cos()) * orbit.dist;
    *cam.2 = Transform::from_translation(eye).looking_at(orbit.target, Vec3::Y);

    hangar.pick = None;
    if let (Some(c), false) = (cursor, over_ui)
        && hangar.drag.is_none_or(|(_, _, m)| !m)
        && let Ok(ray) = cam.0.viewport_to_world(cam.1, c)
    {
        let o = ray.origin;
        let d = ray.direction.as_vec3();
        let blocks = &hangar.design.blocks;
        let hit = raycast(o, d, 400.0, |p| blocks.iter().position(|b| b.pos() == p));
        hangar.pick = match hit {
            Some(h) => Some((Some(h.val), h.cell + h.normal)),
            None => {
                let py = hangar.layer as f32 - 0.5;
                if d.y.abs() > 1e-5 {
                    let t = (py - o.y) / d.y;
                    let p = o + d * t;
                    let cell = IVec3::new(p.x.round() as i32, hangar.layer, p.z.round() as i32);
                    (t > 0.0 && in_grid(cell)).then_some((None, cell))
                } else {
                    None
                }
            }
        };
    }
    let (ref mut gt, ref mut gv) = *ghost;
    **gv = Visibility::Hidden;
    if let Some((_, cell)) = hangar.pick
        && hangar.tool == Tool::Place
        && in_grid(cell)
    {
        gt.translation = cell.as_vec3();
        **gv = Visibility::Visible;
    }

    for b in [MouseButton::Left, MouseButton::Right] {
        if !buttons.just_released(b) {
            continue;
        }
        let click = matches!(hangar.drag, Some((_, db, false)) if db == b);
        hangar.drag = None;
        if !click {
            continue;
        }
        let Some((blk, cell)) = hangar.pick else { continue };
        let now = time.elapsed_secs_f64();
        if keys.any_pressed([KeyCode::AltLeft, KeyCode::AltRight])
            && let Some(i) = blk
        {
            let bl = hangar.design.blocks[i];
            hangar.ty = bl.ty;
            hangar.tool = Tool::Place;
            if bl.ty.paintable() {
                hangar.color = bl.color;
            }
            hangar.ui_dirty = true;
            continue;
        }
        let tool = if b == MouseButton::Right { Tool::Erase } else { hangar.tool };
        match (tool, blk) {
            (Tool::Place, _) => match hangar.place(cell) {
                Ok(()) => sounds.play(&mut commands, now, Sfx::Place, 0.5, 0.0),
                Err(e) => {
                    if in_grid(cell) {
                        hangar.toast(e, ToastKind::Warn);
                    }
                    sounds.play(&mut commands, now, Sfx::Deny, 0.5, 0.0);
                }
            },
            (Tool::Erase, Some(i)) => {
                hangar.erase(i);
                sounds.play(&mut commands, now, Sfx::Erase, 0.5, 0.0);
            }
            (Tool::Paint, Some(i)) => {
                hangar.paint(i);
                sounds.play(&mut commands, now, Sfx::Place, 0.4, 0.0);
            }
            _ => {}
        }
    }
}

fn edit_keys(keys: Res<ButtonInput<KeyCode>>, focus: Res<InputFocus>, editing: Query<(), With<EditableText>>, overlays: Res<Overlays>, modal: Res<ModalReq>, mut w: MessageWriter<UiAction>) {
    if typing(&focus, &editing) || overlays.any(&modal) {
        return;
    }
    let cmd = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::SuperLeft, KeyCode::SuperRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if cmd {
        if keys.just_pressed(KeyCode::KeyZ) {
            w.write(if shift { UiAction::Redo } else { UiAction::Undo });
        }
        if keys.just_pressed(KeyCode::KeyY) {
            w.write(UiAction::Redo);
        }
        return;
    }
    // Shift moves the whole ship: arrows for left/right and up/down, brackets for forward/back
    if shift {
        for (k, d) in [(KeyCode::ArrowLeft, (-1, 0, 0)), (KeyCode::ArrowRight, (1, 0, 0)), (KeyCode::ArrowUp, (0, 1, 0)), (KeyCode::ArrowDown, (0, -1, 0)), (KeyCode::BracketLeft, (0, 0, -1)), (KeyCode::BracketRight, (0, 0, 1))] {
            if keys.just_pressed(k) {
                w.write(UiAction::Nudge(d.0, d.1, d.2));
            }
        }
        return;
    }
    let digits = [KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4, KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8, KeyCode::Digit9, KeyCode::Digit0, KeyCode::Minus, KeyCode::Equal];
    for (i, k) in digits.iter().enumerate() {
        if keys.just_pressed(*k) {
            w.write(UiAction::SelectType(BlockType::ALL[i]));
        }
    }
    for (k, a) in [
        (KeyCode::KeyB, UiAction::Tool(0)),
        (KeyCode::KeyX, UiAction::Tool(1)),
        (KeyCode::KeyG, UiAction::Tool(2)),
        (KeyCode::KeyM, UiAction::Mirror),
        (KeyCode::Comma, UiAction::ShipPrev),
        (KeyCode::Period, UiAction::ShipNext),
        (KeyCode::BracketLeft, UiAction::Layer(-1)),
        (KeyCode::BracketRight, UiAction::Layer(1)),
        (KeyCode::Escape, UiAction::Goto(Screen::Menu)),
    ] {
        if keys.just_pressed(k) {
            w.write(a);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn hangar_actions(mut msgs: MessageReader<UiAction>, mut hangar: ResMut<Hangar>, mut toasts: ResMut<Toasts>, mut modal: ResMut<ModalReq>) {
    for m in msgs.read() {
        match m.clone() {
            UiAction::SelectType(t) => {
                hangar.ty = t;
                hangar.tool = Tool::Place;
            }
            UiAction::Tool(t) => hangar.tool = [Tool::Place, Tool::Erase, Tool::Paint][t as usize],
            UiAction::Color(c) => {
                hangar.color = c;
                if hangar.tool == Tool::Erase {
                    hangar.tool = Tool::Paint;
                }
            }
            UiAction::Layer(d) => hangar.layer = (hangar.layer + d).clamp(GMIN.y, GMAX.y),
            UiAction::Nudge(x, y, z) => {
                if let Err(why) = hangar.nudge(IVec3::new(x as i32, y as i32, z as i32)) {
                    toasts.push(why, ToastKind::Warn);
                }
            }
            UiAction::Undo => {
                if let Some(prev) = hangar.undo.pop() {
                    let cur = std::mem::replace(&mut hangar.design.blocks, prev);
                    hangar.redo.push(cur);
                    hangar.changed();
                } else {
                    toasts.push("没有可以撤销的操作", ToastKind::Info);
                }
            }
            UiAction::Redo => {
                if let Some(nx) = hangar.redo.pop() {
                    let cur = std::mem::replace(&mut hangar.design.blocks, nx);
                    hangar.undo.push(cur);
                    hangar.changed();
                }
            }
            UiAction::Mirror => {
                hangar.mirror = !hangar.mirror;
                toasts.push(if hangar.mirror { "镜像开启：左右同时放置" } else { "镜像关闭" }, ToastKind::Info);
            }
            UiAction::Clear => modal.show("清空机体？", "会删掉所有方块，只留下驾驶舱。之后可以用「撤销」找回。", vec![("取消", UiAction::CloseModal, Kind::Normal), ("清空", UiAction::ClearConfirmed, Kind::Danger)], None),
            UiAction::ClearConfirmed => {
                modal.0 = None;
                let name = hangar.design.name.clone();
                hangar.load_design(Design { name, blocks: preset("blank").blocks });
            }
            UiAction::CopyShare => {
                let code = settings::encode_share(&hangar.design);
                match arboard::Clipboard::new().and_then(|mut c| c.set_text(code)) {
                    Ok(()) => toasts.push("分享码已复制。网页版和桌面版都能导入", ToastKind::Ok),
                    Err(e) => toasts.push(format!("无法访问剪贴板：{e}"), ToastKind::Err),
                }
            }
            UiAction::PasteShare => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                Ok(t) => match settings::decode_share(&t) {
                    Ok(d) => {
                        toasts.push(format!("已导入「{}」，放在最后一架", d.name), ToastKind::Ok);
                        hangar.add_ship(d);
                    }
                    Err(e) => toasts.push(format!("剪贴板里不是有效的分享码：{e}"), ToastKind::Warn),
                },
                Err(e) => toasts.push(format!("无法读取剪贴板：{e}"), ToastKind::Err),
            },
            _ => continue,
        }
        hangar.ui_dirty = true;
    }
    for (s, k) in std::mem::take(&mut hangar.pending_toasts) {
        toasts.push(s, k);
    }
}

fn rebuild_model(mut commands: Commands, mut hangar: ResMut<Hangar>, old: Query<Entity, With<HangarShip>>, mut art: ResMut<BlockArt>, mut mats: ResMut<Assets<StandardMaterial>>) {
    if !hangar.dirty {
        return;
    }
    hangar.dirty = false;
    for e in &old {
        commands.entity(e).despawn();
    }
    let root = commands.spawn((Transform::default(), Visibility::default(), HangarShip, DespawnOnExit(Screen::Hangar))).id();
    for b in &hangar.design.blocks {
        art.spawn_block(&mut commands, &mut mats, root, b, b.pos().as_vec3(), 3);
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh_ui(
    mut commands: Commands,
    mut hangar: ResMut<Hangar>,
    font: Res<UiFont>,
    list: Single<Entity, With<CheckList>>,
    summary: Single<Entity, With<CheckSummary>>,
    bars: Res<StatBars>,
    mut texts: ParamSet<(Single<&mut Text, With<LayerText>>, Single<&mut Text, With<MetaText>>, Query<&mut Text>)>,
    mut gauges: Query<&mut Plate>,
    mut buttons: Query<(&ActionOf, &mut Selected)>,
    mut swatches: Query<(&Swatch, &Hovered, &mut BorderColor)>,
    rows: Query<(&CheckRow, &Hovered)>,
    icons: Res<Icons>,
) {
    // hovering a check highlights its blocks
    let focus: Vec<usize> = rows.iter().filter(|(_, h)| h.get()).flat_map(|(r, _)| r.0.clone()).collect();
    if focus != hangar.focus {
        hangar.focus = focus;
    }
    for (sw, h, mut b) in &mut swatches {
        b.set_all(if sw.0 == hangar.color { ACCENT } else if h.get() { LINE_HI } else { LINE });
    }
    if !hangar.ui_dirty {
        return;
    }
    hangar.ui_dirty = false;
    let f = &*font;
    let a = hangar.analysis.clone();
    let s = &a.stats;

    commands.entity(*list).despawn_children();
    let mut checks = a.checks.clone();
    checks.sort_by_key(|c| match c.level {
        Level::Err => 0,
        Level::Warn => 1,
        Level::Ok => 2,
    });
    for c in &checks {
        let (icon, col) = match c.level {
            Level::Ok => (&icons.check, OK),
            Level::Warn => (&icons.warn, WARN),
            Level::Err => (&icons.error, DANGER),
        };
        let r = commands
            .spawn((
                Node { column_gap: px(10), align_items: AlignItems::Center, padding: UiRect { left: px(16), right: px(12), top: px(8), bottom: px(8) }, ..default() },
                Plate {
                    fill: if c.level == Level::Ok { tone(Color::WHITE, 0.045) } else { tone(col, 0.14) },
                    edge: if c.level == Level::Ok { LINE } else { col.with_alpha(0.4) },
                    chamfer: 8.0,
                    corners: BR,
                    ..default()
                },
                Hovered::default(),
                CheckRow(c.cells.clone()),
                ChildOf(*list),
            ))
            .id();
        commands.spawn((Node { position_type: PositionType::Absolute, left: px(0), top: px(0), bottom: px(0), width: px(3), ..default() }, BackgroundColor(col), Pickable::IGNORE, ChildOf(r)));
        // 18px icon, centred vertically in the row
        commands.spawn((ImageNode { image: icon.clone(), color: col, ..default() }, Node { width: px(18), height: px(18), flex_shrink: 0.0, ..default() }, Pickable::IGNORE, ChildOf(r)));
        commands.spawn((
            col_text(f, c.title, &c.msg, c.level == Level::Ok),
            Pickable::IGNORE,
            ChildOf(r),
        ));
    }
    commands.entity(*summary).despawn_children();
    let (st, sc) = if a.errors > 0 { (format!("{} 项未通过", a.errors), DANGER) } else if a.warns > 0 { (format!("通过 · {} 条提醒", a.warns), WARN) } else { ("全部通过".to_string(), OK) };
    commands.spawn((chip(f, st, sc), ChildOf(*summary)));

    let vals = [
        (s.cost / 350.0, crate::screens::menu::cost_label(s.cost)),
        (s.max_speed / 125.0, format!("{:.0} m/s", s.max_speed)),
        (s.pitch_rate / 2.0, format!("{:.0} 度/秒", s.pitch_rate.to_degrees())),
        (s.yaw_rate / 1.7, format!("{:.0} 度/秒", s.yaw_rate.to_degrees())),
        (s.roll_rate / 3.5, format!("{:.0} 度/秒", s.roll_rate.to_degrees())),
        (s.dps / 200.0, format!("{:.0} DPS", s.dps)),
        (s.dps_sustained / 200.0, format!("{:.0} DPS", s.dps_sustained)),
        (s.hp / 3500.0, format!("{:.0}", s.hp)),
        (s.shield_cap / 200.0, if s.shield_cap > 0.0 { format!("{:.0}", s.shield_cap) } else { "无".into() }),
        (s.energy_cap / 300.0, format!("{:.0} +{:.0}/s", s.energy_cap, s.energy_regen)),
    ];
    for ((fill, val), (frac, label)) in bars.0.iter().zip(vals) {
        if let Ok(mut p) = gauges.get_mut(*fill) {
            p.frac = frac.clamp(0.0, 1.0);
        }
        if let Ok(mut t) = texts.p2().get_mut(*val) {
            t.0 = label;
        }
    }
    let fire = match s.fire_endurance {
        Some(t) => format!("满火力 {t:.0} 秒后能量耗尽，之后火力 {:.0}%", s.dps_sustained / s.dps.max(1.0) * 100.0),
        None if s.energy_draw > 0.0 => "能量够武器一直满火力".to_string(),
        None => "武器不耗能量".to_string(),
    };
    texts.p1().0 = format!("质量 {:.1}  ·  方块 {}/{}\n推重比 {:.2}  ·  升力比 {:.2}\n{fire}", s.mass, s.n, MAX_BLOCKS, s.twr, s.lift_ratio);
    texts.p0().0 = format!("Y {}", hangar.layer);
    for (act, mut sel) in &mut buttons {
        let on = match &act.0 {
            UiAction::SelectType(t) => *t == hangar.ty && hangar.tool == Tool::Place,
            UiAction::Tool(t) => [Tool::Place, Tool::Erase, Tool::Paint][*t as usize] == hangar.tool,
            UiAction::Mirror => hangar.mirror,
            _ => continue,
        };
        if sel.0 != on {
            sel.0 = on;
        }
    }
}

/// Tooltip detail for a part: mass, hit points, weapon output, and what it does.
fn part_detail(t: BlockType) -> String {
    let i = t.info();
    let weapon = t
        .weapon()
        .map(|w| {
            if w.homing {
                format!(" · 单发 {:.0} 伤害", w.dmg)
            } else if w.pellets > 1 {
                format!(" · 每秒 {:.0} 伤害（{} 发弹丸全中）", w.rate * w.dmg * w.pellets as f32, w.pellets)
            } else if w.pierce > 1 {
                format!(" · 单发 {:.0} 伤害，穿透 {} 块", w.dmg, w.pierce)
            } else {
                format!(" · 每秒 {:.0} 伤害", w.rate * w.dmg)
            }
        })
        .unwrap_or_default();
    let weapon = if t.thrust() > 0.0 { format!("{weapon} · 推力 {:.0}", t.thrust()) } else { weapon };
    format!("造价 {} · 质量 {} · 耐久 {}{}\n{}", i.cost, i.mass, i.hp, weapon, i.info)
}

fn col_text(f: &UiFont, title: &str, msg: &str, ok: bool) -> impl Bundle {
    (
        Node { flex_direction: FlexDirection::Column, row_gap: px(2), flex_shrink: 1.0, ..default() },
        children![strong(f, title, 13.0, if ok { INK.with_alpha(0.75) } else { INK }), text(f, msg.to_string(), 12.0, MUTED)],
    )
}

fn draw_gizmos(mut gizmos: Gizmos, hangar: Res<Hangar>, time: Res<Time>) {
    let size = (GMAX - GMIN + IVec3::ONE).as_vec3();
    let centre = (GMAX + GMIN).as_vec3() / 2.0;
    gizmos.cube(Transform::from_translation(centre).with_scale(size), ACCENT.with_alpha(0.12));
    let y = hangar.layer as f32 - 0.5;
    let c = ACCENT.with_alpha(0.06);
    for x in GMIN.x..=GMAX.x + 1 {
        let x = x as f32 - 0.5;
        gizmos.line(Vec3::new(x, y, GMIN.z as f32 - 0.5), Vec3::new(x, y, GMAX.z as f32 + 0.5), c);
    }
    for z in GMIN.z..=GMAX.z + 1 {
        let z = z as f32 - 0.5;
        gizmos.line(Vec3::new(GMIN.x as f32 - 0.5, y, z), Vec3::new(GMAX.x as f32 + 0.5, y, z), c);
    }
    let pulse = 0.55 + 0.45 * (time.elapsed_secs() * 6.0).sin();
    let blocks = &hangar.design.blocks;
    for chk in &hangar.analysis.checks {
        let col = match chk.level {
            Level::Err => DANGER.with_alpha(pulse),
            Level::Warn => WARN.with_alpha(0.8),
            Level::Ok => continue,
        };
        for &i in &chk.cells {
            if let Some(b) = blocks.get(i) {
                gizmos.cube(Transform::from_translation(b.pos().as_vec3()).with_scale(Vec3::splat(1.06)), col);
            }
        }
    }
    for &i in &hangar.focus {
        if let Some(b) = blocks.get(i) {
            gizmos.cube(Transform::from_translation(b.pos().as_vec3()).with_scale(Vec3::splat(1.12)), Color::WHITE);
        }
    }
    if let Some((blk, cell)) = hangar.pick {
        match hangar.tool {
            Tool::Place => {
                let ok = place_reason(blocks, cell, hangar.ty).is_none();
                let col = if ok { ACCENT } else { DANGER };
                gizmos.cube(Transform::from_translation(cell.as_vec3()).with_scale(Vec3::splat(1.04)), col);
                if let Some(m) = hangar.mirror_of(cell)
                    && hangar.ty != BlockType::Cockpit
                {
                    gizmos.cube(Transform::from_translation(m.as_vec3()).with_scale(Vec3::splat(1.04)), col.with_alpha(0.5));
                }
            }
            Tool::Erase | Tool::Paint => {
                if let Some(i) = blk {
                    let col = if hangar.tool == Tool::Erase { DANGER } else { srgb(hangar.color) };
                    let p = blocks[i].pos();
                    gizmos.cube(Transform::from_translation(p.as_vec3()).with_scale(Vec3::splat(1.06)), col);
                    if let Some(m) = hangar.mirror_of(p) {
                        gizmos.cube(Transform::from_translation(m.as_vec3()).with_scale(Vec3::splat(1.06)), col.with_alpha(0.5));
                    }
                }
            }
        }
    }
}
