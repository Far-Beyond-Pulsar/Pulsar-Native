// Ultra-Water's Tessendorf spectrum initialization, translated from GLSL.
// A four-cascade 256² atlas is initialized in one draw with stable PCG noise.

struct SpectrumUniform {
    spec_a: array<vec4f, 2>, // scale, direction, spreading blend, swell
    spec_b: array<vec4f, 2>, // alpha, peak omega, gamma, short-wave fade
    lengths: vec4f,
    cuts: vec4f,
    capillary_fade: f32,
    seed: u32,
    _pad: vec2u,
}
@group(0) @binding(0) var<uniform> u: SpectrumUniform;

const N: i32 = 256;
const PI: f32 = 3.14159265359;
const G: f32 = 9.81;
const ST: f32 = 7.4e-5;

fn pcg2d(input: vec2u) -> vec2u {
    var v = input * 1664525u + vec2u(1013904223u);
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> vec2u(16u));
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> vec2u(16u));
    return v;
}

fn gaussian(id: vec2i, cascade: u32) -> vec2f {
    let h = pcg2d(vec2u(id) + vec2u(cascade * 7919u, cascade * 104729u) + vec2u(u.seed));
    let u1 = (f32(h.x) + 1.0) / 4294967296.0;
    let u2 = f32(h.y) / 4294967296.0;
    let r = sqrt(-2.0 * log(u1));
    return r * vec2f(cos(2.0 * PI * u2), sin(2.0 * PI * u2));
}

fn omega(k: f32) -> f32 {
    return sqrt(G * k + ST * k * k * k);
}

fn jonswap(w: f32, a: vec4f, b: vec4f) -> f32 {
    let wp = b.y;
    let sigma = select(0.09, 0.07, w <= wp);
    let r = exp(-(w - wp) * (w - wp) / (2.0 * sigma * sigma * wp * wp));
    let iw = 1.0 / w;
    let q = wp * iw;
    return a.x * b.x * G * G * pow(iw, 5.0) * exp(-1.25 * q * q * q * q) * pow(b.z, r);
}

fn normalisation(s: f32) -> f32 {
    let s2 = s * s;
    let s3 = s2 * s;
    let s4 = s3 * s;
    if s < 5.0 {
        return -0.000564 * s4 + 0.00776 * s3 - 0.044 * s2 + 0.192 * s + 0.163;
    }
    return -4.80e-08 * s4 + 1.07e-05 * s3 - 9.53e-04 * s2 + 5.90e-02 * s + 3.93e-01;
}

fn directional(theta: f32, w: f32, a: vec4f, b: vec4f) -> f32 {
    let x = w / b.y;
    var s = 0.0;
    if x > 1.0 {
        s = 9.77 * pow(x, -2.5);
    } else {
        s = 6.97 * pow(x, 5.0);
    }
    s += 16.0 * tanh(min(x, 20.0)) * a.w * a.w;
    let t = theta - a.y;
    let cosine_2s = normalisation(s) * pow(abs(cos(0.5 * t)), 2.0 * s);
    return mix(2.0 / PI * cos(t) * cos(t), cosine_2s, a.z);
}

fn h0(id: vec2i, cascade: u32) -> vec2f {
    let patch_length = u.lengths[cascade];
    let dk = 2.0 * PI / patch_length;
    let k = vec2f(id - vec2i(N / 2)) * dk;
    let kl = length(k);
    var lo = 1e-4;
    if cascade > 0u {
        lo = u.cuts[cascade - 1u];
    }
    var hi = 1e4;
    if cascade < 3u {
        hi = u.cuts[cascade];
    }
    if kl < lo || kl > hi || kl <= 1e-12 {
        return vec2f(0.0);
    }
    let w = omega(kl);
    let dwdk = (G + 3.0 * ST * kl * kl) / (2.0 * w);
    let theta = atan2(k.y, k.x);
    var spectral_density = 0.0;
    for (var i = 0u; i < 2u; i += 1u) {
        let a = u.spec_a[i];
        let b = u.spec_b[i];
        if a.x <= 0.0 {
            continue;
        }
        let fade = select(b.w, u.capillary_fade, cascade == 3u);
        spectral_density += jonswap(w, a, b) * directional(theta, w, a, b) * exp(-fade * fade * kl * kl);
    }
    return gaussian(id, cascade) * sqrt(2.0 * spectral_density * dwdk / kl * dk * dk);
}

struct VertexOutput {
    @builtin(position) position: vec4f,
}

@fragment
fn fs_main(@builtin(position) position: vec4f) -> @location(0) vec4f {
    let p = vec2i(position.xy);
    let cascade = u32(p.y / N);
    let id = vec2i(p.x, p.y - i32(cascade) * N);
    let a = h0(id, cascade);
    let b = h0((vec2i(N) - id) % N, cascade);
    return vec4f(a, b.x, -b.y);
}
