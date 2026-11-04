fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453123);
}

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 127.1) * 43758.5453123);
}

fn yiq2rgb(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        dot(c, vec3<f32>(1.0,  0.956,  0.621)),
        dot(c, vec3<f32>(1.0, -0.272, -0.647)),
        dot(c, vec3<f32>(1.0, -1.106,  1.703)),
    );
}

fn rgb2yiq(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        dot(c, vec3<f32>(0.299, 0.587, 0.114)),
        dot(c, vec3<f32>(0.596, -0.274, -0.322)),
        dot(c, vec3<f32>(0.211, -0.523, 0.312)),
    );
}

fn blur_ring(uv: vec2<f32>, radius: f32, n: u32) -> vec3<f32> {
    var acc = vec3<f32>(0.0);
    let inv = 1.0 / f32(n);
    for (var i = 0u; i < n; i++) {
        let a = 6.2832 * f32(i) * inv;
        acc += textureSampleLevel(hdr_input, linear_samp, uv + vec2<f32>(cos(a), sin(a)) * radius, 0.0).rgb;
    }
    return acc * inv;
}

fn barrel_distort(uv: vec2<f32>, amount: f32) -> vec2<f32> {
    let c = uv - vec2<f32>(0.5, 0.5);
    let r2 = dot(c, c);
    return uv + c * r2 * amount;
}

fn ccd_smear(uv: vec2<f32>, px: vec2<f32>) -> vec3<f32> {
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var i = 1u; i < 6u; i++) {
        let o = f32(i) * px.y * 1.5;
        let s = textureSampleLevel(hdr_input, linear_samp, uv - vec2<f32>(0.0, o), 0.0).rgb;
        let luma = dot(s, vec3<f32>(0.299, 0.587, 0.114));
        let bright = smoothstep(0.92, 1.0, luma);
        let w = bright / f32(i);
        acc += s * w;
        wsum += w;
    }
    if wsum > 0.0001 {
        return clamp(acc / wsum, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return vec3<f32>(0.0);
}

fn user_effects(color: vec3<f32>, uv_in: vec2<f32>, dims: vec2<f32>) -> vec3<f32> {
    let tape_jitter  = pp_custom[0].y;
    let jitter_freq  = pp_custom[0].z;
    let flicker_amt  = pp_custom[0].w;
    let noise_amt    = pp_custom[1].x;
    let time         = pp_custom[1].y;

    let px = 1.0 / dims;
    let frame = floor(time * 60.0);

    let uv = barrel_distort(uv_in, 0.06);

    let line = uv.y * dims.y;

    let v_bounce = sin(time * 1.73) * 0.5 + sin(time * 3.11) * 0.25;
    let vu = uv + vec2<f32>(0.0, v_bounce * px.y);

    let roll_pos = fract(time * 0.025 + 0.3);
    let roll_width = 0.06;
    let roll_dist = abs(vu.y - roll_pos);
    let roll_weight = 1.0 - smoothstep(0.0, roll_width, roll_dist);
    let roll_shift = sin(line * 50.0 + time * 2.0) * roll_weight * 2.5;

    let jit = (sin(line * jitter_freq + time * 3.7) * 5.0
             + sin(line * 17.0 + time * 5.3) * 2.0) * tape_jitter;

    let event_period = 4.0;
    let event_seed = floor(time / event_period);
    let event_roll = hash11(event_seed);
    let event_active = step(0.72, event_roll);
    let event_phase = fract(time / event_period);
    let event_window = smoothstep(0.0, 0.08, event_phase) * (1.0 - smoothstep(0.15, 0.3, event_phase));
    let band_seed = event_seed * 13.7 + floor(line / 18.0);
    let band_active = step(0.5, hash11(band_seed));
    let tear_shift = (hash11(band_seed + 0.33) * 2.0 - 1.0) * 40.0
        * event_active * event_window * band_active;

    let ju = vu + vec2<f32>((jit + roll_shift + tear_shift) * px.x, 0.0);

    let yuv = blur_ring(ju, 0.5 * px.x, 5u);
    let y = rgb2yiq(yuv).x;

    let i_uv = ju + vec2<f32>(0.6 * px.x, 0.0);
    let i_base = rgb2yiq(blur_ring(i_uv, 3.0 * px.x, 9u)).y;

    let q_uv = ju + vec2<f32>(1.2 * px.x, 0.0);
    let q_base = rgb2yiq(blur_ring(q_uv, 1.5 * px.x, 9u)).z;

    let phase = sin(time * 0.37) * 0.08 + sin(time * 0.73) * 0.04;
    let cp = cos(phase);
    let sp = sin(phase);
    let i = i_base * cp - q_base * sp;
    let q = i_base * sp + q_base * cp;

    var result = yiq2rgb(vec3<f32>(y, i, q));

    let smear = ccd_smear(ju, px);
    result += smear * 0.25;

    for (var g = 0u; g < 4u; g++) {
        let gf = f32(g);
        let g_speed  = 0.008 + hash11(gf * 3.1 + 1.0) * 0.03;
        let g_offset = hash11(gf * 7.7 + 2.0);
        let g_on_period = 2.0 + hash11(gf * 5.3 + 3.0) * 3.0;
        let g_on_seed = floor(time / g_on_period) + gf * 91.7;
        let g_active = step(0.45, hash11(g_on_seed));
        let g_opacity = 0.4 + hash11(gf * 2.3 + 4.0) * 0.4;

        let gp = fract(g_offset + time * g_speed);
        let gd = abs(vu.y - gp);
        let gw = (1.0 - smoothstep(0.0, 0.02 + hash11(gf) * 0.015, gd)) * g_active;
        if gw > 0.005 {
            let gn = hash21(vec2<f32>(line + gf * 100.0, frame));
            result = mix(result, vec3<f32>(gn * 0.4 + 0.1), gw * g_opacity);
        }
    }

    let hs_pos = 0.965 + sin(time * 0.6) * 0.006;
    let hs_dist = uv.y - hs_pos;
    let hs_band = smoothstep(-0.02, 0.0, hs_dist) * (1.0 - smoothstep(0.0, 0.035, hs_dist));
    if hs_band > 0.001 {
        let noise_dims_hs = vec2<f32>(textureDimensions(noise_tex));
        let hn_px = vec2<f32>(uv.x * dims.x * 0.4 + time * 90.0, frame * 0.7);
        let hn = textureSampleLevel(noise_tex, noise_samp, hn_px / noise_dims_hs, 0.0).r;
        result = mix(result, vec3<f32>(hn), hs_band * 0.85);
        result += vec2<f32>(hash11(line + frame), 0.0).x * hs_band * 0.15;
    }

    result = pow(max(result, vec3<f32>(0.0)), vec3<f32>(1.4));
    result = 1.0 - pow(max(1.0 - result, vec3<f32>(0.0)), vec3<f32>(1.6));
    result = mix(result, result * result * result, 0.15);

    let scan = sin(uv.y * dims.y * 3.14159);
    result *= 1.0 - 0.06 * (1.0 - scan * scan);

    let noise_dims = vec2<f32>(textureDimensions(noise_tex));
    let jump = vec2<f32>(hash11(frame * 1.7 + 0.3), hash11(frame * 2.3 + 5.1)) * noise_dims;
    let grain_px = uv * dims + jump;
    let grain_uv = grain_px / noise_dims;
    let tex_grain = textureSampleLevel(noise_tex, noise_samp, grain_uv, 0.0).r;
    let px_id = floor(uv * dims);
    let seed = frame + hash21(px_id) * 1000.0;
    let r1 = hash21(vec2<f32>(seed + 1.0, seed * 0.3 + 2.0));

    let luma = dot(result, vec3<f32>(0.299, 0.587, 0.114));
    let noise_strength = (0.015 + 0.03 * (1.0 - luma)) * noise_amt;
    let grain = (tex_grain * 2.0 - 1.0) * noise_strength;
    result += grain;

    let cn = (r1 * 2.0 - 1.0) * 0.018 * (1.0 - luma) * noise_amt;
    result += vec3<f32>(cn * 0.35, cn * -0.2, cn * 0.4);

    let edge = length(fwidth(result));
    let crawl = sin(uv.x * dims.x * 0.5 + time * 50.0) * edge * 0.06;
    result += vec3<f32>(crawl * 0.4, -crawl * 0.25, crawl * 0.6);

    let center_dist = length(uv - vec2<f32>(0.5, 0.5));

    // ── Lens flare ───────────────────────────────────────────────────────────
    // Ghost reflections sampled from the bloom bright-pass textures:
    // for each pixel, ghost _i_ of a bright light source appears at
    // G_i = C + (L - C) * (-k_i), i.e. mirrored across center.
    // We look for the light L = C - (P - C)/k_i on the opposite side
    // and tint it with a prismatic spectrum + chromatic separation.
    let c = vec2<f32>(0.5, 0.5);
    let from_c = uv - c;
    let fdist = length(from_c);
    if fdist > 0.003 {
        let intensities = array<f32, 6>(0.35, 0.25, 0.18, 0.12, 0.08, 0.05);
        let ghost_k = array<f32, 6>(0.3, 0.5, 0.8, 1.2, 1.8, 2.8);
        for (var gi = 0u; gi < 6u; gi++) {
            let k = ghost_k[gi];
            let light_uv = c - from_c / k;
            if all(light_uv >= vec2<f32>(0.0)) && all(light_uv <= vec2<f32>(1.0)) {
                let ca = f32(gi) * 0.002;
                let lr = textureSampleLevel(bloom_2, linear_samp, light_uv + vec2<f32>(ca, 0.0), 0.0).r;
                let lg = textureSampleLevel(bloom_2, linear_samp, light_uv, 0.0).g;
                let lb = textureSampleLevel(bloom_2, linear_samp, light_uv - vec2<f32>(ca, 0.0), 0.0).b;
                let light_col = vec3<f32>(lr, lg, lb);
                let ll = max(max(lr, lg), lb);
                if ll > 0.01 {
                    let w = intensities[gi] * smoothstep(0.0, 0.3, ll);
                    let hue = vec3<f32>(0.0, 2.1, 4.2) + f32(gi) * 1.2;
                    let spectral = 0.5 + 0.5 * cos(hue);
                    result += light_col * spectral * w;
                }
            }
        }
        // Halo: central bright glow
        let halo_col = textureSampleLevel(bloom_0, linear_samp, uv, 0.0).rgb;
        let hl = max(max(halo_col.r, halo_col.g), halo_col.b);
        if hl > 0.02 {
            let hw = exp(-fdist * fdist * 25.0) * smoothstep(0.0, 0.2, hl);
            result += halo_col * hw * 0.4;
        }
    }

    // ── Edge chromatic aberration (camcorder lens) ───────────────────────────
    let ca_amt = center_dist * center_dist * 0.0025;
    let ca_r = textureSampleLevel(hdr_input, linear_samp, ju + vec2<f32>(ca_amt, 0.0), 0.0).r;
    let ca_b = textureSampleLevel(hdr_input, linear_samp, ju - vec2<f32>(ca_amt, 0.0), 0.0).b;
    result = mix(result, vec3<f32>(ca_r, result.g, ca_b), 0.6);

    let fl = hash21(vec2<f32>(frame * 0.01, 0.5));
    result *= 1.0 - 0.025 * flicker_amt * (fl * 2.0 - 1.0);

    let vig = 1.0 - smoothstep(0.35, 0.85, center_dist) * 0.4;
    result *= vig;

    return clamp(result, vec3<f32>(0.0), vec3<f32>(1.0));
}
