//! Vector icons. The SVG files in `assets/icons` are rasterised once at startup with resvg and
//! given mipmaps, so they stay crisp at any size. Icons are drawn in white and only their alpha is
//! kept: tint them with `ImageNode::color`.

use bevy::prelude::*;
use resvg::{tiny_skia, usvg};

#[derive(Resource)]
pub struct Icons {
    pub check: Handle<Image>,
    pub warn: Handle<Image>,
    pub error: Handle<Image>,
    pub left: Handle<Image>,
    pub right: Handle<Image>,
    /// Large thin glyphs for the lobby's carousel.
    pub arrow_left: Handle<Image>,
    pub arrow_right: Handle<Image>,
    pub plus: Handle<Image>,
}

/// Raster size (px); a power of two so the mip chain divides evenly.
const RASTER: u32 = 128;
/// For the glyphs shown well over 100 px.
const RASTER_BIG: u32 = 256;

pub fn load_icons(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut icon = |svg: &str, size: u32| images.add(rasterize(svg, size));
    commands.insert_resource(Icons {
        check: icon(include_str!("../../assets/icons/check.svg"), RASTER),
        warn: icon(include_str!("../../assets/icons/warn.svg"), RASTER),
        error: icon(include_str!("../../assets/icons/error.svg"), RASTER),
        left: icon(include_str!("../../assets/icons/chevron-left.svg"), RASTER),
        right: icon(include_str!("../../assets/icons/chevron-right.svg"), RASTER),
        arrow_left: icon(include_str!("../../assets/icons/arrow-left.svg"), RASTER_BIG),
        arrow_right: icon(include_str!("../../assets/icons/arrow-right.svg"), RASTER_BIG),
        plus: icon(include_str!("../../assets/icons/plus.svg"), RASTER_BIG),
    });
}

fn rasterize(svg: &str, size: u32) -> Image {
    let tree = usvg::Tree::from_data(svg.as_bytes(), &usvg::Options::default()).expect("bundled SVG icon");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("icon pixmap");
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // keep only coverage: white everywhere, so mip filtering can't pull in dark fringes
    let mut px = pixmap.take();
    for p in px.chunks_exact_mut(4) {
        p[0] = 255;
        p[1] = 255;
        p[2] = 255;
    }
    crate::art::with_mips(px, size as usize)
}
