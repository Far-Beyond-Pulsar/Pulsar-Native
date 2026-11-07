//! Does the GPU object-batch pipeline actually sort/group/range correctly?
//!
//! `ObjectBatchPass` has real correctness risk that "it compiles and
//! doesn't crash" can't rule out: a race in an atomic-counter compaction, a
//! boundary-detection bug that merges two different draw groups (see
//! `object_batch.wgsl`'s `same_draw_group` doc for the specific hash-
//! collision hazard this guards against), an off-by-one in the two-level
//! scan. This runs the real pipeline against a batch of random synthetic
//! `StaticObjectComponent` rows and checks the GPU's answer against a CPU
//! reference computed the same way `helio::Scene::rebuild_instance_buffers`
//! groups objects (by `(mesh_slot, material_slot)` into draw calls, by
//! `(material_class, graph_hash)` into ranges, split by material shading
//! flags) -- same discipline the sprites plugin's sprite cull's own
//! `gpu_sort_validation.rs` already established for a comparable pipeline.

use std::collections::{BTreeMap, HashMap};

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

const IDENTITY_MAT4: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

const FLAG_TRANSPARENT_ONLY: u32 = 1 << 8;
const FLAG_FORWARD_SHADING: u32 = 1 << 9;
const INSTANCE_FLAG_MOVABLE: u32 = 1 << 3;

struct Rng(u64);
impl Rng {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.next_u32() % (hi - lo)
    }
}

/// `class_and_hash_by_material[material_slot]` -- in the real system,
/// `material_class`/`graph_hash` are resolved ONCE from the material itself
/// (`Renderer::material_batch_key`, called by `StaticObjectComponent::
/// new()`), so every row sharing a `material_slot` MUST agree on both.
/// Randomizing them independently per row (as an earlier version of this
/// test did) violates that invariant and produces rows the real system can
/// never actually construct -- the sort key legitimately incorporates
/// `graph_hash` precisely BECAUSE it's a stable per-material property.
fn make_row(
    rng: &mut Rng,
    mesh_count: u32,
    material_count: u32,
    class_and_hash_by_material: &[(u32, u64)],
) -> StaticObjectComponent {
    let mesh_slot = rng.range(0, mesh_count);
    let material_slot = rng.range(0, material_count);
    let (material_class, graph_hash) = class_and_hash_by_material[material_slot as usize];
    StaticObjectComponent {
        mesh_slot,
        mesh_generation: 1, // live
        material_slot,
        material_generation: 1,
        transform: IDENTITY_MAT4,
        prev_transform: IDENTITY_MAT4,
        normal_mat: [[0.0; 4]; 3],
        bounds: [0.0, 0.0, 0.0, 1.0],
        // Mesh's static draw params are a function of mesh_slot alone in
        // this synthetic test (every row with the same mesh_slot must agree
        // on these, exactly like real mesh-asset data would).
        index_count: 3 * (mesh_slot + 1),
        first_index: mesh_slot * 100,
        vertex_offset: (mesh_slot * 50) as i32,
        material_class,
        graph_hash_lo: graph_hash as u32,
        graph_hash_hi: (graph_hash >> 32) as u32,
        flags: if rng.next_u32() % 2 == 0 {
            INSTANCE_FLAG_MOVABLE
        } else {
            0
        },
        skin_vertex_offset: 0,
        bone_palette_offset: 0,
        bone_palette_count: 0,
        _skin_pad: 0,
    }
}

fn dead_row() -> StaticObjectComponent {
    StaticObjectComponent {
        mesh_slot: 0,
        mesh_generation: 0, // dead -- must be skipped entirely
        material_slot: 0,
        material_generation: 0,
        transform: [[0.0; 4]; 4],
        prev_transform: [[0.0; 4]; 4],
        normal_mat: [[0.0; 4]; 3],
        bounds: [0.0; 4],
        index_count: 0,
        first_index: 0,
        vertex_offset: 0,
        material_class: 0,
        graph_hash_lo: 0,
        graph_hash_hi: 0,
        flags: 0,
        skin_vertex_offset: 0,
        bone_palette_offset: 0,
        bone_palette_count: 0,
        _skin_pad: 0,
    }
}

/// Mirrors `helio::Scene::rebuild_instance_buffers`'s grouping/ranging
/// algorithm exactly (see that function's own doc), operating on the same
/// synthetic rows the GPU pipeline consumes.
struct CpuReference {
    /// `(mesh_slot, material_slot)` -> instance count in that draw group.
    groups: BTreeMap<(u32, u32), u32>,
    /// `(class, graph_hash)` -> set of `(mesh_slot, material_slot)` group
    /// keys that must end up in ONE contiguous range together (same class
    /// and graph_hash never split across two ranges by this pipeline).
    class_hash_of_group: HashMap<(u32, u32), (u32, u64)>,
    shading_of_group: HashMap<(u32, u32), u32>, // 0=opaque,1=transparent,2=forward
    live_count: u32,
    movable_count: u32,
    static_count: u32,
    /// Static transparent-only rows: the coloured transmittance casters.
    transmissive_count: u32,
}

fn cpu_reference(rows: &[StaticObjectComponent], materials: &[TestMaterial]) -> CpuReference {
    let mut groups: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut class_hash_of_group = HashMap::new();
    let mut shading_of_group = HashMap::new();
    let mut live_count = 0u32;
    let mut movable_count = 0u32;
    let mut static_count = 0u32;
    let mut transmissive_count = 0u32;

    for row in rows {
        if row.mesh_generation == 0 {
            continue;
        }
        live_count += 1;
        let transparent =
            (materials[row.material_slot as usize].flags & FLAG_TRANSPARENT_ONLY) != 0;
        let movable = (row.flags & INSTANCE_FLAG_MOVABLE) != 0;
        match (transparent, movable) {
            (true, false) => transmissive_count += 1,
            (true, true) => {} // moving glass casts no shadow
            (false, true) => movable_count += 1,
            (false, false) => static_count += 1,
        }
        let key = (row.mesh_slot, row.material_slot);
        *groups.entry(key).or_insert(0) += 1;
        let hash = ((row.graph_hash_hi as u64) << 32) | row.graph_hash_lo as u64;
        let raster_flags = materials[row.material_slot as usize].flags & 5;
        class_hash_of_group.insert(key, (row.material_class | (raster_flags << 16), hash));

        let mat_flags = materials[row.material_slot as usize].flags;
        let shading = if (mat_flags & FLAG_FORWARD_SHADING) != 0 {
            2
        } else if (mat_flags & FLAG_TRANSPARENT_ONLY) != 0 {
            1
        } else {
            0
        };
        shading_of_group.insert(key, shading);
    }

    CpuReference {
        groups,
        class_hash_of_group,
        shading_of_group,
        live_count,
        movable_count,
        static_count,
        transmissive_count,
    }
}

struct GpuResult {
    instances: Vec<(u32, u32)>, // (mesh_id, material_id) per sorted instance
    draw_calls: Vec<(u32, u32, i32, u32, u32)>, // (index_count, first_index, vertex_offset, first_instance, instance_count)
    /// (index_count, instance_count, first_index, base_vertex, first_instance) -- wgpu ABI order.
    indirect: Vec<(u32, u32, u32, i32, u32)>,
    opaque: Vec<(u32, u64, u32, u32)>,
    transparent: Vec<(u32, u64, u32, u32)>,
    forward: Vec<(u32, u64, u32, u32)>,
    shadow_static_count: u32,
    shadow_movable_count: u32,
    shadow_transmissive_count: u32,
    shadow_membership_fingerprint: (u32, u32),
    group_count: u32,
}

async fn run_gpu(rows: &[StaticObjectComponent], materials: &[TestMaterial]) -> Option<GpuResult> {
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
            label: Some("ObjectBatch Test Device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        })
        .await
        .expect("adapter must create a device");
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        panic!("object batch GPU validation error: {error:?}");
    }));

    use wgpu::util::DeviceExt;
    let static_objects_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Test StaticObjects"),
        contents: bytemuck::cast_slice(rows),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let materials_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Test Materials"),
        contents: bytemuck::cast_slice(materials),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let mut pass = ObjectBatchPass::new(&device);
    pass.run_once_for_testing(
        &device,
        &queue,
        &static_objects_buf,
        &materials_buf,
        rows.len() as u32,
    );

    // ── Blocking readback (test-only; production uses the async path) ──
    let read_buf = |src: &wgpu::Buffer, label: &str| -> Vec<u8> {
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: src.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(src, 0, &staging, 0, src.size());
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().expect("map callback").expect("map succeeded");
        let data = staging
            .slice(..)
            .get_mapped_range()
            .expect("mapped")
            .to_vec();
        staging.unmap();
        data
    };

    let counts_bytes = read_buf(pass.group_count_buffer(), "counts readback (group)");
    let group_count = bytemuck::cast_slice::<u8, u32>(&counts_bytes)[0];

    let bucket_bytes = read_buf(
        pass.range_bucket_counts_buffer(),
        "counts readback (buckets)",
    );
    let buckets: &[u32] = bytemuck::cast_slice(&bucket_bytes);
    let (n_opaque, n_transparent, n_forward) = (buckets[0], buckets[1], buckets[2]);

    let shadow_bytes = read_buf(pass.shadow_counts_buffer(), "counts readback (shadow)");
    let shadow_counts: &[u32] = bytemuck::cast_slice(&shadow_bytes);
    let (shadow_static_count, shadow_movable_count, shadow_transmissive_count) =
        (shadow_counts[0], shadow_counts[1], shadow_counts[2]);
    let shadow_membership_fingerprint = (shadow_counts[3], shadow_counts[4]);

    let instances_bytes = read_buf(pass.instances_buffer(), "instances readback");
    // GpuInstanceDataOut: 192-byte transform payload, 4 base words and 4 skin words.
    let mut instances = Vec::new();
    let stride = 224usize;
    let live_count = rows.iter().filter(|r| r.mesh_generation != 0).count();
    for i in 0..live_count {
        let base = i * stride;
        let words: &[u32] = bytemuck::cast_slice(&instances_bytes[base + 192..base + 200]);
        instances.push((words[0], words[1])); // mesh_id, material_id
    }

    let draw_calls_bytes = read_buf(pass.draw_calls_buffer(), "draw calls readback");
    let mut draw_calls = Vec::new();
    for i in 0..group_count as usize {
        let base = i * 20;
        let words: &[u32] = bytemuck::cast_slice(&draw_calls_bytes[base..base + 20]);
        let vertex_offset = words[2] as i32;
        draw_calls.push((words[0], words[1], vertex_offset, words[3], words[4]));
    }

    // `indirect_buffer()` must carry the SAME per-group data as `draw_calls`,
    // just reordered to wgpu's hardware ABI -- see `indirect_out`'s doc.
    let indirect_bytes = read_buf(pass.indirect_buffer(), "indirect readback");
    let mut indirect = Vec::new();
    for i in 0..group_count as usize {
        let base = i * 20;
        let words: &[u32] = bytemuck::cast_slice(&indirect_bytes[base..base + 20]);
        let base_vertex = words[3] as i32;
        // (index_count, instance_count, first_index, base_vertex, first_instance)
        indirect.push((words[0], words[1], words[2], base_vertex, words[4]));
    }

    let read_ranges = |buf: &wgpu::Buffer, n: u32, label: &str| -> Vec<(u32, u64, u32, u32)> {
        let bytes = read_buf(buf, label);
        let words: &[u32] = bytemuck::cast_slice(&bytes);
        (0..n as usize)
            .map(|i| {
                let b = i * 5;
                let class = words[b];
                let hash = ((words[b + 2] as u64) << 32) | words[b + 1] as u64;
                (class, hash, words[b + 3], words[b + 4])
            })
            .collect()
    };
    if std::env::var("OBJ_BATCH_DEBUG").is_ok() {
        let shading_bytes = read_buf(pass.debug_group_shading_buffer(), "shading debug");
        let shading: &[u32] = bytemuck::cast_slice(&shading_bytes);
        let class_bytes = read_buf(pass.debug_group_material_class_buffer(), "class debug");
        let class: &[u32] = bytemuck::cast_slice(&class_bytes);
        println!("group_count={group_count}");
        for g in 0..group_count {
            let (_, _, _, fi, _) = draw_calls[g as usize];
            let (mesh_id, material_id) = instances[fi as usize];
            println!(
                "group {g}: mesh={mesh_id} material={material_id} class={} shading={} first_instance={fi}",
                class[g as usize], shading[g as usize]
            );
        }
        let dump_ranges = |label: &str, ranges: &[(u32, u64, u32, u32)]| {
            for &(c, h, s, cnt) in ranges {
                println!("  {label} class={c} hash={h} start={s} count={cnt}");
            }
        };
        let opaque_dbg = read_ranges(pass.opaque_ranges_buffer(), n_opaque, "opaque dbg");
        let transparent_dbg = read_ranges(
            pass.transparent_ranges_buffer(),
            n_transparent,
            "transparent dbg",
        );
        let forward_dbg = read_ranges(pass.forward_ranges_buffer(), n_forward, "forward dbg");
        dump_ranges("opaque", &opaque_dbg);
        dump_ranges("transparent", &transparent_dbg);
        dump_ranges("forward", &forward_dbg);
    }

    let opaque = read_ranges(
        pass.opaque_ranges_buffer(),
        n_opaque,
        "opaque ranges readback",
    );
    let transparent = read_ranges(
        pass.transparent_ranges_buffer(),
        n_transparent,
        "transparent ranges readback",
    );
    let forward = read_ranges(
        pass.forward_ranges_buffer(),
        n_forward,
        "forward ranges readback",
    );

    Some(GpuResult {
        instances,
        draw_calls,
        indirect,
        opaque,
        transparent,
        forward,
        shadow_static_count,
        shadow_movable_count,
        shadow_transmissive_count,
        shadow_membership_fingerprint,
        group_count,
    })
}

#[test]
fn same_count_static_caster_swap_changes_membership_fingerprint() {
    let mut rng = Rng(0x51AD_0B1E);
    let row = make_row(&mut rng, 1, 1, &[(0, 0)]);
    let materials = [TestMaterial::zeroed()];
    let mut rows = vec![row.clone(), row];
    rows[0].flags = 0;
    rows[1].flags = INSTANCE_FLAG_MOVABLE;
    let before = pollster::block_on(run_gpu(&rows, &materials)).expect("GPU required");

    rows[0].flags = INSTANCE_FLAG_MOVABLE;
    rows[1].flags = 0;
    let after = pollster::block_on(run_gpu(&rows, &materials)).expect("GPU required");

    assert_eq!(before.shadow_static_count, after.shadow_static_count);
    assert_ne!(
        before.shadow_membership_fingerprint, after.shadow_membership_fingerprint,
        "same-count caster identity swap must invalidate the static shadow atlas"
    );
}

#[test]
fn large_identical_batch_is_partitioned_without_losing_instances() {
    let mut rng = Rng(123);
    let row = make_row(&mut rng, 1, 1, &[(0, 0)]);
    let materials = [TestMaterial::zeroed()];
    // Two complete chunks plus a partial final chunk; zeros exercise sparse
    // gather before splitting the live sorted stream.
    let mut rows = vec![row; 10_003];
    rows.extend(vec![dead_row(); 13]);
    let gpu = pollster::block_on(run_gpu(&rows, &materials)).expect("GPU required");
    assert_eq!(gpu.group_count, 3);
    assert_eq!(gpu.instances.len(), 10_003);
    let mut next = 0;
    for (draw, indirect) in gpu.draw_calls.iter().zip(&gpu.indirect) {
        let &(index_count, first_index, vertex_offset, first_instance, count) = draw;
        assert_eq!(first_instance, next);
        assert!(count > 0 && count <= 4096);
        assert_eq!(
            *indirect,
            (
                index_count,
                count,
                first_index,
                vertex_offset,
                first_instance
            )
        );
        next += count;
    }
    assert_eq!(next, 10_003);
    assert!(gpu
        .instances
        .iter()
        .all(|id| *id == (row.mesh_slot, row.material_slot)));
    assert_eq!(gpu.shadow_static_count + gpu.shadow_movable_count, 10_003);
    assert_eq!(gpu.opaque, vec![(0, 0, 0, 3)]);
}

#[test]
fn gpu_object_batch_matches_cpu_reference() {
    const N: usize = 4000;
    let mut rng = Rng(0xB16B_00B5_5CA1_AB1E);
    let mesh_count = 12;
    let material_count = 20;
    let class_count = 4;
    let hash_count = 6;

    // Each material_slot gets ONE fixed (class, graph_hash) -- see
    // `make_row`'s doc for why this must hold.
    let class_and_hash_by_material: Vec<(u32, u64)> = (0..material_count)
        .map(|_| (rng.range(0, class_count), rng.range(0, hash_count) as u64))
        .collect();

    let mut rows: Vec<StaticObjectComponent> = (0..N)
        .map(|_| {
            make_row(
                &mut rng,
                mesh_count,
                material_count,
                &class_and_hash_by_material,
            )
        })
        .collect();
    // Interleave dead rows to exercise the liveness gate.
    for i in (0..rows.len()).step_by(7) {
        if i < rows.len() {
            rows[i] = dead_row();
        }
    }

    // Different materials can share a shader class/hash while requiring
    // different passes. Shading must split the range at these transitions.
    let materials: Vec<TestMaterial> = (0..material_count)
        .map(|i| {
            let bucket = i as u64;
            let mut flags = 0u32;
            // Same shader class/hash, different raster PSO: leaves must not
            // be merged back into the single-sided opaque segment.
            if bucket % 3 == 0 {
                flags |= 5;
            }
            if bucket % 5 == 0 {
                flags |= FLAG_TRANSPARENT_ONLY;
            }
            if bucket % 7 == 0 {
                flags |= FLAG_FORWARD_SHADING;
            }
            TestMaterial {
                base_color: [1.0; 4],
                emissive: [0.0; 4],
                roughness_metallic: [0.5, 0.0, 1.5, 0.5],
                tex_base_color: u32::MAX,
                tex_normal: u32::MAX,
                tex_roughness: u32::MAX,
                tex_emissive: u32::MAX,
                tex_occlusion: u32::MAX,
                workflow: 0,
                flags,
                material_class: 0,
                class_params: [0.0; 4],
            }
        })
        .collect();

    let expected = cpu_reference(&rows, &materials);

    let Some(gpu) = pollster::block_on(run_gpu(&rows, &materials)) else {
        eprintln!("skipping gpu_object_batch_matches_cpu_reference: no GPU adapter available");
        return;
    };

    // ── 1. Live instance count ──
    assert_eq!(
        gpu.instances.len() as u32,
        expected.live_count,
        "GPU instance count doesn't match live row count"
    );

    // ── 2. Draw-call group count and per-group instance counts ──
    assert_eq!(gpu.group_count as usize, expected.groups.len(), "GPU draw-call group count doesn't match CPU reference's distinct (mesh,material) pair count");
    assert_eq!(gpu.draw_calls.len(), expected.groups.len());

    let mut gpu_groups: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    let mut total_instances_seen = 0u32;
    for &(index_count, first_index, vertex_offset, first_instance, instance_count) in
        &gpu.draw_calls
    {
        assert!(
            instance_count > 0,
            "a draw-call group with zero instances should never exist"
        );
        // Every instance in this group must actually share the SAME
        // (mesh_id, material_id) -- this is the exact bug the
        // `same_draw_group` fix in `object_batch.wgsl` prevents (a hash
        // collision incorrectly merging two different groups).
        let mut group_key: Option<(u32, u32)> = None;
        for slot in first_instance..first_instance + instance_count {
            let (mesh_id, material_id) = gpu.instances[slot as usize];
            match group_key {
                None => group_key = Some((mesh_id, material_id)),
                Some(k) => assert_eq!(
                    k,
                    (mesh_id, material_id),
                    "draw-call group at first_instance={first_instance} mixes instances from different (mesh,material) pairs -- a real merge bug"
                ),
            }
        }
        let key = group_key.expect("non-empty group");
        // The draw call's own mesh-derived fields must match what that
        // mesh_slot's rows were constructed with in this test.
        let mesh_slot = key.0;
        assert_eq!(index_count, 3 * (mesh_slot + 1));
        assert_eq!(first_index, mesh_slot * 100);
        assert_eq!(vertex_offset, (mesh_slot * 50) as i32);

        *gpu_groups.entry(key).or_insert(0) += instance_count;
        total_instances_seen += instance_count;
    }
    assert_eq!(total_instances_seen, expected.live_count, "draw-call groups' instance counts don't sum to the live row count -- some instance was dropped or double-counted");
    assert_eq!(
        gpu_groups, expected.groups,
        "GPU per-group instance counts don't match the CPU reference"
    );

    // ── 2b. `indirect_buffer()` carries the same data as `draw_calls`, just
    // reordered to wgpu's hardware indirect-draw ABI ──
    assert_eq!(gpu.indirect.len(), gpu.draw_calls.len());
    for (
        g,
        (
            &(dc_index, dc_first_idx, dc_vertex_off, dc_first_inst, dc_inst_count),
            &(ind_index, ind_inst_count, ind_first_idx, ind_base_vertex, ind_first_inst),
        ),
    ) in gpu.draw_calls.iter().zip(gpu.indirect.iter()).enumerate()
    {
        assert_eq!(
            (
                ind_index,
                ind_first_idx,
                ind_base_vertex,
                ind_first_inst,
                ind_inst_count
            ),
            (
                dc_index,
                dc_first_idx,
                dc_vertex_off,
                dc_first_inst,
                dc_inst_count
            ),
            "group {g}: indirect_buffer() disagrees with draw_calls_buffer()"
        );
    }

    // ── 3. Range tables: every group appears in exactly one bucket, in a
    // contiguous run sharing (class, graph_hash) ──
    let mut seen_groups_in_ranges: std::collections::HashSet<u32> =
        std::collections::HashSet::new();
    let mut range_shading_bucket: HashMap<u32, u32> = HashMap::new(); // group index -> bucket id
    for (bucket_id, ranges) in [
        (0u32, &gpu.opaque),
        (1, &gpu.transparent),
        (2, &gpu.forward),
    ] {
        for &(class, hash, start, count) in ranges {
            assert!(
                count > 0,
                "a range with zero draw-call groups should never exist"
            );
            for g in start..start + count {
                assert!(seen_groups_in_ranges.insert(g), "draw-call group {g} appears in more than one range -- ranges must partition the group array");
                range_shading_bucket.insert(g, bucket_id);
                let (mesh_slot, material_slot) = {
                    let (i, _, _, fi, _) = gpu.draw_calls[g as usize];
                    let _ = i;
                    let (mesh_id, material_id) = gpu.instances[fi as usize];
                    (mesh_id, material_id)
                };
                let expected_key = (mesh_slot, material_slot);
                let (expected_class, expected_hash) = expected.class_hash_of_group[&expected_key];
                assert_eq!(
                    class, expected_class,
                    "range's material_class doesn't match its groups' real material_class"
                );
                assert_eq!(
                    hash, expected_hash,
                    "range's graph_hash doesn't match its groups' real graph_hash"
                );
                let expected_shading = expected.shading_of_group[&expected_key];
                assert_eq!(bucket_id, expected_shading, "group {expected_key:?} ended up in the wrong opaque/transparent/forward bucket");
            }
        }
    }
    assert_eq!(
        seen_groups_in_ranges.len() as u32,
        gpu.group_count,
        "every draw-call group must appear in exactly one range"
    );

    // ── 4. Shadow partition counts ──
    assert_eq!(
        gpu.shadow_static_count, expected.static_count,
        "shadow static-partition count mismatch"
    );
    assert_eq!(
        gpu.shadow_movable_count, expected.movable_count,
        "shadow movable-partition count mismatch"
    );
    assert_eq!(
        gpu.shadow_transmissive_count, expected.transmissive_count,
        "shadow transmissive-partition count mismatch"
    );
}

#[test]
fn gpu_object_batch_handles_all_dead_rows() {
    let rows: Vec<StaticObjectComponent> = (0..100).map(|_| dead_row()).collect();
    let materials = vec![TestMaterial {
        base_color: [1.0; 4],
        emissive: [0.0; 4],
        roughness_metallic: [0.5, 0.0, 1.5, 0.5],
        tex_base_color: u32::MAX,
        tex_normal: u32::MAX,
        tex_roughness: u32::MAX,
        tex_emissive: u32::MAX,
        tex_occlusion: u32::MAX,
        workflow: 0,
        flags: 0,
        material_class: 0,
        class_params: [0.0; 4],
    }];

    let Some(gpu) = pollster::block_on(run_gpu(&rows, &materials)) else {
        eprintln!("skipping gpu_object_batch_handles_all_dead_rows: no GPU adapter available");
        return;
    };
    assert_eq!(gpu.instances.len(), 0);
    assert_eq!(gpu.group_count, 0);
    assert_eq!(gpu.draw_calls.len(), 0);
    assert!(gpu.opaque.is_empty());
    assert!(gpu.transparent.is_empty());
    assert!(gpu.forward.is_empty());
    assert_eq!(gpu.shadow_static_count, 0);
    assert_eq!(gpu.shadow_movable_count, 0);
}

/// More distinct pipelines than the readback's initial staging holds: the
/// readback must grow instead of publishing a truncated range table.
#[test]
fn readback_grows_past_initial_range_capacity_without_truncating() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .expect("adapter must create a device");
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            panic!("object batch GPU validation error: {error:?}");
        }));

        // One opaque material per pipeline hash: every row is its own range.
        const PIPELINES: u32 = 200;
        let materials: Vec<TestMaterial> = (0..PIPELINES).map(|_| TestMaterial::zeroed()).collect();
        let class_and_hash: Vec<(u32, u64)> = (0..PIPELINES).map(|i| (0, i as u64 + 1)).collect();
        let rows: Vec<StaticObjectComponent> = (0..PIPELINES)
            .map(|i| {
                let mut row = make_row(&mut Rng(i as u64 + 1), 1, 1, &[class_and_hash[i as usize]]);
                row.material_slot = i;
                row
            })
            .collect();

        use wgpu::util::DeviceExt;
        let static_objects = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test StaticObjects"),
            contents: bytemuck::cast_slice(&rows),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let materials_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test Materials"),
            contents: bytemuck::cast_slice(&materials),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let mut pass = ObjectBatchPass::new(&device);
        for _ in 0..12 {
            pass.run_once_for_testing(&device, &queue, &static_objects, &materials_buf, PIPELINES);
            pass.poll_readback_for_testing(&device, &queue);
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            let published = pass.opaque_ranges().len() as u32;
            assert!(
                published == 0 || published == PIPELINES,
                "published a truncated range table: {published} of {PIPELINES}"
            );
        }
        let ranges = pass.opaque_ranges();
        assert_eq!(
            ranges.len() as u32,
            PIPELINES,
            "readback never grew to fit every range"
        );
        let mut hashes: Vec<u64> = ranges.iter().map(|&(_, hash, _, _)| hash).collect();
        hashes.sort_unstable();
        assert_eq!(hashes, (1..=PIPELINES as u64).collect::<Vec<_>>());
        assert_eq!(pass.instance_count(), PIPELINES);
    });
}

/// Motion vectors come from the previous frame's transform, not from the
/// row's authored `prev_transform`: an object that stops moving must report
/// zero motion on the next frame even though its authored `prev_transform`
/// still holds the transform from before its last edit.
#[test]
fn instance_prev_transform_is_last_frames_transform() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
        let Ok(adapter) = instance.request_adapter(&Default::default()).await else {
            eprintln!("skipping: no GPU adapter available");
            return;
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .expect("adapter must create a device");
        device.on_uncaptured_error(std::sync::Arc::new(|error| {
            panic!("object batch GPU validation error: {error:?}");
        }));

        let at = |x: f32| {
            let mut m = IDENTITY_MAT4;
            m[3][0] = x;
            m
        };
        let mut row = make_row(&mut Rng(7), 1, 1, &[(0, 0)]);
        row.transform = at(1.0);
        row.prev_transform = at(1.0);

        use wgpu::util::DeviceExt;
        let static_objects = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test StaticObjects"),
            contents: bytemuck::bytes_of(&row),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let materials = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Test Materials"),
            contents: bytemuck::bytes_of(&TestMaterial::zeroed()),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut pass = ObjectBatchPass::new(&device);
        // Returns (model x, prev_model x) of the single instance.
        let mut frame = |row: &StaticObjectComponent| -> (f32, f32) {
            queue.write_buffer(&static_objects, 0, bytemuck::bytes_of(row));
            pass.run_once_for_testing(&device, &queue, &static_objects, &materials, 1);
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 224,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_buffer_to_buffer(pass.instances_buffer(), 0, &staging, 0, 224);
            queue.submit([encoder.finish()]);
            staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let words: Vec<f32> =
                bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
            // model is floats 0..16 (x translation at 12); prev_model 32..48.
            (words[12], words[32 + 12])
        };

        assert_eq!(frame(&row), (1.0, 1.0), "a new object has no motion");
        let moved = StaticObjectComponent {
            prev_transform: at(1.0),
            transform: at(2.0),
            ..row
        };
        assert_eq!(
            frame(&moved),
            (2.0, 1.0),
            "a moving object reports last frame's transform"
        );
        assert_eq!(
            frame(&moved),
            (2.0, 2.0),
            "a stopped object has no motion even though its authored prev_transform is stale"
        );

        // The row is retired and reused by a different object: no inherited motion.
        frame(&dead_row());
        let replacement = StaticObjectComponent {
            material_generation: 2,
            transform: at(9.0),
            prev_transform: at(9.0),
            ..row
        };
        assert_eq!(
            frame(&replacement),
            (9.0, 9.0),
            "a reused row must not inherit motion"
        );
    });
}
