//!use atmosphere
//!use atmosphere_sky_table
//!use gi_common
//!use gi_sample
//!use sun_transmittance
// Probe updates (README.md): the sun found, last frame's rays integrated
// into their probes, probes that came into the clipmap reset, this frame's
// rays written for the tracers.

@group(0) @binding(0) var<uniform> frame_gi: GiFrame;
@group(0) @binding(1) var<storage, read_write> gi_probes: array<u32>;
@group(0) @binding(2) var<storage, read_write> gi_rays: array<GiRay>;
@group(0) @binding(3) var<storage, read> gi_hits: array<GiHit>;
// Two update lists (GI_MAX_UPDATES probes each: this frame's and last
// frame's, by `counts.w`), then the probes to reset.
@group(0) @binding(4) var<storage, read> gi_lists: array<u32>;
// The sun: towards it (xyz), its illuminance at the eye (rgb).
@group(0) @binding(5) var<storage, read_write> gi_sun: array<vec4<f32>, 2>;
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
@group(0) @binding(6) var<storage, read> scene_lights: array<SceneLight>;
@group(0) @binding(7) var<uniform> atmosphere: AtmosphereFrame;
@group(0) @binding(8) var<uniform> atmosphere_sky_table: array<vec4<f32>, 2304>;
// A ray's hit's light beyond the sun's and the probes' (`GiMeshTracePass`:
// the scene's lights on it, the light it gives off), radiance RGB in half
// floats; zero where no tracer wrote any.
@group(0) @binding(9) var<storage, read> gi_light: array<vec2<u32>>;
// The sun's transmittance through the media above (the frame's
// `sun_transmittance`: the clouds'): the sun at a hit
// under a cloud is the cloud's share of it, so the shade under a cloud is
// not lit by bounce from ground the cloud shades.
@group(0) @binding(10) var sun_transmittance_map: texture_2d<f32>;
@group(0) @binding(11) var<uniform> sun_transmittance_frame: SunTransmittanceFrame;

const PI: f32 = 3.14159265;
const LUMINANCE: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// The sun: the atmosphere's, through the air to the eye; without one, the
// scene's first directional light.
@compute @workgroup_size(1)
fn gi_sun_main() {
    if atmosphere.planet.w > 0.5 {
        let towards = atmosphere.sun.xyz;
        gi_sun[0] = vec4<f32>(towards, 0.0);
        gi_sun[1] = vec4<f32>(atmosphere.sun_illuminance.rgb * atmosphere_transmittance_towards(atmosphere, vec3<f32>(0.0), towards), 0.0);
        return;
    }
    gi_sun[0] = vec4<f32>(0.0, 1.0, 0.0, 0.0);
    gi_sun[1] = vec4<f32>(0.0);
    for (var i = 0u; i < arrayLength(&scene_lights); i++) {
        let light = scene_lights[i];
        if light.light_type == 0u && light.color_intensity.w > 0.0 && dot(light.direction_outer.xyz, light.direction_outer.xyz) > 0.0 {
            gi_sun[0] = vec4<f32>(-normalize(light.direction_outer.xyz), 0.0);
            gi_sun[1] = vec4<f32>(light.color_intensity.rgb * light.color_intensity.w, 0.0);
            return;
        }
    }
}

// The sky's radiance along unit direction `d` from eye-relative point
// `from_camera` (km): the atmosphere's sky table there (irradiance / pi in
// L2 harmonics, cosine-convolved) back to radiance, band by band; without
// an atmosphere, the flat sky and ground.
fn sky_radiance(from_camera: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    if atmosphere.planet.w < 0.5 { return select(frame_gi.ground.rgb, frame_gi.sky.rgb, d.y >= 0.0); }
    // The basis over each band's convolution (1, 2/3, 1/4).
    return atmosphere_sky_table_eval(atmosphere, from_camera, d, vec3<f32>(1.0, 1.5, 4.0));
}

// Probes that came into the clipmap (after `gi_integrate`, which may have
// written the probe that was there): no light, no distances, no updates.
@compute @workgroup_size(64)
fn gi_reset(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= frame_gi.counts.z { return; }
    let at = gi_lists[2u * GI_MAX_UPDATES + id.x] * GI_PROBE_WORDS;
    for (var w = 0u; w < GI_PROBE_WORDS; w++) { gi_probes[at + w] = 0u; }
    gi_probes[at + GI_STATE_WORD] = GI_ACTIVE;
}

var<workgroup> w_radiance: array<vec3<f32>, 64>;
var<workgroup> w_dir: array<vec3<f32>, 64>;
var<workgroup> w_t: array<f32, 64>;
var<workgroup> w_inside: atomic<u32>;

// Last frame's probes, a workgroup each, a lane a ray: each ray's radiance
// (the sky, or its hit's: albedo times the sun's light and the probes'
// there, plus its `gi_light`; nothing from a back face), projected on L1
// harmonics and blended into the probe; its distances into the probe's
// moments (a lane a texel). A probe whose rays mostly meet back faces is
// inside the ground (or a wall).
@compute @workgroup_size(64)
fn gi_integrate(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    if wg.x >= frame_gi.counts.y { return; }
    let index = gi_lists[(1u - frame_gi.counts.w) * GI_MAX_UPDATES + wg.x];
    let probe = gi_probe_position(index);
    let spacing = frame_gi.levels[gi_probe_cell(index).w].origin.w;
    // Moments past a few spacings tell nothing more.
    let far = 3.5 * spacing;
    let dir = gi_ray_dir(li, frame_gi.prev_rotation);
    let hit = gi_hits[wg.x * GI_RAYS + li];
    if li == 0u { atomicStore(&w_inside, 0u); }
    workgroupBarrier();
    var radiance = vec3<f32>(0.0);
    var t = far;
    if hit.t > 0.0 {
        t = min(hit.t, far);
        let n = gi_unpack_unit(hit.normal);
        if dot(n, dir) > 0.0 {
            atomicAdd(&w_inside, 1u);
        } else {
            // Lambertian on the side the ray meets; a surface that lets
            // light through adds that share of its far side's light.
            let albedo = unpack4x8unorm(hit.albedo);
            let at = probe + dir * hit.t;
            let sun = gi_sun[1].rgb * sun_transmittance_at(at);
            let front = sun * albedo.a * max(dot(n, gi_sun[0].xyz), 0.0) / PI + gi_irradiance(at, n).rgb;
            let through = f32((hit.light >> 8u) & 255u) / 255.0;
            var back = vec3<f32>(0.0);
            if through > 0.0 {
                let seen = f32((hit.light >> 16u) & 255u) / 255.0;
                back = sun * seen * max(dot(-n, gi_sun[0].xyz), 0.0) / PI + gi_irradiance(at, -n).rgb;
            }
            // And the light it gives off, and the scene's lights' on it.
            let glow = f32(hit.light >> 24u) / 16.0;
            let lit = gi_light[wg.x * GI_RAYS + li];
            let extra = vec3<f32>(unpack2x16float(lit.x), unpack2x16float(lit.y).x);
            radiance = albedo.rgb * (front + through * back + glow) + extra;
        }
    } else {
        radiance = sky_radiance(probe * 0.001, dir);
    }
    // Fireflies: a ray meeting a surface a few centimetres from a point
    // light (its fitting) carries tens of times the room's light, and would
    // dominate its probe for many updates. Once the probe holds light, a
    // ray is capped at 16 times its mean irradiance / pi, plus 2 (so a dark
    // probe still sees a sunlit opening).
    let held = gi_probes[index * GI_PROBE_WORDS + GI_STATE_WORD] >> 8u;
    if held > 0u {
        let dc = vec3<f32>(gi_half2(gi_probes[index * GI_PROBE_WORDS]), gi_half2(gi_probes[index * GI_PROBE_WORDS + 1u]).x) * 0.282095;
        let cap = 16.0 * dot(max(dc, vec3<f32>(0.0)), LUMINANCE) + 2.0;
        let l = dot(radiance, LUMINANCE);
        if l > cap { radiance *= cap / l; }
    }
    w_radiance[li] = radiance;
    w_dir[li] = dir;
    w_t[li] = t;
    workgroupBarrier();
    let at = index * GI_PROBE_WORDS;
    let state = gi_probes[at + GI_STATE_WORD];
    let updates = state >> 8u;
    // Moments: a lane a texel, its nearby rays' distances.
    {
        let texel = vec2<f32>(f32(li % GI_MOMENTS_SIDE), f32(li / GI_MOMENTS_SIDE));
        let d = gi_oct_decode((texel + 0.5) / f32(GI_MOMENTS_SIDE));
        var sum = vec2<f32>(0.0);
        var weights = 0.0;
        for (var k = 0u; k < GI_RAYS; k++) {
            let w = pow(max(dot(d, w_dir[k]), 0.0), 16.0);
            sum += vec2<f32>(w_t[k], w_t[k] * w_t[k]) * w;
            weights += w;
        }
        if weights > 1e-6 {
            let fresh = sum / weights;
            let old = gi_half2(gi_probes[at + GI_MOMENTS_WORD + li]);
            let alpha = select(max(1.0 / f32(updates + 1u), 0.05), 1.0, updates == 0u);
            gi_probes[at + GI_MOMENTS_WORD + li] = pack2x16float(mix(old, fresh, alpha));
        }
    }
    if li != 0u { return; }
    // Irradiance / pi: radiance on L1 harmonics, convolved (bands 1, 2/3).
    var sh = array<vec3<f32>, 4>(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    for (var k = 0u; k < GI_RAYS; k++) {
        let b = gi_sh_basis(w_dir[k]);
        sh[0] += w_radiance[k] * b.x;
        sh[1] += w_radiance[k] * b.y;
        sh[2] += w_radiance[k] * b.z;
        sh[3] += w_radiance[k] * b.w;
    }
    // Radiance's coefficients by Monte Carlo over the sphere; irradiance /
    // pi's are theirs times 1 and 2/3 (uniform radiance 1 gives 1).
    let scale = 4.0 * PI / f32(GI_RAYS);
    sh[0] *= scale;
    sh[1] *= scale * (2.0 / 3.0);
    sh[2] *= scale * (2.0 / 3.0);
    sh[3] *= scale * (2.0 / 3.0);
    let alpha = select(max(1.0 / f32(updates + 1u), 0.03), 1.0, updates == 0u);
    // Words: (r0 g0) (b0 r1) (g1 b1) (r2 g2) (b2 r3) (g3 b3).
    let fresh = array<vec2<f32>, 6>(vec2<f32>(sh[0].r, sh[0].g), vec2<f32>(sh[0].b, sh[1].r), vec2<f32>(sh[1].g, sh[1].b), vec2<f32>(sh[2].r, sh[2].g), vec2<f32>(sh[2].b, sh[3].r), vec2<f32>(sh[3].g, sh[3].b));
    for (var w = 0u; w < 6u; w++) {
        gi_probes[at + w] = pack2x16float(mix(gi_half2(gi_probes[at + w]), fresh[w], alpha));
    }
    let inside = atomicLoad(&w_inside) * 4u > GI_RAYS;
    gi_probes[at + GI_STATE_WORD] = (min(updates + 1u, 255u) << 8u) | select(GI_ACTIVE, 0u, inside);
}

// This frame's rays: from each probe to update, its set turned by this
// frame's rotation.
@compute @workgroup_size(64)
fn gi_rays_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= frame_gi.counts.x * GI_RAYS { return; }
    let index = gi_lists[frame_gi.counts.w * GI_MAX_UPDATES + id.x / GI_RAYS];
    gi_rays[id.x] = GiRay(vec4<f32>(gi_probe_position(index), 1e30), gi_ray_dir(id.x % GI_RAYS, frame_gi.rotation), index);
}
