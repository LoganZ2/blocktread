// The surface of every panel, button, chip and gauge: flat fill with optional chamfered corners, a
// hairline edge, an accent strip, faint grain, segmented gauges and a hazard-striped band.
// Deliberately no gradients or glow: hover and selection change flat colours only.

import bevy_ui_render::ui_vertex_output::UiVertexOutput;

struct Plate {
    fill: vec4<f32>,
    edge: vec4<f32>,
    // accent strip and lit gauge cells
    mark: vec4<f32>,
    // x: chamfer, y: edge width, z: accent strip length (all logical px), w: grain strength
    shape: vec4<f32>,
    // x: chamfered corners (1 TL, 2 TR, 4 BR, 8 BL), y: gauge cells (0 = plain plate), z: gauge fill 0..1,
    // w: physical pixels per logical pixel (sizes above are logical; `in.size` is physical)
    extra: vec4<f32>,
    // colour of the hazard stripes
    stripe: vec4<f32>,
    // x: width of the striped band at the left edge (logical px, 0 = none)
    more: vec4<f32>,
}

@group(1) @binding(0) var<uniform> plate: Plate;

fn has(mask: u32, bit: u32) -> bool {
    return (mask & bit) != 0u;
}

// Signed distance in pixels to the node rectangle with chamfered corners (negative inside).
fn plate_sdf(p: vec2<f32>, size: vec2<f32>, c: f32, mask: u32) -> f32 {
    var d = max(max(-p.x, p.x - size.x), max(-p.y, p.y - size.y));
    let k = 0.70710678;
    if has(mask, 1u) { d = max(d, (c - (p.x + p.y)) * k); }
    if has(mask, 2u) { d = max(d, (c - ((size.x - p.x) + p.y)) * k); }
    if has(mask, 4u) { d = max(d, (c - ((size.x - p.x) + (size.y - p.y))) * k); }
    if has(mask, 8u) { d = max(d, (c - (p.x + (size.y - p.y))) * k); }
    return d;
}

// Premultiplied "over".
fn over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return src + dst * (1.0 - src.a);
}

fn pm(c: vec4<f32>, a: f32) -> vec4<f32> {
    let al = clamp(c.a * a, 0.0, 1.0);
    return vec4<f32>(c.rgb * al, al);
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let size = in.size;
    let p = in.uv * size;
    let mask = u32(plate.extra.x);
    let scale = plate.extra.w;
    let chamfer = min(plate.shape.x * scale, min(size.x, size.y) * 0.5);
    let d = plate_sdf(p, size, chamfer, mask);
    let cover = clamp(0.5 - d, 0.0, 1.0);
    if cover <= 0.0 {
        discard;
    }

    var col = vec4<f32>(0.0);
    let cells = plate.extra.y;
    if cells > 0.0 {
        let cell = size.x / cells;
        let gap = min(2.0 * scale, cell * 0.3);
        if (p.x % cell) < cell - gap {
            col = select(pm(plate.fill, 1.0), pm(plate.mark, 1.0), p.x < plate.extra.z * size.x);
        }
    } else {
        col = pm(plate.fill, 1.0);
        let grain = plate.shape.w;
        if grain > 0.0 && col.a > 0.0 {
            // add the noise in (roughly) gamma space: in linear space a fixed amount would be
            // amplified into visible static on near-black fills
            let n = hash(floor(p)) - 0.5;
            let g = sqrt(col.rgb / col.a) + vec3<f32>(n * 0.016 * grain);
            col = vec4<f32>(max(g, vec3<f32>(0.0)) * max(g, vec3<f32>(0.0)) * col.a, col.a);
        }
    }

    // hazard stripes: diagonal bands across the left end
    let band = plate.more.x * scale;
    if band > 0.0 && p.x < band {
        let period = 10.0 * scale;
        if ((p.x + p.y) % period) < period * 0.5 {
            col = over(col, pm(plate.stripe, 1.0));
        }
    }

    let edge_w = plate.shape.y * scale;
    if edge_w > 0.0 {
        let e = clamp(0.5 - d, 0.0, 1.0) - clamp(0.5 - (d + edge_w), 0.0, 1.0);
        col = over(col, pm(plate.edge, e));
    }

    // accent strip on the top edge, just after the top-left chamfer
    let acc = plate.shape.z * scale;
    if acc > 0.0 {
        let x0 = select(0.0, chamfer, has(mask, 1u));
        if p.y < 2.0 * scale && p.x >= x0 && p.x < x0 + acc {
            col = over(col, pm(plate.mark, 1.0));
        }
    }

    col = col * cover;
    if col.a <= 0.002 {
        discard;
    }
    return vec4<f32>(col.rgb / col.a, col.a);
}
