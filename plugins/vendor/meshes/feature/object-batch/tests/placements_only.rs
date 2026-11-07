//! Moving objects must not cost a re-sort. After a full run, a run that only
//! rewrites each sorted slot's placement (`run_placements_for_testing`) has
//! to leave the batch exactly as a full run over the moved rows would: the
//! same instances, bounds, draw calls and shadow partitions, with each moved
//! object's previous transform rolled forward for motion vectors.

use bytemuck::{Pod, Zeroable};
use helio_mesh_host::StaticObjectComponent;
use helio_pass_object_batch::ObjectBatchPass;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TestMaterial {
    base_color: [f32; 4],
    emissive: [f32; 4],
    roughness_metallic: [f32; 4],
    tex_base_color: u32,
    tex_normal: u32,
    tex_roughness: u32,
    tex_emissive: u32,
    tex_occlusion: u32,
    workflow: u32,
    flags: u32,
    material_class: u32,
    class_params: [f32; 4],
}

const MOVABLE: u32 = 1 << 3;
const STRIDE: usize = 224;

struct Rng(u64);
impl Rng {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn translation(x: f32, y: f32, z: f32) -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [x, y, z, 1.0],
    ]
}

fn rows(rng: &mut Rng, count: usize) -> Vec<StaticObjectComponent> {
    (0..count)
        .map(|i| {
            let mesh_slot = rng.next_u32() % 7;
            let material_slot = rng.next_u32() % 5;
            let (x, y, z) = (rng.unit() * 50.0, rng.unit() * 50.0, rng.unit() * 50.0);
            StaticObjectComponent {
                mesh_slot,
                mesh_generation: 1,
                material_slot,
                material_generation: 1,
                transform: translation(x, y, z),
                prev_transform: translation(x, y, z),
                normal_mat: [[0.0; 4]; 3],
                bounds: [x, y, z, 1.0 + (i % 3) as f32],
                index_count: 3 * (mesh_slot + 1),
                first_index: mesh_slot * 100,
                vertex_offset: (mesh_slot * 50) as i32,
                material_class: material_slot % 3,
                graph_hash_lo: material_slot,
                graph_hash_hi: 0,
                flags: if rng.next_u32() % 3 == 0 { MOVABLE } else { 0 },
                skin_vertex_offset: 0,
                bone_palette_offset: 0,
                bone_palette_count: 0,
                _skin_pad: 0,
            }
        })
        .collect()
}

fn materials() -> Vec<TestMaterial> {
    (0..5)
        .map(|i| TestMaterial {
            base_color: [1.0; 4],
            emissive: [0.0; 4],
            roughness_metallic: [0.5, 0.0, 0.0, 0.0],
            tex_base_color: u32::MAX,
            tex_normal: u32::MAX,
            tex_roughness: u32::MAX,
            tex_emissive: u32::MAX,
            tex_occlusion: u32::MAX,
            workflow: 0,
            flags: 0,
            material_class: i % 3,
            class_params: [0.0; 4],
        })
        .collect()
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, src: &wgpu::Buffer) -> Vec<u8> {
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: src.size(),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(src, 0, &staging, 0, src.size());
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv().unwrap().unwrap();
    let data = staging.slice(..).get_mapped_range().unwrap().to_vec();
    staging.unmap();
    data
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .ok()?;
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .ok()
    })
}

#[test]
fn a_placements_run_matches_a_full_run_over_the_moved_rows() {
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: object batch placements");
        return;
    };
    use wgpu::util::DeviceExt;
    let mut rng = Rng(0xfeed);
    let before = rows(&mut rng, 1500);
    let mut after = before.clone();
    // Move a few hundred objects; nothing else about them changes.
    for row in after.iter_mut() {
        if rng.next_u32() % 4 == 0 {
            let (x, y, z) = (rng.unit() * 50.0, rng.unit() * 50.0, rng.unit() * 50.0);
            row.transform = translation(x, y, z);
            row.bounds[0] = x;
            row.bounds[1] = y;
            row.bounds[2] = z;
        }
    }
    let materials = materials();
    let buffer = |rows: &[StaticObjectComponent]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(rows),
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let materials_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&materials),
        usage: wgpu::BufferUsages::STORAGE,
    });

    // The pass under test: a full run over `before`, then only placements.
    let mut pass = ObjectBatchPass::new(&device);
    pass.run_once_for_testing(
        &device,
        &queue,
        &buffer(&before),
        &materials_buf,
        before.len() as u32,
    );
    let draw_calls_before = read(&device, &queue, pass.draw_calls_buffer());
    let indirect_before = read(&device, &queue, pass.indirect_buffer());
    let first_instances = read(&device, &queue, pass.instances_buffer());
    pass.run_placements_for_testing(&device, &queue, &buffer(&after), &materials_buf);

    // The reference: a full run over `after` on a fresh pass.
    let mut reference = ObjectBatchPass::new(&device);
    reference.run_once_for_testing(
        &device,
        &queue,
        &buffer(&after),
        &materials_buf,
        after.len() as u32,
    );

    let live = after.len();
    let got = read(&device, &queue, pass.instances_buffer());

    // Rows are told apart by where they are (random, so unique), before and
    // after the move. Equal sort keys may tie in either order between two
    // separate full runs, so the slots are compared against the first run's
    // own order, not the reference's.
    let key = |bounds: &[f32; 4]| {
        [
            bounds[0].to_bits(),
            bounds[1].to_bits(),
            bounds[2].to_bits(),
        ]
    };
    let slot_key = |bytes: &[u8], i: usize| {
        let b: &[f32] = bytemuck::cast_slice(&bytes[i * STRIDE + 112..i * STRIDE + 128]);
        key(&[b[0], b[1], b[2], b[3]])
    };
    let before_row: std::collections::HashMap<_, _> = before
        .iter()
        .enumerate()
        .map(|(i, r)| (key(&r.bounds), i))
        .collect();
    let after_row: std::collections::HashMap<_, _> = after
        .iter()
        .enumerate()
        .map(|(i, r)| (key(&r.bounds), i))
        .collect();
    let words = |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let mut moved = 0;
    let mut seen = vec![false; live];
    for i in 0..live {
        let was = before_row[&slot_key(&first_instances, i)];
        let now = after_row[&slot_key(&got, i)];
        assert_eq!(was, now, "slot {i}: the object in it changed");
        assert!(
            !std::mem::replace(&mut seen[now], true),
            "row {now} in two slots"
        );
        let row = &after[now];
        let slot = &got[i * STRIDE..(i + 1) * STRIDE];
        assert_eq!(
            &slot[..64],
            bytemuck::bytes_of(&row.transform),
            "slot {i}: transform"
        );
        assert_eq!(
            &slot[112..128],
            bytemuck::bytes_of(&row.bounds),
            "slot {i}: bounds"
        );
        assert_eq!(words(slot, 192), row.mesh_slot, "slot {i}: mesh");
        assert_eq!(words(slot, 196), row.material_slot, "slot {i}: material");
        assert_eq!(words(slot, 200), row.flags, "slot {i}: flags");
        // Motion vectors: where it was last run, which is where it was.
        assert_eq!(
            &slot[128..192],
            bytemuck::bytes_of(&before[now].transform),
            "slot {i}: previous transform"
        );
        if row.transform != before[now].transform {
            moved += 1;
        }
    }
    assert!(moved > 100, "the test must actually move objects ({moved})");

    // Bounds follow the instances.
    let aabbs = read(&device, &queue, pass.aabbs_buffer());
    for i in 0..live {
        let row = &after[after_row[&slot_key(&got, i)]];
        let r = row.bounds[3];
        let want = [row.bounds[0] - r, row.bounds[1] - r, row.bounds[2] - r];
        let have: &[f32] = bytemuck::cast_slice(&aabbs[i * 32..i * 32 + 12]);
        assert_eq!(have, want, "slot {i}: aabb");
    }

    // The grouping did not move, and is what a full run over the moved rows
    // finds too.
    assert_eq!(
        read(&device, &queue, pass.draw_calls_buffer()),
        draw_calls_before,
        "a placements run must leave the draw calls alone"
    );
    assert_eq!(
        read(&device, &queue, pass.indirect_buffer()),
        indirect_before,
        "a placements run must leave the indirect draws alone"
    );
    assert_eq!(
        read(&device, &queue, reference.draw_calls_buffer()),
        draw_calls_before,
        "moving objects does not change the grouping a full run finds"
    );
}
/// Gathering only the moved slots must leave every output byte-identical to
/// gathering all of them: instances, bounds, previous transforms and the
/// motion-vector state a later frame reads.
#[test]
fn a_moved_rows_run_is_byte_identical_to_a_placements_run_over_every_row() {
    let Some((device, queue)) = device() else {
        eprintln!("GPU_VALIDATION_SKIPPED_NO_ADAPTER: object batch moved rows");
        return;
    };
    use wgpu::util::DeviceExt;
    let mut rng = Rng(0xbeef);
    let before = rows(&mut rng, 1500);
    let mut after = before.clone();
    let mut moved_slots = Vec::new();
    for (i, row) in after.iter_mut().enumerate() {
        if rng.next_u32() % 9 == 0 {
            let (x, y, z) = (rng.unit() * 50.0, rng.unit() * 50.0, rng.unit() * 50.0);
            row.transform = translation(x, y, z);
            row.bounds[0] = x;
            row.bounds[1] = y;
            row.bounds[2] = z;
            moved_slots.push(i as u32);
        }
    }
    assert!(moved_slots.len() > 100);
    let materials = materials();
    let buffer = |rows: &[StaticObjectComponent]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(rows),
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let materials_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&materials),
        usage: wgpu::BufferUsages::STORAGE,
    });
    // `[total, fresh, slots..]`, with a stale slot and a duplicate-free tail
    // of slots that did not move (they gather to the same bytes).
    let mut list = vec![moved_slots.len() as u32 + 1, moved_slots.len() as u32 + 1];
    list.extend_from_slice(&moved_slots);
    list.push(u32::MAX); // a slot past every row: must be ignored
    let list_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&list),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let mut all = ObjectBatchPass::new(&device);
    all.run_once_for_testing(
        &device,
        &queue,
        &buffer(&before),
        &materials_buf,
        before.len() as u32,
    );
    all.run_placements_for_testing(&device, &queue, &buffer(&after), &materials_buf);

    let mut only_moved = ObjectBatchPass::new(&device);
    only_moved.run_once_for_testing(
        &device,
        &queue,
        &buffer(&before),
        &materials_buf,
        before.len() as u32,
    );
    only_moved.run_moved_placements_for_testing(
        &device,
        &queue,
        &buffer(&after),
        &materials_buf,
        &list_buf,
    );

    // Two separate full runs may order equal sort keys differently, so slots
    // are matched by where their object is (unique after the move), not by
    // index.
    let live = after.len();
    let by_position = |instances: &[u8], aabbs: &[u8]| {
        (0..live)
            .map(|i| {
                let slot = &instances[i * STRIDE..(i + 1) * STRIDE];
                (
                    slot[112..124].to_vec(),
                    (slot.to_vec(), aabbs[i * 32..(i + 1) * 32].to_vec()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>()
    };
    let all_rows = by_position(
        &read(&device, &queue, all.instances_buffer()),
        &read(&device, &queue, all.aabbs_buffer()),
    );
    let moved_only = by_position(
        &read(&device, &queue, only_moved.instances_buffer()),
        &read(&device, &queue, only_moved.aabbs_buffer()),
    );
    assert_eq!(all_rows.len(), live, "every object has its own position");
    assert_eq!(moved_only.len(), live);
    for (position, (slot, aabb)) in &all_rows {
        let (other_slot, other_aabb) = &moved_only[position];
        assert_eq!(slot, other_slot, "instance at {position:?} differs");
        assert_eq!(aabb, other_aabb, "aabb at {position:?} differs");
    }
}
