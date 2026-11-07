// Irradiance from the probes at a point (README.md, "Deferred lighting"):
// the finest level whose grid holds it, the 8 probes around it weighted by
// position, by facing and by visibility (their distance moments), blended
// into the next level near its grid's edge. Needs `gi_common`, and
// `frame_gi` and `gi_probes` bound.

// Probe `index`'s irradiance / pi in direction `n` (its L1 harmonics).
fn gi_probe_irradiance(index: u32, n: vec3<f32>) -> vec3<f32> {
    let at = index * GI_PROBE_WORDS;
    let a = gi_half2(gi_probes[at]);
    let b = gi_half2(gi_probes[at + 1u]);
    let c = gi_half2(gi_probes[at + 2u]);
    let d = gi_half2(gi_probes[at + 3u]);
    let e = gi_half2(gi_probes[at + 4u]);
    let f = gi_half2(gi_probes[at + 5u]);
    let basis = gi_sh_basis(n);
    let r = vec4<f32>(a.x, b.y, d.x, e.y);
    let g = vec4<f32>(a.y, c.x, d.y, f.x);
    let bl = vec4<f32>(b.x, c.y, e.x, f.y);
    return max(vec3<f32>(dot(r, basis), dot(g, basis), dot(bl, basis)), vec3<f32>(0.0));
}

// Probe `index`'s distance moments toward unit direction `d` (bilinear over
// its octahedral texels).
fn gi_probe_moments(index: u32, d: vec3<f32>) -> vec2<f32> {
    let side = f32(GI_MOMENTS_SIDE);
    let uv = gi_oct_encode(d) * side - 0.5;
    let i0 = clamp(vec2<i32>(floor(uv)), vec2<i32>(0), vec2<i32>(i32(GI_MOMENTS_SIDE) - 1));
    let i1 = min(i0 + 1, vec2<i32>(i32(GI_MOMENTS_SIDE) - 1));
    let w = clamp(uv - vec2<f32>(i0), vec2<f32>(0.0), vec2<f32>(1.0));
    let at = index * GI_PROBE_WORDS + GI_MOMENTS_WORD;
    let m00 = gi_half2(gi_probes[at + u32(i0.y) * GI_MOMENTS_SIDE + u32(i0.x)]);
    let m10 = gi_half2(gi_probes[at + u32(i0.y) * GI_MOMENTS_SIDE + u32(i1.x)]);
    let m01 = gi_half2(gi_probes[at + u32(i1.y) * GI_MOMENTS_SIDE + u32(i0.x)]);
    let m11 = gi_half2(gi_probes[at + u32(i1.y) * GI_MOMENTS_SIDE + u32(i1.x)]);
    return mix(mix(m00, m10, w.x), mix(m01, m11, w.x), w.y);
}

// Level `level`'s irradiance / pi at eye-relative point `p` with normal `n`,
// and the weight the probes found (0: none usable).
fn gi_level_irradiance(level: u32, p: vec3<f32>, n: vec3<f32>) -> vec4<f32> {
    let grid = frame_gi.levels[level];
    let spacing = grid.origin.w;
    // Off the surface by a fifth of the spacing (along its normal), so the
    // surface's own probes do not see it behind itself.
    let q = p + n * (0.2 * spacing);
    let local = (q - grid.origin.xyz) / spacing;
    let first = vec3<u32>(clamp(floor(local), vec3<f32>(0.0), vec3<f32>(f32(GI_SIDE) - 2.0)));
    let f = clamp(local - vec3<f32>(first), vec3<f32>(0.0), vec3<f32>(1.0));
    var sum = vec3<f32>(0.0);
    var weights = 0.0;
    for (var c = 0u; c < 8u; c++) {
        let o = vec3<u32>(c & 1u, (c >> 1u) & 1u, c >> 2u);
        let index = gi_probe_index(level, first + o);
        let state = gi_probes[index * GI_PROBE_WORDS + GI_STATE_WORD];
        if (state & GI_ACTIVE) == 0u || (state >> 8u) == 0u { continue; }
        let tri = mix(1.0 - f, f, vec3<f32>(o));
        var w = tri.x * tri.y * tri.z;
        let probe = grid.origin.xyz + vec3<f32>(first + o) * spacing;
        let to_point = q - probe;
        let distance = max(length(to_point), 1e-4);
        let dir = to_point / distance;
        // Facing: probes behind the surface count less (smoothly).
        let facing = (dot(-dir, n) + 1.0) * 0.5;
        w *= facing * facing + 0.2;
        // Visibility (Chebyshev): a point farther than the probe's rays
        // reached toward it is likely behind something.
        let m = gi_probe_moments(index, dir);
        if distance > m.x {
            let variance = abs(m.y - m.x * m.x);
            let delta = distance - m.x;
            let chebyshev = variance / (variance + delta * delta);
            w *= max(chebyshev * chebyshev * chebyshev, 0.0);
        }
        w = max(w, 0.0);
        sum += gi_probe_irradiance(index, n) * w;
        weights += w;
    }
    if weights <= 1e-5 { return vec4<f32>(0.0); }
    return vec4<f32>(sum / weights, 1.0);
}

// Irradiance / pi at eye-relative point `p` with normal `n` (rgb), and how
// much the probes hold it (a: 1 inside the clipmap, falling to 0 at its
// outer edge).
fn gi_irradiance(p: vec3<f32>, n: vec3<f32>) -> vec4<f32> {
    for (var level = 0u; level < GI_LEVELS; level++) {
        let grid = frame_gi.levels[level];
        let local = (p - grid.origin.xyz) / grid.origin.w;
        // Probes from one in, so all 8 around the point are in the grid.
        let edge = min(min(local.x, local.y), min(local.z, min(min(f32(GI_SIDE) - 2.0 - local.x, f32(GI_SIDE) - 2.0 - local.y), f32(GI_SIDE) - 2.0 - local.z))) - 1.0;
        if edge < 0.0 { continue; }
        let here = gi_level_irradiance(level, p, n);
        // Near the grid's edge, into the next level's.
        let keep = clamp(edge / 2.0, 0.0, 1.0);
        if keep >= 1.0 || here.w == 0.0 {
            if here.w > 0.0 { return here; }
            continue;
        }
        if level + 1u < GI_LEVELS {
            let next = gi_level_irradiance(level + 1u, p, n);
            if next.w > 0.0 { return vec4<f32>(mix(next.rgb, here.rgb, keep), 1.0); }
        }
        return vec4<f32>(here.rgb, keep);
    }
    return vec4<f32>(0.0);
}
