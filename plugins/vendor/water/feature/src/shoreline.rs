//! Terrain-derived shoreline fields for the Ultra-Water travel-time model.
//!
//! A map is baked from an elevation provider, never inferred from water
//! volume bounds. A terrain feature answers [`WaterTerrainProvider`] (water's
//! api); water names no terrain (the voxel terrain's provider is
//! `helio_pass_voxel_planet::VoxelShoreTerrain`).
use rayon::prelude::*;

pub use helio_water_api::WaterTerrainProvider;

pub const SHORE_MAP_SIZE: u32 = 512;
const N: usize = SHORE_MAP_SIZE as usize;
const COUNT: usize = N * N;

/// Baked water shoreline and shallow-water propagation fields.
#[derive(Debug)]
pub struct WaterShoreMap {
    /// Absolute-world height of the ocean rest plane used to bake this map.
    pub(crate) rest_y: f64,
    /// Absolute-world XZ location at the center of the map.
    pub(crate) center_xz: [f64; 2],
    /// Full map width/depth in metres.
    pub(crate) extent_xz: [f32; 2],
    /// Row-major: terrain elevation relative to rest plane, signed coast
    /// distance (positive in water), and signed-distance gradient X/Z.
    pub(crate) shore: Vec<[f32; 4]>,
    /// Row-major: signed travel time, direction toward shore X/Z and shoaling.
    pub(crate) travel: Vec<[f32; 4]>,
}

impl WaterShoreMap {
    /// Bake a 512² map from actual terrain elevations. The terrain elevation
    /// is sampled once per texel; coast distance is then computed from the
    /// water/land boundary, followed by a fast-swept eikonal solution using
    /// shallow-water speed `sqrt(g * depth)`.
    pub fn from_terrain(
        center_xz: [f64; 2],
        extent_xz: [f32; 2],
        rest_y: f64,
        provider: &impl WaterTerrainProvider,
    ) -> Result<Self, &'static str> {
        validate_domain(center_xz, extent_xz)?;
        if !rest_y.is_finite() {
            return Err("ocean rest height must be finite");
        }
        let dx = extent_xz[0] / N as f32;
        let dz = extent_xz[1] / N as f32;
        let elevation: Result<Vec<f32>, &'static str> = (0..COUNT)
            .into_par_iter()
            .map(|idx| {
                let (i, j) = (idx % N, idx / N);
                let x =
                    center_xz[0] + ((i as f64 + 0.5) / N as f64 - 0.5) * f64::from(extent_xz[0]);
                let z =
                    center_xz[1] + ((j as f64 + 0.5) / N as f64 - 0.5) * f64::from(extent_xz[1]);
                let h = provider
                    .elevation_y(x, z)
                    .ok_or("terrain provider has no elevation at a shore-map sample")?;
                let relative = (h - rest_y) as f32;
                if !relative.is_finite() {
                    return Err("terrain provider returned a non-finite elevation");
                }
                Ok(relative)
            })
            .collect();
        let elevation = elevation?;

        // Water is where the ocean rest plane is above terrain. The zero
        // contour is the only shoreline source; no water-volume geometry is
        // involved.
        let sea: Vec<bool> = elevation.iter().map(|h| *h <= 0.0).collect();
        let wet = flood_small_islets(&sea);
        let signed_distance = coast_distance(&sea, dx, dz);
        let coast_gradient = scalar_gradient(&signed_distance, dx, dz);
        let travel_fields = bake_travel(&elevation, &wet, &signed_distance, dx, dz);

        let mut shore = Vec::with_capacity(COUNT);
        let mut travel = Vec::with_capacity(COUNT);
        for idx in 0..COUNT {
            let (gx, gz) = coast_gradient[idx];
            shore.push([elevation[idx], signed_distance[idx], gx, gz]);
            travel.push(travel_fields[idx]);
        }
        Self::new(center_xz, extent_xz, rest_y, shore, travel)
    }

    /// Validate and construct a complete map from pre-baked fields.
    pub fn new(
        center_xz: [f64; 2],
        extent_xz: [f32; 2],
        rest_y: f64,
        shore: Vec<[f32; 4]>,
        travel: Vec<[f32; 4]>,
    ) -> Result<Self, &'static str> {
        validate_domain(center_xz, extent_xz)?;
        if !rest_y.is_finite() {
            return Err("ocean rest height must be finite");
        }
        if shore.len() != COUNT || travel.len() != COUNT {
            return Err("shore and travel fields must each contain exactly 512×512 texels");
        }
        if shore
            .iter()
            .chain(&travel)
            .flatten()
            .any(|v| !v.is_finite())
        {
            return Err("shore map fields must contain only finite values");
        }
        Ok(Self {
            rest_y,
            center_xz,
            extent_xz,
            shore,
            travel,
        })
    }
}

fn validate_domain(center_xz: [f64; 2], extent_xz: [f32; 2]) -> Result<(), &'static str> {
    if !center_xz.iter().all(|v| v.is_finite())
        || !extent_xz.iter().all(|v| v.is_finite() && *v > 0.0)
    {
        return Err("shore map center and positive extent must be finite");
    }
    Ok(())
}

fn coast_distance(sea: &[bool], dx: f32, dz: f32) -> Vec<f32> {
    if sea.iter().all(|&v| v) {
        return vec![extent_distance(dx, dz); COUNT];
    }
    if sea.iter().all(|&v| !v) {
        return vec![-extent_distance(dx, dz); COUNT];
    }
    let to_land = edt2d(&sea.iter().map(|&v| !v).collect::<Vec<_>>(), dx, dz);
    let to_sea = edt2d(sea, dx, dz);
    let half_texel = 0.5 * (dx * dz).sqrt();
    let mut sdf = vec![0.0; COUNT];
    for i in 0..COUNT {
        sdf[i] = if sea[i] {
            (to_land[i].sqrt() - f64::from(half_texel)) as f32
        } else {
            -(to_sea[i].sqrt() - f64::from(half_texel)) as f32
        };
    }
    box_blur(&box_blur(&sdf, 2), 2)
}

fn extent_distance(dx: f32, dz: f32) -> f32 {
    N as f32 * (dx * dx + dz * dz).sqrt()
}

fn edt2d(mask: &[bool], dx: f32, dz: f32) -> Vec<f64> {
    let mut field: Vec<f64> = mask
        .iter()
        .map(|&inside| if inside { 0.0 } else { 1.0e20 })
        .collect();
    let mut f = vec![0.0; N];
    let mut d = vec![0.0; N];
    let mut v = vec![0usize; N];
    let mut z = vec![0.0; N + 1];
    for x in 0..N {
        for y in 0..N {
            f[y] = field[y * N + x];
        }
        edt_1d(&f, &mut d, &mut v, &mut z, f64::from(dz));
        for y in 0..N {
            field[y * N + x] = d[y];
        }
    }
    for y in 0..N {
        for x in 0..N {
            f[x] = field[y * N + x];
        }
        edt_1d(&f, &mut d, &mut v, &mut z, f64::from(dx));
        for x in 0..N {
            field[y * N + x] = d[x];
        }
    }
    field
}

fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64], spacing: f64) {
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..N {
        let mut s = intersection(f, q, v[k], spacing);
        while s <= z[k] && k > 0 {
            k -= 1;
            s = intersection(f, q, v[k], spacing);
        }
        if s <= z[k] {
            v[0] = q;
            z[0] = f64::NEG_INFINITY;
            z[1] = f64::INFINITY;
            k = 0;
        } else {
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
        }
    }
    k = 0;
    for q in 0..N {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let delta = (q as f64 - v[k] as f64) * spacing;
        d[q] = delta * delta + f[v[k]];
    }
}

fn intersection(f: &[f64], q: usize, p: usize, spacing: f64) -> f64 {
    let scale = spacing * spacing;
    (f[q] + (q * q) as f64 * scale - f[p] - (p * p) as f64 * scale)
        / (2.0 * (q as f64 - p as f64) * scale)
}

fn box_blur(source: &[f32], radius: usize) -> Vec<f32> {
    let width = 2 * radius + 1;
    let mut temp = vec![0.0; COUNT];
    let mut out = vec![0.0; COUNT];
    for y in 0..N {
        let row = y * N;
        let mut sum = 0.0;
        for i in 0..width {
            sum += source[row + i.saturating_sub(radius).min(N - 1)];
        }
        for x in 0..N {
            temp[row + x] = sum / width as f32;
            let add = (x + radius + 1).min(N - 1);
            let remove = x.saturating_sub(radius);
            sum += source[row + add] - source[row + remove];
        }
    }
    for x in 0..N {
        let mut sum = 0.0;
        for i in 0..width {
            sum += temp[i.saturating_sub(radius).min(N - 1) * N + x];
        }
        for y in 0..N {
            out[y * N + x] = sum / width as f32;
            let add = (y + radius + 1).min(N - 1);
            let remove = y.saturating_sub(radius);
            sum += temp[add * N + x] - temp[remove * N + x];
        }
    }
    out
}

fn scalar_gradient(field: &[f32], dx: f32, dz: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::with_capacity(COUNT);
    for y in 0..N {
        for x in 0..N {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(N - 1);
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(N - 1);
            out.push((
                (field[y * N + xp] - field[y * N + xm]) / (2.0 * dx),
                (field[yp * N + x] - field[ym * N + x]) / (2.0 * dz),
            ));
        }
    }
    out
}

fn flood_small_islets(sea: &[bool]) -> Vec<bool> {
    let mut wet = sea.to_vec();
    let mut seen = vec![false; COUNT];
    let mut stack = Vec::with_capacity(COUNT);
    let mut component = Vec::with_capacity(64);
    for start in 0..COUNT {
        if sea[start] || seen[start] {
            continue;
        }
        stack.clear();
        component.clear();
        stack.push(start);
        seen[start] = true;
        while let Some(i) = stack.pop() {
            component.push(i);
            let x = i % N;
            let y = i / N;
            let neighbors = [
                (x > 0).then_some(i.wrapping_sub(1)),
                (x + 1 < N).then_some(i + 1),
                (y > 0).then_some(i.wrapping_sub(N)),
                (y + 1 < N).then_some(i + N),
            ];
            for next in neighbors.into_iter().flatten() {
                if !sea[next] && !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        if component.len() < 64 {
            for &i in &component {
                wet[i] = true;
            }
        }
    }
    wet
}

fn bake_travel(height: &[f32], wet: &[bool], sdf: &[f32], dx: f32, dz: f32) -> Vec<[f32; 4]> {
    const G: f32 = 9.81;
    const DMIN: f32 = 0.3;
    const DMAX: f32 = 14.0;
    let n = N + 2;
    let mut speed = vec![0.0; COUNT];
    for i in 0..COUNT {
        speed[i] = (G * (-height[i]).clamp(DMIN, DMAX)).sqrt();
    }
    let mut time = vec![f32::INFINITY; n * n];
    let mut locked = vec![2u8; n * n];
    for y in 0..N {
        locked[(y + 1) * n + 1..(y + 1) * n + 1 + N].fill(1);
    }
    for y in 0..N {
        for x in 0..N {
            let k = y * N + x;
            let mut cross_x = f32::INFINITY;
            let mut cross_y = f32::INFINITY;
            for (nx, ny, horizontal) in [
                (x.wrapping_sub(1), y, true),
                (x + 1, y, true),
                (x, y.wrapping_sub(1), false),
                (x, y + 1, false),
            ] {
                if nx >= N || ny >= N {
                    continue;
                }
                let other = ny * N + nx;
                if wet[other] != wet[k] {
                    let crossing = (height[k] / (height[k] - height[other])).abs();
                    if horizontal {
                        cross_x = cross_x.min(crossing);
                    } else {
                        cross_y = cross_y.min(crossing);
                    }
                }
            }
            if cross_x.is_infinite() && cross_y.is_infinite() {
                continue;
            }
            let x_distance = cross_x * dx;
            let z_distance = cross_y * dz;
            let distance = if cross_x.is_infinite() {
                z_distance
            } else if cross_y.is_infinite() {
                x_distance
            } else {
                x_distance * z_distance
                    / (x_distance * x_distance + z_distance * z_distance)
                        .sqrt()
                        .max(1.0e-6)
            };
            let p = k + 2 * y + n + 1;
            time[p] = distance / speed[k];
            locked[p] = 2;
        }
    }
    for _ in 0..12 {
        let mut changed = 0usize;
        for (sx, sy) in [(1isize, 1isize), (-1, 1), (1, -1), (-1, -1)] {
            for jj in 0..N {
                let y = if sy > 0 { jj } else { N - 1 - jj };
                for ii in 0..N {
                    let x = if sx > 0 { ii } else { N - 1 - ii };
                    let k = y * N + x;
                    let p = k + 2 * y + n + 1;
                    if locked[p] != 1 {
                        continue;
                    }
                    locked[p] = 0;
                    let left = time[p - 1].min(time[p + 1]);
                    let up = time[p - n].min(time[p + n]);
                    let next = solve_eikonal(left, up, dx, dz, speed[k]);
                    if next < time[p] - 1.0e-4 {
                        time[p] = next;
                        changed += 1;
                        for q in [p - 1, p + 1, p - n, p + n] {
                            if locked[q] == 0 {
                                locked[q] = 1;
                            }
                        }
                    }
                }
            }
        }
        if changed == 0 {
            break;
        }
    }
    let mut tau = vec![0.0; COUNT];
    for i in 0..COUNT {
        let raw = time[i + 2 * (i / N) + n + 1];
        tau[i] = if !raw.is_finite() {
            0.0
        } else if wet[i] {
            raw
        } else {
            -raw
        };
    }
    tau = box_blur(&box_blur(&tau, 2), 2);
    let grad = scalar_gradient(&tau, dx, dz);
    let mut dir_x = vec![0.0; COUNT];
    let mut dir_z = vec![0.0; COUNT];
    let mut coherence = vec![0.0; COUNT];
    for i in 0..COUNT {
        let (gx, gz) = grad[i];
        let magnitude = (gx * gx + gz * gz).sqrt();
        if magnitude > 1.0e-9 {
            dir_x[i] = -gx / magnitude;
            dir_z[i] = -gz / magnitude;
        }
        let q = ((magnitude * speed[i] - 0.7) / 0.25).clamp(0.0, 1.0);
        coherence[i] = q * q * (3.0 - 2.0 * q);
    }
    let mut curvature = vec![0.0; COUNT];
    for y in 0..N {
        for x in 0..N {
            let i = y * N + x;
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(N - 1);
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(N - 1);
            let kappa = -(dir_x[y * N + xp] - dir_x[y * N + xm]) / (2.0 * dx)
                - (dir_z[yp * N + x] - dir_z[ym * N + x]) / (2.0 * dz);
            curvature[i] = kappa.clamp(-1.0 / 25.0, 1.0 / 25.0) * coherence[i];
        }
    }
    curvature = box_blur(&box_blur(&curvature, 2), 2);
    let fade = box_blur(&box_blur(&coherence, 2), 2);
    let tube = ray_tube(&curvature, &dir_x, &dir_z, sdf, dx, dz);
    let mut out = vec![[0.0; 4]; COUNT];
    for y in 0..N {
        for x in 0..N {
            let i = y * N + x;
            let g = sample_half(&tube, x as f32 * 0.5, y as f32 * 0.5);
            let green = ((-height[i]).max(0.6) / DMAX).clamp(0.04, 1.0).powf(-0.25);
            let shoal = (green * (0.5 * g).exp()).clamp(0.3, 2.5) * fade[i];
            out[i] = [tau[i], dir_x[i], dir_z[i], shoal.clamp(0.0, 2.5)];
        }
    }
    out
}

fn solve_eikonal(a: f32, b: f32, dx: f32, dz: f32, speed: f32) -> f32 {
    if !a.is_finite() && !b.is_finite() {
        return f32::INFINITY;
    }
    if !a.is_finite() {
        return b + dz / speed;
    }
    if !b.is_finite() {
        return a + dx / speed;
    }
    let ix2 = 1.0 / (dx * dx);
    let iz2 = 1.0 / (dz * dz);
    let aa = ix2 + iz2;
    let bb = -2.0 * (a * ix2 + b * iz2);
    let cc = a * a * ix2 + b * b * iz2 - 1.0 / (speed * speed);
    let root = (-bb + (bb * bb - 4.0 * aa * cc).max(0.0).sqrt()) / (2.0 * aa);
    if root >= a.max(b) {
        root
    } else if a <= b {
        a + dx / speed
    } else {
        b + dz / speed
    }
}

fn ray_tube(
    curvature: &[f32],
    dir_x: &[f32],
    dir_z: &[f32],
    sdf: &[f32],
    dx: f32,
    dz: f32,
) -> Vec<f32> {
    const RAY_L: f32 = 50.0;
    const RAY_STEP: f32 = 3.0;
    const RAY_BAND: f32 = 300.0;
    let w = N / 2;
    let mut out = vec![0.0; w * w];
    let texel = (dx * dz).sqrt();
    let step = RAY_STEP * texel;
    let steps = (2.5 * RAY_L / step).ceil() as usize;
    for j in 0..w {
        for i in 0..w {
            let x0 = (2 * i).min(N - 1);
            let y0 = (2 * j).min(N - 1);
            let k = y0 * N + x0;
            let band = ((RAY_BAND - sdf[k]) / 80.0).clamp(0.0, 1.0);
            if sdf[k] < -3.0 * texel || band == 0.0 {
                continue;
            }
            let (mut x, mut y) = (x0 as f32, y0 as f32);
            let mut g = 0.0;
            for s in 0..steps {
                let ii = (x.floor() as usize).min(N - 2);
                let jj = (y.floor() as usize).min(N - 2);
                let fx = x - ii as f32;
                let fy = y - jj as f32;
                let m = jj * N + ii;
                let sample = |field: &[f32]| {
                    let a = field[m] * (1.0 - fx) + field[m + 1] * fx;
                    let b = field[m + N] * (1.0 - fx) + field[m + N + 1] * fx;
                    a * (1.0 - fy) + b * fy
                };
                let weight = (-(s as f32) * step / RAY_L).exp() * step;
                g += weight * sample(curvature);
                x -= RAY_STEP * texel / dx * sample(dir_x);
                y -= RAY_STEP * texel / dz * sample(dir_z);
                if x < 0.0 || y < 0.0 || x > N as f32 - 1.0 || y > N as f32 - 1.0 {
                    break;
                }
            }
            out[j * w + i] = g * band;
        }
    }
    out
}

fn sample_half(field: &[f32], x: f32, y: f32) -> f32 {
    let w = N / 2;
    let x = x.min((w - 1) as f32);
    let y = y.min((w - 1) as f32);
    let i = (x as usize).min(w - 2);
    let j = (y as usize).min(w - 2);
    let fx = x - i as f32;
    let fy = y - j as f32;
    let k = j * w + i;
    let a = field[k] * (1.0 - fx) + field[k + 1] * fx;
    let b = field[k + w] * (1.0 - fx) + field[k + w + 1] * fx;
    a * (1.0 - fy) + b * fy
}
