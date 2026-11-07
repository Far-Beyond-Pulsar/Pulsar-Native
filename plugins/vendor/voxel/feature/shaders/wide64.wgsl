// Wide products with hardware 64-bit integers (`wgpu::Features::SHADER_INT64`):
// the same bits as the 16-bit limb versions in noise.wgsl, which they
// replace (`engine::source`).
fn mul_wide(a: u32, b: u32) -> vec2<u32> {
    let p = u64(a) * u64(b);
    return vec2<u32>(u32(p & 0xfffffffflu), u32(p >> 32u));
}

fn mul_shr(a: u32, b: u32, s: u32) -> u32 {
    return u32(((u64(a) * u64(b)) >> s) & 0xfffffffflu);
}

fn mul_shr_signed(a: i32, b: i32, s: u32) -> i32 {
    return i32((i64(a) * i64(b) + i64((u64(1) << s) >> 1u)) >> s);
}
