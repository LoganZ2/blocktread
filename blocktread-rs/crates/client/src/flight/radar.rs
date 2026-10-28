//! The arena drawn under the radar's dots (see `radar.wesl`): its layout baked once into a small
//! texture, which the shader turns with the vehicle every frame.

use super::Session;
use crate::ui::*;
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;
use bevy::ui_render::prelude::{MaterialNode, UiMaterial, UiMaterialPlugin};
use blocktread_shared::arena::SAW_R;
use blocktread_shared::solid::Style;
use blocktread_shared::terrain::Terrain;

/// How far the radar reaches on an arena (out on the open ground it is 900 m).
pub const ARENA_RANGE: f32 = 150.0;
/// The baked map: its half side in metres, and its texels along a side (a metre each).
const MAP_HALF: f32 = 128.0;
const MAP_N: u32 = 256;

/// The node the map is drawn on, inside the radar.
#[derive(Component)]
pub struct RadarMap;

/// The map's node: whether it shows, and its material once it has one.
type MapNode<'a> = (Entity, &'a mut Visibility, Option<&'a MaterialNode<RadarMaterial>>);

#[derive(ShaderType, Clone, Copy, Debug, Default)]
struct RadarUniform {
    view: Vec4,
    axes: Vec4,
    wall: Vec4,
    pit: Vec4,
    hill: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct RadarMaterial {
    #[uniform(0)]
    u: RadarUniform,
    #[texture(1)]
    #[sampler(2)]
    map: Handle<Image>,
}

impl UiMaterial for RadarMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://blocktread_client/flight/radar.wesl".into()
    }
}

pub struct RadarPlugin;

impl Plugin for RadarPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "radar.wesl");
        app.add_plugins(UiMaterialPlugin::<RadarMaterial>::default());
    }
}

/// The layout as seen from above: walls and cover in red, pits and traps (the saws' slots, the
/// vents) in green, the hill and its ramps in blue. Walls are widened a little so that the thin ones still show at radar scale.
fn bake(ground: &Terrain) -> Image {
    let arena = ground.arena.as_ref();
    let mut data = Vec::with_capacity((MAP_N * MAP_N * 4) as usize);
    for j in 0..MAP_N {
        for i in 0..MAP_N {
            let (x, z) = (-MAP_HALF + i as f32 + 0.5, -MAP_HALF + j as f32 + 0.5);
            let mut px = [0u8; 4];
            for s in ground.solids_near(Vec3::new(x, 1.0, z), 2.0) {
                match s.style {
                    Style::Wall | Style::Cover | Style::Crate => {
                        let near = |y: f32| s.distance(Vec3::new(x, y, z)).0 < 0.5;
                        if near(1.0) || near(3.5) {
                            px[0] = 255;
                        }
                    }
                    Style::Platform | Style::Ramp if s.top(x, z).is_some_and(|t| t > 0.3) => px[2] = 255,
                    _ => {}
                }
            }
            let slot = |a: &blocktread_shared::arena::Arena| {
                a.saws.iter().any(|s| {
                    let d = Vec3::new(x, 0.0, z) - s.mid;
                    let along = d.dot(s.dir);
                    along.abs() < s.reach + SAW_R && (d - s.dir * along).length() < 1.2
                })
            };
            if arena.is_some_and(|a| a.in_pit(x, z) || slot(a) || a.vents.iter().any(|v| v.area.contains(x, z))) {
                px[1] = 255;
            }
            px[3] = 255;
            data.extend_from_slice(&px);
        }
    }
    let mut img = Image::new(Extent3d { width: MAP_N, height: MAP_N, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    img
}

/// Bakes the map the first time there is an arena to show, then turns it with the vehicle.
pub fn update_radar_map(
    mut commands: Commands,
    session: Res<Session>,
    mut nodes: Query<MapNode, With<RadarMap>>,
    mut mats: ResMut<Assets<RadarMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let ground = &session.core.terrain;
    let view = session.view();
    for (e, mut vis, mat) in &mut nodes {
        let shown = ground.arena.is_some() && view.is_some();
        vis.set_if_neq(if shown { Visibility::Inherited } else { Visibility::Hidden });
        let Some(m) = view.filter(|_| shown) else { continue };
        let (right, back) = ((m.rot * Vec3::X).with_y(0.0).normalize_or(Vec3::X), (m.rot * Vec3::Z).with_y(0.0).normalize_or(Vec3::Z));
        let lin = |c: Color, a: f32| c.with_alpha(a).to_linear().to_vec4();
        let u = RadarUniform {
            view: Vec4::new(m.pos.x, m.pos.z, ARENA_RANGE, MAP_HALF),
            axes: Vec4::new(right.x, right.z, back.x, back.z),
            wall: lin(INK, 0.62),
            pit: lin(DANGER, 0.45),
            hill: lin(ACCENT, 0.3),
        };
        if let Some(mut mat) = mat.and_then(|h| mats.get_mut(&h.0)) {
            mat.u = u;
        } else {
            let map = images.add(bake(ground));
            commands.entity(e).insert(MaterialNode(mats.add(RadarMaterial { u, map })));
        }
    }
}
