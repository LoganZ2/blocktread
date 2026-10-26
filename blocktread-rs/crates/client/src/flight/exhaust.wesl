// One puff of exhaust gas: an additive, camera-facing quad. uv runs -1..1 across the quad,
// uv_b is (age 0..1, seed). The vertex colour carries the puff's colour and brightness for its
// age (computed on the CPU), so this shader only shapes it: a soft round core that turbulence
// erodes into wisps as the gas ages. The turbulence is one noise field in world space, shared by
// every overlapping puff, so the wisps run continuously along the stream instead of each puff
// eroding into its own blob.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::{view, globals};

// global brightness and tint, for tuning the whole effect at once
@group(3) @binding(0) var<uniform> tint: vec4<f32>;

fn hash3(p: vec3<f32>) -> f32 {
    let q = fract(p * 0.1031);
    let r = q + dot(q, q.yzx + 33.33);
    return fract((r.x + r.y) * r.z);
}

fn noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = hash3(i);
    let n100 = hash3(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash3(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash3(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash3(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash3(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash3(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash3(i + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

fn fbm(p: vec3<f32>) -> f32 {
    var a = 0.5;
    var s = 0.0;
    var q = p;
    for (var i = 0; i < 3; i++) {
        s += a * noise3(q);
        q = q * 2.03 + vec3<f32>(17.1, 3.7, 9.2);
        a *= 0.5;
    }
    return s / 0.875;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let r2 = dot(in.uv, in.uv);
    if r2 >= 1.0 {
        discard;
    }
    let age = in.uv_b.x;

    // soft round puff, fully transparent at the quad's edge
    let soft = exp(-r2 * 3.2) * (1.0 - smoothstep(0.55, 1.0, r2));
    // churning turbulence in world space; older gas only survives where the noise is dense
    let wp = in.world_position.xyz;
    let n = fbm(wp * 0.55 + vec3<f32>(0.0, globals.time * 0.6, globals.time * 0.25));
    let fine = noise3(wp * 2.1 - vec3<f32>(globals.time * 0.9, 0.0, 0.0));
    let dense = n * 0.8 + fine * 0.2;
    let erode = smoothstep(age * 0.75 - 0.25, age * 0.75 + 0.2, dense);
    let body = soft * mix(0.8 + 0.2 * dense, erode, smoothstep(0.08, 0.55, age));

    // the stream passes the chase camera: fade puffs that would smear across the lens
    let d = distance(view.world_position, in.world_position.xyz);
    let near = smoothstep(1.5, 9.0, d);

    return vec4<f32>(in.color.rgb * tint.rgb * body * near, 0.0);
}
