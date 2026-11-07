/// Depth-clear vertex shader.
///
/// Draws the face's 8x8 pages as 64 instanced quads (six vertices each) at
/// depth = 1.0 (far plane), tiling the whole tile. Used by `ShadowPass` to
/// GPU-clear individual shadow atlas faces before re-rendering movable-object
/// shadow geometry onto them. When the layer is gated on the dynamic dirty bit
/// (`face.dynamic == 1`) only the pages in the face's page mask are cleared,
/// so the rest of the face keeps its cached depth; otherwise every page is.
///
/// Pipeline state:
///   - DepthCompare: Always  →  overwrites whatever is in the depth buffer.
///   - DepthWrite:   true
///   - Fragment:     none    →  depth write happens automatically.
///   - CullMode:     none    →  the quads are always front-facing.

struct Face { value:u32, dynamic:u32, pad0:u32, pad1:u32 }
@group(0) @binding(2) var<uniform> face:Face;
@group(0) @binding(4) var<storage,read> dirty:array<u32>;
/// Per-face 8x8 page masks of the dynamic layer (two words per face).
@group(0) @binding(7) var<storage,read> pages:array<vec2<u32>>;

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, @builtin(instance_index) page: u32) -> @builtin(position) vec4<f32> {
    // `dynamic` is the dirty bit this layer is gated on (1 dynamic, 2 static); 0 clears unconditionally.
    if face.dynamic != 0u && (dirty[face.value] & face.dynamic) == 0u { return vec4f(2,2,2,1); }
    if face.dynamic == 1u {
        let mask = pages[face.value];
        let word = select(mask.y, mask.x, page < 32u);
        if ((word >> (page & 31u)) & 1u) == 0u { return vec4f(2,2,2,1); }
    }
    // Page `page` is column `page % 8`, row `page / 8` (rows count up from the
    // bottom of clip space). Two triangles per page.
    var corners = array<vec2f, 6>(
        vec2f(0, 0), vec2f(1, 0), vec2f(0, 1),
        vec2f(0, 1), vec2f(1, 0), vec2f(1, 1),
    );
    let cell = vec2f(f32(page % 8u), f32(page / 8u));
    let ndc = (cell + corners[vid]) * 0.25 - vec2f(1.0);
    return vec4f(ndc, 1.0, 1.0);
}
