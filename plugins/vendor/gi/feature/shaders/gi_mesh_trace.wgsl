//!use gi_common
//!use sun_occlusion
//!use sun_transmittance
enable wgpu_ray_query;
// The scene's meshes as a tracer of the probes' rays (README.md, "Tracers"),
// after the others: a ray queries the TLAS up to the hit already written
// (the voxel terrain's) and keeps the nearer. A mesh hit is shaded from its
// instance's surface (`RayInstance`: the material's colours, the mesh's
// normals; a proxy's face, see `RAY_INSTANCE_PROXY`). Thin transmitting
// sheets (glass: non-opaque in their BLAS) are passed by probe rays and
// tint shadow rays. Cut-outs (`RAY_INSTANCE_MASKED`: alpha-tested
// materials, non-opaque too) stop a ray only where their alpha says the
// surface is (`cut_out_surface`), and are shaded as any other mesh hit.
// Whichever hit stands, if it
// faces the ray, takes the sun (a shadow ray through the meshes, times the
// terrain's own visibility: the voxel tracer's, or the sun's occlusion
// maps for a mesh hit) and one of the scene's point and spot lights, chosen
// by its unshadowed light (weighted reservoir), with a shadow ray: their
// light off the surface, over the light's chance, into `gi_light` (the
// hit's sun byte then 0: its sun is there). `gi_gather_lights` lists those lights first, once a
// frame (`local_lights`), so hits look through the live ones only.

@group(0) @binding(0) var<uniform> frame_gi: GiFrame;
@group(0) @binding(1) var<storage, read> gi_rays: array<GiRay>;
@group(0) @binding(2) var<storage, read_write> gi_hits: array<GiHit>;
@group(0) @binding(3) var<storage, read> gi_sun: array<vec4<f32>, 2>;
@group(0) @binding(4) var<storage, read_write> gi_light: array<vec2<u32>>;
@group(0) @binding(5) var<uniform> trace: MeshTrace;
@group(0) @binding(6) var acc_struct: acceleration_structure;
@group(0) @binding(7) var<storage, read> instances: array<RayInstance>;
// `PackedVertex` rows (10 words: position at 0, normal snorm8x4 at 8) and
// mesh-local indices: the scene's mesh pools.
@group(0) @binding(8) var<storage, read> vertices: array<u32>;
@group(0) @binding(9) var<storage, read> indices: array<u32>;
@group(0) @binding(10) var<storage, read> scene_lights: array<SceneLight>;
@group(0) @binding(11) var<uniform> sun_occlusion_frame: SunOcclusionFrame;
@group(0) @binding(12) var sun_occlusion_maps: texture_2d<f32>;
@group(0) @binding(18) var<storage, read> sun_occlusion_pages: array<u32>;
// The live point and spot lights (`gi_gather_lights`): their count, then
// theirs.
@group(0) @binding(13) var<storage, read_write> local_lights: LocalLights;
// The proxy pool (`helio_core::RayProxies`): vertices (a position, then a
// uv0: 5 floats) and mesh-local indices of the instances flagged
// `RAY_INSTANCE_PROXY`.
@group(0) @binding(14) var<storage, read> proxy_vertices: array<f32>;
@group(0) @binding(15) var<storage, read> proxy_indices: array<u32>;
// The sun's transmittance through the media above (the frame's
// `sun_transmittance`: the clouds') at a hit.
@group(0) @binding(16) var sun_transmittance_map: texture_2d<f32>;
@group(0) @binding(17) var<uniform> sun_transmittance_frame: SunTransmittanceFrame;
// Group 1: the scene's material rows and texture table, for cut-outs'
// alpha (`helio_mats::vg_alpha`, spliced in before this file).

struct MeshTrace {
    // The eye in the TLAS's frame (the renderer's origin).
    eye: vec4<f32>,
    // x: the scene's light rows, y: this frame's seed.
    info: vec4<u32>,
}

const MAX_LIGHTS: u32 = 256u;

// A point or spot light: position, range; direction (spot), outer cosine;
// radiant colour (colour times intensity), type; inner cosine.
struct LocalLight {
    position_range: vec4<f32>,
    direction_outer: vec4<f32>,
    color_type: vec4<f32>,
    inner: vec4<f32>,
}

struct LocalLights {
    count: vec4<u32>,
    lights: array<LocalLight, MAX_LIGHTS>,
}

// `helio_core::RayInstance`.
struct RayInstance {
    first_index: u32,
    base_vertex: u32,
    flags: u32,
    material: u32,
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    transmission: vec4<f32>,
}

// Helio's lights (`GpuLight`, 128 bytes).
struct SceneLight {
    position_range: vec4<f32>,
    direction_outer: vec4<f32>,
    color_intensity: vec4<f32>,
    shadow_index: u32,
    light_type: u32,
    inner_angle: f32,
    flags: u32,
    rest: array<vec4<u32>, 4>,
}

const PI: f32 = 3.14159265;
const VERTEX_WORDS: u32 = 10u;
// `helio_core::RAY_INSTANCE_PROXY`: the instance's triangles are in the proxy
// pool, positions and UVs only (a virtual-geometry asset's coarse cut).
const RAY_INSTANCE_PROXY: u32 = 1u;
// `helio_core::RAY_INSTANCE_MASKED`: a cut-out, its material alpha-tested.
const RAY_INSTANCE_MASKED: u32 = 2u;
// `helio_core::RAY_INSTANCE_TWO_SIDED`: both faces are outside.
const RAY_INSTANCE_TWO_SIDED: u32 = 4u;
const PROXY_FLOATS: u32 = 5u;
// A probe ray's cone: the width of its footprint a metre out, the side of
// the square of one of its probe's GI_RAYS rays' solid angle
// (sqrt(4 pi / 64)).
const RAY_SPREAD: f32 = 0.443;
// Cut-out candidates a ray tests at most (the cost bound in dense
// canopies); any further one stops it, as a surface.
const MAX_CUT_OUTS: u32 = 16u;
// The tracer's id in a hit's `light` byte 0.
const MESH_TRACER: u32 = 2u;
// Flag: a shadow ray ends at its first opaque hit.
const FIRST_HIT: u32 = 0x04u;

fn position(v: u32) -> vec3<f32> {
    let at = v * VERTEX_WORDS;
    return vec3<f32>(bitcast<f32>(vertices[at]), bitcast<f32>(vertices[at + 1u]), bitcast<f32>(vertices[at + 2u]));
}

fn proxy_position(v: u32) -> vec3<f32> {
    let at = PROXY_FLOATS * v;
    return vec3<f32>(proxy_vertices[at], proxy_vertices[at + 1u], proxy_vertices[at + 2u]);
}

fn proxy_uv(v: u32) -> vec2<f32> {
    let at = PROXY_FLOATS * v + 3u;
    return vec2<f32>(proxy_vertices[at], proxy_vertices[at + 1u]);
}

fn vertex_normal(v: u32) -> vec3<f32> {
    return unpack4x8snorm(vertices[v * VERTEX_WORDS + 8u]).xyz;
}

fn vertex_uv(v: u32) -> vec2<f32> {
    let at = v * VERTEX_WORDS + 4u;
    return vec2<f32>(bitcast<f32>(vertices[at]), bitcast<f32>(vertices[at + 1u]));
}

// Whether instance `i` is a cut-out.
fn is_cut_out(i: u32) -> bool {
    return i < arrayLength(&instances) && (instances[i].flags & RAY_INSTANCE_MASKED) != 0u;
}

// Whether the cut-out candidate `c`, met along unit `dir` by a ray whose
// footprint there is `width` across, is its surface: its material's alpha
// at the hit (uv0 by the barycentrics) passes the material's cutoff, as the
// raster's alpha test (`helio_mats::vg_alpha`). The alpha is sampled at the
// mip whose texels match the footprint on the triangle (ray cones). Past
// the finest mip a texel averages many, so the test turns stochastic there
// (alpha against a uniform threshold drawn from `seed`): a canopy then
// stops rays in proportion to its coverage at any distance, where a fixed
// cutoff would let its thin leaves vanish as their mips fall below it. A
// single-sided cut-out is not there from behind (the raster culls it).
fn cut_out_surface(c: RayIntersection, dir: vec3<f32>, width: f32, seed: u32) -> bool {
    let instance = instances[c.instance_index];
    let base = instance.first_index + 3u * c.primitive_index;
    var o: array<vec3<f32>, 3>;
    var uv: array<vec2<f32>, 3>;
    var v: vec3<u32>;
    let proxy = (instance.flags & RAY_INSTANCE_PROXY) != 0u;
    if proxy {
        v = vec3<u32>(proxy_indices[base], proxy_indices[base + 1u], proxy_indices[base + 2u]) + instance.base_vertex;
        o = array<vec3<f32>, 3>(proxy_position(v.x), proxy_position(v.y), proxy_position(v.z));
        uv = array<vec2<f32>, 3>(proxy_uv(v.x), proxy_uv(v.y), proxy_uv(v.z));
    } else {
        v = vec3<u32>(indices[base], indices[base + 1u], indices[base + 2u]) + instance.base_vertex;
        o = array<vec3<f32>, 3>(position(v.x), position(v.y), position(v.z));
        uv = array<vec2<f32>, 3>(vertex_uv(v.x), vertex_uv(v.y), vertex_uv(v.z));
    }
    let b = vec3<f32>(1.0 - c.barycentrics.x - c.barycentrics.y, c.barycentrics);
    if (instance.flags & RAY_INSTANCE_TWO_SIDED) == 0u {
        // Its outside in object space (the vertex normals'; a proxy's
        // winding), against the ray there.
        var outside = cross(o[1] - o[0], o[2] - o[0]);
        if !proxy {
            outside = vertex_normal(v.x) * b.x + vertex_normal(v.y) * b.y + vertex_normal(v.z) * b.z;
        }
        if dot(outside, c.world_to_object * vec4<f32>(dir, 0.0)) > 0.0 { return false; }
    }
    let a = vga_load(instance.material);
    // Twice the triangle's world area and twice its uv area.
    let w = c.object_to_world;
    let face = cross(w * vec4<f32>(o[1] - o[0], 0.0), w * vec4<f32>(o[2] - o[0], 0.0));
    let area = length(face);
    let du = uv[1] - uv[0];
    let dv = uv[2] - uv[0];
    let uv_area = abs(du.x * dv.y - du.y * dv.x);
    // The footprint on the triangle: wider the more it slants.
    let slant = max(abs(dot(face, dir)) / max(area, 1e-30), 0.05);
    let footprint = max(width * width / slant, 1e-12);
    let lod = vga_lod_for_area(a, uv_area, area / footprint);
    let alpha = vga_alpha_level(a, uv[0] * b.x + uv[1] * b.y + uv[2] * b.z, lod);
    let u = (f32(hash(seed ^ (c.primitive_index * 0x9e3779b9u) ^ (c.instance_index * 0x85ebca6bu)) >> 8u) + 0.5) / 16777216.0;
    return alpha > 0.001 && alpha >= mix(a.cutoff, u, saturate(lod));
}

// How far a ray leaving frame point `p` starts off its surface: a few
// ulps of f32 there, and 2 mm.
fn offset(p: vec3<f32>) -> f32 {
    return 0.002 + 4e-6 * max(abs(p.x), max(abs(p.y), abs(p.z)));
}

// The share of light frame point `start` receives along `towards` (unit)
// from `distance` away: none past an opaque mesh or a cut-out's surface
// (its footprint `width` across, `seed` for its test), tinted by each sheet
// it crosses (its candidates; traversal order does not matter for a
// product).
fn transmittance(start: vec3<f32>, towards: vec3<f32>, distance: f32, width: f32, seed: u32) -> vec3<f32> {
    var q: ray_query;
    rayQueryInitialize(&q, acc_struct, RayDesc(FIRST_HIT, 0xFFu, 0.0, distance, start, towards));
    var through = vec3<f32>(1.0);
    var cut_outs = 0u;
    while rayQueryProceed(&q) {
        let sheet = rayQueryGetCandidateIntersection(&q);
        if is_cut_out(sheet.instance_index) {
            cut_outs++;
            if cut_outs <= MAX_CUT_OUTS && !cut_out_surface(sheet, towards, width, seed) { continue; }
            rayQueryTerminate(&q);
            return vec3<f32>(0.0);
        }
        through *= select(vec3<f32>(0.0), instances[sheet.instance_index].transmission.rgb, sheet.instance_index < arrayLength(&instances));
        if all(through <= vec3<f32>(0.0)) {
            rayQueryTerminate(&q);
            return vec3<f32>(0.0);
        }
    }
    if rayQueryGetCommittedIntersection(&q).kind != RAY_QUERY_INTERSECTION_NONE { return vec3<f32>(0.0); }
    return through;
}

fn hash(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// The light one of the scene's point and spot lights reflects off a
// Lambertian surface of `albedo` at frame point `p` with normal `n`: one
// light, chosen in proportion to its unshadowed irradiance there (as
// deferred lighting attenuates it), over its chance, if a shadow ray
// reaches it. Directional lights are the sun's (`gi_sun`). `width`: the
// footprint at `p`, for cut-outs on the way.
fn local_light(p: vec3<f32>, n: vec3<f32>, albedo: vec3<f32>, width: f32, seed: u32) -> vec3<f32> {
    var total = 0.0;
    var chosen = vec4<f32>(0.0);
    var chosen_e = vec3<f32>(0.0);
    for (var i = 0u; i < local_lights.count.x; i++) {
        let light = local_lights.lights[i];
        let to_light = light.position_range.xyz - p;
        let d = length(to_light);
        if d >= light.position_range.w || d < 1e-4 { continue; }
        let l = to_light / d;
        let cosine = dot(n, l);
        if cosine <= 0.0 { continue; }
        let x = d / light.position_range.w;
        var atten = max(0.0, 1.0 - x * x * x * x) / (d * d + 0.0001);
        if light.color_type.w == 2.0 {
            atten *= smoothstep(light.direction_outer.w, light.inner.x, dot(-l, light.direction_outer.xyz));
        }
        let e = light.color_type.rgb * atten * cosine;
        let w = luminance(e);
        if w <= 0.0 { continue; }
        total += w;
        if f32(hash(seed ^ (i * 0x9e3779b9u)) >> 8u) / 16777216.0 * total < w {
            chosen = vec4<f32>(l, d);
            chosen_e = e;
        }
    }
    if total <= 0.0 { return vec3<f32>(0.0); }
    let through = transmittance(p + n * offset(p), chosen.xyz, chosen.w - 2.0 * offset(p), width, hash(seed ^ 0x68e31da4u));
    return albedo / PI * chosen_e * through * (total / luminance(chosen_e));
}

// The scene's live point and spot lights, in row order, the first
// `MAX_LIGHTS` of them.
@compute @workgroup_size(1)
fn gi_gather_lights() {
    var count = 0u;
    for (var i = 0u; i < min(trace.info.x, arrayLength(&scene_lights)) && count < MAX_LIGHTS; i++) {
        let light = scene_lights[i];
        if light.light_type == 0u || light.color_intensity.w <= 0.0 || light.position_range.w <= 0.0 { continue; }
        local_lights.lights[count] = LocalLight(light.position_range, light.direction_outer, vec4<f32>(light.color_intensity.rgb * light.color_intensity.w, f32(light.light_type)), vec4<f32>(light.inner_angle, 0.0, 0.0, 0.0));
        count++;
    }
    local_lights.count = vec4<u32>(count, 0u, 0u, 0u);
}

@compute @workgroup_size(64)
fn gi_mesh_trace(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= frame_gi.counts.x * GI_RAYS { return; }
    let ray = gi_rays[id.x];
    var hit = gi_hits[id.x];
    let origin = ray.origin.xyz + trace.eye.xyz;
    let far = select(min(ray.origin.w, 1e9), hit.t, hit.t > 0.0);
    // The query starts `behind` the probe, so a surface the probe lies on
    // is met whichever side f32 rounding put the probe (a hit at t = 0 is
    // not reported). Sheets are candidates, never confirmed: probes see
    // through glass. Cut-outs are confirmed where their surface is.
    let behind = offset(origin);
    let seed = hash(id.x ^ trace.info.y);
    var q: ray_query;
    rayQueryInitialize(&q, acc_struct, RayDesc(0u, 0xFFu, 0.0, far + behind, origin - ray.dir * behind, ray.dir));
    var cut_outs = 0u;
    while rayQueryProceed(&q) {
        let candidate = rayQueryGetCandidateIntersection(&q);
        if !is_cut_out(candidate.instance_index) { continue; }
        cut_outs++;
        if cut_outs > MAX_CUT_OUTS || cut_out_surface(candidate, ray.dir, RAY_SPREAD * max(candidate.t - behind, 0.0), seed) {
            rayQueryConfirmIntersection(&q);
        }
    }
    var c = rayQueryGetCommittedIntersection(&q);
    c.t -= behind;
    let sun_towards = gi_sun[0].xyz;
    let has_sun = any(gi_sun[1].rgb > vec3<f32>(0.0));
    var n: vec3<f32>;
    var albedo: vec3<f32>;
    var extra = vec3<f32>(0.0);
    // The terrain's share of the sun at the hit.
    var terrain_sun = 0.0;
    if c.kind != RAY_QUERY_INTERSECTION_NONE && c.instance_index < arrayLength(&instances) {
        // The triangle: its frame corners for the face's normal, its
        // vertex normals (object space, to the frame by the inverse
        // transpose) for the side the mesh calls outside. A proxy has no
        // vertex normals: its winding says outside (its object-space face).
        let instance = instances[c.instance_index];
        let base = instance.first_index + 3u * c.primitive_index;
        var local: vec3<f32>;
        var o: array<vec3<f32>, 3>;
        if (instance.flags & RAY_INSTANCE_PROXY) != 0u {
            let v = vec3<u32>(proxy_indices[base], proxy_indices[base + 1u], proxy_indices[base + 2u]) + instance.base_vertex;
            o = array<vec3<f32>, 3>(proxy_position(v.x), proxy_position(v.y), proxy_position(v.z));
            let f = cross(o[1] - o[0], o[2] - o[0]);
            local = f / max(length(f), 1e-30);
        } else {
            let v = vec3<u32>(indices[base], indices[base + 1u], indices[base + 2u]) + instance.base_vertex;
            let b = vec3<f32>(1.0 - c.barycentrics.x - c.barycentrics.y, c.barycentrics);
            o = array<vec3<f32>, 3>(position(v.x), position(v.y), position(v.z));
            local = vertex_normal(v.x) * b.x + vertex_normal(v.y) * b.y + vertex_normal(v.z) * b.z;
        }
        let p0 = c.object_to_world * vec4<f32>(o[0], 1.0);
        let p1 = c.object_to_world * vec4<f32>(o[1], 1.0);
        let p2 = c.object_to_world * vec4<f32>(o[2], 1.0);
        var face = cross(p1 - p0, p2 - p0);
        face = face / max(length(face), 1e-20);
        let w = c.world_to_object;
        var shading = vec3<f32>(dot(w[0], local), dot(w[1], local), dot(w[2], local));
        if dot(shading, shading) < 1e-8 {
            // No vertex normals: two-sided.
            shading = -ray.dir;
        }
        shading = normalize(shading);
        if dot(face, shading) < 0.0 { face = -face; }
        // Two-sided: the side the ray meets is its outside.
        if (instance.flags & RAY_INSTANCE_TWO_SIDED) != 0u && dot(face, ray.dir) > 0.0 {
            face = -face;
            shading = -shading;
        }
        // Its inside: a back face (the probe is in a wall or a solid); and
        // a probe on a surface (within `behind` of it): inside it, so
        // probes on a floor are dropped alike, not some of them.
        if dot(face, ray.dir) > 0.0 || c.t < behind {
            gi_hits[id.x] = GiHit(max(c.t, 1e-3), gi_pack_unit(ray.dir), 0u, MESH_TRACER);
            return;
        }
        n = select(face, shading, dot(shading, ray.dir) < 0.0);
        albedo = clamp(instance.base_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        extra = max(instance.emissive.rgb, vec3<f32>(0.0));
        // A tenth of a probe ray's reach a texel: GI's light is coarse.
        terrain_sun = sun_occlusion(ray.origin.xyz + ray.dir * c.t + n * 0.05, n, max(0.01 * c.t, 0.1));
        hit = GiHit(c.t, gi_pack_unit(n), pack4x8unorm(vec4<f32>(albedo, 0.0)), MESH_TRACER);
        gi_hits[id.x] = hit;
    } else {
        // Another tracer's hit, or none.
        if hit.t <= 0.0 { return; }
        n = gi_unpack_unit(hit.normal);
        if dot(n, ray.dir) >= 0.0 { return; }
        let a = unpack4x8unorm(hit.albedo);
        albedo = a.rgb;
        terrain_sun = a.a;
        gi_hits[id.x].albedo = hit.albedo & 0x00ffffffu;
    }
    let p = origin + ray.dir * hit.t;
    // The ray's footprint at its hit: shadow rays from it keep that width.
    let width = RAY_SPREAD * hit.t;
    let towards = dot(n, sun_towards);
    if has_sun && terrain_sun > 0.0 && towards > 0.0 {
        let through = transmittance(p + n * offset(p), sun_towards, 1e7, width, hash(seed ^ 0x2545f491u));
        extra += albedo / PI * gi_sun[1].rgb * sun_transmittance_at(ray.origin.xyz + ray.dir * hit.t) * terrain_sun * towards * through;
    }
    if local_lights.count.x > 0u {
        extra += local_light(p, n, albedo, width, seed);
    }
    if any(extra > vec3<f32>(0.0)) {
        let e = min(extra, vec3<f32>(60000.0));
        gi_light[id.x] = vec2<u32>(pack2x16float(e.rg), pack2x16float(vec2<f32>(e.b, 0.0)));
    }
}
