//! GPU test of the ray-traced backend's dense-instance collection: which
//! instances overlap dense tiles, with their transform, deduplicated, read
//! back asynchronously and bounded.

use bytemuck::cast_slice;
use helio_pass_vg_dense::*;
use helio_vg_format::testing::{TestCluster, TestPage};
use helio_vg_format::{GpuVgInstance, GpuVisibleCluster, INST_ENABLED};
use std::sync::Arc;

struct Gpu {
    device: Arc<wgpu::Device>,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;
    Some(Gpu {
        device: Arc::new(device),
        queue,
    })
}

fn buf(g: &Gpu, bytes: &[u8]) -> wgpu::Buffer {
    let b = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bytes.len() as u64).max(16).next_multiple_of(4),
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    g.queue.write_buffer(&b, 0, bytes);
    b
}

const IDENTITY: [[f32; 4]; 4] = [
    [1., 0., 0., 0.],
    [0., 1., 0., 0.],
    [0., 0., 1., 0.],
    [0., 0., 0., 1.],
];

/// Instance `i` is the matrix scale `1 + 0.1 i`, translated by `(0, 0, 0.01 i)`
/// (column-major, as `static_objects` stores it).
fn object_row(i: u32) -> [f32; 63] {
    let s = 1.0 + 0.1 * i as f32;
    let mut m = IDENTITY;
    m[0][0] = s;
    m[1][1] = s;
    m[3][2] = 0.01 * i as f32;
    let mut row = [0f32; 63];
    row[4..20].copy_from_slice(cast_slice(&m));
    row
}

struct Scene {
    tile_mask: wgpu::Buffer,
    cull_state: wgpu::Buffer,
    visible: wgpu::Buffer,
    pool: wgpu::Buffer,
    quant: wgpu::Buffer,
    instances: wgpu::Buffer,
    objects: wgpu::Buffer,
    tiles: u32,
}

const VIEWPORT: [u32; 2] = [64, 48];

/// Three clusters (one triangle each, covering the screen centre), cluster `i`
/// belonging to instance `i`; instance rows 0..3.
fn scene(g: &Gpu, dense_everywhere: bool) -> Scene {
    let tris = [[0u8, 1, 2]];
    let positions = [[-0.5f32, -0.5, 0.5], [0.5, -0.5, 0.5], [0.0, 0.5, 0.5]];
    let clusters: Vec<TestCluster<'_>> = (0..3)
        .map(|_| TestCluster {
            positions: &positions,
            tris: &tris,
            material: 0,
            attrs: None,
        })
        .collect();
    let page = TestPage::build(0, &clusters);
    let visible: Vec<GpuVisibleCluster> = (0..3)
        .map(|i| GpuVisibleCluster {
            instance: i,
            cluster_slot: page.slots[i as usize],
        })
        .collect();
    let tiles = VIEWPORT[0].div_ceil(16) * VIEWPORT[1].div_ceil(16);
    let mask: Vec<u32> = (0..tiles).map(|_| dense_everywhere as u32).collect();
    let mut cull_state = vec![0u32; 128];
    // Phase 0, bin 0: {first 0, count 3}.
    cull_state[helio_vg_format::CULL_STATE_RANGES as usize + 1] = 3;
    let instances: Vec<GpuVgInstance> = (0..3)
        .map(|i| GpuVgInstance {
            asset_slot: 7,
            flags: INST_ENABLED,
            lod_error_scale: 1.0 + i as f32,
            object_row: i,
        })
        .collect();
    let mut objects = Vec::new();
    for i in 0..3 {
        objects.extend_from_slice(&object_row(i));
    }
    Scene {
        tile_mask: buf(g, cast_slice(&mask)),
        cull_state: buf(g, cast_slice(&cull_state)),
        visible: buf(g, cast_slice(&visible)),
        pool: buf(g, &page.pool()),
        quant: buf(g, cast_slice(&[page.quant])),
        instances: buf(g, cast_slice(&instances)),
        objects: buf(g, cast_slice(&objects)),
        tiles,
    }
}

fn run_frame(
    g: &Gpu,
    pass: &mut DenseNonRtPass,
    collect: &mut RtCollect,
    s: &Scene,
) -> CollectResult {
    let frame = DenseFrame {
        view_proj: IDENTITY,
        inv_view_proj: IDENTITY,
        camera_pos: [0.0; 3],
        viewport: VIEWPORT,
        visible_capacity: 3,
        ray_t: [0.0, 1.0],
    };
    let inputs = DenseNonRtInputs {
        tile_mask: &s.tile_mask,
        cull_state: &s.cull_state,
        visible: &s.visible,
        page_pool: &s.pool,
        asset_quant: &s.quant,
        vg_instances: &s.instances,
        objects: &s.objects,
    };
    let mut enc = g.device.create_command_encoder(&Default::default());
    {
        let mut rec = helio_core::CommandRecorder::from_encoder(&mut enc);
        pass.record_setup_rec(&g.device, &g.queue, &mut rec, &frame, &inputs);
        collect.record_rec(
            &g.device,
            &g.queue,
            &mut rec,
            pass.buffers().unwrap(),
            &inputs,
            3,
        );
    }
    g.queue.submit([enc.finish()]);
    // The next frame's prepare: start mapping, then wait for the result (a
    // test may block; the renderer polls without waiting).
    collect.begin_maps();
    for _ in 0..1000 {
        let _ = g.device.poll(wgpu::PollType::wait_indefinitely());
        if let Some(r) = collect.poll(&g.device) {
            return r;
        }
    }
    panic!("the candidate readback never arrived");
}

#[test]
fn collect_reports_each_dense_instance_once_with_its_transform() {
    let Some(g) = gpu() else { return };
    let cfg = DenseConfig::default();
    let mut pass = DenseNonRtPass::new(&g.device, cfg).unwrap();
    let mut collect = RtCollect::new(&g.device, &cfg, 8);
    let s = scene(&g, true);

    let r = run_frame(&g, &mut pass, &mut collect, &s);
    assert_eq!(r.total, 3, "{r:?}");
    assert_eq!(r.uncovered, 3, "nothing is in the TLAS yet");
    assert_eq!(r.dense_tiles, s.tiles);
    assert!(!r.overflowed());
    let mut records = r.records.clone();
    records.sort_by_key(|c| c.instance);
    for (i, c) in records.iter().enumerate() {
        let sc = 1.0 + 0.1 * i as f32;
        assert_eq!(c.instance, i as u32);
        assert_eq!(c.asset_slot, 7);
        assert_eq!(c.lod_error_scale, 1.0 + i as f32);
        // Row-major 3x4: scale on the diagonal, translation in the last column.
        let want = [
            sc,
            0.0,
            0.0,
            0.0,
            0.0,
            sc,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.01 * i as f32,
        ];
        for (k, (a, b)) in c.transform.iter().zip(&want).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "instance {i} transform[{k}]: {a} vs {b}"
            );
        }
    }

    // Tell the GPU instances 0 and 1 are in the TLAS: only 2 is uncovered.
    collect.set_covered(&g.queue, &[1, 0]);
    let r = run_frame(&g, &mut pass, &mut collect, &s);
    assert_eq!((r.total, r.uncovered), (3, 1), "{r:?}");
    // Unchanged set: no new epoch.
    let epoch = collect.covered_epoch();
    collect.set_covered(&g.queue, &[0, 1]);
    assert_eq!(collect.covered_epoch(), epoch);
    collect.set_covered(&g.queue, &[0, 1, 2]);
    let r = run_frame(&g, &mut pass, &mut collect, &s);
    assert_eq!((r.total, r.uncovered), (3, 0), "{r:?}");
    collect.set_covered(&g.queue, &[]);
    let r = run_frame(&g, &mut pass, &mut collect, &s);
    assert_eq!(r.uncovered, 3, "{r:?}");
}

#[test]
fn collect_ignores_instances_outside_dense_tiles_and_is_bounded() {
    let Some(g) = gpu() else { return };
    let cfg = DenseConfig::default();
    let mut pass = DenseNonRtPass::new(&g.device, cfg).unwrap();
    // No dense tile: nothing is collected.
    let mut collect = RtCollect::new(&g.device, &cfg, 8);
    let s = scene(&g, false);
    let r = run_frame(&g, &mut pass, &mut collect, &s);
    assert_eq!((r.total, r.dense_tiles), (0, 0), "{r:?}");
    // Capacity 2 of 3 instances: the total is exact, the records are bounded.
    let mut small = RtCollect::new(&g.device, &cfg, 2);
    let s = scene(&g, true);
    let r = run_frame(&g, &mut pass, &mut small, &s);
    assert_eq!(r.total, 3);
    assert_eq!(r.records.len(), 2);
    assert!(r.overflowed());
}
