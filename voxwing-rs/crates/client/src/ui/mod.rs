//! The game's UI layer: theme, font selection, the action message every control emits, and
//! the component kit (`kit`) that screens build from.
//!
//! Visual language: a ship's bridge display. Shader-drawn plates with chamfered corners, hairline
//! edges and faint scanlines (`plate`), teal accent, signal orange reserved for alerts. Three
//! bundled typefaces: a condensed display face for titles, a squared technical face for numbers
//! and Latin captions, and a clean CJK sans for body text.

pub mod icons;
pub mod kit;
pub mod plate;

pub use plate::{BL, BR, Plate, TL, TR};

use crate::Screen;
use bevy::picking::hover::HoverMap;
use bevy::prelude::*;
use bevy::text::{FontWeight, LineHeight};
use voxwing_shared::blocks::BlockType;

// palette
pub const PANEL: Color = Color::srgba(0.026, 0.034, 0.045, 0.86);
pub const PANEL_SOLID: Color = Color::srgb(0.032, 0.041, 0.053);
pub const LINE: Color = Color::srgba(1.0, 1.0, 1.0, 0.09);
pub const LINE_HI: Color = Color::srgba(1.0, 1.0, 1.0, 0.22);
pub const INK: Color = Color::srgb(0.91, 0.945, 0.97);
pub const MUTED: Color = Color::srgb(0.54, 0.58, 0.62);
pub const ACCENT: Color = Color::srgb(0.31, 0.84, 0.76);
pub const ON_ACCENT: Color = Color::srgb(0.02, 0.07, 0.08);
pub const SIGNAL: Color = Color::srgb(1.0, 0.56, 0.25);
pub const OK: Color = Color::srgb(0.36, 0.89, 0.56);
pub const WARN: Color = Color::srgb(1.0, 0.8, 0.3);
pub const DANGER: Color = Color::srgb(1.0, 0.33, 0.42);

// layout scale: every size and gap is a multiple of 4
/// Inset of screen-level panels and bars from the window edge.
pub const EDGE: f32 = 24.0;
/// Inner padding of panels and dialogs.
pub const PAD: f32 = 20.0;
/// Gap between sections inside a panel.
pub const GAP: f32 = 16.0;
/// Gap between items within a section.
pub const GAP_S: f32 = 8.0;
/// Control heights: medium (buttons, fields, tabs, toggle rows) and large (the main action).
pub const H_MD: f32 = 36.0;
pub const H_LG: f32 = 52.0;

/// The UI typefaces, all bundled (SIL Open Font License, see `assets/fonts`):
/// Noto Sans SC for body text, Chakra Petch for numbers and Latin captions, and Smiley Sans
/// (得意黑) for display titles. Each list ends in system CJK families so names players type with
/// characters outside the bundled subset still render.
#[derive(Resource, Clone)]
pub struct UiFont {
    pub body: FontSource,
    pub tech: FontSource,
    pub display: FontSource,
}

const SYSTEM_CJK: &str = "PingFang SC, Hiragino Sans GB, Microsoft YaHei, Noto Sans CJK SC, sans-serif";

/// Strong handles to the bundled font assets. Bevy frees an asset when its last strong handle
/// drops, which would unregister the font before any text could use it.
#[derive(Resource)]
pub struct BundledFonts(#[allow(dead_code)] Vec<Handle<Font>>);

pub fn load_fonts(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let files: [&[u8]; 6] = [
        include_bytes!("../../assets/fonts/NotoSansSC-VF-subset.ttf"),
        include_bytes!("../../assets/fonts/SmileySans-Oblique.ttf"),
        include_bytes!("../../assets/fonts/ChakraPetch-Regular.ttf"),
        include_bytes!("../../assets/fonts/ChakraPetch-Medium.ttf"),
        include_bytes!("../../assets/fonts/ChakraPetch-SemiBold.ttf"),
        include_bytes!("../../assets/fonts/ChakraPetch-Bold.ttf"),
    ];
    let handles = files.into_iter().map(|bytes| fonts.add(Font::from_bytes(bytes.to_vec()))).collect();
    commands.insert_resource(BundledFonts(handles));
    let list = |head: &str| FontSource::Families(format!("{head}, {SYSTEM_CJK}").into());
    commands.insert_resource(UiFont { body: list("Noto Sans SC"), tech: list("Chakra Petch, Noto Sans SC"), display: list("Smiley Sans, Smiley Sans Oblique, Noto Sans SC") });
}

fn face(src: &FontSource, size: f32, weight: u16) -> TextFont {
    TextFont { font: src.clone(), font_size: FontSize::Px(size), weight: FontWeight(weight), ..default() }
}

/// Body text font.
pub fn tf(font: &UiFont, size: f32) -> TextFont {
    face(&font.body, size, 500)
}

/// Body text (medium weight: regular strokes look too thin light-on-dark).
pub fn text(font: &UiFont, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s.into()), face(&font.body, size, 500), TextColor(color))
}

/// Multi-line body text with comfortable line spacing.
pub fn para(font: &UiFont, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s.into()), face(&font.body, size, 500), TextColor(color), LineHeight::RelativeToFont(1.55))
}

/// Emphasised body text: labels, column heads, section captions.
pub fn strong(font: &UiFont, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s.into()), face(&font.body, size, 700), TextColor(color))
}

/// Headings: screen titles, dialog titles, names.
pub fn heading(font: &UiFont, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s.into()), face(&font.body, size, 800), TextColor(color))
}

/// Numbers, readouts and Latin captions.
pub fn tech(font: &UiFont, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s.into()), face(&font.tech, size, 600), TextColor(color))
}

/// The display face, for the few hero moments only (game title, combat announcements).
pub fn display(font: &UiFont, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(s.into()), face(&font.display, size, 400), TextColor(color))
}

/// Settings a slider or toggle can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetKey {
    Sound,
    Volume,
    Sensitivity,
    InvertY,
    Bloom,
    Vsync,
    ShowFps,
    ShowHelp,
}

/// Everything a control can ask for. One message type keeps all handlers in plain systems.
#[derive(Message, Clone, Debug, PartialEq)]
pub enum UiAction {
    Goto(Screen),
    Quit,
    QuitConfirmed,
    OpenSettings,
    CloseSettings,
    CloseModal,
    // display settings
    DisplayMode(&'static str),
    WindowSize(u32, u32),
    FullscreenSize(u32, u32),
    FpsCap(u32),
    KeepDisplay,
    RevertDisplay,
    Set(SetKey, f32),
    // hangar
    SelectType(BlockType),
    Tool(u8),
    Color(u32),
    Layer(i32),
    /// Move the whole ship by this many cells (x, y, z).
    Nudge(i8, i8, i8),
    Undo,
    Redo,
    Mirror,
    Clear,
    ClearConfirmed,
    /// The ship carousel: previous / next ship, a new blank one, delete the current one.
    ShipPrev,
    ShipNext,
    NewShip,
    DeleteShip,
    DeleteShipConfirmed,
    CopyShare,
    PasteShare,
    // lobby
    /// The mode to queue for: "ffa" or "2v2".
    HostMode(&'static str),
    /// Budget tier to queue in (a `balance.ron` key, empty = no limit).
    HostBudget(String),
    /// Start matchmaking, or cancel it while queued.
    Launch,
    /// Spectate instead of play.
    Watch,
    // ship pick, results
    /// Lock in the picked ship.
    ConfirmPick,
    /// Show this seat's ship in the showcase (None: your own).
    ViewSlot(Option<usize>),
    /// After the results: queue again (or watch the next match).
    PlayAgain,
    // flight
    Resume,
    /// Leave the match (and the server) for the lobby.
    LeaveFlight,
}

/// Marks a control as currently chosen within its group (tool, tab, palette tile, ...).
#[derive(Component, Default)]
pub struct Selected(pub bool);

#[derive(Component)]
pub struct ActionOf(pub UiAction);

/// True when the mouse is over any UI node (3D clicks and drags should then be ignored).
pub fn pointer_over_ui(hover: &HoverMap, nodes: &Query<(), (With<Node>, Without<IgnoreHover>)>) -> bool {
    hover.0.values().any(|hits| hits.keys().any(|e| nodes.contains(*e)))
}

/// Full-screen layout containers that should not count as "the mouse is over UI".
#[derive(Component)]
pub struct IgnoreHover;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UiAction>().add_plugins((bevy::input_focus::tab_navigation::TabNavigationPlugin, plate::PlatePlugin));
        app.add_systems(Startup, icons::load_icons);
        kit::register(app);
    }
}
