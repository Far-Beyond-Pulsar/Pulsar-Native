// Bit-exact integer noise; mirror of src/noise.rs. Terrain programs build
// on these so CPU and GPU fields agree to the bit.
const NOISE_ONE: i32 = 65536; // noise unit (Q16)
// Bound on how fast `noise` changes: Q16 units per lattice unit, plus
// NOISE_ROUNDING (noise::NOISE_SLOPE, tested).
const NOISE_SLOPE: f32 = 262144.0;
const NOISE_ROUNDING: i32 = 16;

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    var v = (bitcast<u32>(x) * 0x8da6b343u) ^ (bitcast<u32>(y) * 0xd8163841u)
        ^ (bitcast<u32>(z) * 0xcb1ab31fu) ^ seed;
    v ^= v >> 16u;
    v *= 0x7feb352du;
    v ^= v >> 15u;
    v *= 0x846ca68bu;
    return v ^ (v >> 16u);
}

fn grad(hash: u32, x: i32, y: i32, z: i32) -> i32 {
    let h = hash & 15u;
    let u = select(y, x, h < 8u);
    let v = select(select(z, x, h == 12u || h == 14u), y, h < 4u);
    return select(-u, u, (h & 1u) == 0u) + select(-v, v, (h & 2u) == 0u);
}

fn mul16(a: i32, w: i32) -> i32 {
    return (a * (w >> 8u) + ((a * (w & 255)) >> 8u)) >> 8u;
}

fn fade(t: i32) -> i32 {
    let tu = bitcast<u32>(t);
    let t2 = (tu * tu) >> 16u;
    let t3 = (t2 * tu) >> 16u;
    let inner = 6 * i32(t2) - 15 * t + 10 * NOISE_ONE;
    return mul16(i32(t3), inner);
}

fn lerp_q16(a: i32, b: i32, w: i32) -> i32 {
    return a + mul16(b - a, w);
}

fn to_q12(v: i32, shift: u32) -> i32 {
    if shift >= 16u { return v >> (shift - 16u); }
    return v << (16u - shift);
}

fn noise(p: vec3<i32>, shift: u32, seed: u32) -> i32 {
    let mask = (1 << shift) - 1;
    let c = p >> vec3<u32>(shift);
    let f = vec3<i32>(to_q12(p.x & mask, shift), to_q12(p.y & mask, shift), to_q12(p.z & mask, shift));
    let w = vec3<i32>(fade(f.x), fade(f.y), fade(f.z));
    let g000 = grad(hash3(c.x, c.y, c.z, seed), f.x, f.y, f.z);
    let g100 = grad(hash3(c.x + 1, c.y, c.z, seed), f.x - NOISE_ONE, f.y, f.z);
    let g010 = grad(hash3(c.x, c.y + 1, c.z, seed), f.x, f.y - NOISE_ONE, f.z);
    let g110 = grad(hash3(c.x + 1, c.y + 1, c.z, seed), f.x - NOISE_ONE, f.y - NOISE_ONE, f.z);
    let g001 = grad(hash3(c.x, c.y, c.z + 1, seed), f.x, f.y, f.z - NOISE_ONE);
    let g101 = grad(hash3(c.x + 1, c.y, c.z + 1, seed), f.x - NOISE_ONE, f.y, f.z - NOISE_ONE);
    let g011 = grad(hash3(c.x, c.y + 1, c.z + 1, seed), f.x, f.y - NOISE_ONE, f.z - NOISE_ONE);
    let g111 = grad(hash3(c.x + 1, c.y + 1, c.z + 1, seed), f.x - NOISE_ONE, f.y - NOISE_ONE, f.z - NOISE_ONE);
    let x00 = lerp_q16(g000, g100, w.x);
    let x10 = lerp_q16(g010, g110, w.x);
    let x01 = lerp_q16(g001, g101, w.x);
    let x11 = lerp_q16(g011, g111, w.x);
    let y0 = lerp_q16(x00, x10, w.y);
    let y1 = lerp_q16(x01, x11, w.y);
    return clamp(lerp_q16(y0, y1, w.z), -NOISE_ONE, NOISE_ONE);
}

// The gradient codes (`grad`'s four hash bits) of the eight corners of
// lattice cell `c`, corner x + 2 y + 4 z at bit 4 (x + 2 y + 4 z): `noise_at`
// evaluates any point of the cell from them, so points sharing a cell hash
// its corners once.
fn noise_codes(c: vec3<i32>, seed: u32) -> u32 {
    var codes = 0u;
    for (var n = 0u; n < 8u; n++) {
        codes |= (hash3(c.x + i32(n & 1u), c.y + i32((n >> 1u) & 1u), c.z + i32(n >> 2u), seed) & 15u) << (4u * n);
    }
    return codes;
}

// `noise` at `p` from its lattice cell's `noise_codes` (the same bits).
fn noise_at(p: vec3<i32>, shift: u32, codes: u32) -> i32 {
    let mask = (1 << shift) - 1;
    let f = vec3<i32>(to_q12(p.x & mask, shift), to_q12(p.y & mask, shift), to_q12(p.z & mask, shift));
    let w = vec3<i32>(fade(f.x), fade(f.y), fade(f.z));
    let g000 = grad(codes, f.x, f.y, f.z);
    let g100 = grad(codes >> 4u, f.x - NOISE_ONE, f.y, f.z);
    let g010 = grad(codes >> 8u, f.x, f.y - NOISE_ONE, f.z);
    let g110 = grad(codes >> 12u, f.x - NOISE_ONE, f.y - NOISE_ONE, f.z);
    let g001 = grad(codes >> 16u, f.x, f.y, f.z - NOISE_ONE);
    let g101 = grad(codes >> 20u, f.x - NOISE_ONE, f.y, f.z - NOISE_ONE);
    let g011 = grad(codes >> 24u, f.x, f.y - NOISE_ONE, f.z - NOISE_ONE);
    let g111 = grad(codes >> 28u, f.x - NOISE_ONE, f.y - NOISE_ONE, f.z - NOISE_ONE);
    let x00 = lerp_q16(g000, g100, w.x);
    let x10 = lerp_q16(g010, g110, w.x);
    let x01 = lerp_q16(g001, g101, w.x);
    let x11 = lerp_q16(g011, g111, w.x);
    let y0 = lerp_q16(x00, x10, w.y);
    let y1 = lerp_q16(x01, x11, w.y);
    return clamp(lerp_q16(y0, y1, w.z), -NOISE_ONE, NOISE_ONE);
}

fn scale_q16(n: i32, amplitude: i32) -> i32 {
    return n * (amplitude >> 16u) + ((n * ((amplitude & 0xffff) >> 4u)) >> 12u);
}

fn div_floor(a: i32, b: i32) -> i32 {
    let q = a / b;
    return select(q, q - 1, (a % b != 0) && ((a < 0) != (b < 0)));
}

fn rem_floor(a: i32, b: i32) -> i32 {
    let r = a % b;
    return select(r, r + b, r < 0);
}

// Fine noise (Q24); mirror of `noise_fine` in src/noise.rs.
const FINE_ONE: i32 = 16777216;

// a * b / 2^24 rounded to nearest, |a| < 2^28, |b| <= 2^24: the exact
// product (one wide multiply with hardware 64-bit integers).
fn mul_fine(a: i32, b: i32) -> i32 {
    return mul_shr_signed(a, b, 24u);
}

fn fade_q24(t: i32) -> i32 {
    let t2 = mul_fine(t, t);
    let t3 = mul_fine(t2, t);
    return mul_fine(6 * t2 - 15 * t + 10 * FINE_ONE, t3);
}

fn fine_fraction(v: i32, shift: u32) -> i32 {
    if shift >= 24u { return v >> (shift - 24u); }
    return v << (24u - shift);
}

fn lerp_q24(a: i32, b: i32, w: i32) -> i32 {
    return a + mul_fine(b - a, w);
}

fn noise_fine(p: vec3<i32>, shift: u32, seed: u32) -> i32 {
    let mask = (1 << shift) - 1;
    let c = p >> vec3<u32>(shift);
    let f = vec3<i32>(fine_fraction(p.x & mask, shift), fine_fraction(p.y & mask, shift), fine_fraction(p.z & mask, shift));
    let w = vec3<i32>(fade_q24(f.x), fade_q24(f.y), fade_q24(f.z));
    let g000 = grad(hash3(c.x, c.y, c.z, seed), f.x, f.y, f.z);
    let g100 = grad(hash3(c.x + 1, c.y, c.z, seed), f.x - FINE_ONE, f.y, f.z);
    let g010 = grad(hash3(c.x, c.y + 1, c.z, seed), f.x, f.y - FINE_ONE, f.z);
    let g110 = grad(hash3(c.x + 1, c.y + 1, c.z, seed), f.x - FINE_ONE, f.y - FINE_ONE, f.z);
    let g001 = grad(hash3(c.x, c.y, c.z + 1, seed), f.x, f.y, f.z - FINE_ONE);
    let g101 = grad(hash3(c.x + 1, c.y, c.z + 1, seed), f.x - FINE_ONE, f.y, f.z - FINE_ONE);
    let g011 = grad(hash3(c.x, c.y + 1, c.z + 1, seed), f.x, f.y - FINE_ONE, f.z - FINE_ONE);
    let g111 = grad(hash3(c.x + 1, c.y + 1, c.z + 1, seed), f.x - FINE_ONE, f.y - FINE_ONE, f.z - FINE_ONE);
    let x00 = lerp_q24(g000, g100, w.x);
    let x10 = lerp_q24(g010, g110, w.x);
    let x01 = lerp_q24(g001, g101, w.x);
    let x11 = lerp_q24(g011, g111, w.x);
    let y0 = lerp_q24(x00, x10, w.y);
    let y1 = lerp_q24(x01, x11, w.y);
    return clamp(lerp_q24(y0, y1, w.z), -FINE_ONE, FINE_ONE);
}

// Fine noise and its gradient (FINE_ONE per lattice spacing per axis; zero
// where clamped); mirror of `noise_fine_grad`. `.x` is the value.
fn noise_fine_grad(p: vec3<i32>, shift: u32, seed: u32) -> vec4<i32> {
    let mask = (1 << shift) - 1;
    let c = p >> vec3<u32>(shift);
    let f = vec3<i32>(fine_fraction(p.x & mask, shift), fine_fraction(p.y & mask, shift), fine_fraction(p.z & mask, shift));
    let w = vec3<i32>(fade_q24(f.x), fade_q24(f.y), fade_q24(f.z));
    let u = vec3<i32>(FINE_ONE) - f;
    let dw = vec3<i32>(
        30 * mul_fine(mul_fine(f.x, f.x), mul_fine(u.x, u.x)),
        30 * mul_fine(mul_fine(f.y, f.y), mul_fine(u.y, u.y)),
        30 * mul_fine(mul_fine(f.z, f.z), mul_fine(u.z, u.z)));
    var v: array<i32, 8>;
    var gx: array<i32, 8>;
    var gy: array<i32, 8>;
    var gz: array<i32, 8>;
    for (var index = 0; index < 8; index++) {
        let d = vec3<i32>(index & 1, (index >> 1u) & 1, index >> 2u);
        let h = hash3(c.x + d.x, c.y + d.y, c.z + d.z, seed);
        v[index] = grad(h, f.x - d.x * FINE_ONE, f.y - d.y * FINE_ONE, f.z - d.z * FINE_ONE);
        gx[index] = grad(h, FINE_ONE, 0, 0);
        gy[index] = grad(h, 0, FINE_ONE, 0);
        gz[index] = grad(h, 0, 0, FINE_ONE);
    }
    let x00 = lerp_q24(v[0], v[1], w.x);
    let x10 = lerp_q24(v[2], v[3], w.x);
    let x01 = lerp_q24(v[4], v[5], w.x);
    let x11 = lerp_q24(v[6], v[7], w.x);
    let y0 = lerp_q24(x00, x10, w.y);
    let y1 = lerp_q24(x01, x11, w.y);
    let value = lerp_q24(y0, y1, w.z);
    if abs(value) > FINE_ONE { return vec4<i32>(clamp(value, -FINE_ONE, FINE_ONE), 0, 0, 0); }
    let along_x = lerp_q24(lerp_q24(v[1] - v[0], v[3] - v[2], w.y), lerp_q24(v[5] - v[4], v[7] - v[6], w.y), w.z);
    let along_y = lerp_q24(x10 - x00, x11 - x01, w.z);
    let along_z = y1 - y0;
    return vec4<i32>(value,
        mul_fine(along_x, dw.x) + trilerp_q24(gx, w),
        mul_fine(along_y, dw.y) + trilerp_q24(gy, w),
        mul_fine(along_z, dw.z) + trilerp_q24(gz, w));
}

fn trilerp_q24(k: array<i32, 8>, w: vec3<i32>) -> i32 {
    let y0 = lerp_q24(lerp_q24(k[0], k[1], w.x), lerp_q24(k[2], k[3], w.x), w.y);
    let y1 = lerp_q24(lerp_q24(k[4], k[5], w.x), lerp_q24(k[6], k[7], w.x), w.y);
    return lerp_q24(y0, y1, w.z);
}

// sin(2 pi t), t in Q16 turns (wrapping), Q16 (`noise::sin_turns`).
fn sin_turns(t: i32) -> i32 {
    let x = (t << 16u) >> 16u;
    let xn = x * 2;
    let a = xn >> 2u;
    let y = 4 * xn - 4 * ((a * abs(a)) >> 12u);
    let b = y >> 2u;
    return y + mul16(((b * abs(b)) >> 12u) - y, 14746);
}

// Exact 64-bit products and Q30 unit vectors from 32-bit operations
// (mirror of the same functions in src/noise.rs).
const Q30: u32 = 1073741824u;

// Wide products from 16-bit limbs, for devices without 64-bit integers.
// The engine replaces the region between the markers with `wide64.wgsl`
// (hardware 64-bit multiplies, the same bits) when the device has them:
// the fixed-point domain mapping and noise run on these (a sixth of
// shading's time emulated).
// wide:begin
// Exact 64-bit product of a and b as (low, high) words.
fn mul_wide(a: u32, b: u32) -> vec2<u32> {
    let a1 = a >> 16u;
    let a0 = a & 0xffffu;
    let b1 = b >> 16u;
    let b0 = b & 0xffffu;
    let mid = a1 * b0 + a0 * b1;
    let mid_carry = select(0u, 0x10000u, mid < a1 * b0);
    let lo0 = a0 * b0;
    let lo = lo0 + (mid << 16u);
    return vec2<u32>(lo, a1 * b1 + (mid >> 16u) + mid_carry + select(0u, 1u, lo < lo0));
}

// `(a * b) >> s` of the exact 64-bit product (`noise::mul_shr`). The
// caller keeps the result within 32 bits.
fn mul_shr(a: u32, b: u32, s: u32) -> u32 {
    let a1 = a >> 16u;
    let a0 = a & 0xffffu;
    let b1 = b >> 16u;
    let b0 = b & 0xffffu;
    let m1 = a1 * b0;
    let m2 = a0 * b1;
    let mid = m1 + m2;
    let mid_carry = select(0u, 0x10000u, mid < m1);
    let lo0 = a0 * b0;
    let lo = lo0 + (mid << 16u);
    let lo_carry = select(0u, 1u, lo < lo0);
    let hi = a1 * b1 + (mid >> 16u) + mid_carry + lo_carry;
    if s == 0u { return lo; }
    if s >= 32u { return hi >> (s - 32u); }
    return (lo >> s) | (hi << (32u - s));
}

// a b / 2^s rounded to nearest (halves up) of the exact signed 64-bit
// product (`noise::mul_shr_signed`): the unsigned product less each
// negative operand's correction in the high word, plus 2^(s - 1).
fn mul_shr_signed(a: i32, b: i32, s: u32) -> i32 {
    let p = mul_wide(bitcast<u32>(a), bitcast<u32>(b));
    var hi = p.y - select(0u, bitcast<u32>(b), a < 0) - select(0u, bitcast<u32>(a), b < 0);
    var lo = p.x;
    if s == 0u { return bitcast<i32>(lo); }
    if s <= 32u {
        let l = lo + (1u << (s - 1u));
        hi += select(0u, 1u, l < lo);
        lo = l;
    } else {
        hi += 1u << (s - 33u);
    }
    if s >= 32u { return bitcast<i32>(hi) >> (s - 32u); }
    return bitcast<i32>((lo >> s) | (hi << (32u - s)));
}
// wide:end

fn add_wide(a: vec2<u32>, b: vec2<u32>) -> vec2<u32> {
    let lo = a.x + b.x;
    return vec2<u32>(lo, a.y + b.y + select(0u, 1u, lo < a.x));
}

// 1 / sqrt(d) in Q30 for d in [1, 4) (`noise::rsqrt_wide`).
fn rsqrt_wide(d: u32) -> u32 {
    var y = 1342177280u - mul_shr(d, 204010946u, 30u);
    for (var step = 0u; step < 5u; step++) {
        let dy2 = mul_shr(d, mul_shr(y, y, 30u), 30u);
        y = mul_shr(y, 3u * Q30 - dy2, 31u);
    }
    return y;
}

// v / |v| in Q30 (`noise::unit_q30`).
fn unit_q30(v: vec3<i32>) -> vec3<i32> {
    var a = vec3<u32>(abs(v));
    let m = a.x | a.y | a.z;
    if m == 0u { return vec3<i32>(0); }
    let bits = 32u - countLeadingZeros(m);
    if bits > 30u { a = a >> vec3<u32>(bits - 30u); } else { a = a << vec3<u32>(30u - bits); }
    var s = mul_shr(a.x, a.x, 30u) + mul_shr(a.y, a.y, 30u) + mul_shr(a.z, a.z, 30u);
    let wide = s < Q30;
    if wide { s = s << 2u; }
    let r = rsqrt_wide(s);
    let w = select(0u, 1u, wide);
    let u = vec3<i32>(vec3<u32>(mul_shr(a.x, r, 30u), mul_shr(a.y, r, 30u), mul_shr(a.z, r, 30u)) << vec3<u32>(w));
    return select(u, -u, v < vec3<i32>(0));
}
