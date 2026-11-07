// The lights' light list (`helio_light_api`, `//!use light_list`): the row
// of the scene's light list (`GpuLight`, 128 bytes, `helio_light_api::GpuLight`
// field for field), its kinds and flags, the screen-tile binning's shape and
// the directional light published with the list. A reader declares its own
// bindings of these rows; it never declares the rows.

struct GpuLight {
    position_range:  vec4<f32>,  // xyz position, w range
    direction_outer: vec4<f32>,  // xyz direction it shines along, w spot outer cos
    color_intensity: vec4<f32>,  // xyz linear colour, w intensity
    shadow_index:    u32,        // first atlas face, or LIGHT_NO_SHADOW
    light_type:      u32,        // LIGHT_DIRECTIONAL / POINT / SPOT / AREA
    inner_angle:     f32,        // spot inner cos
    // Shadow-intent flags: bit 0 an explicit ray-traced intent, bit 1
    // ray-traced shadows on; bits 2-3 the caster allocation's (bit 3 it
    // rewrote the row, bit 2 the shadow-map request it replaced); bits 4-5
    // the caster kinds it skips; bit 6 no contact shadows.
    _pad:            u32,
    // Light shafts (media).
    god_rays_enabled:  u32,
    god_rays_density:  f32,
    god_rays_weight:   f32,
    god_rays_decay:    f32,
    god_rays_exposure: f32,
    // Lens flare.
    flare_enabled:   u32,
    flare_type:      u32,
    flare_intensity: f32,
    flare_scale:     f32,
    flare_tint_r:    f32,
    flare_tint_g:    f32,
    flare_tint_b:    f32,
    // IES profile and light function.
    ies_profile_index:    i32,
    light_function_index: i32,
    ies_angle_scale:      f32,
    ies_angle_offset:     f32,
}

const LIGHT_DIRECTIONAL: u32 = 0u;
const LIGHT_POINT: u32 = 1u;
const LIGHT_SPOT: u32 = 2u;
const LIGHT_AREA: u32 = 3u;
// `shadow_index` of a light with no shadow map.
const LIGHT_NO_SHADOW: u32 = 0xffffffffu;

// The binning: a pixel's tile is (x, y) / LIGHT_TILE_SIZE, row-major; its
// lights are `tile_light_lists[tile * LIGHT_MAX_PER_TILE + i]`, `i` below
// `tile_light_counts[tile]`.
const LIGHT_TILE_SIZE: u32 = 16u;
const LIGHT_MAX_PER_TILE: u32 = 64u;

// The directional lights published with the list (`helio_light_api::
// DIRECTIONAL_LIGHTS`), in list order; the first is the primary sun.
const MAX_DIRECTIONAL_LIGHTS: u32 = 32u;
struct DirectionalLight {
    // Toward it, w 1.
    toward:   vec4<f32>,
    // Colour times intensity (rgb).
    radiance: vec4<f32>,
    // Its emitter disk (the flare words): angular radius, brightness,
    // texture kind, enabled.
    disk:     vec4<f32>,
}
struct DirectionalLights {
    lights: array<DirectionalLight, 32>,
    // x: how many of `lights` are live.
    count:  vec4<u32>,
}

// Whether `light` is a sun: a directional light with intensity and a
// direction. The lights' directional-lights kernel picks such lights
// (`helio_pass_light_cull::directional`) into `DIRECTIONAL_LIGHTS`, for every
// host; a reader wanting them reads that row, never scans the list.
fn light_is_sun(light: GpuLight) -> bool {
    return light.light_type == LIGHT_DIRECTIONAL && light.color_intensity.w > 0.0
        && dot(light.direction_outer.xyz, light.direction_outer.xyz) > 0.0;
}

// Whether `light` asks for a shadow map: its caster allocation's verdict
// once it rewrote the row, else the authored index.
fn light_shadow_requested(light: GpuLight) -> bool {
    return select(light.shadow_index != LIGHT_NO_SHADOW, (light._pad & 4u) != 0u, (light._pad & 8u) != 0u);
}

// The tile of the pixel at `frag_xy` among `num_tiles_x` tiles a row.
fn light_tile_index(frag_xy: vec2<f32>, num_tiles_x: u32) -> u32 {
    let tile = vec2<u32>(frag_xy) / LIGHT_TILE_SIZE;
    return tile.y * num_tiles_x + tile.x;
}
