// The clouds' noise volumes (`clouds_common.wgsl`), built once: each tiles
// (every lattice wraps at the volume's edge), so the layer repeats it
// seamlessly; the planet's weather (`cloud_planet_weather.wgsl`) and the
// lookups' incommensurate sizes keep the repeats from showing.
//
// - weather (64^3), the mesoscale's and the cumulus groups' field: r a
//   fractal noise, b cells (the distance to the nearest of a Worley's
//   points: 0 at the cells' centres, about 1 at their edges); g and a their
//   standard deviations under each texel (zero at the top level).
// - shape (128^3): r Schneider's billows (Perlin dilated by a Worley fbm,
//   remapped by it), g their standard deviation under each texel.
// - detail (32^3): r g b Worley fbm at three frequencies (the wisps).
//
// `downsample` builds each mip from the one above, as the mean of its
// texels; `downsample_spread` keeps the spread of the pairs (r, g) and
// (b, a) too, so a mip's footprint knows how much it hides.

@group(0) @binding(0) var noise_out: texture_storage_3d<rgba8unorm, write>;
@group(0) @binding(1) var noise_in: texture_3d<f32>;

fn pcg3d(v0: vec3<u32>) -> vec3<u32> {
    var v = v0 * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

// Uniform in [0, 1)^3 for lattice cell `cell` wrapped to `period`.
fn lattice_random(cell: vec3<i32>, period: i32, seed: u32) -> vec3<f32> {
    let wrapped = vec3<u32>(((cell % period) + period) % period);
    return vec3<f32>(pcg3d(wrapped + vec3<u32>(seed, seed * 7u, seed * 13u)) >> vec3<u32>(8u)) / 16777216.0;
}

// Gradient noise over `period` cells across the volume (p in cells), about
// -1..1.
fn perlin(p: vec3<f32>, period: i32, seed: u32) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    var corners: array<f32, 8>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        let g = normalize(lattice_random(i + o, period, seed) * 2.0 - 1.0 + vec3<f32>(1e-4));
        corners[k] = dot(g, f - vec3<f32>(o));
    }
    let x0 = mix(corners[0], corners[1], u.x);
    let x1 = mix(corners[2], corners[3], u.x);
    let x2 = mix(corners[4], corners[5], u.x);
    let x3 = mix(corners[6], corners[7], u.x);
    return mix(mix(x0, x1, u.y), mix(x2, x3, u.y), u.z) * 1.6;
}

// Distance to the nearest of one random point per cell (`period` cells
// across), in cells: 0 at the points, about 1 at most.
fn worley(p: vec3<f32>, period: i32, seed: u32) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = p - floor(p);
    var nearest = 1e9;
    for (var k = 0; k < 27; k++) {
        let o = vec3<i32>(k % 3 - 1, (k / 3) % 3 - 1, k / 9 - 1);
        let point = vec3<f32>(o) + lattice_random(i + o, period, seed);
        let d = point - f;
        nearest = min(nearest, dot(d, d));
    }
    return min(sqrt(nearest), 1.0);
}

// Billowy Worley fbm (1 at the points): three octaves from `cells` across.
fn worley_fbm(t: vec3<f32>, cells: i32, seed: u32) -> f32 {
    let a = worley(t * f32(cells), cells, seed);
    let b = worley(t * f32(cells * 2), cells * 2, seed + 1u);
    let c = worley(t * f32(cells * 4), cells * 4, seed + 2u);
    return 1.0 - (a * 0.625 + b * 0.25 + c * 0.125);
}

// Perlin fbm, `octaves` from `cells` across, mapped to about 0..1.
fn perlin_fbm(t: vec3<f32>, cells: i32, octaves: i32, seed: u32) -> f32 {
    var sum = 0.0;
    var amplitude = 0.5;
    var total = 0.0;
    var n = cells;
    for (var o = 0; o < octaves; o++) {
        sum += perlin(t * f32(n), n, seed + u32(o)) * amplitude;
        total += amplitude;
        amplitude *= 0.5;
        n *= 2;
    }
    return clamp(sum / total * 0.5 + 0.5, 0.0, 1.0);
}

fn texel_coordinate(id: vec3<u32>) -> vec3<f32> {
    return (vec3<f32>(id) + 0.5) / vec3<f32>(textureDimensions(noise_out));
}

@compute @workgroup_size(4, 4, 4)
fn weather_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= textureDimensions(noise_out)) { return; }
    let t = texel_coordinate(id);
    let field = perlin_fbm(t, 4, 4, 11u);
    let cells = worley(t * 4.0, 4, 23u) * 0.75 + worley(t * 8.0, 8, 29u) * 0.25;
    textureStore(noise_out, id, vec4<f32>(field, 0.0, cells, 0.0));
}

@compute @workgroup_size(4, 4, 4)
fn shape_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= textureDimensions(noise_out)) { return; }
    let t = texel_coordinate(id);
    let p = perlin_fbm(t, 4, 4, 3u);
    let w = worley_fbm(t, 4, 5u);
    // Perlin dilated by Worley: billows with connected, rounded cores,
    // remapped by Worley fbm at three frequencies (Schneider's base noise).
    let perlin_worley = clamp((p - (w - 1.0)) / (2.0 - w), 0.0, 1.0);
    let fine = w * 0.625 + worley_fbm(t, 8, 9u) * 0.25 + worley_fbm(t, 16, 13u) * 0.125;
    let billows = clamp((perlin_worley - (fine - 1.0)) / (2.0 - fine), 0.0, 1.0);
    textureStore(noise_out, id, vec4<f32>(billows, 0.0, 0.0, 0.0));
}

@compute @workgroup_size(4, 4, 4)
fn detail_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= textureDimensions(noise_out)) { return; }
    let t = texel_coordinate(id);
    textureStore(noise_out, id, vec4<f32>(worley_fbm(t, 2, 17u), worley_fbm(t, 4, 19u), worley_fbm(t, 8, 29u), 1.0));
}

// One mip from the level above (`noise_in`): the mean of its 2x2x2 texels.
@compute @workgroup_size(4, 4, 4)
fn downsample(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= textureDimensions(noise_out)) { return; }
    let size = vec3<i32>(textureDimensions(noise_in));
    var sum = vec4<f32>(0.0);
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        sum += textureLoad(noise_in, min(vec3<i32>(id) * 2 + o, size - 1), 0);
    }
    textureStore(noise_out, id, sum * 0.125);
}

// One mip from the level above, as `downsample`, keeping the pairs' spread:
// r and b the means, g and a the standard deviations of everything under
// the texel (the variances within the texels above plus the spread of their
// means).
@compute @workgroup_size(4, 4, 4)
fn downsample_spread(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id >= textureDimensions(noise_out)) { return; }
    let size = vec3<i32>(textureDimensions(noise_in));
    var mean = vec2<f32>(0.0);
    var square = vec2<f32>(0.0);
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        let t = textureLoad(noise_in, min(vec3<i32>(id) * 2 + o, size - 1), 0);
        mean += t.rb;
        square += t.rb * t.rb + t.ga * t.ga;
    }
    mean *= 0.125;
    let spread = sqrt(max(square * 0.125 - mean * mean, vec2<f32>(0.0)));
    textureStore(noise_out, id, vec4<f32>(mean.x, spread.x, mean.y, spread.y));
}
