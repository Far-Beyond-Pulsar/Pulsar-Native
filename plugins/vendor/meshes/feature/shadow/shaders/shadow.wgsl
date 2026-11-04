struct ShadowFace { mat:mat4x4f, atlas:vec4f, policy:vec4u }
// Shadow caster pass — depth-only, GPU-driven.
//
// Vertex shader projects world-space geometry into each light's clip space using
// the pre-computed shadow matrix for this face.  There is no fragment stage —
// the rasteriser writes depth automatically.
//
// Design mirrors Unreal Engine 4 "Shadow Depth Pass" and Unity HDRP
// "Shadow Caster Pass": position-only transform, depth-write only,
// front-face culled to eliminate self-shadowing acne.

// ── Types ─────────────────────────────────────────────────────────────────────

// Per-instance world transform.  Must match GpuInstanceData in libhelio (224 bytes).
struct GpuInstanceData {
    transform:    mat4x4<f32>,   // offset   0
    normal_mat_0: vec4<f32>,     // offset  64  (unused in shadow pass)
    normal_mat_1: vec4<f32>,     // offset  80
    normal_mat_2: vec4<f32>,     // offset  96
    bounds:       vec4<f32>,     // offset 112
    prev_model:   mat4x4<f32>,   // offset 128  (unused in shadow pass)
    mesh_id:      u32,           // offset 192
    material_id:  u32,           // offset 196
    flags:        u32,           // offset 200
    _pad:         u32,           // offset 204
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

// Which shadow atlas face is being rendered this pass.
// Addressed via dynamic uniform buffer offset — one 16-byte slot per face.
struct FaceIndex {
    value: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

// ── Bindings ──────────────────────────────────────────────────────────────────

// Pre-computed light-space view-projection matrices; one per shadow atlas face.
@group(0) @binding(0) var<storage, read> shadow_matrices: array<ShadowFace>;
// Per-instance world transforms for the entire scene.
@group(0) @binding(1) var<storage, read> instances:       array<GpuInstanceData>;
// Current face selection, updated each pass via dynamic offset into a pre-written buffer.
@group(0) @binding(2) var<uniform>       face:            FaceIndex;
// Coordinate-space transforms (current frame only — shadows have no velocity
// buffer to feed, so the previous-frame copy isn't needed here). Slot 0 is
// identity; a sublevel member casts a real shadow at its placed position by
// going through this, no separate shadow path required.
@group(0) @binding(3) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;

@group(0) @binding(4) var<storage, read> face_dirty: array<u32>;
/// Per-face 8x8 page masks of the dynamic layer (two words per face; see
/// `face_pages` in `helio-pass-shadow-dirty/shaders/shadow_dirty.wgsl`).
@group(0) @binding(7) var<storage, read> face_pages: array<vec2<u32>>;

fn ratio_range(vlo: f32, vhi: f32, wlo: f32, whi: f32) -> vec2f {
    let lo = select(vlo / wlo, vlo / whi, vlo >= 0.0);
    let hi = select(vhi / whi, vhi / wlo, vhi >= 0.0);
    return vec2f(lo, hi);
}

/// Conservative page mask of a world-space sphere in a face's clip space.
/// Keep in step with `sphere_pages` in `shadow_dirty.wgsl`.
fn sphere_pages(m: mat4x4f, center: vec3f, radius: f32) -> vec2u {
    let c = m * vec4f(center, 1.0);
    let row0 = vec3f(m[0][0], m[1][0], m[2][0]);
    let row1 = vec3f(m[0][1], m[1][1], m[2][1]);
    let row3 = vec3f(m[0][3], m[1][3], m[2][3]);
    let ew = radius * length(row3);
    let wlo = c.w - ew;
    if !(wlo > 1e-4) { return vec2u(0xffffffffu, 0xffffffffu); }
    let whi = c.w + ew;
    let ex = radius * length(row0);
    let ey = radius * length(row1);
    let xr = ratio_range(c.x - ex, c.x + ex, wlo, whi);
    let yr = ratio_range(c.y - ey, c.y + ey, wlo, whi);
    let pad = 0.01;
    if xr.y < -1.0 - pad || xr.x > 1.0 + pad || yr.y < -1.0 - pad || yr.x > 1.0 + pad {
        return vec2u(0u);
    }
    let g = 8.0;
    let x0 = u32(clamp(floor((xr.x - pad) * 0.5 * g + 0.5 * g), 0.0, g - 1.0));
    let x1 = u32(clamp(floor((xr.y + pad) * 0.5 * g + 0.5 * g), 0.0, g - 1.0));
    let y0 = u32(clamp(floor((yr.x - pad) * 0.5 * g + 0.5 * g), 0.0, g - 1.0));
    let y1 = u32(clamp(floor((yr.y + pad) * 0.5 * g + 0.5 * g), 0.0, g - 1.0));
    let cols = ((1u << (x1 - x0 + 1u)) - 1u) << x0;
    var lo = 0u;
    var hi = 0u;
    for (var j = y0; j <= y1; j++) {
        if j < 4u { lo |= cols << (j * 8u); } else { hi |= cols << ((j - 4u) * 8u); }
    }
    return vec2u(lo, hi);
}
struct SkinVertex { joints: vec4<u32>, weights: vec4<f32> }
@group(0) @binding(5) var<storage, read> skin_vertices: array<SkinVertex>;
@group(0) @binding(6) var<storage, read> bone_palette: array<mat4x4<f32>>;

struct ShadowVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) material_id: u32,
}

// ── Vertex stage ──────────────────────────────────────────────────────────────

@vertex
fn vs_main(
    @location(0)             position: vec3<f32>,
    @location(1)             uv: vec2<f32>,
    @builtin(vertex_index)    vertex_index: u32,
    // Per-instance vertex data, offset by the indirect draw's `first_instance`
    // on every backend (`helio_pass_gbuffer::draw_slots`).
    @location(15)            slot:     u32,
) -> ShadowVertex {
    // `_pad0` is the dirty bit this layer is gated on (1 dynamic, 2 static); 0 draws unconditionally.
    if face._pad0 != 0u && (face_dirty[face.value] & face._pad0) == 0u { return ShadowVertex(vec4f(2,2,2,1), uv, 0u); }
    let inst  = instances[slot];
    var local_position = position;
    if inst.bone_palette_count > 0u {
        let skin = skin_vertices[vertex_index + inst.skin_vertex_offset];
        var blended = mat4x4<f32>(vec4f(0.0), vec4f(0.0), vec4f(0.0), vec4f(0.0));
        var total = 0.0;
        for (var i = 0u; i < 4u; i = i + 1u) {
            if skin.weights[i] > 0.0 && skin.joints[i] < inst.bone_palette_count {
                blended += bone_palette[inst.bone_palette_offset + skin.joints[i]] * skin.weights[i];
                total += skin.weights[i];
            }
        }
        if total > 0.00001 { local_position = (blended * (1.0 / total) * vec4f(position, 1.0)).xyz; }
    }
    let space = coordinate_spaces[(inst.flags >> 8u) & 0xFFu];
    if face._pad0 == 1u {
        // Redrawing only the dirty pages of the dynamic layer: a caster whose
        // bounds touch none of them has nothing to add, and the rest of its
        // pixels already hold its depth.
        let center = (space * vec4f(inst.bounds.xyz, 1.0)).xyz;
        let radius = inst.bounds.w * max(length(space[0].xyz), max(length(space[1].xyz), length(space[2].xyz)));
        let touched = sphere_pages(shadow_matrices[face.value].mat, center, radius) & face_pages[face.value];
        if touched.x == 0u && touched.y == 0u { return ShadowVertex(vec4f(2, 2, 2, 1), uv, inst.material_id); }
    }
    let world = space * (inst.transform * vec4<f32>(local_position, 1.0));
    return ShadowVertex(shadow_matrices[face.value].mat * world, uv, inst.material_id);
}

@fragment
fn fs_main(input: ShadowVertex, @builtin(front_facing) front: bool) {
    // Preserve the solid-mesh front-face policy, while both faces of a leaf
    // card cast the same alpha-tested silhouette.
    if front && (materials[input.material_id].flags & 1u) == 0u { discard; }
    reject_cutout(input.material_id, input.uv);
}
