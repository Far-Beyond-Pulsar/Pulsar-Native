// uv0 of a virtual-geometry cluster vertex, decoded without the shader-f16
// capability. Requires `helio_vg_format::DECODE_WGSL` (+ its page-pool binding).

// f16 bits -> f32.
fn vga_f16(h: u32) -> f32 {
    let e = (h >> 10u) & 31u;
    let m = h & 1023u;
    var v: f32;
    if e == 0u {
        v = f32(m) * 5.9604645e-8;
    } else if e == 31u {
        v = 65504.0;
    } else {
        v = bitcast<f32>(((e + 112u) << 23u) | (m << 13u));
    }
    return select(v, -v, ((h >> 15u) & 1u) != 0u);
}

// uv0 of cluster-local vertex `v` (attribute record word 2: f16x2).
fn vga_uv0(cluster_slot: u32, p: VgdPos, v: u32) -> vec2<f32> {
    let w = vgd_page_word(cluster_slot) + (p.attr_byte >> 2u) + v * (VGD_ATTR_STRIDE / 4u) + 2u;
    let bits = vg_page_pool[w];
    return vec2<f32>(vga_f16(bits & 0xffffu), vga_f16(bits >> 16u));
}
