// Voxel materials drawn with the host's scene materials: Helio's material
// rows (`GpuMaterial`) and its descriptor-indexed texture table, bound as
// the publish pipeline's group 2 (`chunks::scene`). Spliced over the
// `SCENE_MATERIALS` line of chunk_draw.wgsl when the device has binding
// arrays.

struct SceneGpuMaterial {
    base_color:         vec4<f32>,
    emissive:           vec4<f32>,
    roughness_metallic: vec4<f32>,
    tex_base_color:     u32,
    tex_normal:         u32,
    tex_roughness:      u32,
    tex_emissive:       u32,
    tex_occlusion:      u32,
    workflow:           u32,
    flags:              u32,
    material_class:     u32,
    class_params:       vec4<f32>,
}

const SCENE_NO_TEXTURE: u32 = 0xffffffffu;
// `GpuMaterial::flags`: the normal texture is a normal map
// (`helio_mats::FLAG_HAS_NORMAL_MAP`).
const SCENE_HAS_NORMAL_MAP: u32 = 1u << 3u;

// Per voxel material: the scene material rows of its top, side and bottom
// faces (x, y, z; SCENE_NO_TEXTURE: none).
struct SceneMap {
    faces: array<vec4<u32>, 32>,
}

@group(2) @binding(0) var<storage, read> scene_map: SceneMap;
@group(2) @binding(1) var<storage, read> scene_materials: array<SceneGpuMaterial>;
@group(2) @binding(2) var scene_textures: binding_array<texture_2d<f32>, MATERIAL_TEXTURES>;
@group(2) @binding(3) var scene_samplers: binding_array<sampler, MATERIAL_TEXTURES>;
// Shared with WaterSim. Layer 0 holds terrain height/coast distance/coast
// gradient; layer 1 holds travel time/front direction/shoaling. state.zw is
// the map center relative to this frame's eye, matching chunk hit positions.
struct WaterShoreParams { center_extent: vec4<f32>, state: vec4<f32> }
@group(2) @binding(4) var water_shore_fields: texture_2d_array<f32>;
// Kept in group 3: WGPU forbids uniform buffers in a group that contains
// descriptor binding arrays (the scene texture/sampler table in group 2).
@group(3) @binding(0) var<uniform> water_shore_params: WaterShoreParams;

fn swash_hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + vec3<f32>(33.33));
    return fract((p3.x + p3.y) * p3.z);
}

fn swash_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (vec2<f32>(3.0) - 2.0 * f);
    return mix(
        mix(swash_hash12(i), swash_hash12(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(swash_hash12(i + vec2<f32>(0.0, 1.0)), swash_hash12(i + vec2<f32>(1.0)), u.x),
        u.y,
    );
}

fn swash_at(world_xz_from_eye: vec2<f32>) -> vec3<f32> {
    if water_shore_params.state.x < 0.5 { return vec3<f32>(0.0); }
    let extent = max(water_shore_params.center_extent.zw, vec2<f32>(1e-3));
    let uv = (world_xz_from_eye - water_shore_params.state.zw) / extent + vec2<f32>(0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) { return vec3<f32>(0.0); }
    // Map-relative coordinates preserve phase across camera motion and world
    // origin rebases, while keeping the noise domain local to the authored map.
    let map_xz = world_xz_from_eye - water_shore_params.state.zw;
    let size = vec2<f32>(textureDimensions(water_shore_fields, 0));
    let pixel = uv * size - vec2<f32>(0.5);
    let base = vec2<i32>(floor(pixel));
    let f = fract(pixel);
    let hi = vec2<i32>(size) - vec2<i32>(1);
    let p00 = clamp(base, vec2<i32>(0), hi);
    let p10 = clamp(base + vec2<i32>(1, 0), vec2<i32>(0), hi);
    let p01 = clamp(base + vec2<i32>(0, 1), vec2<i32>(0), hi);
    let p11 = clamp(base + vec2<i32>(1, 1), vec2<i32>(0), hi);
    let shore = mix(
        mix(textureLoad(water_shore_fields, p00, 0, 0), textureLoad(water_shore_fields, p10, 0, 0), f.x),
        mix(textureLoad(water_shore_fields, p01, 0, 0), textureLoad(water_shore_fields, p11, 0, 0), f.x),
        f.y,
    );
    if shore.x < -0.3 || shore.y > 60.0 { return vec3<f32>(0.0); }
    let travel = mix(
        mix(textureLoad(water_shore_fields, p00, 1, 0), textureLoad(water_shore_fields, p10, 1, 0), f.x),
        mix(textureLoad(water_shore_fields, p01, 1, 0), textureLoad(water_shore_fields, p11, 1, 0), f.x),
        f.y,
    );
    var shore_dir = -shore.zw / max(length(shore.zw), 1e-3);
    if dot(travel.yz, travel.yz) > 0.25 { shore_dir = normalize(travel.yz); }
    let time = water_shore_params.state.y;
    let phase_noise = swash_noise(map_xz * 0.0042) * 9.0
        + swash_noise(map_xz * 0.011 + vec2<f32>(7.0)) * 2.5;
    let group = 0.45 + 0.55 * swash_noise(map_xz * 0.004
        + vec2<f32>(time * 0.021, -time * 0.017));
    let swell_dir = vec2<f32>(-0.9659258, 0.2588190);
    let exposure = mix(0.25, 1.0, smoothstep(-0.3, 0.7, dot(shore_dir, swell_dir)));
    let amplitude = 0.55;
    let wavelength = 30.0;
    let speed = 1.0;
    let runup = 0.55 * amplitude * exposure * group * smoothstep(0.35, 0.8, length(shore.zw));
    if runup < 0.02 { return vec3<f32>(0.0); }
    if shore.x > runup {
        let residual_wetness = 0.5 * (1.0 - smoothstep(runup, 1.5 * runup, shore.x));
        return vec3<f32>(0.0, 0.0, residual_wetness);
    }
    let omega = sqrt(9.81 * 6.2831853 / wavelength) * speed;
    let phase = fract((omega * time + phase_noise) / 6.2831853);
    let eta = runup * select(
        cos((phase - 0.3) / 0.7 * 1.5708),
        sin(phase / 0.3 * 1.5708),
        phase < 0.3,
    );
    let film = eta - shore.x;
    let edge = select(0.0, smoothstep(-0.01, 0.0, film) * (1.0 - smoothstep(0.0, 0.05, film)), phase < 0.35);
    let pass_phase = 0.3 + 0.7 * acos(clamp(shore.x / runup, 0.0, 1.0)) / 1.5708;
    let dry_phase = select(phase - pass_phase, phase + 1.0 - pass_phase, phase <= pass_phase);
    let dry = dry_phase * 6.2831853 / omega;
    return vec3<f32>(max(film, 0.0), edge, select(exp(-dry / 5.0), 1.0, film > 0.0));
}

// A material texture's coordinate along a face axis: base cells from `b`
// (a cell's first base cell) plus its span `span` of base cells at `t`
// across it, over `tile` base cells a texture. Exact at any planet size
// (the cell index is taken modulo the tile first); a texture wholly inside
// the span is not wrapped (no seam row at the cell's far edge).
fn tile_coord(b: i32, t: f32, span: f32, tile: i32) -> f32 {
    let start = f32(((b % tile) + tile) % tile);
    let x = (start + t * span) / f32(tile);
    return select(fract(x), min(x, 0.99999), x <= 1.0);
}

// Material `m`'s surface on a face of cell `cell` (its index at `level`) of
// `level` (`axis` 2 is the radial one: a top face is its sign 1, a bottom
// its sign 0). `in_cell` is the hit's place in the cell. A texture spans
// the material's tile (`SceneMap`, whole base cells; one by default) and
// runs on across cells and levels, so surfaces keep their size at any voxel
// size and distance; the mip level follows its texels' size in pixels
// (filtered, never derivative based, which would break across cells). The
// normal map's normal is in the face's texture frame (x along u, y against
// v, z out: OpenGL's), straight out without one. `slant` is how much longer
// a pixel's footprint runs along the face than across the view (one over
// the cosine of the view's angle to it): the normals under the whole
// footprint widen the roughness (specular anti-aliasing,
// `normal_map_roughness`).
fn scene_surface(m: u32, axis: u32, sign: u32, level: u32, cell: vec3<i32>, in_cell: vec3<f32>, pixel_m: f32, slant: f32, world_xz_from_eye: vec2<f32>) -> SceneSurface {
    let swash = select(vec3<f32>(0.0), swash_at(world_xz_from_eye), axis == 2u && sign == 1u);
    let none = SceneSurface(vec3<f32>(0.0), 0.0, vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0), 1.0, swash, false);
    let faces = scene_map.faces[min(m, 31u)];
    var row = faces.y;
    if axis == 2u {
        row = select(faces.z, faces.x, sign == 1u);
    }
    if row == SCENE_NO_TEXTURE || row >= arrayLength(&scene_materials) {
        return none;
    }
    let material = scene_materials[row];
    let tile = max(i32(round(bitcast<f32>(faces.w) / params.planet.y)), 1);
    let span = f32(1u << min(level, 15u));
    let base = cell << vec3<u32>(min(level, 15u));
    // u along the face's first index, v down its second (down a side).
    var uv = vec2<f32>(tile_coord(base.x, in_cell.x, span, tile), tile_coord(base.y, in_cell.y, span, tile));
    if axis == 0u {
        uv = vec2<f32>(tile_coord(base.y, in_cell.y, span, tile), 1.0 - tile_coord(base.z, in_cell.z, span, tile));
    } else if axis == 1u {
        uv = vec2<f32>(tile_coord(base.x, in_cell.x, span, tile), 1.0 - tile_coord(base.z, in_cell.z, span, tile));
    }
    uv = clamp(uv, vec2<f32>(0.0), vec2<f32>(0.99999));
    // A texture's pixels on screen; a texel a half pixel wide (not a whole
    // one): TSR averages the jittered samples, so the detail the mip would
    // have averaged away is rebuilt.
    let tile_px = f32(tile) * params.planet.y / max(pixel_m, 1e-6);
    var colour = material.base_color.rgb;
    var texel = vec3<f32>(1.0);
    if material.tex_base_color != SCENE_NO_TEXTURE && material.tex_base_color < MATERIAL_TEXTURES {
        texel = scene_sample(material.tex_base_color, uv, tile_px).rgb;
        colour *= texel;
    }
    var roughness = material.roughness_metallic.x;
    var occlusion = 1.0;
    if material.tex_roughness != SCENE_NO_TEXTURE && material.tex_roughness < MATERIAL_TEXTURES {
        // Occlusion-roughness-metallic packing: roughness is green.
        let orm = scene_sample(material.tex_roughness, uv, tile_px);
        roughness *= orm.g;
        if material.tex_occlusion == material.tex_roughness { occlusion = orm.r; }
    }
    if material.tex_occlusion != SCENE_NO_TEXTURE && material.tex_occlusion < MATERIAL_TEXTURES && material.tex_occlusion != material.tex_roughness {
        occlusion = scene_sample(material.tex_occlusion, uv, tile_px).r;
    }
    var normal = vec3<f32>(0.0, 0.0, 1.0);
    if (material.flags & SCENE_HAS_NORMAL_MAP) != 0u && material.tex_normal != SCENE_NO_TEXTURE && material.tex_normal < MATERIAL_TEXTURES {
        let slot = material.tex_normal;
        let n = scene_sample(slot, uv, tile_px).rgb * 2.0 - 1.0;
        normal = n / max(length(n), 1e-4);
        // The spread of the normals under the pixel's whole footprint, along
        // its longer side (not the half pixel the direction is sampled at):
        // the mip whose texels are that long.
        let footprint = max(scene_texels_per_pixel(slot, tile_px) + log2(clamp(slant, 1.0, 16.0)), 0.0);
        let spread = textureSampleLevel(scene_textures[slot], scene_samplers[slot], uv, footprint).a;
        roughness = normal_map_roughness(roughness, spread, 1.0);
    }
    return SceneSurface(colour, roughness, material.emissive.rgb * material.emissive.w * texel, normal, occlusion, swash, true);
}

// The mip level whose texels are a pixel wide, for texture `slot` spanning
// `tile_px` pixels: log2 of its texels per pixel.
fn scene_texels_per_pixel(slot: u32, tile_px: f32) -> f32 {
    let size = f32(textureDimensions(scene_textures[slot]).x);
    return log2(size / max(tile_px, 1e-6));
}

// Texture `slot` at `uv`, its texels half a pixel wide (see
// `scene_surface`) on a texture `tile_px` pixels wide.
fn scene_sample(slot: u32, uv: vec2<f32>, tile_px: f32) -> vec4<f32> {
    let lod = max(scene_texels_per_pixel(slot, tile_px) - 1.0, 0.0);
    return textureSampleLevel(scene_textures[slot], scene_samplers[slot], uv, lod);
}
