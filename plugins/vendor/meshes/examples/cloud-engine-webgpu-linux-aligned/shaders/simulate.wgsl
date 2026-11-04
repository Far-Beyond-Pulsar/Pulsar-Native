// Compute-driven cloud density simulation.
// The kernel deliberately stays within a portable compute subset: fixed workgroup
// size, bounded loops, POD uniforms, no derivatives, no subgroups and no atomics.

struct SimParams {
  time_step: vec4<f32>,           // dt, time, cloud amount, auto mode
  wind_turbulence: vec4<f32>,     // world-space wind xyz, turbulence
  brush_center_radius: vec4<f32>, // normalized volume xyz, radius relative to X extent
  brush_controls: vec4<f32>,      // strength, active, sign, pattern id
  flow_controls: vec4<f32>,       // dissipation, tearing, rotation, clear pulse
  volume_seed: vec4<f32>,         // volume dimensions xyz, seed
  art_controls: vec4<f32>,        // artistic mode, curl field, ribbon stretch, sculpt
}

@group(0) @binding(0) var<uniform> params: SimParams;
@group(0) @binding(1) var previous_volume: texture_3d<f32>;
@group(0) @binding(2) var volume_sampler: sampler;
@group(0) @binding(3) var next_volume: texture_storage_3d<rgba16float, write>;

fn hash31(p: vec3<f32>) -> f32 {
  var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
  let h = dot(q, q.yzx + vec3<f32>(33.33));
  q = q + vec3<f32>(h);
  return fract((q.x + q.y) * q.z);
}

fn noise3(p: vec3<f32>) -> f32 {
  let cell = floor(p);
  let local = fract(p);
  let blend = local * local * (vec3<f32>(3.0) - 2.0 * local);

  let n000 = hash31(cell + vec3<f32>(0.0, 0.0, 0.0));
  let n100 = hash31(cell + vec3<f32>(1.0, 0.0, 0.0));
  let n010 = hash31(cell + vec3<f32>(0.0, 1.0, 0.0));
  let n110 = hash31(cell + vec3<f32>(1.0, 1.0, 0.0));
  let n001 = hash31(cell + vec3<f32>(0.0, 0.0, 1.0));
  let n101 = hash31(cell + vec3<f32>(1.0, 0.0, 1.0));
  let n011 = hash31(cell + vec3<f32>(0.0, 1.0, 1.0));
  let n111 = hash31(cell + vec3<f32>(1.0, 1.0, 1.0));

  let nx00 = mix(n000, n100, blend.x);
  let nx10 = mix(n010, n110, blend.x);
  let nx01 = mix(n001, n101, blend.x);
  let nx11 = mix(n011, n111, blend.x);
  let nxy0 = mix(nx00, nx10, blend.y);
  let nxy1 = mix(nx01, nx11, blend.y);
  return mix(nxy0, nxy1, blend.z);
}

fn fbm3(position: vec3<f32>) -> f32 {
  var p = position;
  var amplitude = 0.54;
  var sum = 0.0;
  var normalization = 0.0;

  for (var octave: i32 = 0; octave < 3; octave = octave + 1) {
    sum = sum + noise3(p) * amplitude;
    normalization = normalization + amplitude;
    p = p * 2.03 + vec3<f32>(11.7, 7.1, 5.4);
    amplitude = amplitude * 0.5;
  }

  return sum / normalization;
}

fn vertical_band(y: f32, lower: f32, lower_soft: f32, upper_soft: f32, upper: f32) -> f32 {
  return smoothstep(lower, lower_soft, y) * (1.0 - smoothstep(upper_soft, upper, y));
}

fn ellipsoid_blob(p: vec3<f32>, center: vec3<f32>, scale: vec3<f32>) -> f32 {
  let distance = length((p - center) * scale);
  return 1.0 - smoothstep(0.19, 0.52, distance);
}

fn spiral_scroll(
  p: vec3<f32>,
  center: vec3<f32>,
  scale: vec3<f32>,
  turns: f32,
  phase: f32
) -> f32 {
  let q = (p - center) * scale;
  let radius = length(q.xy);
  let angle = atan2(q.y, q.x);
  let crest = cos(angle + radius * turns + phase) * 0.5 + 0.5;
  let tube = smoothstep(0.76, 0.985, crest);
  let radial_window = smoothstep(0.025, 0.10, radius)
    * (1.0 - smoothstep(0.38, 0.64, radius));
  let depth_window = exp(-abs(q.z) * 2.35);
  return tube * radial_window * depth_window;
}

fn cloud_pattern(p: vec3<f32>, pattern: i32, amount: f32, time: f32, seed: f32) -> f32 {
  if (amount <= 0.002) {
    return 0.0;
  }

  let seed_offset = vec3<f32>(seed * 0.017, seed * 0.031, seed * 0.047);
  let source_drift = vec3<f32>(time * 0.008, 0.0, -time * 0.004);
  let coverage_gate = smoothstep(0.0, 0.12, amount);

  // Cumulus: broad macro cells, flatter bases, rounded high-frequency billows.
  if (pattern == 0) {
    let macro_shape = fbm3(p * vec3<f32>(2.0, 2.45, 1.75) + seed_offset + source_drift);
    let billow = fbm3(p * vec3<f32>(5.3, 4.2, 4.7) + seed_offset * 1.7 - source_drift * 0.4);
    let combined = macro_shape * 0.68 + billow * 0.42 - p.y * 0.08;
    let threshold = mix(0.79, 0.43, amount);
    let body = smoothstep(threshold, threshold + 0.16, combined);
    let height = vertical_band(p.y, 0.07, 0.19, 0.72, 0.93);
    let flat_base = smoothstep(0.12, 0.19, p.y);
    return clamp(body * height * flat_base * coverage_gate, 0.0, 1.0);
  }

  // Layered deck: a wide, coherent blanket with soft rolling holes.
  if (pattern == 1) {
    let macro_shape = fbm3(p * vec3<f32>(1.35, 5.8, 1.25) + seed_offset + source_drift * 0.55);
    let holes = fbm3(p * vec3<f32>(3.1, 2.2, 3.0) + seed_offset * 2.2);
    let combined = macro_shape * 0.72 + holes * 0.32;
    let threshold = mix(0.76, 0.37, amount);
    let body = smoothstep(threshold, threshold + 0.15, combined);
    let height = vertical_band(p.y, 0.18, 0.30, 0.57, 0.72);
    return clamp(body * height * 0.9 * coverage_gate, 0.0, 1.0);
  }

  // High wisps: thin ridged strands in the upper part of the volume.
  if (pattern == 2) {
    let warp = fbm3(p * vec3<f32>(1.8, 2.6, 1.2) + seed_offset + source_drift * 1.6);
    let streak_position = vec3<f32>(
      p.x * 7.0 + warp * 2.3,
      p.y * 11.0,
      p.z * 2.1 - p.x * 1.7
    );
    let streak_noise = fbm3(streak_position + seed_offset * 1.4);
    let ridges = 1.0 - abs(streak_noise * 2.0 - 1.0);
    let threshold = mix(0.91, 0.62, amount);
    let strands = smoothstep(threshold, min(1.0, threshold + 0.12), ridges);
    let height = vertical_band(p.y, 0.55, 0.66, 0.88, 0.98);
    return clamp(strands * height * 0.54 * coverage_gate, 0.0, 1.0);
  }

  // Storm towers: dense low bases and vertically stretched convective cores.
  if (pattern == 3) {
    let tower_macro = fbm3(p * vec3<f32>(2.3, 1.35, 2.15) + seed_offset + source_drift * 0.45);
    let vertical_detail = fbm3(p * vec3<f32>(4.4, 3.0, 4.1) + seed_offset * 1.9);
    let base_noise = fbm3(p * vec3<f32>(1.4, 7.0, 1.35) + seed_offset * 0.7);
    let lift = (1.0 - p.y) * 0.18 + smoothstep(0.22, 0.64, p.y) * 0.06;
    let combined = tower_macro * 0.65 + vertical_detail * 0.39 + lift;
    let threshold = mix(0.77, 0.38, amount);
    let towers = smoothstep(threshold, threshold + 0.14, combined);
    let base = smoothstep(mix(0.82, 0.48, amount), 0.92, base_noise)
      * vertical_band(p.y, 0.05, 0.14, 0.34, 0.46);
    let height = vertical_band(p.y, 0.03, 0.11, 0.88, 0.99);
    return clamp(max(towers * height, base) * coverage_gate, 0.0, 1.0);
  }

  // Broken cells: sparse, isolated cloud islands with clear sky between them.
  if (pattern == 4) {
    let macro_shape = fbm3(p * vec3<f32>(2.65, 2.3, 2.45) + seed_offset + source_drift * 0.9);
    let cell = fbm3(p * vec3<f32>(6.2, 4.4, 5.7) + seed_offset * 1.6);
    let combined = macro_shape * 0.74 + cell * 0.35;
    let threshold = mix(0.88, 0.55, amount);
    let islands = smoothstep(threshold, threshold + 0.12, combined);
    let height = vertical_band(p.y, 0.10, 0.22, 0.67, 0.84);
    return clamp(islands * height * coverage_gate, 0.0, 1.0);
  }

  let curl_control = clamp(params.art_controls.y, 0.0, 2.5);
  let ribbon_control = clamp(params.art_controls.z, 0.0, 1.5);
  let turns = mix(7.0, 15.0, curl_control / 2.5);
  let drift_phase = time * 0.055 + seed * 0.031;

  // Moon scrolls: three broad spiral tubes, bulbous heads and a long brush-like tail.
  if (pattern == 5) {
    let scroll_a = spiral_scroll(
      p,
      vec3<f32>(0.24, 0.47, 0.43),
      vec3<f32>(1.0, 1.35, 1.35),
      turns,
      drift_phase
    );
    let scroll_b = spiral_scroll(
      p,
      vec3<f32>(0.72, 0.61, 0.58),
      vec3<f32>(1.08, 1.48, 1.3),
      -turns * 0.92,
      1.7 - drift_phase * 0.7
    );
    let scroll_c = spiral_scroll(
      p,
      vec3<f32>(0.55, 0.28, 0.50),
      vec3<f32>(1.2, 1.62, 1.5),
      turns * 1.12,
      3.0 + drift_phase * 0.45
    );

    let head_a = ellipsoid_blob(p, vec3<f32>(0.13, 0.55, 0.42), vec3<f32>(2.2, 3.0, 3.0));
    let head_b = ellipsoid_blob(p, vec3<f32>(0.84, 0.67, 0.58), vec3<f32>(2.4, 3.1, 3.2));
    let head_c = ellipsoid_blob(p, vec3<f32>(0.50, 0.20, 0.50), vec3<f32>(3.2, 4.5, 3.4));

    let ribbon_frequency = mix(5.5, 12.5, ribbon_control / 1.5);
    let ribbon_y = 0.39
      + sin((p.x + seed * 0.003) * ribbon_frequency + p.z * 1.8) * (0.045 + ribbon_control * 0.055)
      + sin(p.x * 21.0 - p.z * 2.4) * 0.018;
    let ribbon_width = mix(0.12, 0.055, ribbon_control / 1.5);
    let ribbon = (1.0 - smoothstep(ribbon_width * 0.45, ribbon_width, abs(p.y - ribbon_y)))
      * exp(-abs(p.z - 0.49) * 3.6)
      * smoothstep(0.02, 0.16, p.x)
      * (1.0 - smoothstep(0.83, 0.99, p.x));

    let scrolls = max(scroll_a, max(scroll_b, scroll_c));
    let heads = max(head_a, max(head_b, head_c));
    let billow = fbm3(p * vec3<f32>(5.0, 4.2, 4.7) + seed_offset * 1.8);
    let combined = max(heads, max(scrolls * (0.72 + billow * 0.42), ribbon * 0.86));
    let threshold = mix(0.78, 0.23, amount);
    let body = smoothstep(threshold, threshold + 0.19, combined);
    let depth_and_height = vertical_band(p.y, 0.02, 0.09, 0.91, 0.99)
      * vertical_band(p.z, 0.02, 0.08, 0.93, 0.99);
    return clamp(body * depth_and_height * coverage_gate, 0.0, 1.0);
  }

  // Wind ribbons: layered, sinusoidal brush strokes punctuated by compact eddies.
  let frequency = mix(5.0, 13.0, ribbon_control / 1.5);
  let width = mix(0.105, 0.045, ribbon_control / 1.5);
  let ribbon_a_y = 0.24 + sin(p.x * frequency + p.z * 2.2 + drift_phase) * 0.085;
  let ribbon_b_y = 0.47 + sin(p.x * (frequency * 0.78) - p.z * 2.8 + 1.8) * 0.095;
  let ribbon_c_y = 0.70 + sin(p.x * (frequency * 1.12) + p.z * 1.7 + 3.4) * 0.065;
  let band_a = 1.0 - smoothstep(width * 0.35, width, abs(p.y - ribbon_a_y));
  let band_b = 1.0 - smoothstep(width * 0.35, width, abs(p.y - ribbon_b_y));
  let band_c = 1.0 - smoothstep(width * 0.35, width, abs(p.y - ribbon_c_y));
  let depth_gate = exp(-abs(p.z - 0.50) * mix(2.2, 4.5, ribbon_control / 1.5));
  let eddy_a = spiral_scroll(p, vec3<f32>(0.30, 0.37, 0.48), vec3<f32>(1.3, 1.7, 1.5), turns * 1.1, drift_phase);
  let eddy_b = spiral_scroll(p, vec3<f32>(0.74, 0.58, 0.54), vec3<f32>(1.45, 1.9, 1.7), -turns, 2.0 - drift_phase);
  let bands = max(band_a, max(band_b, band_c)) * depth_gate;
  let fine = fbm3(p * vec3<f32>(6.4, 4.2, 5.6) + seed_offset + source_drift);
  let combined = max(bands * (0.68 + fine * 0.34), max(eddy_a, eddy_b) * 0.88);
  let threshold = mix(0.82, 0.30, amount);
  let body = smoothstep(threshold, threshold + 0.17, combined);
  let height = vertical_band(p.y, 0.04, 0.10, 0.90, 0.98);
  return clamp(body * height * coverage_gate, 0.0, 1.0);
}

fn vortex_velocity(p: vec2<f32>, center: vec2<f32>, direction: f32) -> vec2<f32> {
  let delta = p - center;
  let radius_squared = dot(delta, delta);
  let falloff = exp(-radius_squared * 17.0);
  return vec2<f32>(-delta.y, delta.x) * direction * falloff;
}

fn flow_velocity(p: vec3<f32>, time: f32) -> vec3<f32> {
  let wind = params.wind_turbulence.xyz;
  let turbulence = params.wind_turbulence.w;
  let rotation = params.flow_controls.z;
  let centered = p - vec3<f32>(0.5);

  // Analytic low-cost curl-like motion. It is deterministic and ports cleanly.
  let waves = vec3<f32>(
    sin(p.y * 9.0 + p.z * 6.0 + time * 0.37),
    sin(p.z * 7.0 + p.x * 5.0 - time * 0.29),
    sin(p.x * 8.0 + p.y * 10.0 + time * 0.23)
  );
  let gust = vec3<f32>(waves.x - waves.y, (waves.z - waves.x) * 0.24, waves.y - waves.z)
    * (0.31 * turbulence);
  let swirl = vec3<f32>(-centered.z, sin(time * 0.17 + p.x * 3.0) * 0.12, centered.x)
    * (0.64 * rotation);
  let breathing_phase = sin(time * 0.13 + p.y * 4.0 + p.z * 1.7);
  let breathing = centered * breathing_phase * (0.045 * turbulence);
  let convection = vec3<f32>(0.0, (1.0 - p.y) * (0.035 + turbulence * 0.035), 0.0);

  let artistic = params.art_controls.x;
  let curl_strength = params.art_controls.y;
  let ribbon_strength = params.art_controls.z;
  let vortices = vortex_velocity(p.xy, vec2<f32>(0.25, 0.48), 1.0)
    + vortex_velocity(p.xy, vec2<f32>(0.72, 0.62), -0.9)
    + vortex_velocity(p.xy, vec2<f32>(0.53, 0.27), 0.75);
  let art_curl = vec3<f32>(vortices.x, vortices.y * 0.72, sin(p.x * 8.0 + time * 0.18) * 0.045)
    * (0.92 * curl_strength);
  let art_ribbon = vec3<f32>(
    sin(p.y * 8.0 + p.z * 3.0 + time * 0.31),
    cos(p.x * 10.0 - time * 0.23) * 0.18,
    sin(p.x * 5.0 + p.y * 7.0 + time * 0.19) * 0.34
  ) * (0.24 * ribbon_strength);

  return wind + gust + swirl + breathing + convection + (art_curl + art_ribbon) * artistic;
}

@compute @workgroup_size(4, 4, 4)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  let dimensions = textureDimensions(next_volume);
  if (any(gid >= dimensions)) {
    return;
  }

  let coordinate = vec3<i32>(gid);
  let uv = (vec3<f32>(gid) + vec3<f32>(0.5)) / vec3<f32>(dimensions);

  let dt = max(params.time_step.x, 0.0);
  let time = params.time_step.y;
  let amount = clamp(params.time_step.z, 0.0, 1.0);
  let automatic = params.time_step.w;
  let pattern = i32(params.brush_controls.w + 0.5);
  let seed = params.volume_seed.w;
  let world_size = vec3<f32>(16.0, 11.0, 19.0);

  let fine_position = uv * vec3<f32>(13.0, 9.0, 12.0)
    + vec3<f32>(time * 0.07, -time * 0.035, time * 0.045)
    + vec3<f32>(seed * 0.071);
  let fine_noise = noise3(fine_position);
  let formation_target = cloud_pattern(uv, pattern, amount, time, seed);

  // A reset in Auto mode writes the chosen source directly. This avoids a blank
  // warm-up interval and makes formation/style changes deterministic. Draw mode
  // still clears to an empty volume.
  if (params.flow_controls.w > 0.5) {
    let reset_density = select(0.0, formation_target, automatic > 0.5);
    let reset_thickness = clamp(reset_density * 1.8, 0.0, 1.0);
    textureStore(next_volume, coordinate, vec4<f32>(reset_density, fine_noise, formation_target, reset_thickness));
    return;
  }

  let velocity = flow_velocity(uv, time);
  let previous_uv_unwrapped = uv - (velocity / world_size) * dt;
  let previous_uv = vec3<f32>(
    fract(previous_uv_unwrapped.x),
    clamp(previous_uv_unwrapped.y, 0.001, 0.999),
    fract(previous_uv_unwrapped.z)
  );

  var density = textureSampleLevel(previous_volume, volume_sampler, previous_uv, 0.0).r;

  // Natural fade and fine-scale erosion produce soft, non-destructive tearing.
  let dissipation = params.flow_controls.x;
  density = density * exp(-dissipation * dt);

  let edge_weight = clamp(4.0 * density * (1.0 - density), 0.0, 1.0);
  let tear_mask = smoothstep(0.48, 0.88, fine_noise) * (0.12 + edge_weight * 0.88);
  density = max(0.0, density - params.flow_controls.y * tear_mask * dt * 0.34);

  // Auto mode continuously relaxes toward the selected formation while air motion
  // remains free to move, rotate, stretch, erode and fade the current volume.
  if (automatic > 0.5 && dt > 0.0) {
    let source_rate = mix(0.75, 2.7, amount);
    let source_blend = 1.0 - exp(-source_rate * dt);
    density = mix(density, max(density, formation_target), source_blend);
  }

  // Brush injection works while paused. Measuring in world units makes the painted
  // volume genuinely spherical and lets the screen cursor track its projection.
  if (params.brush_controls.y > 0.5) {
    let brush_delta_world = (uv - params.brush_center_radius.xyz) * world_size;
    let brush_distance = length(brush_delta_world);
    let radius_world = max(params.brush_center_radius.w * world_size.x, 0.001);
    let falloff = 1.0 - smoothstep(radius_world * 0.32, radius_world, brush_distance);
    let signed_strength = params.brush_controls.x * params.brush_controls.z;
    density = density + signed_strength * falloff * 0.13;
  }

  density = clamp(density, 0.0, 1.0);
  // Thickness baked per voxel: 1.0 = dense core, 0.0 = wispy edge. No extra fetches in raymarch - w channel is free.
  let thickness = clamp(density * 1.6 - edge_weight * 0.25, 0.0, 1.0);
  textureStore(next_volume, coordinate, vec4<f32>(density, fine_noise, formation_target, thickness));
}
