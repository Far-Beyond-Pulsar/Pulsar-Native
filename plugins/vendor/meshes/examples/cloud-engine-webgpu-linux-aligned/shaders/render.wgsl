// Fullscreen volumetric cloud ray marcher.
// No scene geometry or vertex buffers are required; the vertex shader emits one
// fullscreen triangle and the fragment shader integrates the cloud volume.

struct RenderParams {
  resolution_time: vec4<f32>,          // width, height, time, frame index
  camera_position_tan_fov: vec4<f32>,  // camera xyz, tan(verticalFov / 2)
  camera_forward_exposure: vec4<f32>,  // forward xyz, exposure
  camera_right_steps: vec4<f32>,       // right xyz, primary ray steps
  camera_up_detail: vec4<f32>,         // up xyz, edge detail strength
  sun_direction_intensity: vec4<f32>,  // direction-to-sun xyz, intensity
  sun_color_extinction: vec4<f32>,     // linear sun rgb, extinction
  sky_top_ambient: vec4<f32>,          // linear sky-top rgb, ambient strength
  sky_horizon_seed: vec4<f32>,         // linear horizon rgb, seed
  bounds_min_density: vec4<f32>,       // world-space AABB minimum xyz, density scale
  bounds_max_shadow: vec4<f32>,        // world-space AABB maximum xyz, shadow strength
  options: vec4<f32>,                  // amount, quality tier, paused, reserved
  art_style: vec4<f32>,                // enabled, toon bands, outline, sculpt
  art_cloud_color: vec4<f32>,          // linear cloud rgb, print grain
  art_shadow_color: vec4<f32>,         // linear shadow rgb, ribbon parameter
  art_sky_color: vec4<f32>,            // linear sky rgb, moon angular radius
  art_moon_color: vec4<f32>,           // linear moon rgb, moon glow
}

struct VertexOutput {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
}

@group(0) @binding(0) var<uniform> params: RenderParams;
@group(0) @binding(1) var cloud_volume: texture_3d<f32>;
@group(0) @binding(2) var volume_sampler: sampler;
// The regular Helio scene (cubes, voxels, lights) is rendered first. The cloud
// ray marcher then uses it as its background, keeping those objects on Helio's
// standard GPU-instanced path instead of duplicating a mesh renderer here.
@group(0) @binding(3) var scene_color: texture_2d<f32>;
@group(0) @binding(4) var scene_sampler: sampler;
@group(0) @binding(5) var luna_texture: texture_2d<f32>;
@group(0) @binding(6) var luna_sampler: sampler;
@group(0) @binding(7) var moon2_texture: texture_2d<f32>;

const PI: f32 = 3.141592653589793;
const MAX_PRIMARY_STEPS: u32 = 112u;
const MAX_LIGHT_STEPS: u32 = 8u;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
  let positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 3.0, -1.0),
    vec2<f32>(-1.0,  3.0)
  );

  var output: VertexOutput;
  let position = positions[vertex_index];
  output.position = vec4<f32>(position, 0.0, 1.0);
  output.uv = position * 0.5 + vec2<f32>(0.5);
  return output;
}

fn hash12(p: vec2<f32>) -> f32 {
  let h = dot(p, vec2<f32>(127.1, 311.7));
  return fract(sin(h) * 43758.5453123);
}

fn soft_circle(uv: vec2<f32>, center: vec2<f32>, radius: f32) -> f32 {
  return 1.0 - smoothstep(radius * 0.68, radius, length(uv - center));
}

fn intersect_box(ray_origin: vec3<f32>, ray_direction: vec3<f32>, bounds_min: vec3<f32>, bounds_max: vec3<f32>) -> vec2<f32> {
  let safe_direction = vec3<f32>(
    select(-0.00001, ray_direction.x, abs(ray_direction.x) > 0.00001),
    select(-0.00001, ray_direction.y, abs(ray_direction.y) > 0.00001),
    select(-0.00001, ray_direction.z, abs(ray_direction.z) > 0.00001)
  );
  let inverse_direction = vec3<f32>(1.0) / safe_direction;
  let t0 = (bounds_min - ray_origin) * inverse_direction;
  let t1 = (bounds_max - ray_origin) * inverse_direction;
  let near_values = min(t0, t1);
  let far_values = max(t0, t1);
  let near_distance = max(max(near_values.x, near_values.y), near_values.z);
  let far_distance = min(min(far_values.x, far_values.y), far_values.z);
  return vec2<f32>(near_distance, far_distance);
}

fn sample_cloud(world_position: vec3<f32>) -> vec3<f32> {
  let bounds_min = params.bounds_min_density.xyz;
  let bounds_max = params.bounds_max_shadow.xyz;
  let uv = (world_position - bounds_min) / (bounds_max - bounds_min);

  if (any(uv < vec3<f32>(0.0)) || any(uv > vec3<f32>(1.0))) {
    return vec3<f32>(0.0);
  }

  let sample_value = textureSampleLevel(cloud_volume, volume_sampler, uv, 0.0);
  let base_density = sample_value.r;
  let fine_noise = sample_value.g;
  let thickness = sample_value.a; // baked in simulate.wgsl w channel
  let edge_weight = clamp(4.0 * base_density * (1.0 - base_density), 0.0, 1.0);
  let erosion = (0.53 - fine_noise) * params.camera_up_detail.w * edge_weight * 0.44;
  let shaped_density = max(0.0, base_density - erosion);
  return vec3<f32>(shaped_density * params.bounds_min_density.w, fine_noise, thickness);
}

fn cloud_gradient(world_position: vec3<f32>, center_density: f32) -> vec3<f32> {
  let quality = clamp(params.options.y, 0.0, 2.0);
  let epsilon = mix(0.16, 0.095, quality * 0.5);
  return vec3<f32>(
    sample_cloud(world_position + vec3<f32>(epsilon, 0.0, 0.0)).x - center_density,
    sample_cloud(world_position + vec3<f32>(0.0, epsilon, 0.0)).x - center_density,
    sample_cloud(world_position + vec3<f32>(0.0, 0.0, epsilon)).x - center_density
  );
}

fn light_transmittance(world_position: vec3<f32>, sun_direction: vec3<f32>) -> f32 {
  var optical_depth = 0.0;
  var sample_position = world_position;
  let quality = clamp(params.options.y, 0.0, 2.0);
  let requested_steps = u32(4.0 + quality * 2.0);
  let step_length = mix(0.62, 0.43, quality * 0.5);

  for (var step: u32 = 0u; step < MAX_LIGHT_STEPS; step = step + 1u) {
    if (step >= requested_steps) {
      break;
    }
    sample_position = sample_position + sun_direction * step_length;
    optical_depth = optical_depth + sample_cloud(sample_position).x * step_length;
  }

  let extinction = params.sun_color_extinction.w;
  let shadow_strength = params.bounds_max_shadow.w;
  return exp(-optical_depth * extinction * shadow_strength);
}

fn artistic_light_visibility(world_position: vec3<f32>, sun_direction: vec3<f32>) -> f32 {
  var optical_depth = 0.0;
  var sample_position = world_position;
  let step_length = 0.68;

  // The sculpted renderer is intentionally cheaper than the realistic light
  // march. Three broad samples preserve readable bands and moon-side shadowing.
  for (var step: u32 = 0u; step < 3u; step = step + 1u) {
    sample_position = sample_position + sun_direction * step_length;
    optical_depth = optical_depth + sample_cloud(sample_position).x * step_length;
  }

  return exp(-optical_depth * params.sun_color_extinction.w * params.bounds_max_shadow.w * 0.82);
}

fn henyey_greenstein(cosine_theta: f32, anisotropy: f32) -> f32 {
  let g2 = anisotropy * anisotropy;
  let denominator = pow(max(1.0 + g2 - 2.0 * anisotropy * cosine_theta, 0.0001), 1.5);
  return (1.0 - g2) / (4.0 * PI * denominator);
}

fn realistic_sky_color(ray_direction: vec3<f32>, sun_direction: vec3<f32>) -> vec3<f32> {
  let horizon = params.sky_horizon_seed.rgb;
  let top = params.sky_top_ambient.rgb;
  let sky_factor = pow(clamp(ray_direction.y * 0.52 + 0.48, 0.0, 1.0), 0.62);
  var color = mix(horizon, top, sky_factor);

  let sun_alignment = max(dot(ray_direction, sun_direction), 0.0);
  let sun_disc = smoothstep(0.99972, 0.99994, sun_alignment);
  let sun_glow = pow(sun_alignment, 28.0) * 0.25 + pow(sun_alignment, 160.0) * 0.9;
  let horizon_glow = pow(max(0.0, 1.0 - abs(ray_direction.y)), 9.0) * 0.065;
  let sun_light = params.sun_color_extinction.rgb * params.sun_direction_intensity.w;
  color = color + sun_light * (sun_disc * 5.5 + sun_glow + horizon_glow);
  return color;
}

fn artistic_sky_color(ray_direction: vec3<f32>, sun_direction: vec3<f32>) -> vec3<f32> {
  let base_sky = params.art_sky_color.rgb;
  let vertical = clamp(ray_direction.y * 0.5 + 0.5, 0.0, 1.0);
  let side_falloff = 0.86 + 0.14 * pow(max(1.0 - abs(ray_direction.x), 0.0), 2.0);
  var color = base_sky * mix(0.62, 1.18, vertical) * side_falloff;

  let reference_axis = select(
    vec3<f32>(0.0, 1.0, 0.0),
    vec3<f32>(1.0, 0.0, 0.0),
    abs(sun_direction.y) > 0.92
  );
  let tangent = normalize(cross(reference_axis, sun_direction));
  let bitangent = normalize(cross(sun_direction, tangent));
  let moon_radius = max(params.art_sky_color.w, 0.001);
  let angular_scale = max(sin(moon_radius), 0.001);
  let moon_uv = vec2<f32>(dot(ray_direction, tangent), dot(ray_direction, bitangent)) / angular_scale;
  let moon_radius_uv = length(moon_uv);
  let facing_moon = dot(ray_direction, sun_direction) > 0.0;
  let disc = select(0.0, 1.0 - smoothstep(0.94, 1.02, moon_radius_uv), facing_moon);

  let crater_a = soft_circle(moon_uv, vec2<f32>(-0.28, 0.24), 0.20);
  let crater_b = soft_circle(moon_uv, vec2<f32>(0.35, -0.08), 0.15);
  let crater_c = soft_circle(moon_uv, vec2<f32>(0.02, -0.38), 0.11);
  let crater_d = soft_circle(moon_uv, vec2<f32>(0.42, 0.36), 0.08);
  let moon_cells = floor((moon_uv + vec2<f32>(1.0)) * 8.0);
  let moon_mottle = hash12(moon_cells + vec2<f32>(params.sky_horizon_seed.w));
  let moon_surface = clamp(
    1.03 - crater_a * 0.13 - crater_b * 0.10 - crater_c * 0.11 - crater_d * 0.08
      + (moon_mottle - 0.5) * 0.095,
    0.68,
    1.16
  );

  let alignment = max(dot(ray_direction, sun_direction), 0.0);
  let halo = select(
    0.0,
    exp(-max(moon_radius_uv - 0.9, 0.0) * 3.25) * smoothstep(0.16, 0.98, alignment),
    facing_moon
  );
  let moon_color = params.art_moon_color.rgb;
  color = color + moon_color * (
    disc * moon_surface * (1.25 + params.art_moon_color.w * 0.58)
      + halo * params.art_moon_color.w * 0.12
  );
  return color;
}

fn sky_color(ray_direction: vec3<f32>, sun_direction: vec3<f32>) -> vec3<f32> {
  if (params.art_style.x > 0.5) {
    return artistic_sky_color(ray_direction, sun_direction);
  }
  return realistic_sky_color(ray_direction, sun_direction);
}

fn rotate_2d(point: vec2<f32>, angle: f32) -> vec2<f32> {
  let c = cos(angle);
  let s = sin(angle);
  return vec2<f32>(point.x * c - point.y * s, point.x * s + point.y * c);
}

// This is a sky sprite rather than a finite mesh: it is sampled from the view
// ray, stays at infinity with Helio's sky, and therefore cannot parallax when
// the flycam moves. The phase masks reuse one detailed alpha texture.
fn luna_layer(
  ray_direction: vec3<f32>,
  center_direction: vec3<f32>,
  angular_radius: f32,
  rotation: f32,
  tint: vec3<f32>
) -> vec3<f32> {
  let reference_axis = select(
    vec3<f32>(0.0, 1.0, 0.0),
    vec3<f32>(1.0, 0.0, 0.0),
    abs(center_direction.y) > 0.92
  );
  let tangent = normalize(cross(reference_axis, center_direction));
  let bitangent = normalize(cross(center_direction, tangent));
  let local = vec2<f32>(dot(ray_direction, tangent), dot(ray_direction, bitangent))
    / max(sin(angular_radius), 0.001);
  let distance_from_center = length(local);
  let facing = dot(ray_direction, center_direction) > 0.0;
  let disc = select(0.0, 1.0 - smoothstep(0.985, 1.015, distance_from_center), facing);
  let uv = rotate_2d(local, rotation) * 0.5 + vec2<f32>(0.5);
  let source = textureSampleLevel(luna_texture, luna_sampler, uv, 0.0);

  let alpha = source.a * disc;
  let luminance = dot(source.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
  // Deliberate broad glow: it is tinted and fades outside the authored disc,
  // unlike the former hard white rim from Helio's analytic sun.
  let glow = select(
    0.0,
    (1.0 - smoothstep(1.0, 1.42, distance_from_center)) * 0.085,
    facing
  );
  return tint * (luminance * alpha + glow);
}

fn moon2_layer(
  ray_direction: vec3<f32>,
  center_direction: vec3<f32>,
  angular_radius: f32,
  rotation: f32,
  tint: vec3<f32>
) -> vec3<f32> {
  let reference_axis = select(
    vec3<f32>(0.0, 1.0, 0.0),
    vec3<f32>(1.0, 0.0, 0.0),
    abs(center_direction.y) > 0.92
  );
  let tangent = normalize(cross(reference_axis, center_direction));
  let bitangent = normalize(cross(center_direction, tangent));
  let local = vec2<f32>(dot(ray_direction, tangent), dot(ray_direction, bitangent))
    / max(sin(angular_radius), 0.001);
  let distance_from_center = length(local);
  let facing = dot(ray_direction, center_direction) > 0.0;
  let disc = select(0.0, 1.0 - smoothstep(0.985, 1.015, distance_from_center), facing);
  let uv = rotate_2d(local, rotation) * 0.5 + vec2<f32>(0.5);
  let source = textureSampleLevel(moon2_texture, luna_sampler, uv, 0.0);
  let glow = select(
    0.0,
    (1.0 - smoothstep(1.0, 1.42, distance_from_center)) * 0.085,
    facing
  );
  return tint * (source.r * source.a * disc + glow);
}

fn multi_luna_sky(ray_direction: vec3<f32>) -> vec3<f32> {
  // An equilateral sky triangle: three anchors at one shared elevation and
  // 120-degree azimuth intervals around the complete world horizon.
  let full = luna_layer(
    ray_direction, normalize(vec3<f32>(0.0, 0.45, 0.893)), 0.050, 0.025,
    vec3<f32>(1.00, 0.96, 0.88)
  );
  let moon2 = moon2_layer(
    ray_direction, normalize(vec3<f32>(0.774, 0.45, -0.447)), 0.100, -0.18,
    vec3<f32>(0.60, 0.78, 1.00)
  );
  let companion = luna_layer(
    ray_direction, normalize(vec3<f32>(-0.774, 0.45, -0.447)), 0.200, 0.27,
    vec3<f32>(1.00, 0.55, 0.30)
  );
  return full + moon2 + companion;
}

// Exact sky-colour portion of the original artistic Cloud Engine path. The
// old procedural moon that followed it is intentionally omitted because the
// textured multi-moon layer below now owns celestial rendering.
fn original_artistic_sky_color(ray_direction: vec3<f32>) -> vec3<f32> {
  let vertical = clamp(ray_direction.y * 0.5 + 0.5, 0.0, 1.0);
  let side_falloff = 0.86 + 0.14 * pow(max(1.0 - abs(ray_direction.x), 0.0), 2.0);
  return params.art_sky_color.rgb * mix(0.62, 1.18, vertical) * side_falloff;
}

fn aces_tonemap(color: vec3<f32>) -> vec3<f32> {
  let a = 2.51;
  let b = 0.03;
  let c = 2.43;
  let d = 0.59;
  let e = 0.14;
  return clamp((color * (a * color + vec3<f32>(b))) / (color * (c * color + vec3<f32>(d)) + vec3<f32>(e)), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
  let resolution = max(params.resolution_time.xy, vec2<f32>(1.0));
  let aspect = resolution.x / resolution.y;
  // WebGPU maps clip-space +Y to the top of the framebuffer. The interpolated
  // fullscreen-triangle UV is therefore 1 at the top, so no extra 1-uv flip is
  // needed here. This is the exact projection used by the JavaScript brush ray.
  let screen = vec2<f32>(input.uv.x * 2.0 - 1.0, input.uv.y * 2.0 - 1.0);

  let ray_origin = params.camera_position_tan_fov.xyz;
  let ray_direction = normalize(
    params.camera_forward_exposure.xyz
    + params.camera_right_steps.xyz * (screen.x * aspect * params.camera_position_tan_fov.w)
    + params.camera_up_detail.xyz * (screen.y * params.camera_position_tan_fov.w)
  );

  let sun_direction = normalize(params.sun_direction_intensity.xyz);
  // Fullscreen clip-space +Y is the top of this viewport, while texture V=0
  // is the top row of Helio's offscreen target. Flip V before sampling so the
  // scene camera and the cloud ray camera share the same world orientation.
  let scene_uv = vec2<f32>(input.uv.x, 1.0 - input.uv.y);
  // The regular scene target contains the cubes and axes but clears to black.
  // Fill only that empty background with our controlled blue gradient, leaving
  // no hidden Helio sun disc behind the authored moon textures.
  let scene_sample = textureSampleLevel(scene_color, scene_sampler, scene_uv, 0.0).rgb;
  let scene_presence = smoothstep(0.012, 0.075, max(max(scene_sample.r, scene_sample.g), scene_sample.b));
  let background = mix(original_artistic_sky_color(ray_direction), scene_sample, scene_presence)
    + multi_luna_sky(ray_direction);

  let hit = intersect_box(ray_origin, ray_direction, params.bounds_min_density.xyz, params.bounds_max_shadow.xyz);
  let near_distance = max(hit.x, 0.0);
  let far_distance = hit.y;

  var radiance = background;

  if (far_distance > near_distance) {
    let requested_steps = u32(clamp(params.camera_right_steps.w, 16.0, f32(MAX_PRIMARY_STEPS)));
    let step_length = (far_distance - near_distance) / max(f32(requested_steps), 1.0);
    let frame_jitter = hash12(input.position.xy + vec2<f32>(params.resolution_time.w * 0.754877666));
    var distance_along_ray = near_distance + frame_jitter * step_length;
    var transmittance = 1.0;
    var integrated_light = vec3<f32>(0.0);

    let view_sun_alignment = clamp(dot(ray_direction, sun_direction), -1.0, 1.0);
    let phase_forward = henyey_greenstein(view_sun_alignment, 0.68);
    let phase_backward = henyey_greenstein(view_sun_alignment, -0.22);
    let phase = phase_forward * 0.78 + phase_backward * 0.22;

    for (var step: u32 = 0u; step < MAX_PRIMARY_STEPS; step = step + 1u) {
      if (step >= requested_steps || distance_along_ray > far_distance || transmittance < 0.008) {
        break;
      }

      let world_position = ray_origin + ray_direction * distance_along_ray;
      let density_sample = sample_cloud(world_position);
      let density = density_sample.x;

      if (density > 0.004) {
        let height_fraction = clamp(
          (world_position.y - params.bounds_min_density.y)
          / (params.bounds_max_shadow.y - params.bounds_min_density.y),
          0.0,
          1.0
        );

        if (params.art_style.x > 0.5) {
          let sculpt = clamp(params.art_style.w, 0.0, 1.0);
          let sculpted_density = mix(density, smoothstep(0.035, 0.62, density), sculpt);
          let sample_alpha = 1.0 - exp(
            -sculpted_density * params.sun_color_extinction.w * step_length * mix(1.05, 1.95, sculpt)
          );
          let sun_visibility = artistic_light_visibility(world_position, sun_direction);
          let gradient = cloud_gradient(world_position, density);
          let gradient_strength = clamp(length(gradient) * 3.2, 0.0, 1.0);
          let normal = normalize(-gradient + vec3<f32>(0.0001, 0.0002, 0.0001));
          let diffuse = max(dot(normal, sun_direction), 0.0);
          let view_facing = abs(dot(normal, -ray_direction));
          let rim = pow(max(1.0 - view_facing, 0.0), 2.2);
          let bands = max(params.art_style.y, 2.0);
          let raw_shade = clamp(
            0.16
              + diffuse * 0.43
              + sun_visibility * 0.22
              + height_fraction * 0.10
              + rim * 0.08,
            0.0,
            1.0
          );
          let quantized_shade = floor(raw_shade * (bands - 1.0) + 0.5) / max(bands - 1.0, 1.0);
          let shadow_color = params.art_shadow_color.rgb;
          let cloud_color = params.art_cloud_color.rgb;
          var local_light = mix(shadow_color, cloud_color, quantized_shade);
          local_light = mix(local_light, shadow_color * 0.70, (1.0 - sun_visibility) * 0.34);
          local_light = local_light + params.art_moon_color.rgb
            * diffuse * sun_visibility * params.sun_direction_intensity.w * 0.095;

          let silhouette = pow(max(1.0 - view_facing, 0.0), 1.35);
          let ink_mask = clamp(
            silhouette * (0.25 + gradient_strength * 0.75) * params.art_style.z * 0.92,
            0.0,
            0.86
          );
          local_light = mix(local_light, shadow_color * 0.34, ink_mask);
          local_light = local_light + cloud_color * rim * 0.055;

          integrated_light = integrated_light + transmittance * sample_alpha * local_light;
          transmittance = transmittance * (1.0 - sample_alpha);
        } else {
          let extinction = params.sun_color_extinction.w;
          let sample_alpha = 1.0 - exp(-density * extinction * step_length);
          let sun_visibility = light_transmittance(world_position, sun_direction);

          let ambient_gradient = mix(params.sky_horizon_seed.rgb, params.sky_top_ambient.rgb, 0.25 + height_fraction * 0.55);
          var ambient_occluder = density * 0.28;
          if (params.options.y > 1.5) {
            // Still mode spends one extra density lookup on a cheap skylight probe.
            ambient_occluder = ambient_occluder + sample_cloud(world_position + vec3<f32>(0.0, 0.46, 0.0)).x * 0.55;
          }
          let ambient_visibility = exp(-ambient_occluder * 1.15);
          let ambient = ambient_gradient * params.sky_top_ambient.w
            * (0.75 + height_fraction * 0.32)
            * mix(0.52, 1.0, ambient_visibility);

          let multiple_scattering = (
            sun_visibility
            + sqrt(max(sun_visibility, 0.0)) * 0.28
            + pow(max(sun_visibility, 0.0), 0.25) * 0.10
          ) / 1.38;
          let powder = 1.0 - exp(-density * 2.35);
          let silver_lining = pow(sun_visibility, 3.0)
            * pow(1.0 - clamp(density, 0.0, 1.0), 2.0)
            * (0.04 + phase * 2.4);
          let direct_strength = multiple_scattering * (0.18 + phase * 7.8 + powder * 0.16)
            + silver_lining * 0.34;
          let direct = params.sun_color_extinction.rgb * params.sun_direction_intensity.w * direct_strength;
          let cool_core = vec3<f32>(0.72, 0.82, 0.94) * (1.0 - sun_visibility) * 0.047;
          let local_light = ambient + direct + cool_core;

          integrated_light = integrated_light + transmittance * sample_alpha * local_light;
          transmittance = transmittance * (1.0 - sample_alpha);
        }
      }

      distance_along_ray = distance_along_ray + step_length;
    }

    radiance = integrated_light + background * transmittance;
  }

  let exposure = params.camera_forward_exposure.w;
  if (params.art_style.x > 0.5) {
    let exposed = max(radiance * exposure, vec3<f32>(0.0));
    let compressed = exposed / (vec3<f32>(1.0) + exposed * 0.68);
    let gamma_color = pow(clamp(compressed, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
    let grain_strength = clamp(params.art_cloud_color.w, 0.0, 1.0);
    let grain = (hash12(input.position.xy + vec2<f32>(params.sky_horizon_seed.w * 1.73)) - 0.5)
      * grain_strength * 0.034;
    let levels = mix(96.0, 30.0, grain_strength);
    let printed = floor(clamp(gamma_color + vec3<f32>(grain), vec3<f32>(0.0), vec3<f32>(1.0)) * levels + vec3<f32>(0.5)) / levels;
    return vec4<f32>(printed, 1.0);
  }

  let mapped = aces_tonemap(max(radiance * exposure, vec3<f32>(0.0)));
  let dither = (hash12(input.position.xy + vec2<f32>(params.sky_horizon_seed.w)) - 0.5) / 255.0;
  let display_color = pow(clamp(mapped + vec3<f32>(dither), vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2));
  return vec4<f32>(display_color, 1.0);
}
