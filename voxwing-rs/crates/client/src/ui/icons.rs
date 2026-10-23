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
}

/// Raster size (px); a power of two so the mip chain divides evenly.
const RASTER: u32 = 128;

pub fn load_icons(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut icon = |svg: &str| images.add(rasterize(svg));
    commands.insert_resource(Icons {
        check: icon(include_str!("../../assets/icons/check.svg")),
        warn: icon(include_str!("../../assets/icons/warn.svg")),
        error: icon(include_str!("../../assets/icons/error.svg")),
        left: icon(include_str!("../../assets/icons/chevron-left.svg")),
        right: icon(include_str!("../../assets/icons/chevron-right.svg")),
    });
}

fn rasterize(svg: &str) -> Image {
    let tree = usvg::Tree::from_data(svg.as_bytes(), &usvg::Options::default()).expect("bundled SVG icon");
    let mut pixmap = tiny_skia::Pixmap::new(RASTER, RASTER).expect("icon pixmap");
    let scale = RASTER as f32 / tree.size().width().max(tree.size().height());
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // keep only coverage: white everywhere, so mip filtering can't pull in dark fringes
    let mut px = pixmap.take();
    for p in px.chunks_exact_mut(4) {
        p[0] = 255;
        p[1] = 255;
        p[2] = 255;
    }
    crate::art::with_mips(px, RASTER as usize)
}
