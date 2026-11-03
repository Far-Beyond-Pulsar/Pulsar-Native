// Stockham radix-2 inverse FFT stage, matching Ultra-Water's FFT_FRAG.
@group(0) @binding(0) var input0: texture_2d<f32>;
@group(0) @binding(1) var input1: texture_2d<f32>;

struct FftUniform {
    sub: u32,
    vertical: u32,
    _pad: vec2u,
}
@group(0) @binding(2) var<uniform> u: FftUniform;

const N: i32 = 256;
const PI: f32 = 3.14159265359;

fn cmul(a: vec2f, b: vec2f) -> vec2f {
    return vec2f(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

struct FragmentOutput {
    @location(0) field0: vec4f,
    @location(1) field1: vec4f,
}

@fragment
fn fs_main(@builtin(position) position: vec4f) -> FragmentOutput {
    let p = vec2i(position.xy);
    let i = select(p.x, p.y % N, u.vertical == 1u);
    let half_sub = i32(u.sub >> 1u);
    let e = (i / i32(u.sub)) * half_sub + (i & (half_sub - 1));
    var pe = vec2i(0);
    var po = vec2i(0);
    if u.vertical == 1u {
        let base = p.y - i;
        pe = vec2i(p.x, base + e);
        po = vec2i(p.x, base + e + N / 2);
    } else {
        pe = vec2i(e, p.y);
        po = vec2i(e + N / 2, p.y);
    }
    let angle = 2.0 * PI * f32(i & (i32(u.sub) - 1)) / f32(u.sub);
    let twiddle = vec2f(cos(angle), sin(angle));
    let even0 = textureLoad(input0, pe, 0);
    let odd0 = textureLoad(input0, po, 0);
    let even1 = textureLoad(input1, pe, 0);
    let odd1 = textureLoad(input1, po, 0);
    var out: FragmentOutput;
    out.field0 = even0 + vec4f(cmul(twiddle, odd0.xy), cmul(twiddle, odd0.zw));
    out.field1 = even1 + vec4f(cmul(twiddle, odd1.xy), cmul(twiddle, odd1.zw));
    return out;
}
