// Fire, marched through as a glowing volume: the flames the arena's vents breathe, the grates
// that heat up under them, and the fireballs of explosions rolling up into smoke.
//
// Every shape is a box in one mesh (see `fire.rs`), and its vertices say what it is:
//   uv.x    the shape: 0 a vent's flames, 1 its grate (a plate, not a box), 2 a fireball
//   colour  flames and grate: the grate's corners (x0, z0, x1, z1); fireball: centre, radius
//   uv.y    flames: how hard they burn, 0..1; grate: its heat, 0..1; fireball: its age, 0..1
//   uv_b    flames: (how high they reach, seed); grate: (unused, seed); fireball: (heat, seed)
// Flames and fireballs are marched from where the view ray enters the shape to where it leaves:
// a density from noise carried up through the fire, a colour from its temperature, and smoke
// that dims what is behind it. Out comes premultiplied colour: the fire glows over the scene and
// the smoke darkens it.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::{view, globals};

// four tiling fields of gradient noise, one per channel
@group(3) @binding(0) var noise: texture_3d<f32>;
@group(3) @binding(1) var noise_s: sampler;

// how far a vent's box reaches past its grate on every side (`PAD` in fire.rs)
const PAD: f32 = 0.5;

fn tex(q: vec3<f32>) -> vec4<f32> {
    return textureSampleLevel(noise, noise_s, q, 0.0);
}

// `octaves` octaves of the noise, centred on 0 and stretched to about -1..1
fn fbm(q: vec3<f32>, octaves: i32) -> f32 {
    var s = 0.0;
    var a = 0.5;
    var x = q;
    for (var i = 0; i < octaves; i++) {
        s += a * tex(x).r;
        x = x * 2.03 + vec3<f32>(0.37, 0.11, 0.73);
        a *= 0.5;
    }
    return (s / (1.0 - 2.0 * a) - 0.5) * 4.0;
}

// The colour fire glows at a temperature from 0 (cold) to 1 (white-hot), in linear HDR: deep red
// embers, orange, yellow, and nearly white at the heart.
fn glow(t: f32) -> vec3<f32> {
    let s = clamp(t, 0.0, 1.0) * 4.0;
    let c1 = vec3<f32>(0.5, 0.035, 0.008);
    let c2 = vec3<f32>(2.0, 0.38, 0.03);
    let c3 = vec3<f32>(4.2, 1.8, 0.25);
    let c4 = vec3<f32>(7.0, 4.6, 1.6);
    if s < 1.0 {
        return c1 * s;
    }
    if s < 2.0 {
        return mix(c1, c2, s - 1.0);
    }
    if s < 3.0 {
        return mix(c2, c3, s - 2.0);
    }
    return mix(c3, c4, s - 3.0);
}

// Where the ray from ro along rd is inside the box lo..hi: (in, out); out < in if it misses.
fn box_span(ro: vec3<f32>, rd: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec2<f32> {
    let inv = 1.0 / rd;
    let a = (lo - ro) * inv;
    let b = (hi - ro) * inv;
    let n = min(a, b);
    let f = max(a, b);
    return vec2<f32>(max(max(n.x, n.y), n.z), min(min(f.x, f.y), f.z));
}

// Where the ray from ro along rd is inside the ball at c of radius r.
fn ball_span(ro: vec3<f32>, rd: vec3<f32>, c: vec3<f32>, r: f32) -> vec2<f32> {
    let oc = ro - c;
    let b = dot(oc, rd);
    let h = b * b - (dot(oc, oc) - r * r);
    if h <= 0.0 {
        return vec2<f32>(1.0, 0.0);
    }
    let s = sqrt(h);
    return vec2<f32>(-b - s, -b + s);
}

// A vent's flames at p: (density, temperature). Tongues of flame rise off the grate and lick
// sideways, thinning out and cooling to red up to the height they reach.
fn flames(p: vec3<f32>, r: vec4<f32>, burn: f32, high: f32, seed: f32, t: f32) -> vec2<f32> {
    let h = p.y / high;
    // the footprint: the grate, a little narrower as the flame rises
    let inset = min(min(p.x - r.x, r.z - p.x), min(p.z - r.y, r.w - p.z)) - h * 0.6;
    let foot = smoothstep(-0.3, 0.8, inset);
    if foot <= 0.0 || h >= 1.0 {
        return vec2<f32>(0.0);
    }
    let rise = vec3<f32>(0.0, t * 3.6, 0.0);
    // a slow, broad field bends the rising gas from side to side, and along the grate some
    // stretches flare up while others die back
    let slow = tex(p * vec3<f32>(0.045, 0.03, 0.045) - rise * 0.02 + seed) - 0.5;
    let bend = slow.gba;
    let surge = slow.r;
    let n = fbm((p - rise) * vec3<f32>(0.1, 0.045, 0.1) + bend * 0.5 + seed, 3);
    let f = 0.55 + n * 0.65 + surge * 0.7 - h * 1.05 - (1.0 - foot) * 0.8;
    let dens = smoothstep(0.0, 0.15, f) * burn;
    let temp = clamp(f + (1.0 - h) * 0.45 - 0.02, 0.0, 1.0);
    return vec2<f32>(dens, temp);
}

// A fireball at p: (density, temperature). A lumpy ball whose billows turn over as it rises; it
// goes up white-hot and cools through orange into smoke.
fn fireball(p: vec3<f32>, c: vec3<f32>, r: f32, age: f32, heat: f32, seed: f32) -> vec2<f32> {
    let rel = (p - c) / r;
    let d = length(rel);
    let n = fbm(rel * 0.3 + vec3<f32>(0.0, -age * 0.4, 0.0) + seed, 4);
    let shell = 1.0 - d * 1.15 + n * 0.4;
    let dens = smoothstep(0.0, 0.18, shell) * (1.0 - smoothstep(0.55, 1.0, age));
    let temp = heat * clamp(shell * 1.5 + n * 0.5, 0.0, 1.0) * exp(-age * 4.5);
    return vec2<f32>(dens, temp);
}

// A vent's grate at p, glowing with its heat: in patches, the hottest of them drifting slowly.
fn grate(p: vec3<f32>, heat: f32, seed: f32, t: f32) -> vec4<f32> {
    let n = fbm(vec3<f32>(p.x * 0.12, t * 0.08 + seed, p.z * 0.12), 3);
    let fine = tex(vec3<f32>(p.x * 0.4, t * 0.3, p.z * 0.4) + seed).a - 0.5;
    let g = heat * clamp(0.7 + n * 0.35 + fine * 0.5, 0.0, 1.2);
    let soot = vec3<f32>(0.012, 0.009, 0.007);
    return vec4<f32>(soot + glow(g * 0.75) * g * 0.5, 1.0);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let shape = in.uv.x;
    let k = in.color;
    let t = globals.time;
    if shape > 0.5 && shape < 1.5 {
        return grate(in.world_position.xyz, in.uv.y, in.uv_b.y, t);
    }
    let ball = shape > 1.5;
    let ro = view.world_position;
    let rd = normalize(in.world_position.xyz - ro);
    var span: vec2<f32>;
    var inside: bool;
    if ball {
        span = ball_span(ro, rd, k.xyz, k.w);
        inside = all(abs(ro - k.xyz) < vec3<f32>(k.w));
    } else {
        let lo = vec3<f32>(k.x - PAD, 0.0, k.y - PAD);
        let hi = vec3<f32>(k.z + PAD, in.uv_b.x, k.w + PAD);
        span = box_span(ro, rd, lo, hi);
        inside = all(ro > lo) && all(ro < hi);
    }
    // march each shape once: through its near faces from outside, its far faces from within
    if front == inside {
        discard;
    }
    let t0 = max(span.x, 0.0);
    let t1 = span.y;
    if t1 <= t0 {
        discard;
    }

    // steps of about a quarter of a metre through flames, a tenth of a fireball's radius
    let unit = select(0.25, k.w * 0.1, ball);
    let n = clamp(i32((t1 - t0) / unit), 8, 32);
    let dt = (t1 - t0) / f32(n);
    // every pixel starts its steps at its own offset, so that they don't show as bands
    let ign = fract(52.9829189 * fract(dot(in.position.xy, vec2<f32>(0.06711056, 0.00583715))));
    let jit = fract(ign + f32(globals.frame_count % 32u) * 0.618034);

    var col = vec3<f32>(0.0);
    var clear = 1.0;
    for (var i = 0; i < n; i++) {
        let p = ro + rd * (t0 + (f32(i) + jit) * dt);
        var s: vec2<f32>;
        // light given off and light blocked, per metre at full density; the smoke's own colour
        var shine: f32;
        var block: f32;
        var grey = vec3<f32>(0.0);
        if ball {
            let age = in.uv.y;
            s = fireball(p, k.xyz, k.w, age, in.uv_b.x, in.uv_b.y);
            // per radius, so that a fireball looks the same at any size
            shine = 2.2 / k.w;
            block = (0.5 + 3.5 * smoothstep(0.08, 0.45, age)) / k.w;
            // lit from above
            let lit = 0.55 + 0.45 * clamp((p.y - k.y) / k.w * 0.5 + 0.5, 0.0, 1.0);
            grey = vec3<f32>(0.11, 0.105, 0.1) * lit;
        } else {
            s = flames(p, k, in.uv.y, in.uv_b.x, in.uv_b.y, t);
            shine = 1.0;
            block = 1.8;
            // soot: where the flame has cooled it shows as smoke
            grey = vec3<f32>(0.05, 0.042, 0.036) * (1.0 - smoothstep(0.2, 0.5, s.y));
        }
        if s.x > 0.001 {
            let a = 1.0 - exp(-s.x * block * dt);
            col += clear * (glow(s.y) * s.x * shine * dt + grey * a);
            clear *= 1.0 - a;
            if clear < 0.01 {
                break;
            }
        }
    }
    if clear > 0.998 && max(col.r, max(col.g, col.b)) < 0.002 {
        discard;
    }
    return vec4<f32>(col, 1.0 - clear);
}
