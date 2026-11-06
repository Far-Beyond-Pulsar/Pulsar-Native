//!use shadow_atlas
// The views' rows (`LightMatrix`), the cube-face order and the budgeted
// atlas's addressing are the shadow atlas's (`//!use shadow_atlas`).

struct CascadeConfig {
    split_distance: f32,
    depth_bias: f32,
    filter_radius: f32,
    pcss_light_size: f32,
}

struct ShadowConfig {
    cascades: array<CascadeConfig, 4>,
    enable_pcss: u32,
    pcss_blocker_samples: u32,
    pcss_filter_samples: u32,
    pcf_sample_count: u32,
}

@group(0) @binding(3) var<uniform> shadow_config: ShadowConfig;
@group(0) @binding(4) var shadow_atlas: texture_depth_2d_array;
@group(0) @binding(5) var shadow_sampler: sampler_comparison;
@group(0) @binding(6) var <storage, read> shadow_matrices: array<LightMatrix>;
// Static casters are cached in their own atlas; visibility is the min of both.
@group(0) @binding(8) var static_shadow_atlas: texture_depth_2d_array;
// Translucent casters (stained glass): rgb = 1 - T, a = 1 - nearest pane depth.
@group(0) @binding(9) var shadow_transmittance: texture_2d_array<f32>;
@group(0) @binding(10) var transmittance_sampler: sampler;

// Vogel disk sampling - blue-noise-like spiral pattern for high-quality PCF
fn vogel_disk_sample(sample_idx: u32, sample_count: u32, theta: f32) -> vec2<f32> {
    let GOLDEN_ANGLE = 2.39996323;
    let r = sqrt(f32(sample_idx) + 0.5) / sqrt(f32(sample_count));
    let angle = f32(sample_idx) * GOLDEN_ANGLE + theta;
    return vec2<f32>(cos(angle), sin(angle)) * r;
}

// Per-pixel hash for PCF rotation (reduces banding artifacts)
fn hash22(p: vec2<f32>) -> f32 {
    let p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    let d = dot(p3, vec3<f32>(p3.y + 33.33, p3.z + 33.33, p3.x + 33.33));
    return fract((p3.x + p3.y) * d);
}

fn pcss_blocker_search(
    layer: u32,
    shadow_uv: vec2<f32>,
    receiver_depth: f32,
    search_radius: f32,
    blocker_samples: u32,
    theta: f32
) -> vec2<f32> {
    var blocker_sum = 0.0;
    var blocker_count = 0.0;

    for (var i = 0u; i < blocker_samples; i++) {
        let offset = vogel_disk_sample(i, blocker_samples, theta) * search_radius;
        let sample_uv = shadow_uv + offset;
        let pixel_coord = vec2<i32>(sample_uv * budget_resolution(layer));

        if any(pixel_coord < vec2<i32>(0)) || any(pixel_coord >= vec2<i32>(i32(budget_resolution(layer)))) {
            continue;
        }

        let occluder_depth = min(budget_depth_dynamic(pixel_coord, u32(i32(layer))),
                                 budget_depth_static(pixel_coord, u32(i32(layer))));
        if occluder_depth < receiver_depth - 0.0001 {
            blocker_sum += occluder_depth;
            blocker_count += 1.0;
        }
    }

    if blocker_count < 0.5 {
        return vec2<f32>(0.0, 0.0);
    }

    return vec2<f32>(blocker_sum / blocker_count, blocker_count);
}

fn pcss_penumbra_size(receiver_depth: f32, avg_blocker_depth: f32, light_size: f32) -> f32 {
    return (receiver_depth - avg_blocker_depth) / max(avg_blocker_depth, 0.001) * light_size;
}

fn sample_cascade_shadow(layer: u32, cascade_idx: u32, cascade_scale: f32, world_pos: vec3<f32>, frag_coord: vec2<f32>, frame: u32) -> f32 {
    if layer >= arrayLength(&shadow_matrices) { return 1.0; }
    let light_clip = shadow_matrices[layer].mat * vec4<f32>(world_pos, 1.0);
    if light_clip.w <= 0.0 { return 1.0; }

    let ndc = light_clip.xyz / light_clip.w;
    let shadow_uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);

    if any(shadow_uv < vec2<f32>(0.0)) || any(shadow_uv > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }
    // LessEqual comparison samplers treat a receiver slightly nearer to the
    // light as lit. A cascade keeps its depth-space bias (coarse cascades
    // compensate for lower depth precision); a local light's face biases in
    // world space (`shadow_receiver_depth`).
    let receiver_depth = shadow_receiver_depth(layer, world_pos, ndc.z, shadow_config.cascades[cascade_idx].depth_bias);
    let theta = hash22(frag_coord) * 6.28318530718;

    // OPTIMIZATION: Adaptive PCF sample count based on cascade distance
    let base_count = shadow_config.pcf_sample_count;
    var pcf_count: u32;
    switch cascade_idx {
        case 0u: { pcf_count = base_count; }
        case 1u: { pcf_count = max(base_count * 3u / 4u, 4u); }
        case 2u: { pcf_count = max(base_count / 2u, 4u); }
        default: { pcf_count = max(base_count / 4u, 4u); }
    }

    var lit_sum = 0.0;
    for (var i = 0u; i < pcf_count; i++) {
        let offset = vogel_disk_sample(i, pcf_count, theta) * (cascade_scale / budget_resolution(layer));
        lit_sum += min(budget_compare_dynamic(shadow_uv + offset, u32(i32(layer)), receiver_depth),
                       budget_compare_static(shadow_uv + offset, u32(i32(layer)), receiver_depth));
    }

    return lit_sum / f32(pcf_count);
}

fn sample_cascade_shadow_pcss(layer: u32, cascade_idx: u32, world_pos: vec3<f32>, frag_coord: vec2<f32>, frame: u32) -> f32 {
    let config = shadow_config.cascades[cascade_idx];
    if layer >= arrayLength(&shadow_matrices) { return 1.0; }
    let light_clip = shadow_matrices[layer].mat * vec4<f32>(world_pos, 1.0);
    if light_clip.w <= 0.0 { return 1.0; }

    let ndc = light_clip.xyz / light_clip.w;
    let shadow_uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);

    if any(shadow_uv < vec2<f32>(0.0)) || any(shadow_uv > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return 1.0;
    }

    let blocker_receiver_depth = ndc.z;
    let compare_depth = max(0.0, ndc.z - config.depth_bias);
    let theta = hash22(frag_coord) * 6.28318530718;

    // Blocker search uses unbiased depth so nearby occluders are correctly identified.
    let search_radius = config.pcss_light_size / budget_resolution(layer);
    let blocker = pcss_blocker_search(layer, shadow_uv, blocker_receiver_depth, search_radius, shadow_config.pcss_blocker_samples, theta);

    if blocker.y < 0.5 {
        return 1.0;
    }

    let penumbra = pcss_penumbra_size(blocker_receiver_depth, blocker.x, config.pcss_light_size);
    let filter_radius = clamp(penumbra / budget_resolution(layer), config.filter_radius / budget_resolution(layer), config.filter_radius * 3.0 / budget_resolution(layer));

    var lit_sum = 0.0;

    for (var i = 0u; i < shadow_config.pcss_filter_samples; i++) {
        let offset = vogel_disk_sample(i, shadow_config.pcss_filter_samples, theta) * filter_radius;
        lit_sum += min(budget_compare_dynamic(shadow_uv + offset, u32(i32(layer)), compare_depth),
                       budget_compare_static(shadow_uv + offset, u32(i32(layer)), compare_depth));
    }

    return lit_sum / f32(shadow_config.pcss_filter_samples);
}

fn screen_occluded(light: CompactLight, position: vec3<f32>, normal: vec3<f32>) -> bool {
    let max_distance=globals.ambient.w;
    if max_distance<=0.0 { return false; }
    let origin=position+normal*0.02;
    let inc=incident(light,origin);
    var ray_length=max_distance;
    if light.light_type!=0u { ray_length=min(ray_length,length(light.position_range.xyz-origin)); }
    if ray_length<0.025 || dot(normal,inc.direction)<=0.0 { return false; }
    // Traverse current-frame nearest depths from large cells down to pixels.
    // NDC is linear along a projected segment, including perspective cameras.
    let clip0=cameras[0].view_proj*vec4<f32>(origin,1.0);
    let clip1=cameras[0].view_proj*vec4<f32>(origin+inc.direction*ray_length,1.0);
    if clip0.w<=0.0 || clip1.w<=0.0 { return false; }
    let ndc0=clip0.xyz/clip0.w; let ndc1=clip1.xyz/clip1.w;
    let screen=vec2<f32>(globals.screen_size);
    let start=(ndc0.xy*vec2<f32>(0.5,-0.5)+0.5)*screen;
    let finish=(ndc1.xy*vec2<f32>(0.5,-0.5)+0.5)*screen;
    let delta=finish-start;
    let span=max(abs(delta.x),abs(delta.y));
    if span<0.5 { return false; }
    let top=i32(textureNumLevels(screen_depth_bounds))-1;
    var level=min(top,max(0,i32(floor(log2(max(span/8.0,1.0))))));
    var t=min(0.5/span,0.5);
    for(var iteration=0u;iteration<96u;iteration++) {
        if t>=1.0 { break; }
        let point=start+delta*t;
        if any(point<vec2<f32>(0.0)) || any(point>=screen) { break; }
        let z=mix(ndc0.z,ndc1.z,t);
        // Reversed depth: at or past infinity (0), or before the near plane (1).
        if z<=0.0 || z>1.0 { break; }
        let cell_size=f32(TILE_SIZE)*exp2(f32(level));
        let cell=floor(point/cell_size);
        let boundary=(cell+select(vec2<f32>(0.0),vec2<f32>(1.0),delta>=vec2<f32>(0.0)))*cell_size;
        let exits=select(vec2<f32>(1e20),(boundary-point)/select(vec2<f32>(1.0),delta,abs(delta)>vec2<f32>(1e-6)),abs(delta)>vec2<f32>(1e-6));
        let cell_end=min(1.0,t+max(min(exits.x,exits.y),0.0));
        let nearest=textureLoad(screen_depth_bounds,vec2<i32>(cell),level).r;
        // The segment's farthest point is still in front of the cell's nearest surface.
        if !helio_depth_is_nearer(nearest,helio_depth_farthest(z,mix(ndc0.z,ndc1.z,cell_end))) {
            t=cell_end+0.001/span; level=min(level+1,top); continue;
        }
        if level>0 { level-=1; continue; }
        let p=vec2<i32>(point);
        let pixel_boundary=floor(point)+select(vec2<f32>(0.0),vec2<f32>(1.0),delta>=vec2<f32>(0.0));
        let pixel_exits=select(vec2<f32>(1e20),(pixel_boundary-point)/select(vec2<f32>(1.0),delta,abs(delta)>vec2<f32>(1e-6)),abs(delta)>vec2<f32>(1e-6));
        let pixel_end=min(1.0,t+max(min(pixel_exits.x,pixel_exits.y),0.0));
        let depth=textureLoad(gbuf_depth,p,0);
        if !helio_is_sky(depth) {
            let surface=world_position(vec2<f32>(p)+0.5,depth);
            let ray_start=world_position(point,z);
            let ray_end=world_position(start+delta*pixel_end,mix(ndc0.z,ndc1.z,pixel_end));
            let surface_z=-(cameras[0].view*vec4<f32>(surface,1.0)).z;
            let z0=-(cameras[0].view*vec4<f32>(ray_start,1.0)).z;
            let z1=-(cameras[0].view*vec4<f32>(ray_end,1.0)).z;
            let thickness=max(0.025,max(z0,z1)*0.002);
            if max(z0,z1)-surface_z>0.003 && min(z0,z1)-surface_z<thickness { return true; }
        }
        t=pixel_end+0.001/span;
    }
    return false;
}

fn scalar_shadow_factor(light_idx: u32, world_pos: vec3<f32>, N: vec3<f32>, frag_coord: vec2<f32>, frame: u32) -> f32 {
    if !ENABLE_SHADOWS { return 1.0; }
    if light_idx >= globals.light_count { return 1.0; }

    let light = lights[light_idx];
    let requested=select(light.shadow_index!=0xffffffffu,(light._pad&4u)!=0u,(light._pad&8u)!=0u);
    if !requested || (light._pad&48u)==48u {return 1.0;}
    if (light._pad&64u)==0u && screen_occluded(light,world_pos,N) { return 0.0; }
    if light.shadow_index == 4294967295u { return 1.0; }

    var light_dir: vec3<f32>;
    if light.light_type == 0u {
        light_dir = normalize(-light.direction_outer.xyz);
    } else {
        light_dir = normalize(light.position_range.xyz - world_pos);
    }
    let NdotL = max(dot(N, light_dir), 0.0);
    let normal_offset = N * NORMAL_OFFSET_SCALE * (1.0 - NdotL);
    let biased_pos = world_pos + normal_offset;

    var layer: u32;
    if light.light_type > 0u && light.light_type < 2u {
        let to_frag = biased_pos - light.position_range.xyz;
        layer = light.shadow_index + point_light_face(to_frag);
        return sample_cascade_shadow(layer, 0u, 1.0, biased_pos, frag_coord, frame);
    } else if light.light_type == 0u {
        let dist = length(world_pos - cameras[0].position_near.xyz);
        let splits = globals.csm_splits;

        var cascade_a = 3u;
        var cascade_b = 3u;
        var blend = 0.0;
        const BLEND_ZONE = 0.1;

        if dist < splits.x * (1.0 - BLEND_ZONE / 2.0) {
            cascade_a = 0u;
        } else if dist < splits.x * (1.0 + BLEND_ZONE / 2.0) {
            cascade_a = 0u;
            cascade_b = 1u;
            blend = smoothstep(splits.x * (1.0 - BLEND_ZONE / 2.0), splits.x * (1.0 + BLEND_ZONE / 2.0), dist);
        } else if dist < splits.y * (1.0 - BLEND_ZONE / 2.0) {
            cascade_a = 1u;
        } else if dist < splits.y * (1.0 + BLEND_ZONE / 2.0) {
            cascade_a = 1u;
            cascade_b = 2u;
            blend = smoothstep(splits.y * (1.0 - BLEND_ZONE / 2.0), splits.y * (1.0 + BLEND_ZONE / 2.0), dist);
        } else if dist < splits.z * (1.0 - BLEND_ZONE / 2.0) {
            cascade_a = 2u;
        } else if dist < splits.z * (1.0 + BLEND_ZONE / 2.0) {
            cascade_a = 2u;
            cascade_b = 3u;
            blend = smoothstep(splits.z * (1.0 - BLEND_ZONE / 2.0), splits.z * (1.0 + BLEND_ZONE / 2.0), dist);
        } else {
            cascade_a = 3u;
        }

        let use_pcss = shadow_config.enable_pcss != 0u && shadow_config.cascades[cascade_a].pcss_light_size > 0.0;

        let layer_a = light.shadow_index + cascade_a;
        var shadow_a: f32;
        if use_pcss {
            shadow_a = sample_cascade_shadow_pcss(layer_a, cascade_a, biased_pos, frag_coord, frame);
        } else {
            let cascade_scale_a = 1.0 + f32(cascade_a) * 1.5;
            shadow_a = sample_cascade_shadow(layer_a, cascade_a, cascade_scale_a, biased_pos, frag_coord, frame);
        }

        if blend <= 0.001 { return shadow_a; }

        if cascade_b != cascade_a && blend > 0.001 {
            let use_pcss_b = shadow_config.enable_pcss != 0u && shadow_config.cascades[cascade_b].pcss_light_size > 0.0;
            let layer_b = light.shadow_index + cascade_b;
            var shadow_b: f32;
            if use_pcss_b {
                shadow_b = sample_cascade_shadow_pcss(layer_b, cascade_b, biased_pos, frag_coord, frame);
            } else {
                let cascade_scale_b = 1.0 + f32(cascade_b) * 1.5;
                shadow_b = sample_cascade_shadow(layer_b, cascade_b, cascade_scale_b, biased_pos, frag_coord, frame);
            }
            return mix(shadow_a, shadow_b, blend);
        }

        return shadow_a;
    } else {
        layer = light.shadow_index;
        return sample_cascade_shadow(layer, 0u, 1.0, biased_pos, frag_coord, frame);
    }
}

// Tint of the stained-glass panes a receiver lies behind. Each of the four
// nearest texels is depth-tested on its own, then the results are weighted
// bilinearly. Bilinearly filtering the pane depth (alpha) first and testing
// once failed the test along every pane edge wherever a neighbouring texel
// held no pane: an untinted rim and stair-stepped edges, most visible in the
// coarse cascades, where a half-resolution texel covers several centimetres.
fn glass_tint(uv:vec2f, layer:i32, receiver:f32)->vec3f {
    return budget_glass_tint(shadow_transmittance,uv,layer,receiver);
}

// Tint of the translucent casters between a light and a receiver, sampled on
// the same face as the depth shadow (nearest cascade for directional lights).
fn glass_transmittance(id: u32, position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let light = lights[id];
    if light.shadow_index == 4294967295u { return vec3<f32>(1.0); }
    var light_dir = normalize(-light.direction_outer.xyz);
    if light.light_type != 0u { light_dir = normalize(light.position_range.xyz - position); }
    let biased = position + normal * NORMAL_OFFSET_SCALE * (1.0 - max(dot(normal, light_dir), 0.0));
    var layer = light.shadow_index;
    if light.light_type == 1u {
        layer += point_light_face(biased - light.position_range.xyz);
    } else if light.light_type == 0u {
        let dist = length(position - cameras[0].position_near.xyz);
        let splits = globals.csm_splits;
        layer += select(select(select(3u, 2u, dist < splits.z), 1u, dist < splits.y), 0u, dist < splits.x);
    }
    if layer >= arrayLength(&shadow_matrices) || budget_layer(layer) >= textureNumLayers(shadow_transmittance) {
        return vec3<f32>(1.0);
    }
    let clip = shadow_matrices[layer].mat * vec4<f32>(biased, 1.0);
    if clip.w <= 0.0 { return vec3<f32>(1.0); }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, -ndc.y * 0.5 + 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return vec3<f32>(1.0);
    }
    return glass_tint(uv, i32(layer), 1.0 - ndc.z);
}

fn shadow_factor(id: u32, position: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>, frame: u32) -> Visibility {
    let lit = scalar_shadow_factor(id,position,normal,pixel,frame);
    // The RGB-visibility pipelines run only while glass casters exist.
    if USE_RAY_TRANSMISSION && lit > 0.0 {
        return visibility_from_rgb(lit * glass_transmittance(id,position,normal));
    }
    return Visibility(lit);
}

fn shadow_receiver(position: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> vec3<f32> {
    return position;
}
fn shadow_factor_from_receiver(id: u32, origin: vec3<f32>, position: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>, frame: u32) -> Visibility {
    return shadow_factor(id,position,normal,pixel,frame);
}

// One comparison against the dynamic or the static casters' atlas, and an
// occluder depth from either (the shadow atlas's `budget_*`; 32 and 16 are
// the caster-kind bits each atlas holds).
fn budget_compare_dynamic(uv:vec2f,layer:u32,depth:f32)->f32 {
    return budget_compare(shadow_atlas,shadow_sampler,uv,layer,depth,32u);
}
fn budget_depth_dynamic(pixel:vec2i,layer:u32)->f32 {
    return budget_depth(shadow_atlas,pixel,layer,32u);
}
fn budget_compare_static(uv:vec2f,layer:u32,depth:f32)->f32 {
    return budget_compare(static_shadow_atlas,shadow_sampler,uv,layer,depth,16u);
}
fn budget_depth_static(pixel:vec2i,layer:u32)->f32 {
    return budget_depth(static_shadow_atlas,pixel,layer,16u);
}
