// Attribute decode (requires vg_decode.wgsl before it). Separate because
// `unpack2x16float` needs the float16-in-float32 shader capability, which
// position-only passes (raster tiers) should not depend on.

struct VgVertexAttr {
    normal: vec3<f32>,
    tangent: vec3<f32>,
    bitangent_sign: f32,
    uv0: vec2<f32>,
    uv1: vec2<f32>,
}

fn vgd_attr(cluster_slot: u32, p: VgdPos, v: u32) -> VgVertexAttr {
    let w = vgd_page_word(cluster_slot) + (p.attr_byte >> 2u) + v * (VGD_ATTR_STRIDE / 4u);
    let n = unpack4x8snorm(vg_page_pool[w]);
    let t = unpack4x8snorm(vg_page_pool[w + 1u]);
    var a: VgVertexAttr;
    a.normal = n.xyz;
    a.tangent = t.xyz;
    a.bitangent_sign = select(-1.0, 1.0, t.w >= 0.0);
    a.uv0 = unpack2x16float(vg_page_pool[w + 2u]);
    a.uv1 = unpack2x16float(vg_page_pool[w + 3u]);
    return a;
}
