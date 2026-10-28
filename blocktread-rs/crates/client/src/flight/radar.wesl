// The arena under the radar's dots: its walls, pits and hill, turned with the vehicle so that
// ahead is up, out to the radar's rim. Flat colours, like every other panel.

import bevy_ui_render::ui_vertex_output::UiVertexOutput;

struct Radar {
    // xy: the vehicle's x and z; z: metres from the middle to the rim; w: the map's half side
    view: vec4<f32>,
    // the vehicle's right (xy) and back (zw), flat on the ground
    axes: vec4<f32>,
    wall: vec4<f32>,
    pit: vec4<f32>,
    hill: vec4<f32>,
}

@group(1) @binding(0) var<uniform> radar: Radar;
@group(1) @binding(1) var map: texture_2d<f32>;
@group(1) @binding(2) var map_s: sampler;

fn over(dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    return src + dst * (1.0 - src.a);
}

fn pm(c: vec4<f32>, a: f32) -> vec4<f32> {
    let al = clamp(c.a * a, 0.0, 1.0);
    return vec4<f32>(c.rgb * al, al);
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let d = in.uv * 2.0 - 1.0;
    let r = length(d);
    // a pixel's worth of soft edge at the rim
    let rim = clamp((1.0 - r) * in.size.x * 0.5, 0.0, 1.0);
    if rim <= 0.0 {
        discard;
    }
    let world = radar.view.xy + (radar.axes.xy * d.x + radar.axes.zw * d.y) * radar.view.z;
    let m = textureSample(map, map_s, world / (2.0 * radar.view.w) + 0.5);
    var col = vec4<f32>(0.0);
    col = over(col, pm(radar.hill, m.b));
    col = over(col, pm(radar.pit, m.g));
    col = over(col, pm(radar.wall, m.r));
    col = col * rim;
    if col.a <= 0.002 {
        discard;
    }
    return vec4<f32>(col.rgb / col.a, col.a);
}
