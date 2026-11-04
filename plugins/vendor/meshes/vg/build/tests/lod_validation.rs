//! LOD validation: how many triangles a cut selects for a given screen, and
//! whether the cut still LOOKS like the mesh.
//!
//! 1. `selection_is_bounded_by_screen_pixels_across_resolutions`: for nine
//!    resolutions (16:9 from 640x360 to 8K, ultrawide, portrait, 4:3) and eight
//!    object sizes, the selected triangle count must not exceed two per pixel
//!    the object can cover (never more than two per pixel of the frame). A DAG
//!    cannot go below its root triangles, so the per-object bound is
//!    `max(2 * covered pixels, root triangles)`; the cases where the strict
//!    bound is only broken because of that floor are printed.
//! 2. `selection_is_monotone_in_distance_and_resolution`.
//! 3. `lod_coverage_matches_the_full_detail_mesh`: a small CPU rasterizer renders
//!    the full-detail mesh and every cut at the size they would be seen at, with
//!    supersampling, and compares covered area. Sparse foliage (thin needles with
//!    air between them) is the case that matters: it must keep its collective
//!    coverage without its pieces growing.
//! 4. `write_lod_montages` writes a montage per fixture (full detail, then the cut
//!    selected for 512, 256, ... 8 px tall, each rendered large) to `VG_LOD_OUT`
//!    for looking at.
//!
//! Real cached assets: `VG_CACHE_DIR=<dir> cargo test --release -p helio-vg-build
//! --test lod_validation real_cached_assets -- --ignored --nocapture`.
#![allow(dead_code)]
mod common;
mod measure;

use common::TestMesh;
use helio_vg_build::*;
use std::sync::OnceLock;

type V3 = [f32; 3];

const RESOLUTIONS: [(u32, u32); 9] = [
    (640, 360),
    (1280, 720),
    (1920, 1080),
    (2560, 1440),
    (3440, 1440),
    (3840, 2160),
    (7680, 4320),
    (1080, 1920),
    (1600, 1200),
];
/// Projected object diameter as a fraction of the screen height.
const SIZES: [f32; 8] = [1.2, 0.6, 0.3, 0.15, 0.075, 0.0375, 0.019, 0.01];

fn pixel_scale(height: u32) -> f32 {
    height as f32 / (2.0 * (60f32.to_radians() * 0.5).tan())
}

// ── fixtures ────────────────────────────────────────────────────────────────

struct Rng(u64);
impl Rng {
    fn f(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
}

fn pos_bytes(p: &[V3]) -> Vec<u8> {
    p.iter()
        .flat_map(|v| v.iter().flat_map(|f| f.to_le_bytes()))
        .collect()
}

fn noisy_sphere_mesh(subdiv: u32) -> TestMesh {
    let (positions, indices) = measure::noisy_sphere(subdiv);
    let mats = vec![0; indices.len() / 3];
    TestMesh {
        attributes: pos_bytes(&positions),
        positions,
        stride: 12,
        indices,
        mats,
    }
}

/// Thin needles with air between them: `n` slender quads (length x width) at
/// random orientations inside a ball of `radius`. Pine foliage in the small.
fn needles(n: usize, radius: f32, length: f32, width: f32, seed: u64) -> TestMesh {
    let mut r = Rng(seed);
    let (mut p, mut idx) = (Vec::new(), Vec::new());
    for _ in 0..n {
        let c = loop {
            let c = [r.f() * 2.0 - 1.0, r.f() * 2.0 - 1.0, r.f() * 2.0 - 1.0];
            if c[0] * c[0] + c[1] * c[1] + c[2] * c[2] <= 1.0 {
                break c.map(|x| x * radius);
            }
        };
        let unit = |v: V3| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let d = unit([r.f() - 0.5, r.f() - 0.5, r.f() - 0.5]);
        let t = unit([r.f() - 0.5, r.f() - 0.5, r.f() - 0.5]);
        let s = unit([
            d[1] * t[2] - d[2] * t[1],
            d[2] * t[0] - d[0] * t[2],
            d[0] * t[1] - d[1] * t[0],
        ]);
        let b = p.len() as u32;
        for (a, w) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            p.push([
                c[0] + 0.5 * length * a * d[0] + 0.5 * width * w * s[0],
                c[1] + 0.5 * length * a * d[1] + 0.5 * width * w * s[1],
                c[2] + 0.5 * length * a * d[2] + 0.5 * width * w * s[2],
            ]);
        }
        idx.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let mats = vec![0; idx.len() / 3];
    TestMesh {
        attributes: pos_bytes(&p),
        positions: p,
        stride: 12,
        indices: idx,
        mats,
    }
}

struct Fixture {
    name: &'static str,
    mesh: TestMesh,
    bytes: Vec<u8>,
    radius: f32,
}

fn fixtures() -> &'static Vec<Fixture> {
    static F: OnceLock<Vec<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        let mut out = Vec::new();
        for (name, mesh, radius) in [
            ("noisy-sphere", noisy_sphere_mesh(7), 1.1f32),
            ("needles", needles(60_000, 3.0, 0.12, 0.008, 11), 3.1),
        ] {
            let bytes = build_vgeo(&mesh.input(), &BuildOptions::default()).unwrap();
            out.push(Fixture {
                name,
                mesh,
                bytes,
                radius,
            });
        }
        out
    })
}

// ── selection ───────────────────────────────────────────────────────────────

fn select(r: &VgeoReader, d: f32, height: u32, tau: f32) -> Vec<SelectedCluster> {
    r.select_clusters(
        tau,
        &Projection {
            eye: [0.0, 0.0, d],
            scale: pixel_scale(height),
        },
    )
    .unwrap()
}

fn tri_count(sel: &[SelectedCluster]) -> usize {
    sel.iter().map(|s| s.cluster.tri_count() as usize).sum()
}

/// Distance at which an object of `radius` projects to `fraction` of `height`.
fn distance_for(radius: f32, fraction: f32, height: u32) -> f32 {
    2.0 * radius * pixel_scale(height) / (fraction * height as f32)
}

fn root_triangles(r: &VgeoReader) -> usize {
    tri_count(&select(r, 1.0e9, 1080, 1.0))
}

fn check_selection_bound(foliage: bool) {
    let mut rows = 0;
    let mut floor_limited = Vec::new();
    for fx in fixtures()
        .iter()
        .filter(|f| (f.name == "needles") == foliage)
    {
        let r = VgeoReader::parse(&fx.bytes).unwrap();
        let floor = root_triangles(&r);
        println!(
            "{}: {} source triangles, root floor {} triangles",
            fx.name,
            fx.mesh.tri_count(),
            floor
        );
        for &(w, h) in &RESOLUTIONS {
            let frame = 2.0 * f64::from(w) * f64::from(h);
            for &f in &SIZES {
                let d = distance_for(fx.radius, f, h);
                let t = tri_count(&select(&r, d, h, 1.0));
                let disc = std::f64::consts::PI * (f64::from(f) * f64::from(h) * 0.5).powi(2);
                let strict = 2.0 * disc.min(f64::from(w) * f64::from(h));
                let allowed = strict.max(floor as f64);
                rows += 1;
                assert!(
                    t as f64 <= frame,
                    "{} {w}x{h} size {f}: {t} triangles exceed 2 per frame pixel",
                    fx.name
                );
                assert!(
                    t <= fx.mesh.tri_count(),
                    "{}: {t} exceeds the source",
                    fx.name
                );
                assert!(
                    t as f64 <= allowed,
                    "{} {w}x{h} size {f}: {t} triangles for about {strict:.0} allowed (floor {floor})",
                    fx.name
                );
                if t as f64 > strict {
                    floor_limited.push(format!(
                        "{} {w}x{h} size {f}: {t} vs strict {strict:.0}",
                        fx.name
                    ));
                }
            }
        }
    }
    println!("{rows} (resolution, size) cases checked");
    println!(
        "{} cases exceed the strict 2/pixel bound only because of the root floor:",
        floor_limited.len()
    );
    for line in floor_limited.iter().take(40) {
        println!("  {line}");
    }
}

#[test]
fn selection_is_monotone_in_distance_and_resolution() {
    for fx in fixtures() {
        let r = VgeoReader::parse(&fx.bytes).unwrap();
        for &(w, h) in &RESOLUTIONS {
            let mut last = usize::MAX;
            for &f in &SIZES {
                let t = tri_count(&select(&r, distance_for(fx.radius, f, h), h, 1.0));
                // Smaller on screen must not select more (clusters are discrete: 5% slack).
                assert!(
                    t as f64 <= last as f64 * 1.05 + 130.0,
                    "{} {w}x{h} size {f}: {t} after {last}",
                    fx.name
                );
                last = t;
            }
        }
        // The same object at the same distance selects at least as much on a taller screen.
        let d = distance_for(fx.radius, 0.3, 1080);
        let mut prev = 0usize;
        for h in [360u32, 720, 1080, 1440, 2160, 4320] {
            let t = tri_count(&select(&r, d, h, 1.0));
            assert!(
                t as f64 >= prev as f64 * 0.95,
                "{}: height {h} selected {t} after {prev}",
                fx.name
            );
            prev = t;
        }
        // And a larger tolerance never selects more.
        let mut prev = usize::MAX;
        for tau in [0.5f32, 1.0, 2.0, 4.0, 8.0, 16.0, 64.0] {
            let t = tri_count(&select(&r, d, 1080, tau));
            assert!(
                t <= prev,
                "{}: tau {tau} selected {t} after {prev}",
                fx.name
            );
            prev = t;
        }
    }
}

// ── CPU rasterizer ──────────────────────────────────────────────────────────

struct Render {
    w: u32,
    h: u32,
    rgb: Vec<u8>,
    /// Fraction of the supersamples covered by any triangle.
    coverage: f64,
}

/// Renders `tris` with the camera on the +z axis at `eye_z` looking at the
/// origin. `scale` is pixels per unit at depth 1 (the real `pixel_scale` for an
/// "as seen" view). Supersampled `ss x ss`; double-sided flat shading.
fn render(tris: &[[V3; 3]], eye_z: f32, scale: f32, w: u32, h: u32, ss: u32) -> Render {
    let (sw, sh) = ((w * ss) as usize, (h * ss) as usize);
    let sscale = scale * ss as f32;
    let mut depth = vec![f32::INFINITY; sw * sh];
    let mut shade = vec![0u8; sw * sh];
    let light = {
        let l = [0.4f32, 0.8, 0.5];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };
    for tri in tris {
        let mut s = [[0f32; 3]; 3];
        let mut ok = true;
        for (k, v) in tri.iter().enumerate() {
            let z = eye_z - v[2];
            if z < 1.0e-3 {
                ok = false;
                break;
            }
            s[k] = [
                sw as f32 * 0.5 + v[0] / z * sscale,
                sh as f32 * 0.5 - v[1] / z * sscale,
                1.0 / z,
            ];
        }
        if !ok {
            continue;
        }
        let e1 = [
            tri[1][0] - tri[0][0],
            tri[1][1] - tri[0][1],
            tri[1][2] - tri[0][2],
        ];
        let e2 = [
            tri[2][0] - tri[0][0],
            tri[2][1] - tri[0][1],
            tri[2][2] - tri[0][2],
        ];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        let lambert = ((n[0] * light[0] + n[1] * light[1] + n[2] * light[2]) / nl).abs();
        let value = (255.0 * (0.22 + 0.78 * lambert)).clamp(0.0, 255.0) as u8;
        let min_x = s
            .iter()
            .map(|p| p[0])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i64;
        let max_x = s
            .iter()
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(sw as f32) as i64;
        let min_y = s
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min)
            .floor()
            .max(0.0) as i64;
        let max_y = s
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max)
            .ceil()
            .min(sh as f32) as i64;
        let area =
            (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[1][1] - s[0][1]) * (s[2][0] - s[0][0]);
        if area.abs() < 1.0e-12 {
            continue;
        }
        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let w0 = ((s[1][0] - p[0]) * (s[2][1] - p[1])
                    - (s[1][1] - p[1]) * (s[2][0] - p[0]))
                    / area;
                let w1 = ((s[2][0] - p[0]) * (s[0][1] - p[1])
                    - (s[2][1] - p[1]) * (s[0][0] - p[0]))
                    / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let inv_z = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                let z = 1.0 / inv_z;
                let i = y as usize * sw + x as usize;
                if z < depth[i] {
                    depth[i] = z;
                    shade[i] = value;
                }
            }
        }
    }
    let covered = depth.iter().filter(|d| d.is_finite()).count();
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut hit) = (0u32, 0u32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let i = ((y * ss + sy) as usize) * sw + (x * ss + sx) as usize;
                    if depth[i].is_finite() {
                        sum += u32::from(shade[i]);
                        hit += 1;
                    }
                }
            }
            let bg = 24.0;
            let cov = hit as f32 / (ss * ss) as f32;
            let c = if hit > 0 {
                sum as f32 / hit as f32
            } else {
                0.0
            };
            let v = (c * cov + bg * (1.0 - cov)) as u8;
            let o = ((y * w + x) * 3) as usize;
            rgb[o] = (f32::from(v) * 0.95) as u8;
            rgb[o + 1] = v;
            rgb[o + 2] = (f32::from(v) * 0.9) as u8;
        }
    }
    Render {
        w,
        h,
        rgb,
        coverage: covered as f64 / (sw * sh) as f64,
    }
}

fn cut_triangles(r: &VgeoReader, sel: &[SelectedCluster]) -> Vec<[V3; 3]> {
    let mut out = Vec::new();
    for s in sel {
        let dec = r.decode(s).unwrap();
        for t in &dec.tris {
            out.push([
                dec.positions[t[0] as usize],
                dec.positions[t[1] as usize],
                dec.positions[t[2] as usize],
            ]);
        }
    }
    out
}

fn mesh_triangles(m: &TestMesh) -> Vec<[V3; 3]> {
    m.indices
        .chunks_exact(3)
        .map(|t| {
            [
                m.positions[t[0] as usize],
                m.positions[t[1] as usize],
                m.positions[t[2] as usize],
            ]
        })
        .collect()
}

fn check_coverage(foliage: bool) {
    let height = 1080;
    let scale = pixel_scale(height);
    let mut failures = Vec::new();
    for fx in fixtures()
        .iter()
        .filter(|f| (f.name == "needles") == foliage)
    {
        let r = VgeoReader::parse(&fx.bytes).unwrap();
        let leaf = mesh_triangles(&fx.mesh);
        for &px in &[128.0f32, 64.0, 32.0, 16.0, 8.0] {
            // Object `px` pixels tall, seen at its real size in a 256x256 window.
            let d = 2.0 * fx.radius * scale / px;
            let full = render(&leaf, d, scale, 256, 256, 4);
            let sel = select(&r, d, height, 1.0);
            let cut = render(&cut_triangles(&r, &sel), d, scale, 256, 256, 4);
            let ratio = cut.coverage / full.coverage.max(1e-9);
            // The error budget moves a silhouette by up to tau pixels: that much
            // coverage may change on a solid object. Sparse foliage has no such
            // excuse: its pieces are the coverage.
            let radius_px = f64::from(px) * 0.5;
            let tolerance = if fx.name == "needles" {
                0.25
            } else {
                0.05 + 2.5 / radius_px
            };
            println!(
                "{:>12} {:>5.0} px: {:>7} -> {:>7} tris, coverage {:.4} -> {:.4} (x{:.3}, allowed +-{:.2})",
                fx.name,
                px,
                leaf.len(),
                tri_count(&sel),
                full.coverage,
                cut.coverage,
                ratio,
                tolerance
            );
            if (ratio - 1.0).abs() > tolerance {
                failures.push(format!(
                    "{} at {px} px: coverage x{ratio:.3} (allowed +-{tolerance:.2})",
                    fx.name
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "coverage not preserved:\n  {}",
        failures.join("\n  ")
    );
}

// ── images for looking at ───────────────────────────────────────────────────

fn montage(tiles: &[Render], columns: u32, path: &std::path::Path) {
    let (tw, th) = (tiles[0].w, tiles[0].h);
    let rows = (tiles.len() as u32).div_ceil(columns);
    let mut img = image::RgbImage::new(tw * columns, th * rows);
    for (i, t) in tiles.iter().enumerate() {
        let (cx, cy) = ((i as u32 % columns) * tw, (i as u32 / columns) * th);
        for y in 0..th {
            for x in 0..tw {
                let o = ((y * tw + x) * 3) as usize;
                img.put_pixel(
                    cx + x,
                    cy + y,
                    image::Rgb([t.rgb[o], t.rgb[o + 1], t.rgb[o + 2]]),
                );
            }
        }
        // Frame between tiles.
        for x in 0..tw {
            img.put_pixel(cx + x, cy, image::Rgb([255, 80, 0]));
        }
        for y in 0..th {
            img.put_pixel(cx, cy + y, image::Rgb([255, 80, 0]));
        }
    }
    img.save(path).unwrap();
}

fn lod_montage(
    name: &str,
    r: &VgeoReader,
    leaf: Option<&[[V3; 3]]>,
    radius: f32,
    out: &std::path::Path,
) {
    let height = 1080;
    let scale = pixel_scale(height);
    let tile = 400u32;
    // Zoomed: every cut fills the tile, whatever distance it was selected for.
    let dz = 3.0 * radius;
    let zoom = 0.45 * tile as f32 * dz / radius;
    let mut tiles = Vec::new();
    let mut counts = Vec::new();
    if let Some(leaf) = leaf {
        tiles.push(render(leaf, dz, zoom, tile, tile, 2));
        counts.push(leaf.len());
    }
    for px in [512.0f32, 256.0, 128.0, 64.0, 32.0, 16.0, 8.0] {
        let d = 2.0 * radius * scale / px;
        let sel = select(r, d, height, 1.0);
        counts.push(tri_count(&sel));
        tiles.push(render(&cut_triangles(r, &sel), dz, zoom, tile, tile, 2));
    }
    let path = out.join(format!("lod-{name}.png"));
    montage(&tiles, 4, &path);
    println!("{name}: tile order [full detail, 512, 256, 128, 64, 32, 16, 8 px] triangles {counts:?} -> {}", path.display());
}

fn out_dir() -> std::path::PathBuf {
    let dir = std::env::var_os("VG_LOD_OUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("vg_lod_validation"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn write_lod_montages() {
    let out = out_dir();
    for fx in fixtures() {
        let r = VgeoReader::parse(&fx.bytes).unwrap();
        let leaf = mesh_triangles(&fx.mesh);
        lod_montage(fx.name, &r, Some(&leaf), fx.radius, &out);
    }
}

// ── real cached assets ──────────────────────────────────────────────────────

/// Full-detail triangles of a cache (positions only), for rebuilding it with the
/// current builder.
fn leaves_of(r: &VgeoReader) -> (Vec<V3>, Vec<u32>) {
    let roots = r
        .select_clusters(
            1e30,
            &Projection {
                eye: [0.0; 3],
                scale: 1.0,
            },
        )
        .unwrap();
    let c = roots[0].cluster.sphere;
    let leaves = r
        .select_clusters(
            1e-12,
            &Projection {
                eye: [c[0], c[1], c[2]],
                scale: 1.0,
            },
        )
        .unwrap();
    let (mut pos, mut idx) = (Vec::new(), Vec::new());
    for s in &leaves {
        let d = r.decode(s).unwrap();
        let base = pos.len() as u32;
        pos.extend(d.positions.iter().copied());
        for t in &d.tris {
            idx.extend(t.iter().map(|&v| base + v as u32));
        }
    }
    (pos, idx)
}
#[test]
#[ignore = "diagnostic: set VG_CACHE_DIR (and VG_LOD_OUT for images)"]
fn real_cached_assets_ladder_and_images() {
    let Some(dir) = std::env::var_os("VG_CACHE_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let out = out_dir();
    let mut files: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("vgeo"))
        .map(|e| (e.metadata().unwrap().len(), e.path()))
        .filter(|(len, _)| (4 << 20..700 << 20).contains(len))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.truncate(8);
    for (_, path) in files {
        let mut bytes = std::fs::read(&path).unwrap();
        if std::env::var_os("VG_REBUILD").is_some() {
            // Rebuild from the cache's full-detail triangles with the current builder.
            let (pos, idx) = leaves_of(&VgeoReader::parse(&bytes).unwrap());
            let attrs = pos_bytes(&pos);
            let input = MeshInput {
                positions: &pos,
                attributes: &attrs,
                attribute_stride: 12,
                indices: &idx,
                tri_material: &[],
            };
            let t = std::time::Instant::now();
            bytes = build_vgeo(&input, &BuildOptions::default()).unwrap();
            println!("rebuilt {} triangles in {:?}", idx.len() / 3, t.elapsed());
        }
        let r = VgeoReader::parse(&bytes).unwrap();
        // Bounding sphere from the root cut.
        let roots = select(&r, 1.0e9, 1080, 1.0);
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for s in &roots {
            for k in 0..3 {
                lo[k] = lo[k].min(s.cluster.sphere[k] - s.cluster.sphere[3]);
                hi[k] = hi[k].max(s.cluster.sphere[k] + s.cluster.sphere[3]);
            }
        }
        let centre = [
            (lo[0] + hi[0]) / 2.0,
            (lo[1] + hi[1]) / 2.0,
            (lo[2] + hi[2]) / 2.0,
        ];
        let radius = (0..3).map(|k| (hi[k] - lo[k]) / 2.0).fold(0.0f32, f32::max) * 1.2;
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let source = r.header.asset.triangle_count;
        let floor = tri_count(&roots);
        println!("{name}: {source} source triangles, root floor {floor}, radius {radius:.3}");
        // The cut is selected for an eye at distance d from the object CENTRE.
        for &(w, h) in &[(1280u32, 720u32), (1920, 1080), (3840, 2160)] {
            for &f in &SIZES {
                let d = distance_for(radius, f, h);
                let sel = r
                    .select_clusters(
                        1.0,
                        &Projection {
                            eye: [centre[0], centre[1], centre[2] + d],
                            scale: pixel_scale(h),
                        },
                    )
                    .unwrap();
                let t = tri_count(&sel);
                let disc = std::f64::consts::PI * (f64::from(f) * f64::from(h) * 0.5).powi(2);
                let strict = 2.0 * disc.min(f64::from(w) * f64::from(h));
                let flag = if t as f64 > strict.max(floor as f64) {
                    "  <-- OVER (not explained by the root floor)"
                } else if t as f64 > strict {
                    "  (root floor)"
                } else {
                    ""
                };
                println!("  {w}x{h} size {f:.3}: {t:>9} triangles, strict {strict:>10.0}{flag}");
            }
        }
        // Triangles at 1080p against the error tolerance: how much tolerance the
        // 2-triangles-per-pixel bound would need.
        for &f in &SIZES {
            let d = distance_for(radius, f, 1080);
            let disc = std::f64::consts::PI * (f64::from(f) * 1080.0 * 0.5).powi(2);
            let row: Vec<String> = [1.0f32, 2.0, 3.0, 4.0, 6.0]
                .iter()
                .map(|&tau| {
                    let t = tri_count(
                        &r.select_clusters(
                            tau,
                            &Projection {
                                eye: [centre[0], centre[1], centre[2] + d],
                                scale: pixel_scale(1080),
                            },
                        )
                        .unwrap(),
                    );
                    format!("tau{tau}:{t}")
                })
                .collect();
            println!(
                "    1080p size {f:.3} (strict {:.0}): {}",
                2.0 * disc,
                row.join("  ")
            );
        }
        // Montage of the cuts (recentred on the asset).
        let tile = 400u32;
        let dz = 3.0 * radius;
        let zoom = 0.45 * tile as f32 * dz / radius;
        let scale = pixel_scale(1080);
        let mut tiles = Vec::new();
        for px in [512.0f32, 256.0, 128.0, 64.0, 32.0, 16.0, 8.0, 4.0] {
            let d = 2.0 * radius * scale / px;
            let sel = r
                .select_clusters(
                    1.0,
                    &Projection {
                        eye: [centre[0], centre[1], centre[2] + d],
                        scale,
                    },
                )
                .unwrap();
            let tris: Vec<[V3; 3]> = cut_triangles(&r, &sel)
                .into_iter()
                .map(|t| t.map(|v| [v[0] - centre[0], v[1] - centre[1], v[2] - centre[2]]))
                .collect();
            tiles.push(render(&tris, dz, zoom, tile, tile, 2));
        }
        montage(&tiles, 4, &out.join(format!("real-{name}.png")));
    }
}

// ── the checks as tests ─────────────────────────────────────────────────────
// Solid geometry passes. Sparse foliage and the real cached assets do not: those
// are real defects, kept as explicit known-failing tests (run them with
// `-- --ignored`) so they cannot be loosened or forgotten. Plan section 13.

#[test]
fn solid_selection_is_bounded_by_screen_pixels_across_resolutions() {
    check_selection_bound(false);
}

#[test]
fn foliage_selection_is_bounded_by_screen_pixels_across_resolutions() {
    check_selection_bound(true);
}

#[test]
fn solid_lod_coverage_matches_the_full_detail_mesh() {
    check_coverage(false);
}

#[test]
fn foliage_lod_coverage_matches_the_full_detail_mesh() {
    check_coverage(true);
}

// ── a forest, the way the demo stacks instances ─────────────────────────────

/// A tree cache cooked by the current builder (`cargo vg-stress -- --prepare
/// --force-recook` writes them to `VG_CACHE_DIR`).
fn rebuilt_tree(dir: &std::path::Path, _scratch: &std::path::Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(format!("{name}.vgeo"))).unwrap()
}

/// Frustum-visible instances of a forest of the demo's tree assets, seen from the
/// ground: (asset index, eye in the asset's space, distance to its sphere's
/// nearest point). The demo scatters about 2,000 trees of about one tree radius
/// (6 m) over 250 m: a 42 x 42 tree-radius field that the camera walks into.
fn forest_view(
    radius: f32,
    assets: usize,
    w: u32,
    h: u32,
    cam: [f32; 3],
    n: usize,
) -> Vec<(usize, [f32; 3])> {
    let mut rng = Rng(5);
    let tan_h = (60f32.to_radians() * 0.5).tan() * w as f32 / h as f32;
    let mut out = Vec::new();
    for i in 0..n {
        let x = (rng.f() - 0.5) * 42.0 * radius;
        let z = (rng.f() - 0.5) * 42.0 * radius;
        let rel = [x - cam[0], -cam[1], z - cam[2]];
        if rel[2] < 1.0 || (rel[0] / rel[2]).abs() > tan_h * 1.05 {
            continue;
        }
        out.push((i % assets, [cam[0] - x, cam[1], cam[2] - z]));
    }
    out
}

#[test]
#[ignore = "benchmark: VG_CACHE_DIR and VG_LOD_OUT; rebuilds the demo's trees with the current builder"]
fn forest_density_is_computed_screen_wide() {
    use helio_vg_format::density::{contribution, solve, STEPS};
    use helio_vg_format::LodTable;
    let Some(dir) = std::env::var_os("VG_CACHE_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let scratch = out_dir();
    let blobs: Vec<Vec<u8>> = [
        "3309fbf6647b16c6d5ee7c21bc0699e4",
        "cc287a2e4ef7133c7e3d7405bcbef51d",
        "611bb360b577134e28f466b7505aba90",
        "69cc570cecf3381abcc8506e856a73c6",
    ]
    .iter()
    .map(|n| rebuilt_tree(&dir, &scratch, n))
    .collect();
    let readers: Vec<VgeoReader> = blobs
        .iter()
        .map(|b| VgeoReader::parse(b).unwrap())
        .collect();
    let tables: Vec<(LodTable, [f32; 4])> = readers
        .iter()
        .map(|r| {
            (
                LodTable::from_header(&r.header.asset),
                r.header.asset.bounds,
            )
        })
        .collect();
    let radius = tables[0].1[3];
    println!(
        "tree radii {:?}",
        tables.iter().map(|t| t.1[3]).collect::<Vec<_>>()
    );
    const TARGET: f64 = 1.5;
    let (mut worst_tpp, mut worst_predict, mut sum_tpp, mut frames) =
        (0.0f64, 1.0f64, 0.0f64, 0usize);
    let mut worst_jump = 1.0f32;
    for &(w, h) in &[(1920u32, 1080u32), (3440, 1440)] {
        let px = f64::from(w) * f64::from(h);
        let scale = pixel_scale(h);
        println!("{w}x{h}:");
        let mut calib = helio_vg_format::density::VisibleFraction::default();
        let mut pending = std::collections::VecDeque::new();
        let mut last_g = 1.0f32;
        let mut last_g_raw = 0.0f32;
        for frame in 0..90 {
            let cam = [0.0, 0.3 * radius, (-18.0 + frame as f32 * 0.15) * radius];
            let view = forest_view(radius, readers.len(), w, h, cam, 2100);
            let mut sums = [0.0f64; STEPS];
            for (a, eye) in &view {
                let (table, bounds) = &tables[*a];
                let c = [eye[0] - bounds[0], eye[1] - bounds[1], eye[2] - bounds[2]];
                let depth = c[2].abs();
                let d = depth.max(0.7 * bounds[3]).max(0.1);
                let base = d / scale;
                for (s, v) in sums.iter_mut().zip(contribution(table, base)) {
                    *s += f64::from(v);
                }
            }
            let (g_wanted, predicted) = solve(&sums, calib.predicted_budget(TARGET * px) as f64);
            let g = helio_vg_format::density::slew(last_g_raw, g_wanted);
            last_g_raw = g;
            let actual: u64 = view
                .iter()
                .map(|(a, eye)| {
                    tri_count(
                        &readers[*a]
                            .select_clusters(g, &Projection { eye: *eye, scale })
                            .unwrap(),
                    ) as u64
                })
                .sum();
            let tpp = actual as f64 / px;
            pending.push_back((predicted as f32, actual as f32));
            if pending.len() > 2 {
                let (p, o) = pending.pop_front().unwrap();
                calib.update(p, o);
            }
            worst_tpp = worst_tpp.max(tpp);
            sum_tpp += tpp;
            frames += 1;
            worst_predict = worst_predict.max(actual as f64 / predicted.max(1.0));
            if frame > 0 {
                worst_jump = worst_jump.max((g / last_g).max(last_g / g));
            }
            last_g = g;
            if frame % 15 == 0 || frame == 89 {
                println!("  frame {frame:>2}: {} trees, tolerance x{g:.2}, predicted {:.2}/px, actual {tpp:.2}/px", view.len(), predicted / px);
            }
        }
    }
    println!("worst {worst_tpp:.2}/px, mean {:.2}/px, worst actual/predicted {worst_predict:.2}, worst per-frame change x{worst_jump:.3}", sum_tpp / frames as f64);
    assert!(worst_tpp <= 2.0, "worst density {worst_tpp}");
    assert!(worst_predict <= 1.5, "prediction off by {worst_predict}");
    assert!(worst_jump <= 1.25, "tolerance jumped x{worst_jump}");
}

#[test]
#[ignore = "diagnostic: what makes a forest frame expensive"]
fn forest_spike_anatomy() {
    let Some(dir) = std::env::var_os("VG_CACHE_DIR").map(std::path::PathBuf::from) else {
        return;
    };
    let scratch = out_dir();
    let names = [
        "3309fbf6647b16c6d5ee7c21bc0699e4",
        "cc287a2e4ef7133c7e3d7405bcbef51d",
        "611bb360b577134e28f466b7505aba90",
        "69cc570cecf3381abcc8506e856a73c6",
    ];
    let blobs: Vec<Vec<u8>> = names
        .iter()
        .map(|n| rebuilt_tree(&dir, &scratch, n))
        .collect();
    let readers: Vec<VgeoReader> = blobs
        .iter()
        .map(|b| VgeoReader::parse(b).unwrap())
        .collect();
    let radius = 13.5f32;
    let (w, h) = (3440u32, 1440u32);
    let scale = pixel_scale(h);
    for frame in [80, 89] {
        let cam = [0.0, 0.3 * radius, (-18.0 + frame as f32 * 0.15) * radius];
        let mut rng = Rng(5);
        let mut rows = Vec::new();
        for i in 0..2100 {
            let x = (rng.f() - 0.5) * 42.0 * radius;
            let z = (rng.f() - 0.5) * 42.0 * radius;
            let rel = [x - cam[0], -cam[1], z - cam[2]];
            if rel[2] < 1.0 || (rel[0] / rel[2]).abs() > 0.57 * w as f32 / h as f32 * 1.05 {
                continue;
            }
            let r = &readers[i % 4];
            let eye = [cam[0] - x, cam[1], cam[2] - z];
            let t = tri_count(&r.select_clusters(4.0, &Projection { eye, scale }).unwrap());
            rows.push((t, i % 4, rel[2] / radius));
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        let total: usize = rows.iter().map(|r| r.0).sum();
        println!(
            "frame {frame}: {} trees in view, {total} triangles; top:",
            rows.len()
        );
        for r in rows.iter().take(6) {
            println!("   {:>9} tris asset {} at {:.2} radii", r.0, r.1, r.2);
        }
    }
}
