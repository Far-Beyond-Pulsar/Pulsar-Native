//! A mesh's geometry as imported, cached and uploaded: UVs, normals,
//! sections and the vertex pool's layout survive each step.

fn gpu() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("GPU adapter required");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap()
}

#[test]
fn gpu_mesh_upload_preserves_uv0_uv1_normals_and_stride() {
    use meshes_component::components::StaticMeshComponent;
    use pulsar_scenedb::gpu::*;
    use std::sync::Arc;
    let (device, queue) = gpu();
    let context = EngineGpuContext::new(Arc::new(device), Arc::new(queue));
    let mut store = SceneGpuStore::new(
        &context,
        SceneGpuConfig {
            classes: vec![RegionClassConfig {
                capacity: 8,
                max_resident_cells: 1,
            }],
            tombstone_headroom: 1,
            max_cells_metadata: 8,
        },
    );
    StaticMeshComponent::register_gpu_columns_growable(&mut store, 8, context.device());
    let store = Arc::new(store);
    let mut world = pulsar_scenedb::World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        store.clone(),
        Arc::clone(context.queue()),
    ));
    let vertices: Vec<_> = (0..3)
        .map(|i| {
            let mut v = helio_mesh_api::PackedVertex::from_components(
                [i as f32, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [i as f32 * 0.25, 0.75],
                [1.0, 0.0, 0.0],
                -1.0,
            );
            v.tex_coords1 = [0.125, i as f32 * 0.5];
            v
        })
        .collect();
    let entity = world.spawn();
    world.insert(
        entity,
        StaticMeshComponent {
            vertices: vertices.clone(),
            indices: vec![0, 1, 2],
            ..Default::default()
        },
    );
    world.flush_gpu_mirror(context.queue()).unwrap();
    let pool = store
        .interned_var_len_pool::<helio_mesh_api::PackedVertex>(BufferKey::of("builtin_mesh_vertex"))
        .unwrap()
        .underlying()
        .clone();
    assert_eq!(std::mem::size_of::<helio_mesh_api::PackedVertex>(), 40);
    assert_eq!(
        std::mem::offset_of!(helio_mesh_api::PackedVertex, tex_coords0),
        16
    );
    assert_eq!(
        std::mem::offset_of!(helio_mesh_api::PackedVertex, tex_coords1),
        24
    );
    assert_eq!(
        std::mem::offset_of!(helio_mesh_api::PackedVertex, normal),
        32
    );
    let staging = context.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 120,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = context.device().create_command_encoder(&Default::default());
    pool.with_buffer(&mut |b| encoder.copy_buffer_to_buffer(b, 0, &staging, 0, 120));
    context.queue().submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    context
        .device()
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let data = staging.slice(..).get_mapped_range().unwrap();
    assert_eq!(&*data, bytemuck::cast_slice::<_, u8>(&vertices));
}

#[test]
fn fbx_import_and_native_cache_preserve_uvs_normals_and_sections() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../assets/meshes/primitives/SM_Cube.fbx");
    let asset = meshes_component::mesh_files::load_mesh_asset_upload(&path).expect("bundled FBX");
    assert!(!asset.sections.is_empty());
    assert!(asset
        .geometry
        .vertices
        .iter()
        .any(|v| v.tex_coords0 != asset.geometry.vertices[0].tex_coords0));
    assert!(asset.geometry.vertices.iter().all(|v| v.normal != 0));
    let bytes = meshes_component::mesh_cache::encode_asset(&asset, 123);
    let (decoded, id) = meshes_component::mesh_cache::decode_asset(&bytes).unwrap();
    assert_eq!(id, 123);
    assert_eq!(decoded.sections, asset.sections);
    assert_eq!(decoded.geometry.indices, asset.geometry.indices);
    assert_eq!(
        bytemuck::cast_slice::<_, u8>(&decoded.geometry.vertices),
        bytemuck::cast_slice::<_, u8>(&asset.geometry.vertices)
    );
    for section in &decoded.sections {
        assert!((section.material_slot as usize) < decoded.material_slots.len());
        assert!(
            (section.first_index + section.index_count) as usize <= decoded.geometry.indices.len()
        );
    }
}

#[test]
#[ignore = "set PULSAR_MATERIAL_TEST_MESH to inspect a local FBX without checking it into source control"]
fn inspect_local_mesh_uvs_and_sections() {
    let path = std::env::var("PULSAR_MATERIAL_TEST_MESH").expect("PULSAR_MATERIAL_TEST_MESH");
    let asset = meshes_component::mesh_files::load_mesh_asset_upload(std::path::Path::new(&path))
        .expect("import FBX");
    let vertices = &asset.geometry.vertices;
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for vertex in vertices {
        for c in 0..2 {
            min[c] = min[c].min(vertex.tex_coords0[c]);
            max[c] = max[c].max(vertex.tex_coords0[c]);
        }
    }
    eprintln!(
        "{path}: vertices={}, UV0={min:?}..{max:?}, sections={:?}",
        vertices.len(),
        asset.sections
    );
    let bytes = std::fs::read(&path).unwrap();
    let has_uv_layer = bytes
        .windows(b"LayerElementUV".len())
        .any(|w| w == b"LayerElementUV");
    eprintln!("Source FBX declares a UV layer: {has_uv_layer}");
    if has_uv_layer {
        assert!(
            max[0] - min[0] > 0.01 && max[1] - min[1] > 0.01,
            "source UV layer collapsed during import"
        );
    } else {
        assert_eq!(min, [0.0; 2]);
        assert_eq!(max, [0.0; 2]);
    }
    assert!(asset
        .geometry
        .indices
        .iter()
        .all(|i| (*i as usize) < vertices.len()));
}

