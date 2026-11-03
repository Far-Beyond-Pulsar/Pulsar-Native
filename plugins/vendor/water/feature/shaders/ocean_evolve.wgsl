// Time-evolve the reference H0 spectrum and pack two real fields into the
// complex channels consumed by the paired Stockham IFFT outputs.
@group(0) @binding(0) var h0_texture: texture_2d<f32>;

struct EvolveUniform {
    lengths: vec4f,
    time: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}
@group(0) @binding(1) var<uniform> u: EvolveUniform;

const N: i32 = 256;
const PI: f32 = 3.14159265359;
const G: f32 = 9.81;
const ST: f32 = 7.4e-5;
const LOOP: f32 = 300.0;

fn cmul(a: vec2f, b: vec2f) -> vec2f {
    return vec2f(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn omega(k: f32) -> f32 {
    return sqrt(G * k + ST * k * k * k);
}

struct VertexOutput {
    @builtin(position) position: vec4f,
}

struct FragmentOutput {
    @location(0) field0: vec4f,
    @location(1) field1: vec4f,
}

@fragment
fn fs_main(@builtin(position) position: vec4f) -> FragmentOutput {
    let p = vec2i(position.xy);
    let cascade = u32(p.y / N);
    let local = vec2i(p.x, p.y - i32(cascade) * N);
    let k = vec2f(local - vec2i(N / 2)) * (2.0 * PI / u.lengths[cascade]);
    let kl = length(k);
    let ik = select(0.0, 1.0 / kl, kl > 1e-6);
    let w0 = 2.0 * PI / LOOP;
    // Quantization makes every phase an integer number of cycles per LOOP;
    // reducing the phase before sin/cos avoids fp32 drift in the capillary band.
    let phase = 2.0 * PI * fract(floor(omega(kl) / w0) * (u.time / LOOP));
    let e = vec2f(cos(phase), sin(phase));
    let h0 = textureLoad(h0_texture, p, 0);
    let h = cmul(h0.xy, e) + cmul(h0.zw, vec2f(e.x, -e.y));
    let ih = vec2f(-h.y, h.x);

    let dx = ih * k.x * ik;
    let dz = ih * k.y * ik;
    let dxdx = -h * k.x * k.x * ik;
    let dzdz = -h * k.y * k.y * ik;
    let dxdz = -h * k.x * k.y * ik;
    let dydx = ih * k.x;
    let dydz = ih * k.y;

    var out: FragmentOutput;
    // `Ifft(A + iB) = a + ib`; keep the pair packed through every stage.
    out.field0 = vec4f(dx.x - dz.y, dx.y + dz.x, h.x - dxdz.y, h.y + dxdz.x);
    out.field1 = vec4f(dydx.x - dydz.y, dydx.y + dydz.x, dxdx.x - dzdz.y, dxdx.y + dzdz.x);
    return out;
}
