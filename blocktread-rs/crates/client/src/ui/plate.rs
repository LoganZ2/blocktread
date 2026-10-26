//! Shader-drawn plates: the surface of every panel, button, chip and gauge (see `plate.wesl`).
//!
//! Put a [`Plate`] on any UI node; `sync_plates` turns it into a material and keeps the material
//! in step whenever the component changes, so animation systems only touch the component.

use bevy::asset::embedded_asset;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bevy::ui::{ComputedNode, UiSystems};
use bevy::ui_render::prelude::{MaterialNode, UiMaterial, UiMaterialPlugin};

#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub struct Plate {
    pub fill: Color,
    pub edge: Color,
    /// Accent strip and lit gauge cells.
    pub mark: Color,
    /// Cut corners, `chamfer` px: bits 1 top left, 2 top right, 4 bottom right, 8 bottom left.
    /// (The field-kit look keeps plates square; the shader still cuts them when asked.)
    pub chamfer: f32,
    pub edge_w: f32,
    /// Length of the accent strip on the top edge (0 = none).
    pub accent: f32,
    pub corners: u8,
    /// Gauge cells (0 = plain plate).
    pub segments: f32,
    /// Gauge fill, 0..1.
    pub frac: f32,
    /// Film-grain strength on the fill (0 = none).
    pub grain: f32,
    /// Width of a hazard-striped band at the left edge (0 = none), striped in `stripe` over the fill.
    pub stripes: f32,
    pub stripe: Color,
}

impl Default for Plate {
    fn default() -> Self {
        Plate { fill: Color::NONE, edge: Color::NONE, mark: Color::NONE, chamfer: 0.0, edge_w: 1.0, accent: 0.0, corners: 0, segments: 0.0, frac: 0.0, grain: 0.0, stripes: 0.0, stripe: Color::NONE }
    }
}

impl Plate {
    /// Segmented gauge in `color`, `cells` wide, filled to `frac`.
    pub fn gauge(color: Color, cells: f32, frac: f32) -> Plate {
        Plate { fill: Color::srgba(1.0, 1.0, 1.0, 0.045), mark: color, edge_w: 0.0, segments: cells, frac, ..default() }
    }

    /// `scale`: physical pixels per logical pixel, so chamfers and hairlines keep their logical
    /// size on high-DPI screens.
    fn uniform(&self, scale: f32) -> PlateUniform {
        let v = |c: Color| c.to_linear().to_vec4();
        PlateUniform {
            fill: v(self.fill),
            edge: v(self.edge),
            mark: v(self.mark),
            shape: Vec4::new(self.chamfer, self.edge_w, self.accent, self.grain),
            extra: Vec4::new(self.corners as f32, self.segments, self.frac, scale),
            stripe: v(self.stripe),
            more: Vec4::new(self.stripes, 0.0, 0.0, 0.0),
        }
    }
}

#[derive(ShaderType, Clone, Copy, Debug, Default)]
struct PlateUniform {
    fill: Vec4,
    edge: Vec4,
    mark: Vec4,
    shape: Vec4,
    extra: Vec4,
    stripe: Vec4,
    more: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct PlateMaterial {
    #[uniform(0)]
    u: PlateUniform,
}

impl UiMaterial for PlateMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://blocktread_client/ui/plate.wesl".into()
    }
}

pub struct PlatePlugin;

impl Plugin for PlatePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "plate.wesl");
        // after layout, which is where a node learns its target's scale factor
        app.add_plugins(UiMaterialPlugin::<PlateMaterial>::default()).add_systems(PostUpdate, sync_plates.after(UiSystems::Layout));
    }
}

fn sync_plates(mut commands: Commands, q: Query<(Entity, Ref<Plate>, &ComputedNode, Option<&MaterialNode<PlateMaterial>>)>, mut mats: ResMut<Assets<PlateMaterial>>) {
    for (e, p, node, mat) in &q {
        let scale = 1.0 / node.inverse_scale_factor.max(1e-3);
        match mat {
            Some(mat) => {
                let stale = mats.get(&mat.0).is_some_and(|m| m.u.extra.w != scale);
                if (p.is_changed() || stale)
                    && let Some(mut m) = mats.get_mut(&mat.0)
                {
                    m.u = p.uniform(scale);
                }
            }
            None => {
                commands.entity(e).insert(MaterialNode(mats.add(PlateMaterial { u: p.uniform(scale) })));
            }
        }
    }
}
