//! Headless GPU test of the whole raster: Tier 1 vis buffer, Tier 2 (64-bit
//! atomic kernel when the device has it, and the tiled fallback), tile
//! counters and classification, against the CPU reference. Skips (passes)
//! when no adapter/device can be created.

use glam::{Mat4, Vec3};
use helio_core::GpuCameraUniforms;
use helio_pass_vg_raster::abi::*;
use helio_pass_vg_raster::reference::{self, RefTri};
use helio_pass_vg_raster::stream::{BuiltPool, PagePool};
use helio_pass_vg_raster::*;
use helio_vg_format::{GpuVgInstance, INST_ENABLED};
use wgpu::util::DeviceExt;

const W: u32 = 64;
const H: u32 = 64;

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    int64: bool,
}

/// GPU tests share one adapter; two devices live at once misbehaved on the
/// development machine (a stale, uncleared attachment), so they run serially.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let want = wgpu::Features::INDIRECT_FIRST_INSTANCE;
    if !adapter.features().contains(want) {
        eprintln!("skipping: INDIRECT_FIRST_INSTANCE unavailable");
        return None;
    }
    let opt = optional_features() & adapter.features();
    let features = want
        | if supports_int64_atomics(opt) {
            opt
        } else {
            wgpu::Features::empty()
        };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("vg-raster test"),
        required_features: features,
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;
    device.on_uncaptured_error(std::sync::Arc::new(|e| panic!("wgpu error: {e:?}")));
    Some(Gpu {
        int64: supports_int64_atomics(device.features()),
        device,
        queue,
    })
}

/// A flat n x n grid of CCW (as seen from +Z) triangles on plane z, spanning
/// [x0, x0+size] x [y0, y0+size], in asset space.
fn grid(x0: f32, y0: f32, size: f32, z: f32, n: usize) -> (Vec<[f32; 3]>, Vec<[u8; 3]>) {
    let mut pos = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            pos.push([
                x0 + size * i as f32 / n as f32,
                y0 + size * j as f32 / n as f32,
                z,
            ]);
        }
    }
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u8;
    let mut tris = Vec::new();
    for j in 0..n {
        for i in 0..n {
            tris.push([at(i, j), at(i + 1, j), at(i, j + 1)]);
            tris.push([at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    (pos, tris)
}

fn buffer_init<T: bytemuck::Pod>(
    g: &Gpu,
    label: &str,
    data: &[T],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    g.device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(data),
            usage,
        })
}

fn read_buffer(g: &Gpu, src: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let rb = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(src, 0, &rb, 0, size);
    g.queue.submit([enc.finish()]);
    map(g, &rb)
}

fn map(g: &Gpu, rb: &wgpu::Buffer) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    rb.slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    g.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let v = rb.slice(..).get_mapped_range().unwrap().to_vec();
    rb.unmap();
    v
}

fn read_texture(g: &Gpu, tex: &wgpu::Texture, aspect: wgpu::TextureAspect) -> Vec<u32> {
    let bpr = W * 4; // 256: already aligned
    let rb = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tex readback"),
        size: (bpr * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &rb,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit([enc.finish()]);
    bytemuck::cast_slice(&map(g, &rb)).to_vec()
}

struct Scene {
    pool: PagePool,
    built: BuiltPool,
    camera: GpuCameraUniforms,
    /// Cluster index ranges: [0] HW, [1..] SW.
    sw_first: u32,
    sw_count: u32,
    hw_tris: u32,
}

fn scene() -> Scene {
    let mut pool = PagePool::default();
    // A: HW cluster, 4x4 grid (32 tris) at z = -2, nonzero offsets keep pixel
    // centers off triangle edges. At z=-2 with a 90 deg fov, ndc = x / 2.
    let (p, t) = grid(-1.013, -0.987, 2.0, -2.0, 4);
    pool.push(&p, &t, 0, 0);
    // B: SW cluster, 8x8 grid (128 tris) of ~1.5 px triangles at z = -1.
    let (p, t) = grid(-0.4017, -0.3983, 0.8, -1.0, 8);
    pool.push(&p, &t, 0, 0);
    // C: SW cluster overlapping B, farther (z = -1.5): loses where B covers.
    let (p, t) = grid(-0.3011, -0.2989, 1.0, -1.5, 8);
    pool.push(&p, &t, 0, 0);

    let view = Mat4::IDENTITY;
    let proj = helio_core::depth::perspective(std::f32::consts::FRAC_PI_2, 1.0, 0.1);
    let camera = GpuCameraUniforms::new(
        view,
        proj,
        Vec3::ZERO,
        0.1,
        1000.0,
        0,
        [0.0, 0.0],
        proj * view,
    );
    let built = pool.build();
    Scene {
        pool,
        built,
        camera,
        sw_first: 1,
        sw_count: 2,
        hw_tris: 32,
    }
}

struct Resources {
    camera: wgpu::Buffer,
    pages: wgpu::Buffer,
    quant: wgpu::Buffer,
    visible: wgpu::Buffer,
    instances: wgpu::Buffer,
    objects: wgpu::Buffer,
    hw_indirect: wgpu::Buffer,
    sw_meta: wgpu::Buffer,
    sw_dispatch: wgpu::Buffer,
}

fn resources(g: &Gpu, s: &Scene) -> Resources {
    let st = wgpu::BufferUsages::STORAGE;
    let mut pages = s.built.bytes.clone();
    while pages.len() % 16 != 0 {
        pages.push(0);
    }
    pages.extend_from_slice(&[0u8; 16]);
    let inst = GpuVgInstance {
        asset_slot: 0,
        flags: INST_ENABLED,
        lod_error_scale: 1.0,
        object_row: 0,
    };
    let mut object = [0f32; 63];
    object[4..20].copy_from_slice(&Mat4::IDENTITY.to_cols_array());
    let mut hw = [DrawIndirectArgs::default(); 4];
    hw[hw_bin_for_tris(s.hw_tris)] = DrawIndirectArgs {
        vertex_count: 3 * HW_BIN_CAPS[hw_bin_for_tris(s.hw_tris)],
        instance_count: 1,
        first_vertex: 0,
        first_instance: 0,
    };
    Resources {
        camera: buffer_init(g, "camera", &[s.camera], st),
        pages: buffer_init(g, "pages", &pages, st),
        quant: buffer_init(g, "quant", &[s.built.quant], st),
        visible: buffer_init(g, "visible", &s.built.visible, st),
        instances: buffer_init(g, "instances", &[inst], st),
        objects: buffer_init(g, "objects", &object, st),
        hw_indirect: buffer_init(g, "hw indirect", &hw, wgpu::BufferUsages::INDIRECT),
        sw_meta: buffer_init(
            g,
            "sw meta",
            &[SwMeta {
                first: s.sw_first,
                count: s.sw_count,
                _pad: [0; 2],
            }],
            st,
        ),
        sw_dispatch: buffer_init(
            g,
            "sw dispatch",
            &sw_dispatch_args(s.sw_count),
            wgpu::BufferUsages::INDIRECT,
        ),
    }
}

/// Reference triangles for every cluster of the scene, built from the
/// *decoded* (quantized) positions so the GPU and CPU see the same inputs.
fn reference_tris(s: &Scene, range: std::ops::Range<usize>) -> Vec<RefTri> {
    let size = [W as f32, H as f32];
    let vp = Mat4::from_cols_array(&s.camera.view_proj);
    let mut out = Vec::new();
    for vi in range {
        let entry = &s.pool.entries[vi];
        let pos = &s.built.page.decoded[vi];
        let scr: Vec<_> = pos
            .iter()
            .map(|p| {
                let c = vp * glam::Vec4::new(p[0], p[1], p[2], 1.0);
                reference::to_screen([c.x, c.y, c.z, c.w], size)
            })
            .collect();
        for (t, tri) in entry.tris.iter().enumerate() {
            let i = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            out.push(RefTri {
                v: [scr[i[0]], scr[i[1]], scr[i[2]]],
                id: vis_pack(vi as u32, t as u32),
                mirrored: false,
            });
        }
    }
    out
}

fn vis_pack(c: u32, t: u32) -> u32 {
    helio_vg_format::vis_pack(c, t)
}

struct Output {
    vis: Vec<u32>,
    depth: Vec<f32>,
    keys: Vec<u64>,
    tile_counters: Vec<u32>,
    stats: Vec<u32>,
    mask: Vec<u32>,
    dense_args: Vec<u32>,
    fallback: bool,
}

fn run(g: &Gpu, s: &Scene, force_fallback: bool, thresholds: (u32, u32)) -> Output {
    let cfg = VgRasterConfig {
        force_tile_fallback: force_fallback,
        dense_tri_threshold: thresholds.0,
        dense_hit_threshold: thresholds.1,
        ..Default::default()
    };
    let raster = VgRaster::new(&g.device, cfg);
    let mut targets = VgTargets::new(&g.device, W, H, &raster);
    let r = resources(g, s);
    let inputs = VgRasterInputs {
        camera: &r.camera,
        view_index: 0,
        pages: &r.pages,
        asset_quant: &r.quant,
        visible: &r.visible,
        vg_instances: &r.instances,
        objects: &r.objects,
        hw_indirect: &r.hw_indirect,
        hw_indirect_offset: 0,
        sw_meta: &r.sw_meta,
        sw_meta_offset_words: 0,
        sw_dispatch: &r.sw_dispatch,
        sw_dispatch_offset: 0,
        masked: None,
        dense: None,
    };
    let mut enc = g.device.create_command_encoder(&Default::default());
    raster.encode(&g.device, &g.queue, &mut enc, &mut targets, &inputs);
    g.queue.submit([enc.finish()]);

    let tiles = (targets.tiles_x * targets.tiles_y) as u64;
    let u32s = |b: &wgpu::Buffer, n: u64| -> Vec<u32> {
        bytemuck::cast_slice(&read_buffer(g, b, n * 4)).to_vec()
    };
    Output {
        vis: read_texture(g, &targets.vis, wgpu::TextureAspect::All),
        depth: read_texture(g, &targets.depth, wgpu::TextureAspect::DepthOnly)
            .into_iter()
            .map(f32::from_bits)
            .collect(),
        keys: bytemuck::cast_slice(&read_buffer(g, &targets.sw_keys, (W * H * 8) as u64)).to_vec(),
        tile_counters: u32s(&targets.tile_counters, tiles * 2),
        stats: u32s(&targets.stats, stat::COUNT as u64),
        // This frame's mask is "last frame" after the encode swap.
        mask: u32s(targets.dense_mask_last_frame(), tiles),
        dense_args: u32s(&targets.dense_args, 4),
        fallback: raster.uses_tile_fallback(),
    }
}

fn check_tier1(s: &Scene, o: &Output) {
    let hw_ref = reference::raster(&reference_tris(s, 0..1), W, H, None, true);
    let mut covered = 0;
    let mut mismatch = 0;
    for i in 0..(W * H) as usize {
        let expect = hw_ref.keys[i] != 0;
        let got = o.vis[i] != VIS_EMPTY;
        covered += expect as u32;
        if expect != got || (expect && o.vis[i] != hw_ref.keys[i] as u32) {
            mismatch += 1;
        }
        if got {
            assert_eq!(
                helio_vg_format::vis_cluster(o.vis[i]),
                0,
                "Tier 1 drew a non-HW cluster"
            );
            assert!(
                o.depth[i] > 0.0,
                "pixel {} ({},{}) vis {:#x} depth {} fallback {}",
                i,
                i % 64,
                i / 64,
                o.vis[i],
                o.depth[i],
                o.fallback
            );
        } else {
            assert_eq!(
                o.depth[i], 0.0,
                "depth must stay cleared where nothing was drawn"
            );
        }
    }
    assert!(covered > 600, "HW footprint unexpectedly small: {covered}");
    assert!(
        mismatch * 100 <= covered,
        "Tier 1 differs from the reference at {mismatch}/{covered} pixels"
    );
}

fn check_tier2(s: &Scene, o: &Output) {
    let r = reference::raster(&reference_tris(s, 1..3), W, H, Some(&o.depth), true);
    let covered = r.keys.iter().filter(|&&k| k != 0).count();
    assert!(covered > 300, "SW footprint unexpectedly small: {covered}");
    let mut mismatch = 0;
    for i in 0..(W * H) as usize {
        let (a, b) = (r.keys[i], o.keys[i]);
        if (a == 0) != (b == 0) || (a != 0 && (a as u32 != b as u32)) {
            mismatch += 1;
        } else if a != 0 {
            let (da, db) = (
                f32::from_bits((a >> 32) as u32),
                f32::from_bits((b >> 32) as u32),
            );
            assert!((da - db).abs() <= 1e-5 * da.max(1e-3), "depth {da} vs {db}");
        }
    }
    assert!(
        mismatch * 100 <= covered as u32 * 1,
        "Tier 2 differs from the reference at {mismatch}/{covered} pixels"
    );
    assert_eq!(o.stats[stat::BIN_OVERFLOW], 0);
    assert_eq!(o.stats[stat::CLIPPED_TRIS], 0);
    // Where B (nearer) and C overlap, B wins: B has ids with cluster 1.
    let b_wins = o
        .keys
        .iter()
        .filter(|&&k| k != 0 && helio_vg_format::vis_cluster(k as u32) == 1)
        .count();
    let c_wins = o
        .keys
        .iter()
        .filter(|&&k| k != 0 && helio_vg_format::vis_cluster(k as u32) == 2)
        .count();
    assert!(b_wins > 200 && c_wins > 20, "B {b_wins} C {c_wins}");
    // Tile triangle counters match the reference exactly (hit counts may differ at edges).
    for t in 0..(o.tile_counters.len() / 2) {
        assert_eq!(
            o.tile_counters[t * 2],
            r.tile_counters[t * 2],
            "tile {t} triangle count"
        );
    }
    let (gh, rh): (u32, u32) = (
        o.tile_counters.iter().skip(1).step_by(2).sum(),
        r.tile_counters.iter().skip(1).step_by(2).sum(),
    );
    assert!(gh.abs_diff(rh) * 50 <= rh.max(1), "hits {gh} vs {rh}");
}

fn check_classify(o: &Output, thresholds: (u32, u32)) {
    let want = reference::classify(
        &o.tile_counters,
        &vec![0; o.mask.len()],
        thresholds.0,
        thresholds.1,
        0.5,
    );
    assert_eq!(o.mask, want, "dense mask");
    let count = want.iter().sum::<u32>();
    assert!(
        count > 0 && (count as usize) < want.len(),
        "test thresholds should split the tiles: {count}"
    );
    assert_eq!(o.dense_args[3], count);
    assert_eq!(o.dense_args[0], count.min(65535));
    assert_eq!(o.dense_args[1], 1);
    assert_eq!(o.dense_args[2], 1);
}

#[test]
fn raster_matches_reference() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = gpu() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let s = scene();
    // Hits per tile where B and C overlap are ~2x256; A-only tiles ~256.
    let thr = (u32::MAX, 300);

    let fallback = run(&g, &s, true, thr);
    assert!(fallback.fallback);
    check_tier1(&s, &fallback);
    check_tier2(&s, &fallback);
    check_classify(&fallback, thr);

    if g.int64 {
        let atomic = run(&g, &s, false, thr);
        assert!(!atomic.fallback);
        check_tier1(&s, &atomic);
        check_tier2(&s, &atomic);
        check_classify(&atomic, thr);
        // Both kernels produce the same keys.
        let diff = atomic
            .keys
            .iter()
            .zip(&fallback.keys)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(
            diff, 0,
            "atomic and tiled kernels disagree at {diff} pixels"
        );
    } else {
        eprintln!("64-bit atomics unavailable on this adapter: atomic kernel not exercised");
    }
}

#[test]
fn dense_mask_has_hysteresis_across_frames() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(g) = gpu() else { return };
    let s = scene();
    // Threshold just above the A-only hit level: only the overlap tiles are dense.
    let raster = VgRaster::new(
        &g.device,
        VgRasterConfig {
            force_tile_fallback: true,
            dense_tri_threshold: u32::MAX,
            dense_hit_threshold: 300,
            ..Default::default()
        },
    );
    let mut targets = VgTargets::new(&g.device, W, H, &raster);
    let r = resources(&g, &s);
    let inputs = VgRasterInputs {
        camera: &r.camera,
        view_index: 0,
        pages: &r.pages,
        asset_quant: &r.quant,
        visible: &r.visible,
        vg_instances: &r.instances,
        objects: &r.objects,
        hw_indirect: &r.hw_indirect,
        hw_indirect_offset: 0,
        sw_meta: &r.sw_meta,
        sw_meta_offset_words: 0,
        sw_dispatch: &r.sw_dispatch,
        sw_dispatch_offset: 0,
        masked: None,
        dense: None,
    };
    let tiles = (targets.tiles_x * targets.tiles_y) as u64;
    let mut dense = Vec::new();
    for _ in 0..3 {
        let mut enc = g.device.create_command_encoder(&Default::default());
        raster.encode(&g.device, &g.queue, &mut enc, &mut targets, &inputs);
        g.queue.submit([enc.finish()]);
        let m: Vec<u32> =
            bytemuck::cast_slice(&read_buffer(&g, targets.dense_mask_last_frame(), tiles * 4))
                .to_vec();
        dense.push(m.iter().sum::<u32>());
    }
    // Static scene: the classification is stable frame to frame.
    assert!(dense[0] > 0);
    assert_eq!(dense[0], dense[1]);
    assert_eq!(dense[1], dense[2]);
}
