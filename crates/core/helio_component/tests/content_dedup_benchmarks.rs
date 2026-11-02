//! BM3/BM4 (Pulsar-Native#632/#661): bulk-hydrate wall time and memoized
//! content-id lookup, on the real `StaticMeshComponent`/`MeshAssetPath`
//! chain. Matches `mesh_cache.rs`'s/SceneDB's `content_dedup_benchmarks.rs`
//! `Instant`-timed idiom.
//!
//! **Known, disclosed gap vs. the mission's ideal performance profile**:
//! this proves the GPU/VRAM side collapses to ONE resident allocation for
//! N entities sharing an asset (the actually scarce, dedup-critical
//! resource) — it does NOT prove "exactly 1 disk read / 1 decode" for the
//! CPU side, because `hydrate_static_mesh_component` calls
//! `load_mesh_upload` once per ENTITY, independently, with no cross-entity
//! CPU-side cache of the decoded `MeshUpload` itself (only
//! `mesh_cache::content_id_for_path`'s path->id memoization is shared).
//! BM4 below measures that memoization's own speedup instead, which is the
//! piece this codebase actually caches.

#![cfg(feature = "gpu")]

use helio::PackedVertex;
use helio_component::components::{MeshAssetPath, MeshGeometryKey, StaticMeshComponent};
use pulsar_scenedb::World;
use pulsar_scenedb::gpu::{
    EngineGpuContext, GpuMirrorHandle, RegionClassConfig, SceneGpuConfig, SceneGpuStore,
};
use std::sync::{Arc, Once};
use std::time::Instant;

fn test_context() -> EngineGpuContext {
    // Creating devices from many tests at once stalls some drivers; take turns.
    static DEVICE_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _device_creation = DEVICE_CREATION.lock().unwrap_or_else(|p| p.into_inner());
    let instance = wgpu::Instance::new(helio_core::wgpu_instance_descriptor());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no adapter — GPU tests need a local GPU");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("content-dedup-benchmarks"),
        ..Default::default()
    }))
    .expect("device");
    EngineGpuContext::new(Arc::new(device), Arc::new(queue))
}

static INIT: Once = Once::new();
fn project_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join("pulsar_content_dedup_bench_root");
    INIT.call_once(|| {
        std::fs::create_dir_all(&root).expect("create bench project root");
        engine_state::EngineContext::new().set_global();
        engine_state::set_project_path(root.to_string_lossy().into_owned());
    });
    root
}

#[test]
fn bm3_bulk_hydrate_10k_entities_one_shared_asset() {
    let root = project_root();
    let mesh = helio::MeshUpload {
        vertices: vec![
            PackedVertex {
                position: [1.0, 2.0, 3.0],
                ..Default::default()
            };
            24
        ],
        indices: (0..36u32).collect(),
    };
    let id = helio_component::mesh_cache::content_id_for_bytes(&mesh);
    std::fs::write(
        root.join("bm3_asset.mesh"),
        helio_component::mesh_cache::encode(&mesh, id),
    )
    .unwrap();

    let ctx = test_context();
    let mut store = SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![RegionClassConfig {
                capacity: 64,
                max_resident_cells: 1,
            }],
            tombstone_headroom: 8,
            max_cells_metadata: 16,
        },
    );
    StaticMeshComponent::register_gpu_columns_growable(&mut store, 16_384, ctx.device());
    let store = Arc::new(store);
    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));

    const N: usize = 10_000;
    let start = Instant::now();
    for _ in 0..N {
        let e = world.spawn();
        let upload = helio_component::subsystems::load_mesh_upload(&root.join("bm3_asset.mesh"))
            .expect("load bench mesh");
        world.insert(
            e,
            StaticMeshComponent {
                mesh_asset: MeshAssetPath::new("bm3_asset.mesh"),
                geometry_key: MeshGeometryKey::for_path(&MeshAssetPath::new("bm3_asset.mesh")),
                vertices: upload.vertices,
                indices: upload.indices,
                bounds_local: [0.0, 0.0, 0.0, 0.5],
                ..Default::default()
            },
        );
    }
    let elapsed = start.elapsed();
    eprintln!(
        "BM3: hydrated+inserted {N} entities sharing 1 asset in {elapsed:?} ({:.1} us/entity)",
        elapsed.as_micros() as f64 / N as f64
    );

    let vpool = store
        .interned_var_len_pool::<PackedVertex>(pulsar_scenedb::gpu::BufferKey::of(
            "builtin_mesh_vertex",
        ))
        .unwrap();
    let audit = vpool.audit();
    assert_eq!(
        audit.len(),
        1,
        "10k entities sharing one asset must collapse to exactly 1 interned GPU allocation"
    );
    assert_eq!(
        audit[0].1, N as u64,
        "refcount must equal every entity that referenced it"
    );
}

/// BM4: memoized content-id lookup vs. a cold hash. The FIRST
/// `content_id_for_path` call on a given file canonicalizes + hashes it;
/// every subsequent call for the SAME (unchanged) file is a canonicalize +
/// mtime/len-validated cache hit — no re-hash.
#[test]
fn bm4_memoized_content_id_lookup_vs_cold_hash() {
    let root = project_root();
    let mesh = helio::MeshUpload {
        vertices: vec![PackedVertex::default(); 500], // bigger payload to make the hash cost visible
        indices: (0..500u32).collect(),
    };
    // v1 (no header id) forces content_id_for_path's cold path to hash the
    // whole file on the FIRST call — v2 would short-circuit to a header
    // read regardless of memoization, which wouldn't isolate the effect.
    let mut v1 = Vec::new();
    v1.extend_from_slice(b"PMSH");
    v1.extend_from_slice(&1u32.to_le_bytes());
    v1.extend_from_slice(&(mesh.vertices.len() as u64).to_le_bytes());
    v1.extend_from_slice(&(mesh.indices.len() as u64).to_le_bytes());
    v1.extend_from_slice(bytemuck::cast_slice(&mesh.vertices));
    v1.extend_from_slice(bytemuck::cast_slice(&mesh.indices));
    let path = root.join("bm4_asset.mesh");
    std::fs::write(&path, &v1).unwrap();

    let cold_start = Instant::now();
    let id_cold = helio_component::mesh_cache::content_id_for_path(&path).unwrap();
    let cold_elapsed = cold_start.elapsed();

    const WARM_CALLS: u32 = 10_000;
    let warm_start = Instant::now();
    for _ in 0..WARM_CALLS {
        let id_warm = helio_component::mesh_cache::content_id_for_path(&path).unwrap();
        assert_eq!(id_warm, id_cold);
    }
    let warm_elapsed = warm_start.elapsed();
    let warm_ns_per_call = warm_elapsed.as_nanos() as f64 / WARM_CALLS as f64;

    eprintln!(
        "BM4: cold lookup {cold_elapsed:?}; {WARM_CALLS} warm (memoized) lookups {warm_elapsed:?} ({warm_ns_per_call:.0} ns/call)"
    );
    assert!(
        warm_ns_per_call * 10.0 < cold_elapsed.as_nanos() as f64,
        "a warm memoized lookup should be at least ~10x cheaper than the cold hash (cold {cold_elapsed:?}, warm {warm_ns_per_call:.0}ns)"
    );
}
