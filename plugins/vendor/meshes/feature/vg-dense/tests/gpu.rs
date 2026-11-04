//! Headless GPU tests. Each skips (passes with a message) when no adapter, or
//! for the RT test no ray-query device, can be obtained.

use bytemuck::cast_slice;
use helio_pass_vg_dense::peel_ref::{brute_force_tile, PeelParams, RefTri, RefVertex};
use helio_pass_vg_dense::*;
use helio_vg_format::testing::{TestCluster, TestPage};
use helio_vg_format::{vis_pack, GpuVgInstance, INST_ENABLED};
use std::sync::Arc;

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: wgpu::Queue,
}

fn gpu(want_rt: bool) -> Option<Gpu> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    eprintln!("vg-dense adapter: {:?}", adapter.get_info());
    let rt = want_rt
        && adapter
            .features()
            .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY);
    if want_rt && !rt {
        eprintln!("SKIP: adapter has no EXPERIMENTAL_RAY_QUERY");
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        required_features: if rt {
            wgpu::Features::EXPERIMENTAL_RAY_QUERY
        } else {
            wgpu::Features::empty()
        },
        experimental_features: if rt {
            unsafe { wgpu::ExperimentalFeatures::enabled() }
        } else {
            wgpu::ExperimentalFeatures::disabled()
        },
        ..Default::default()
    }))
    .ok()?;
    Some(Gpu {
        device: Arc::new(device),
        queue,
    })
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    fn f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() as f32 / u32::MAX as f32) * (hi - lo)
    }
}

fn buf(g: &Gpu, label: &str, bytes: &[u8]) -> wgpu::Buffer {
    let b = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (bytes.len() as u64).max(16).next_multiple_of(4),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    g.queue.write_buffer(&b, 0, bytes);
    b
}

struct Scene {
    page: TestPage,
    /// Per cluster, per tri: three NDC-space (x, y, z) vertices.
    tris: Vec<Vec<[[f32; 3]; 3]>>,
}

fn random_scene(seed: u64, n_clusters: usize, max_tris: usize, spread: f32) -> Scene {
    let mut rng = Rng(seed);
    let mut all = Vec::new();
    let mut verts_of = Vec::new();
    let mut idx_of = Vec::new();
    for _ in 0..n_clusters {
        let n = 1 + (rng.next() as usize % max_tris);
        let (cx, cy) = (rng.f(-0.8, 0.8), rng.f(-0.8, 0.8));
        let mut tris = Vec::new();
        for _ in 0..n {
            let (tx, ty) = (cx + rng.f(-spread, spread), cy + rng.f(-spread, spread));
            let mut t = [[0.0; 3]; 3];
            for v in &mut t {
                *v = [
                    tx + rng.f(-0.25, 0.25),
                    ty + rng.f(-0.25, 0.25),
                    rng.f(0.05, 0.95),
                ];
            }
            tris.push(t);
        }
        verts_of.push(tris.iter().flatten().copied().collect::<Vec<[f32; 3]>>());
        idx_of.push(
            (0..n as u8)
                .map(|t| [3 * t, 3 * t + 1, 3 * t + 2])
                .collect::<Vec<[u8; 3]>>(),
        );
        all.push(tris);
    }
    let clusters: Vec<TestCluster<'_>> = verts_of
        .iter()
        .zip(&idx_of)
        .map(|(p, t)| TestCluster {
            positions: p,
            tris: t,
            material: 0,
            attrs: None,
        })
        .collect();
    Scene {
        page: TestPage::build(0, &clusters),
        tris: all,
    }
}

const IDENTITY: [[f32; 4]; 4] = [
    [1., 0., 0., 0.],
    [0., 1., 0., 0.],
    [0., 0., 1., 0.],
    [0., 0., 0., 1.],
];

/// Where cluster `ci` of `n` sits in the unified visible list. The split layout
/// puts the first half at 3.. (phase 0, bin 0) and the rest at 200.. (phase 1,
/// bin 5), with garbage in between that must never be binned.
fn unified(ci: u32, n: u32, split: bool) -> u32 {
    if !split {
        ci
    } else if ci < n / 2 {
        3 + ci
    } else {
        200 + (ci - n / 2)
    }
}

fn expected_tiles(
    scene: &Scene,
    cfg: &DenseConfig,
    viewport: [u32; 2],
    dense: &[bool],
    split: bool,
) -> Vec<(u32, u32)> {
    let p = PeelParams::from(cfg);
    let mut refs = Vec::new();
    for (ci, c) in scene.tris.iter().enumerate() {
        for (ti, _t) in c.iter().enumerate() {
            let t = &scene.page.decoded[ci][ti * 3..ti * 3 + 3];
            let v = |i: usize| RefVertex {
                x: (t[i][0] * 0.5 + 0.5) * viewport[0] as f32,
                y: (0.5 - t[i][1] * 0.5) * viewport[1] as f32,
                z: t[i][2],
                valid: true,
            };
            refs.push(RefTri {
                v: [v(0), v(1), v(2)],
                id: vis_pack(
                    unified(ci as u32, scene.tris.len() as u32, split),
                    ti as u32,
                ),
            });
        }
    }
    let tiles = [
        viewport[0].div_ceil(cfg.tile),
        viewport[1].div_ceil(cfg.tile),
    ];
    let mut out = vec![(0u32, 0u32); (viewport[0] * viewport[1]) as usize];
    for ty in 0..tiles[1] {
        for tx in 0..tiles[0] {
            if !dense[(ty * tiles[0] + tx) as usize] {
                continue;
            }
            let r = brute_force_tile(&refs, [(tx * cfg.tile) as f32, (ty * cfg.tile) as f32], &p);
            for ly in 0..cfg.tile {
                for lx in 0..cfg.tile {
                    let (x, y) = (tx * cfg.tile + lx, ty * cfg.tile + ly);
                    if x < viewport[0] && y < viewport[1] {
                        let i = (ly * cfg.tile + lx) as usize;
                        out[(y * viewport[0] + x) as usize] = (r.z[i].to_bits(), r.id[i]);
                    }
                }
            }
        }
    }
    out
}

struct RunResult {
    keys: Vec<(u32, u32)>,
    stats: DenseStats,
}

fn run_nonrt(
    g: &Gpu,
    scene: &Scene,
    cfg: DenseConfig,
    viewport: [u32; 2],
    dense: &[bool],
) -> RunResult {
    run_nonrt_layout(g, scene, cfg, viewport, dense, false)
}

fn run_nonrt_layout(
    g: &Gpu,
    scene: &Scene,
    cfg: DenseConfig,
    viewport: [u32; 2],
    dense: &[bool],
    split: bool,
) -> RunResult {
    let mut pass = DenseNonRtPass::new(&g.device, cfg).expect("pass");
    let n = scene.tris.len() as u32;
    let garbage = helio_vg_format::GpuVisibleCluster {
        instance: 0,
        cluster_slot: scene.page.slots[0],
    };
    let mut visible = vec![garbage; if split { n as usize + 300 } else { n as usize }];
    for i in 0..n {
        visible[unified(i, n, split) as usize] = helio_vg_format::GpuVisibleCluster {
            instance: 0,
            cluster_slot: scene.page.slots[i as usize],
        };
    }
    let mask: Vec<u32> = dense.iter().map(|&d| d as u32).collect();
    let tile_mask = buf(g, "mask", cast_slice(&mask));
    // The cull's state: unsplit = phase 0, bin 0 holds the whole list; split =
    // phase 0 bin 0 {3, n/2} and phase 1 bin 5 {200, n - n/2}, the rest empty.
    let mut cull_state = vec![0u32; 128];
    let r = helio_vg_format::CULL_STATE_RANGES as usize;
    if split {
        cull_state[r] = 3;
        cull_state[r + 1] = n / 2;
        cull_state[r + helio_vg_format::CULL_RANGES_PER_PHASE as usize + 10] = 200;
        cull_state[r + helio_vg_format::CULL_RANGES_PER_PHASE as usize + 11] = n - n / 2;
    } else {
        cull_state[r + 1] = n;
    }
    let cull_state = buf(g, "cull state", cast_slice(&cull_state));
    let visible_buf = buf(g, "visible", cast_slice(&visible));
    let pool = buf(g, "pool", &scene.page.pool());
    let quant = buf(g, "quant", cast_slice(&[scene.page.quant]));
    let inst = GpuVgInstance {
        asset_slot: 0,
        flags: INST_ENABLED,
        lod_error_scale: 1.0,
        object_row: 0,
    };
    let instances = buf(g, "instances", cast_slice(&[inst]));
    let mut object = [0f32; 63];
    object[4..20].copy_from_slice(cast_slice(&IDENTITY));
    let objects = buf(g, "objects", cast_slice(&object));
    let frame = DenseFrame {
        view_proj: IDENTITY,
        inv_view_proj: IDENTITY,
        camera_pos: [0.0; 3],
        viewport,
        visible_capacity: visible.len() as u32,
        ray_t: [0.0, 1.0],
    };
    let mut enc = g.device.create_command_encoder(&Default::default());
    pass.record(
        &g.device,
        &g.queue,
        &mut enc,
        &frame,
        &DenseNonRtInputs {
            tile_mask: &tile_mask,
            cull_state: &cull_state,
            visible: &visible_buf,
            page_pool: &pool,
            asset_quant: &quant,
            vg_instances: &instances,
            objects: &objects,
        },
    );
    let b = pass.buffers().unwrap();
    b.copy_stats(&mut enc);
    g.queue.submit(Some(enc.finish()));
    let stats = b.read_stats(&g.device);
    let raw = read_buffer(
        &g.device,
        &g.queue,
        &b.dense_key,
        viewport[0] as u64 * viewport[1] as u64 * 8,
    );
    let words: &[u32] = cast_slice(&raw);
    RunResult {
        keys: words.chunks(2).map(|c| (c[0], c[1])).collect(),
        stats,
    }
}

/// Fraction of dense pixels whose id differs and whose depth differs
/// materially (edge-grazing and equal-depth pixels may legitimately differ).
fn mismatch_fraction(
    got: &[(u32, u32)],
    want: &[(u32, u32)],
    covered: impl Fn(usize) -> bool,
) -> (usize, usize) {
    let (mut bad, mut n) = (0, 0);
    for i in 0..got.len() {
        if !covered(i) {
            continue;
        }
        n += 1;
        let (gz, wz) = (f32::from_bits(got[i].0), f32::from_bits(want[i].0));
        if got[i].1 != want[i].1 && (gz - wz).abs() > 1e-4 {
            bad += 1;
        }
    }
    (bad, n)
}

fn check_nonrt(
    cfg: DenseConfig,
    viewport: [u32; 2],
    dense_pick: impl Fn(u32, u32) -> bool,
    seed: u64,
) -> Option<RunResult> {
    let g = gpu(false)?;
    let scene = random_scene(seed, 80, 40, 0.3);
    let tiles = [
        viewport[0].div_ceil(cfg.tile),
        viewport[1].div_ceil(cfg.tile),
    ];
    let dense: Vec<bool> = (0..tiles[0] * tiles[1])
        .map(|t| dense_pick(t % tiles[0], t / tiles[0]))
        .collect();
    let r = run_nonrt(&g, &scene, cfg, viewport, &dense);
    let want = expected_tiles(&scene, &cfg, viewport, &dense, false);
    let covered = |i: usize| {
        let (x, y) = (i as u32 % viewport[0], i as u32 / viewport[0]);
        dense[((y / cfg.tile) * tiles[0] + x / cfg.tile) as usize]
    };
    let (bad, n) = mismatch_fraction(&r.keys, &want, &covered);
    assert!(n > 0);
    assert!(
        bad * 100 <= n,
        "{bad}/{n} dense pixels differ from the z-sort reference (cfg {cfg:?})"
    );
    // Pixels outside dense tiles are untouched (still zero).
    for (i, k) in r.keys.iter().enumerate() {
        if !covered(i) {
            assert_eq!(*k, (0, 0));
        }
    }
    assert_eq!(
        r.stats.tiles_processed,
        dense.iter().filter(|&&d| d).count() as u32
    );
    // Sanity: the scene actually put geometry in the tiles.
    assert!(r.keys.iter().zip(&want).any(|(g, w)| w.0 != 0 && g.0 != 0));
    Some(r)
}

#[test]
fn nonrt_matches_z_sort_reference_all_tiles() {
    let Some(r) = check_nonrt(DenseConfig::default(), [64, 48], |_, _| true, 1) else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    assert_eq!(
        r.stats.bin_overflow + r.stats.slot_overflow + r.stats.clamped_tiles,
        0,
        "{:?}",
        r.stats
    );
}

#[test]
fn nonrt_sparse_tiles_and_partial_edge_tiles() {
    // 100x70 is not a multiple of 16: the right/bottom tiles are partial.
    if check_nonrt(
        DenseConfig::default(),
        [100, 70],
        |x, y| (x + y) % 2 == 0,
        2,
    )
    .is_none()
    {
        eprintln!("SKIP: no GPU adapter");
    }
}

#[test]
fn nonrt_low_shared_memory_config() {
    if check_nonrt(DenseConfig::low_shared_memory(), [60, 45], |_, _| true, 3).is_none() {
        eprintln!("SKIP: no GPU adapter");
    }
}

#[test]
fn nonrt_many_layers_use_extra_peels_but_stay_exact() {
    let cfg = DenseConfig {
        slab_tris: 16,
        nominal_layers: 2,
        max_layers: 4096,
        ..DenseConfig::default()
    };
    let Some(r) = check_nonrt(cfg, [64, 48], |_, _| true, 4) else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    assert!(r.stats.extra_layers > 0, "{:?}", r.stats);
    assert_eq!(r.stats.clamped_tiles, 0);
}

#[test]
fn nonrt_clamp_and_overflow_counters() {
    let Some(g) = gpu(false) else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let scene = random_scene(5, 80, 40, 0.3);
    // Layer cap of 1 with tiny slabs: tiles must clamp and say so.
    let cfg = DenseConfig {
        slab_tris: 8,
        nominal_layers: 1,
        max_layers: 1,
        ..DenseConfig::default()
    };
    let r = run_nonrt(&g, &scene, cfg, [64, 48], &[true; 12]);
    assert!(
        r.stats.clamped_tiles > 0 && r.stats.clamped_tris > 0 && r.stats.overflowed(),
        "{:?}",
        r.stats
    );
    // Tiny shared list pool: entries past it are dropped and counted.
    let cfg = DenseConfig {
        list_pool: 4,
        ..DenseConfig::default()
    };
    let r = run_nonrt(&g, &scene, cfg, [64, 48], &[true; 12]);
    assert!(r.stats.bin_overflow > 0, "{:?}", r.stats);
    // A tiny peel chunk loses nothing: lists longer than the chunk are peeled in
    // several passes and give the same pixels as one pass.
    let whole = run_nonrt(&g, &scene, DenseConfig::default(), [64, 48], &[true; 12]);
    let cfg = DenseConfig {
        cluster_cap: 4,
        ..DenseConfig::default()
    };
    let chunked = run_nonrt(&g, &scene, cfg, [64, 48], &[true; 12]);
    assert_eq!(chunked.stats.bin_overflow, 0, "{:?}", chunked.stats);
    assert_eq!(
        chunked.keys, whole.keys,
        "chunked peel differs from the single pass"
    );
    // Slot cap: only some dense tiles are processed, the rest counted.
    let cfg = DenseConfig {
        max_dense_slots: 3,
        ..DenseConfig::default()
    };
    let r = run_nonrt(&g, &scene, cfg, [64, 48], &[true; 12]);
    assert_eq!(r.stats.tiles_processed, 3);
    assert_eq!(r.stats.slot_overflow, 9, "{:?}", r.stats);
}

// ── 3-RT ───────────────────────────────────────────────────────────────────

mod rt {
    use super::*;
    use glam::{Mat4, Vec3};
    use helio_core::acceleration::{BlasManager, TlasManager};
    use helio_vg_format::{bary_unpack, vis_cluster, vis_tri};

    fn quad(error: f32, cluster: u32) -> ProxyLod {
        ProxyLod {
            error,
            positions: vec![
                [-2.0, -2.0, 0.0],
                [2.0, -2.0, 0.0],
                [2.0, 2.0, 0.0],
                [-2.0, 2.0, 0.0],
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            prim_map: vec![0, 1],
            clusters: vec![cluster],
            cluster_pages: vec![],
        }
    }

    #[test]
    fn rt_primary_visibility_matches_cpu_ray_cast() {
        let Some(g) = gpu(true) else {
            eprintln!("SKIP: no ray-query device");
            return;
        };
        assert!(rt_available(&g.device));
        let cfg = DenseConfig::default();
        let mut pass = match DenseRtPass::new(&g.device, cfg) {
            Ok(p) => p,
            Err(e) => panic!("{e}"),
        };
        let mut blas = BlasManager::new(g.device.clone());
        let mut tlas = TlasManager::new(g.device.clone(), 16);

        pass.register_proxy_asset(
            &g.device,
            &ProxyAsset {
                asset_key: 7,
                lods: vec![quad(0.0, 100), quad(1.0, 200)],
            },
        )
        .unwrap();

        let viewport = [64u32, 48];
        let (near, fov) = (0.1f32, 1.0f32);
        let proj = Mat4::perspective_infinite_reverse_rh(fov, 64.0 / 48.0, near);
        let frame = DenseFrame {
            view_proj: proj.to_cols_array_2d(),
            inv_view_proj: proj.inverse().to_cols_array_2d(),
            camera_pos: [0.0; 3],
            viewport,
            visible_capacity: 0,
            ray_t: [near, 1.0e30],
        };
        let ppu = pixels_per_unit(viewport[1], fov);
        // One instance close (needs the fine LOD), one far (coarse is fine).
        let mk = |id: u32, z: f32| RtInstanceRequest {
            instance_id: id,
            asset_key: 7,
            transform: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., z],
            world_center: [0.0, 0.0, z],
            world_radius: 2.9,
            error_scale: 1.0,
        };
        // The near quad is small on screen (x in [-0.5,0.5] via scale) -- keep both full size but offset in depth:
        // the far one is hidden behind the near one at the centre and visible nowhere else only if it is bigger;
        // make it bigger through its own transform.
        let mut far = mk(2, -400.0);
        far.transform = [100., 0., 0., 0., 0., 100., 0., 0., 0., 0., 1., -400.0];
        far.world_radius = 290.0;
        far.error_scale = 1.0;
        let requests = [mk(1, -5.0), far];

        let mut enc = g.device.create_command_encoder(&Default::default());
        let info = pass
            .prepare_frame(
                &g.device, &g.queue, &mut enc, &mut blas, &mut tlas, &frame, &requests, 1.0, ppu,
            )
            .expect("prepare");
        assert!(
            info.complete() && info.tlas_instances == 2 && info.rt_visible_count == 2,
            "{info:?}"
        );
        let lod_of = |id: u32| {
            info.selections
                .iter()
                .find(|(i, _)| *i == id)
                .unwrap()
                .1
                .lod
        };
        assert_eq!(lod_of(1), 0, "near instance must use the finest proxy");
        assert_eq!(lod_of(2), 1, "far instance may use the coarse proxy");

        let mask = buf(&g, "mask", cast_slice(&vec![1u32; 12]));
        pass.record(&g.device, &g.queue, &mut enc, &frame, &mask, &tlas)
            .expect("record");
        let b = pass.buffers().unwrap();
        b.copy_stats(&mut enc);
        g.queue.submit(Some(enc.finish()));
        let stats = b.read_stats(&g.device);
        assert_eq!(stats.tiles_processed, 12);

        let raw = read_buffer(&g.device, &g.queue, &b.dense_key, 64 * 48 * 8);
        let keys: &[u32] = cast_slice(&raw);
        let bary_raw = read_buffer(&g.device, &g.queue, pass.dense_bary().unwrap(), 64 * 48 * 4);
        let bary: &[u32] = cast_slice(&bary_raw);
        let vis_raw = read_buffer(&g.device, &g.queue, pass.rt_visible(), 2 * 8);
        let vis: &[DenseVisibleGpu] = cast_slice(&vis_raw);

        // CPU ray cast against the same two instances.
        let inv = proj.inverse();
        let mut hits = 0;
        let mut far_hits = 0;
        for y in 0..48u32 {
            for x in 0..64u32 {
                let i = (y * 64 + x) as usize;
                let ndc = glam::Vec2::new(
                    (x as f32 + 0.5) / 64.0 * 2.0 - 1.0,
                    1.0 - (y as f32 + 0.5) / 48.0 * 2.0,
                );
                let p = inv * glam::Vec4::new(ndc.x, ndc.y, 1.0, 1.0);
                let dir = (p.truncate() / p.w).normalize();
                // Near quad: z=-5, |x|,|y| <= 2.  Far quad: z=-400, |x|,|y| <= 200.
                let hit = |z: f32, half: f32| {
                    let t = z / dir.z;
                    let q = dir * t;
                    (t > 0.0 && q.x.abs() <= half && q.y.abs() <= half).then_some((t, q))
                };
                let want = hit(-5.0, 2.0)
                    .map(|h| (1u32, h))
                    .or_else(|| hit(-400.0, 200.0).map(|h| (2u32, h)));
                let (zbits, id) = (keys[i * 2], keys[i * 2 + 1]);
                match want {
                    None => assert_eq!(zbits, 0, "pixel ({x},{y}) should miss"),
                    Some((inst, (t, q))) => {
                        // Skip pixels grazing the quad border or the shared diagonal.
                        let scale = if inst == 1 { 1.0 } else { 100.0 };
                        let local = Vec3::new(q.x / scale, q.y / scale, 0.0);
                        let border = (2.0 - local.x.abs()).min(2.0 - local.y.abs());
                        if border < 0.05 || (local.x - local.y).abs() < 0.05 {
                            continue;
                        }
                        assert_ne!(zbits, 0, "pixel ({x},{y}) should hit instance {inst}");
                        hits += 1;
                        far_hits += (inst == 2) as u32;
                        let row = vis_cluster(id) - DENSE_RT_VISIBLE_BASE;
                        let v = vis[row as usize];
                        assert_eq!(v.instance, inst);
                        assert_eq!(
                            v.cluster_slot,
                            if inst == 1 { 100 } else { 200 },
                            "wrong proxy LOD behind the id"
                        );
                        // Triangle 0 = (0,1,2) lower-right half, triangle 1 = (0,2,3) upper-left.
                        let want_tri = if local.x > local.y { 0 } else { 1 };
                        assert_eq!(vis_tri(id), want_tri, "pixel ({x},{y})");
                        let z = f32::from_bits(zbits);
                        // Reversed infinite projection: z_ndc = near / view_depth, view_depth = -q.z.
                        let want_z = near / -q.z;
                        let _ = t;
                        assert!(
                            (z - want_z).abs() < 1e-4 * want_z.max(1e-3),
                            "depth {z} vs {want_z}"
                        );
                        // Barycentrics reproduce the hit point on the proxy triangle.
                        let (u, w) = bary_unpack(bary[i]);
                        let tri: [[f32; 2]; 3] = if want_tri == 0 {
                            [[-2., -2.], [2., -2.], [2., 2.]]
                        } else {
                            [[-2., -2.], [2., 2.], [-2., 2.]]
                        };
                        let bx = (1.0 - u - w) * tri[0][0] + u * tri[1][0] + w * tri[2][0];
                        let by = (1.0 - u - w) * tri[0][1] + u * tri[1][1] + w * tri[2][1];
                        assert!(
                            (bx - local.x).abs() < 2e-2 && (by - local.y).abs() < 2e-2,
                            "bary ({bx},{by}) vs {local:?}"
                        );
                    }
                }
            }
        }
        assert!(
            hits > 500 && far_hits > 100,
            "hits={hits} far_hits={far_hits}"
        );
        let _ = Vec3::ZERO;
    }

    fn paged_quad(error: f32, local: u32, page: u32) -> ProxyLod {
        ProxyLod {
            cluster_pages: vec![page],
            ..quad(error, local)
        }
    }

    /// Page-addressed proxies: slots come from the residency resolver, an
    /// instance whose pages are not resident is left out (and its pages are
    /// reported as wanted), and unchanged inputs rebuild neither the TLAS nor
    /// the tables; a residency change rebuilds the tables only.
    #[test]
    fn page_addressed_proxies_resolve_slots_and_skip_unchanged_rebuilds() {
        let Some(g) = gpu(true) else {
            eprintln!("SKIP: no ray-query device");
            return;
        };
        let mut pass = DenseRtPass::new(&g.device, DenseConfig::default()).unwrap();
        let mut blas = BlasManager::new(g.device.clone());
        let mut tlas = TlasManager::new(g.device.clone(), 16);
        pass.register_proxy_asset(
            &g.device,
            &ProxyAsset {
                asset_key: 7,
                lods: vec![paged_quad(0.0, 5, 3), paged_quad(1.0, 9, 4)],
            },
        )
        .unwrap();
        assert_eq!(pass.lod_pages(7, 0), &[3]);
        let read_rows = |pass: &DenseRtPass| {
            let raw = read_buffer(&g.device, &g.queue, pass.rt_visible(), 8);
            let rows: &[DenseVisibleGpu] = cast_slice(&raw);
            rows[0]
        };

        let viewport = [64u32, 48];
        let (near, fov) = (0.1f32, 1.0f32);
        let proj = Mat4::perspective_infinite_reverse_rh(fov, 64.0 / 48.0, near);
        let frame = DenseFrame {
            view_proj: proj.to_cols_array_2d(),
            inv_view_proj: proj.inverse().to_cols_array_2d(),
            camera_pos: [0.0; 3],
            viewport,
            visible_capacity: 0,
            ray_t: [near, 1.0e30],
        };
        let ppu = pixels_per_unit(viewport[1], fov);
        let requests = [RtInstanceRequest {
            instance_id: 4,
            asset_key: 7,
            transform: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., -5.0],
            world_center: [0.0, 0.0, -5.0],
            world_radius: 2.9,
            error_scale: 1.0,
        }];
        let mut step = |pass: &mut DenseRtPass, version: u64, resident: bool| {
            let mut enc = g.device.create_command_encoder(&Default::default());
            let resolve = move |_key: u64, page: u32| resident.then_some(page + 10);
            let info = pass
                .prepare_frame_resolved(
                    &g.device, &g.queue, &mut enc, &mut blas, &mut tlas, &frame, &requests, 1.0,
                    ppu, version, &resolve,
                )
                .expect("prepare");
            g.queue.submit(Some(enc.finish()));
            info
        };

        let first = step(&mut pass, 1, true);
        assert!(first.complete() && first.tlas_instances == 1, "{first:?}");
        assert!(
            first.tlas_rebuilt && first.tables_rebuilt && first.blas_builds == 1,
            "{first:?}"
        );
        let row = read_rows(&pass);
        assert_eq!(row.instance, 4);
        assert_eq!(
            row.cluster_slot,
            (13 << helio_vg_format::PAGE_CLUSTER_SHIFT) | 5
        );

        // Identical inputs: nothing is rebuilt.
        let again = step(&mut pass, 1, true);
        assert!(
            !again.tlas_rebuilt && !again.tables_rebuilt && again.blas_builds == 0,
            "{again:?}"
        );
        assert_eq!(again.rt_visible_count, first.rt_visible_count);

        // The pool layout changed: tables are rewritten, the TLAS is not.
        let moved = step(&mut pass, 2, true);
        assert!(moved.tables_rebuilt && !moved.tlas_rebuilt, "{moved:?}");

        // Pages gone: the instance is left out, flagged, and its pages wanted.
        let gone = step(&mut pass, 3, false);
        assert_eq!(gone.unresolved_page_instances, 1, "{gone:?}");
        assert!(!gone.complete() && gone.tlas_instances == 0, "{gone:?}");
        assert_eq!(gone.wanted_lods, vec![(7, 0)]);
    }

    /// An instance whose proxy table would exceed the visible-row cap is
    /// dropped and reported, never truncated silently.
    #[test]
    fn visible_row_cap_drops_instances_and_reports_them() {
        let Some(g) = gpu(true) else {
            eprintln!("SKIP: no ray-query device");
            return;
        };
        let mut pass = DenseRtPass::new(&g.device, DenseConfig::default()).unwrap();
        pass.set_max_visible_rows(1);
        let mut blas = BlasManager::new(g.device.clone());
        let mut tlas = TlasManager::new(g.device.clone(), 16);
        pass.register_proxy_asset(
            &g.device,
            &ProxyAsset {
                asset_key: 7,
                lods: vec![quad(0.0, 1)],
            },
        )
        .unwrap();
        let proj = Mat4::perspective_infinite_reverse_rh(1.0, 64.0 / 48.0, 0.1);
        let frame = DenseFrame {
            view_proj: proj.to_cols_array_2d(),
            inv_view_proj: proj.inverse().to_cols_array_2d(),
            camera_pos: [0.0; 3],
            viewport: [64, 48],
            visible_capacity: 0,
            ray_t: [0.1, 1.0e30],
        };
        let mk = |id: u32| RtInstanceRequest {
            instance_id: id,
            asset_key: 7,
            transform: [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., -5.0],
            world_center: [0.0, 0.0, -5.0],
            world_radius: 2.9,
            error_scale: 1.0,
        };
        let mut enc = g.device.create_command_encoder(&Default::default());
        let info = pass
            .prepare_frame(
                &g.device,
                &g.queue,
                &mut enc,
                &mut blas,
                &mut tlas,
                &frame,
                &[mk(1), mk(2)],
                1.0,
                100.0,
            )
            .unwrap();
        g.queue.submit(Some(enc.finish()));
        assert_eq!(
            (info.tlas_instances, info.dropped_instances),
            (1, 1),
            "{info:?}"
        );
        assert!(!info.complete());
    }

    #[test]
    fn rt_unavailable_is_reported_not_panicked() {
        // A device without the feature must yield Unsupported, never a panic.
        let Some(g) = gpu(false) else {
            eprintln!("SKIP: no GPU adapter");
            return;
        };
        if !rt_available(&g.device) {
            assert!(matches!(
                DenseRtPass::new(&g.device, DenseConfig::default()),
                Err(RtError::Unsupported)
            ));
        }
        let _ = TlasManager::new(g.device.clone(), 1);
    }
}

#[test]
fn nonrt_bins_every_cull_range_of_both_phases_and_nothing_else() {
    let Some(g) = gpu(false) else {
        eprintln!("SKIP: no GPU adapter");
        return;
    };
    let cfg = DenseConfig::default();
    let viewport = [64, 48];
    let scene = random_scene(8, 80, 40, 0.3);
    let dense = vec![true; 12];
    let r = run_nonrt_layout(&g, &scene, cfg, viewport, &dense, true);
    let want = expected_tiles(&scene, &cfg, viewport, &dense, true);
    let (bad, n) = mismatch_fraction(&r.keys, &want, |_| true);
    assert!(
        n > 0 && bad * 100 <= n,
        "{bad}/{n} pixels differ for the split two-phase layout"
    );
    assert!(r.keys.iter().zip(&want).any(|(g, w)| w.0 != 0 && g.0 != 0));
}
