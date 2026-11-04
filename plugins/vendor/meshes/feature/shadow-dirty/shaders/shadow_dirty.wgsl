const MAX_FACES: u32 = 1536u;

/// Minimum world-space displacement (metres) that counts as a "move".
/// Set to ~0.1 mm — below floating point noise threshold at scene scale.
const MOVE_EPSILON: f32 = 0.0001;

// ── Structs ───────────────────────────────────────────────────────────────────

/// Must match GpuInstanceData in libhelio/src/instance.rs (224 bytes).
/// We only need the transform and bounds, so we read partial data.
struct GpuInstance {
    transform:    mat4x4f,  // model matrix, 64 bytes
    normal_mat_0: vec4f,    // 16 bytes
    normal_mat_1: vec4f,    // 16 bytes
    normal_mat_2: vec4f,    // 16 bytes
    bounds:       vec4f,    // xyz = world-space bounding sphere center, w = radius
    prev_model:   mat4x4f,  // 64 bytes
    mesh_id:      u32,
    material_id:  u32,
    flags:        u32,
    lightmap_index: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

/// Matches wgpu DrawIndexedIndirect layout (20 bytes).
struct DrawIndexedIndirect {
    index_count:    u32,
    instance_count: u32,
    first_index:    u32,
    base_vertex:    i32,
    first_instance: u32,
}

/// Must match GpuShadowMatrix in shadow_matrices.wgsl / libhelio (64 bytes).
struct GpuShadowMatrix {
    mat: mat4x4f,
    atlas: vec4f,
    policy: vec4u,
}

struct ShadowDirtyUniforms {
    /// Number of active shadow faces (= shadow_count from SceneResources).
    face_count: u32,
    /// Non-zero: track the pages a moved caster touches. Zero: all pages.
    pages: u32,
    _pad1: u32,
    _pad2: u32,
}

// ── Bindings ──────────────────────────────────────────────────────────────────

@group(0) @binding(0) var<storage, read>         instances:      array<GpuInstance>;
@group(0) @binding(1) var<storage, read>          movable_draws:  array<DrawIndexedIndirect>;
@group(0) @binding(2) var<storage, read_write>    prev_positions: array<Previous>;
@group(0) @binding(3) var<storage, read>          shadow_mats:    array<GpuShadowMatrix>;
/// Per-face dirty flag (0 = clean, 1 = dirty). Also used as clear-draw count by ShadowPass.
@group(0) @binding(4) var<storage, read_write>    face_dirty:     array<atomic<u32>>;
/// Per-face draw counts that drive multi_draw_indexed_indirect_count, in three
/// MAX_FACES regions: movable, static and transmissive casters. 0 = clean face
/// (no draws); a dirty face gets the list's live count (draw every caster).
/// ShadowPass draws a face it does not know to be dirty with these counts, so
/// a face nothing dirtied runs no vertices at all.
@group(0) @binding(5) var<storage, read_write>    face_geom_count: array<atomic<u32>>;
@group(0) @binding(6) var<uniform>                uniforms:       ShadowDirtyUniforms;
/// Per-caster flags written by ShadowMatrixPass when a light matrix changes.
@group(0) @binding(7) var<storage, read_write>     light_dirty:    array<atomic<u32>>;

struct Previous { bounds:vec4f, hash:vec4u }
@group(0) @binding(8) var<storage,read> spaces:array<mat4x4f>;
/// Object Batch's GPU counts, written this frame (`ObjectBatchFrameData::
/// draw_counts_gpu`); word 2 is the movable shadow-caster count.
@group(0) @binding(9) var<storage,read> batch_counts:array<u32>;
/// The movable count the last dispatch saw. Only invocation 0 touches it.
@group(0) @binding(10) var<storage,read_write> last_movable_count:array<u32>;

/// Per-face 8x8 page masks of the dynamic layer, two words per face (rows 0-3
/// then rows 4-7; bit `row * 8 + column`, rows counted up from the bottom of
/// clip space). `ShadowPass` clears a face's words once it has redrawn it.
@group(0) @binding(11) var<storage, read_write> face_pages: array<atomic<u32>>;

/// Live draw calls in `movable_draws` this frame.
fn movable_count()->u32 { return batch_counts[2]; }
/// Live static and transmissive shadow-caster draws this frame.
fn static_count()->u32 { return batch_counts[1]; }
fn transmissive_count()->u32 { return batch_counts[3]; }

// ── Frustum helpers (Gribb-Hartmann) ─────────────────────────────────────────

/// Extract 6 view-frustum half-space planes from a VP matrix (column-major WGSL mat4x4f).
///
/// Each plane is vec4f(nx, ny, nz, d) where the signed distance of a point P from the
/// plane is:  dot(normal, P) + d.  A positive value means "inside" the frustum.
/// The direction convention is: planes[i].xyz points INWARD into the frustum.
fn extract_frustum_planes(m: mat4x4f) -> array<vec4f, 6> {
    // WGSL mat4x4f: m[col][row], i.e. m[c].r accesses column c, row r.
    // We need the rows of the matrix for Gribb-Hartmann:
    let r0 = vec4f(m[0][0], m[1][0], m[2][0], m[3][0]);
    let r1 = vec4f(m[0][1], m[1][1], m[2][1], m[3][1]);
    let r2 = vec4f(m[0][2], m[1][2], m[2][2], m[3][2]);
    let r3 = vec4f(m[0][3], m[1][3], m[2][3], m[3][3]);

    var planes: array<vec4f, 6>;
    planes[0] = r3 + r0;   // Left   plane
    planes[1] = r3 - r0;   // Right  plane
    planes[2] = r3 + r1;   // Bottom plane
    planes[3] = r3 - r1;   // Top    plane
    planes[4] = r2;         // Near   plane  (depth [0,1] convention)
    planes[5] = r3 - r2;   // Far    plane
    return planes;
}

/// Returns true if the sphere (center, radius) intersects or is inside all 6 planes.
fn sphere_vs_frustum(center: vec3f, radius: f32, planes: array<vec4f, 6>) -> bool {
    for (var i = 0u; i < 6u; i++) {
        let p = planes[i];
        // Signed distance from center to plane (positive = inside half-space).
        let dist = dot(p.xyz, center) + p.w;
        if dist < -radius*length(p.xyz) {
            return false;  // entirely outside this plane → not in frustum
        }
    }
    return true;
}

// ── Main ──────────────────────────────────────────────────────────────────────


fn hash_matrix(m:mat4x4f,h:u32)->u32 {
    var result=h;
    for(var c=0u;c<4u;c++){for(var r=0u;r<4u;r++){result=(result^bitcast<u32>(m[c][r]))*16777619u;}}
    return result;
}
const PAGE_GRID: u32 = 8u;
const ALL_PAGES: vec2u = vec2u(0xffffffffu, 0xffffffffu);

/// The NDC interval of `v / w` for `v` in [vlo, vhi] and `w` in [wlo, whi],
/// `wlo > 0`.
fn ratio_range(vlo: f32, vhi: f32, wlo: f32, whi: f32) -> vec2f {
    let lo = select(vlo / wlo, vlo / whi, vlo >= 0.0);
    let hi = select(vhi / whi, vhi / wlo, vhi >= 0.0);
    return vec2f(lo, hi);
}

/// Conservative page mask of the world-space sphere (`center`, `radius`) in
/// the clip space `m`. The sphere's clip-space extent is bounded by the
/// radius times the length of each row of `m`, so the NDC rectangle always
/// contains it. A sphere that may reach the camera plane covers everything.
/// Keep in step with `sphere_pages` in `helio-pass-shadow/shaders/shadow.wgsl`.
fn sphere_pages(m: mat4x4f, center: vec3f, radius: f32) -> vec2u {
    let c = m * vec4f(center, 1.0);
    let row0 = vec3f(m[0][0], m[1][0], m[2][0]);
    let row1 = vec3f(m[0][1], m[1][1], m[2][1]);
    let row3 = vec3f(m[0][3], m[1][3], m[2][3]);
    let ew = radius * length(row3);
    let wlo = c.w - ew;
    if !(wlo > 1e-4) { return ALL_PAGES; }
    let whi = c.w + ew;
    let ex = radius * length(row0);
    let ey = radius * length(row1);
    let xr = ratio_range(c.x - ex, c.x + ex, wlo, whi);
    let yr = ratio_range(c.y - ey, c.y + ey, wlo, whi);
    // Pad by a fraction of a page for rounding at the edges.
    let pad = 0.01;
    if xr.y < -1.0 - pad || xr.x > 1.0 + pad || yr.y < -1.0 - pad || yr.x > 1.0 + pad {
        return vec2u(0u);
    }
    let g = f32(PAGE_GRID);
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

/// `face_dirty` bits: the layer of the shadow atlas that must be redrawn.
const DIRTY_DYNAMIC: u32 = 1u;
const DIRTY_STATIC: u32 = 2u;
/// A movable caster moved, appeared or vanished: only the dynamic layer holds
/// movable casters, so the cached static layer (and glass) is left alone.
fn dirty_dynamic_pages(face:u32, pages:vec2u) {
    atomicOr(&face_dirty[face],DIRTY_DYNAMIC);
    atomicStore(&face_geom_count[face],movable_count());
    atomicOr(&face_pages[face*2u],pages.x);
    atomicOr(&face_pages[face*2u+1u],pages.y);
}
fn dirty_dynamic(face:u32) { dirty_dynamic_pages(face,ALL_PAGES); }
/// The light itself changed: both layers are drawn from its matrix.
fn dirty_all(face:u32) {
    atomicOr(&face_dirty[face],DIRTY_DYNAMIC|DIRTY_STATIC);
    atomicStore(&face_geom_count[face],movable_count());
    atomicOr(&face_pages[face*2u],ALL_PAGES.x);
    atomicOr(&face_pages[face*2u+1u],ALL_PAGES.y);
    atomicStore(&face_geom_count[MAX_FACES+face],static_count());
    atomicStore(&face_geom_count[2u*MAX_FACES+face],transmissive_count());
}

/// A movable draw whose instances changed this frame: the bounds it has now
/// and the ones it had, kept for `faces` to test against every face's frustum.
struct Changed { bounds: vec4f, previous: vec4f, has_previous: vec4u }
@group(0) @binding(12) var<storage, read_write> changed: array<Changed>;
@group(0) @binding(13) var<storage, read_write> changed_count: array<atomic<u32>>;

fn face_total() -> u32 { return min(uniforms.face_count, arrayLength(&shadow_mats)); }

// The work is four small dispatches in this order, so no thread loops over
// every face or every caster alone:
//   casters -> detect -> faces -> finish

/// One thread per light caster (six faces): a moved light dirties both layers
/// of its faces; a change in the movable caster count dirties every face's
/// dynamic layer (a caster added or removed shifts every later draw's slot,
/// and a removed one's silhouette must be erased).
@compute @workgroup_size(64)
fn casters(@builtin(global_invocation_id) gid:vec3u) {
    let caster=gid.x;let count=face_total();
    if caster*6u>=count {return;}
    let force_dirty_all=movable_count()!=last_movable_count[0];
    let moved=atomicExchange(&light_dirty[caster],0u)!=0u;
    for(var f=caster*6u;f<min(caster*6u+6u,count);f++) {
        if moved {dirty_all(f);} else if force_dirty_all {dirty_dynamic(f);}
    }
}

/// One thread per movable draw: hash its instances and, if they changed, add
/// it to the changed list.
@compute @workgroup_size(64)
fn detect(@builtin(global_invocation_id) gid:vec3u) {
    let tid=gid.x;
    if tid>=min(movable_count(),arrayLength(&movable_draws)) || tid>=arrayLength(&prev_positions) {return;}
    let draw=movable_draws[tid];var hash=draw.instance_count^draw.index_count^draw.first_instance;
    var lo=vec3f(3e38);var hi=vec3f(-3e38);
    for(var i=0u;i<draw.instance_count;i++) {
        let inst=instances[draw.first_instance+i];
        let space=spaces[(inst.flags>>8u)&255u];
        hash=hash_matrix(space*inst.transform,hash);
        hash=(hash^inst.flags^inst.mesh_id^inst.material_id)*16777619u;
        // Bounds are world-relative to the instance's coordinate space.
        let center=(space*vec4f(inst.bounds.xyz,1)).xyz;
        let radius=inst.bounds.w*max(length(space[0].xyz),max(length(space[1].xyz),length(space[2].xyz)));
        lo=min(lo,center-vec3f(radius));hi=max(hi,center+vec3f(radius));
    }
    var bounds=vec4f(0);
    if draw.instance_count>0u {bounds=vec4f((lo+hi)*0.5,length(hi-lo)*0.5);}
    let previous=prev_positions[tid];
    prev_positions[tid]=Previous(bounds,vec4u(hash,0,0,0));
    if previous.hash.x==hash && all(previous.bounds==bounds) {return;}
    let slot=atomicAdd(&changed_count[0],1u);
    if slot<arrayLength(&changed) {
        let has_previous=select(0u,1u,previous.bounds.w>0.0 || previous.hash.x!=0u);
        changed[slot]=Changed(bounds,previous.bounds,vec4u(has_previous,0u,0u,0u));
    }
}

/// One thread per face: keep a face that was dirtied earlier and deferred by
/// the shadow budget drawing to this frame's counts, then test every changed
/// draw against the face's frustum. Test both positions: moving OUT of a tile
/// must erase the old silhouette.
@compute @workgroup_size(64)
fn faces(@builtin(global_invocation_id) gid:vec3u) {
    let f=gid.x;
    if f>=face_total() {return;}
    let pending=atomicLoad(&face_dirty[f]);
    if (pending&DIRTY_DYNAMIC)!=0u {
        atomicStore(&face_geom_count[f],movable_count());
    }
    if (pending&DIRTY_STATIC)!=0u {
        atomicStore(&face_geom_count[MAX_FACES+f],static_count());
        atomicStore(&face_geom_count[2u*MAX_FACES+f],transmissive_count());
    }
    let n=min(atomicLoad(&changed_count[0]),arrayLength(&changed));
    if n==0u || shadow_mats[f].atlas.z==0.0 {return;}
    let m=shadow_mats[f].mat;
    let planes=extract_frustum_planes(m);
    var pages=vec2u(0u);
    var touched=false;
    for(var i=0u;i<n;i++) {
        let c=changed[i];
        let has_previous=c.has_previous.x!=0u;
        let now=sphere_vs_frustum(c.bounds.xyz,c.bounds.w,planes);
        let before=has_previous && sphere_vs_frustum(c.previous.xyz,c.previous.w,planes);
        if !(now || before) {continue;}
        touched=true;
        // The pages both the old and the new bounds touch: moving out of a
        // page must erase the old silhouette there. Anything else in the face
        // keeps its cached depth.
        if uniforms.pages!=0u {
            pages|=sphere_pages(m,c.bounds.xyz,c.bounds.w);
            if has_previous {pages|=sphere_pages(m,c.previous.xyz,c.previous.w);}
        } else {
            pages=ALL_PAGES;
        }
    }
    if touched {dirty_dynamic_pages(f,pages);}
}

/// After every other thread has read the last count: remember this frame's.
@compute @workgroup_size(1)
fn finish() {
    last_movable_count[0]=movable_count();
}
