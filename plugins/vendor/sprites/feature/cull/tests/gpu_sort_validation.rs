//! Does the GPU cull + radix sort actually cull and sort correctly?
//!
//! `SpriteCullPass` has real correctness risk that "it compiles and doesn't
//! crash" can't rule out: a race in the scatter kernel's workgroup-shared
//! atomic counters, an off-by-one in the single-thread scan, a wrong bucket
//! computation — all of these produce a pipeline that runs fine and renders
//! *something*, just not the right thing. This runs the real pass against a
//! large batch of random sprites and checks the GPU's answer against a CPU
//! reference computed the same way `helio-pass-sprite-batch`'s (trusted,
//! independently unit-tested) CPU cull/sort used to work before this pass
//! replaced it.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use helio_pass_sprite_cull::SpriteCullPass;
use wgpu::util::DeviceExt;

/// Mirrors `helio_pass_sprite_batch::SpriteInstance`'s `#[repr(C)]` layout
/// exactly (see that crate's doc comment on why the padding fields exist —
/// WGSL storage-buffer alignment rules Rust doesn't impose on its own).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TestSpriteInstance {
    position: [f32; 2],
    size: [f32; 2],
    rotation: f32,
    depth: f32,
    _pad_uv: [f32; 2],
    uv_rect: [f32; 4],
    color: [f32; 4],
    atlas_layer: u32,
    _pad_tail: [u32; 3],
}

struct Rng(u64);
impl Rng {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn next_f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

struct GpuResult {
    visible_count: u32,
    /// `draw_order` indices, in the order the GPU sorted them (length ==
    /// `visible_count`, taken from the front of the readback buffer).
    order: Vec<u32>,
}

async fn run_gpu(
    slots: &[TestSpriteInstance],
    alive: &[u32],
    view_min: [f32; 2],
    view_max: [f32; 2],
    max_visible: u32,
) -> Option<GpuResult> {
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let mut adapter = None;
    for force_fallback_adapter in [false, true] {
        if let Ok(a) = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter,
                apply_limit_buckets: false,
            })
            .await
        {
            adapter = Some(a);
            break;
        }
    }
    let adapter = adapter?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("Sprite Cull Test Device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .expect("adapter must create a device");
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        panic!("sprite cull GPU validation error: {error:?}");
    }));

    let instances_buf = Arc::new(
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test Instances"),
            contents: bytemuck::cast_slice(slots),
            usage: wgpu::BufferUsages::STORAGE,
        }),
    );
    let alive_buf = Arc::new(
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test Alive Flags"),
            contents: bytemuck::cast_slice(alive),
            usage: wgpu::BufferUsages::STORAGE,
        }),
    );

    let mut pass = SpriteCullPass::new(
        &device,
        &queue,
        instances_buf,
        alive_buf,
        slots.len() as u32,
        max_visible,
    );
    pass.set_view_rect(
        [
            (view_min[0] + view_max[0]) * 0.5,
            (view_min[1] + view_max[1]) * 0.5,
        ],
        [
            (view_max[0] - view_min[0]) * 0.5,
            (view_max[1] - view_min[1]) * 0.5,
        ],
    );
    pass.run_once_for_testing(&device, &queue);

    // ── Readback ────────────────────────────────────────────────────────────
    let indirect_staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Indirect Readback"),
        size: 20,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let order_staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Draw Order Readback"),
        size: (max_visible as u64) * 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("Readback"),
    });
    encoder.copy_buffer_to_buffer(&pass.indirect_buf, 0, &indirect_staging, 0, 20);
    encoder.copy_buffer_to_buffer(
        &pass.draw_order_buf,
        0,
        &order_staging,
        0,
        (max_visible as u64) * 4,
    );
    queue.submit([encoder.finish()]);

    let (tx1, rx1) = std::sync::mpsc::channel();
    indirect_staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx1.send(r);
        });
    let (tx2, rx2) = std::sync::mpsc::channel();
    order_staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx2.send(r);
        });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx1.recv()
        .expect("map callback")
        .expect("indirect map succeeded");
    rx2.recv()
        .expect("map callback")
        .expect("order map succeeded");

    let indirect_data = indirect_staging
        .slice(..)
        .get_mapped_range()
        .expect("get_mapped_range");
    let indirect_words: &[u32] = bytemuck::cast_slice(&indirect_data);
    let visible_count = indirect_words[1]; // DrawIndexedIndirectArgs.instance_count
    drop(indirect_data);
    indirect_staging.unmap();

    let order_data = order_staging
        .slice(..)
        .get_mapped_range()
        .expect("get_mapped_range");
    let order_words: &[u32] = bytemuck::cast_slice(&order_data);
    let order = order_words[..visible_count.min(max_visible) as usize].to_vec();
    drop(order_data);
    order_staging.unmap();

    Some(GpuResult {
        visible_count,
        order,
    })
}

fn cpu_reference(
    slots: &[TestSpriteInstance],
    alive: &[u32],
    view_min: [f32; 2],
    view_max: [f32; 2],
) -> Vec<u32> {
    let mut visible: Vec<u32> = (0..slots.len() as u32)
        .filter(|&i| {
            if alive[i as usize] == 0 {
                return false;
            }
            let s = &slots[i as usize];
            let clamped = [
                s.position[0].clamp(view_min[0], view_max[0]),
                s.position[1].clamp(view_min[1], view_max[1]),
            ];
            let dx = s.position[0] - clamped[0];
            let dy = s.position[1] - clamped[1];
            let radius = 0.5 * (s.size[0] * s.size[0] + s.size[1] * s.size[1]).sqrt();
            (dx * dx + dy * dy).sqrt() <= radius
        })
        .collect();
    visible.sort_by(|&a, &b| {
        slots[a as usize]
            .depth
            .partial_cmp(&slots[b as usize].depth)
            .unwrap()
    });
    visible
}

#[test]
fn gpu_cull_and_sort_matches_cpu_reference() {
    const N: usize = 20_000;
    let mut rng = Rng(0xC0FF_EE15_5057_ED00);
    let slots: Vec<TestSpriteInstance> = (0..N)
        .map(|_| TestSpriteInstance {
            position: [rng.range(-2000.0, 2000.0), rng.range(-2000.0, 2000.0)],
            size: [rng.range(4.0, 32.0), rng.range(4.0, 32.0)],
            rotation: 0.0,
            depth: rng.range(-1000.0, 1000.0),
            _pad_uv: [0.0; 2],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            atlas_layer: 0,
            _pad_tail: [0; 3],
        })
        .collect();
    // Alternate alive/dead so the cull pass's alive-flag branch is exercised
    // too, not just the geometric test.
    let alive: Vec<u32> = (0..N).map(|i| (i % 3 != 0) as u32).collect();

    let view_min = [-500.0f32, -500.0];
    let view_max = [500.0f32, 500.0];

    let Some(gpu) = pollster::block_on(run_gpu(&slots, &alive, view_min, view_max, N as u32))
    else {
        eprintln!("skipping gpu_cull_and_sort_matches_cpu_reference: no GPU adapter available");
        return;
    };

    let expected = cpu_reference(&slots, &alive, view_min, view_max);

    assert_eq!(
        gpu.visible_count as usize,
        expected.len(),
        "GPU visible count didn't match CPU reference — culling (or the alive-flag check) disagrees"
    );
    assert_eq!(gpu.order.len(), expected.len());

    // Same *set* of indices (order among ties isn't guaranteed — see the
    // module doc comment on `shaders/sprite_sort.wgsl` for why).
    let mut gpu_sorted = gpu.order.clone();
    gpu_sorted.sort_unstable();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort_unstable();
    assert_eq!(
        gpu_sorted, expected_sorted,
        "GPU output is not a permutation of the CPU-culled visible set"
    );

    // Strictly correct ascending depth order (ties may differ from the CPU
    // run, but must still be adjacent/equal, never inverted).
    for w in gpu.order.windows(2) {
        let (a, b) = (slots[w[0] as usize].depth, slots[w[1] as usize].depth);
        assert!(
            a <= b,
            "GPU draw order is not sorted ascending by depth: {a} appears before {b}"
        );
    }
}

#[test]
fn gpu_cull_excludes_everything_when_view_rect_is_empty_of_sprites() {
    let mut rng = Rng(0xDEAD_BEEF_1234_5678);
    let slots: Vec<TestSpriteInstance> = (0..1000)
        .map(|_| TestSpriteInstance {
            position: [rng.range(100.0, 200.0), rng.range(100.0, 200.0)],
            size: [8.0, 8.0],
            rotation: 0.0,
            depth: rng.range(-10.0, 10.0),
            _pad_uv: [0.0; 2],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            atlas_layer: 0,
            _pad_tail: [0; 3],
        })
        .collect();
    let alive = vec![1u32; slots.len()];

    // View rect nowhere near any sprite.
    let Some(gpu) = pollster::block_on(run_gpu(
        &slots,
        &alive,
        [-10_000.0, -10_000.0],
        [-9_000.0, -9_000.0],
        1000,
    )) else {
        eprintln!("skipping gpu_cull_excludes_everything_when_view_rect_is_empty_of_sprites: no GPU adapter available");
        return;
    };
    assert_eq!(gpu.visible_count, 0);
    assert!(gpu.order.is_empty());
}
