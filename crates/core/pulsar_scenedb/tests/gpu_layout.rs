//! Test 3 (C5): host struct offsets vs naga reflection of the WGSL structs,
//! byte-exact. M2a scope: instance (64 B mat4) + generation (u32/slot). M2b
//! extends coverage to MeshMetadata (72 B), ClusterNode (48 B), and the
//! slot-mirror u32 element, matching `src/gpu/assets.rs`.

/// The WGSL the (future, M3) shaders will declare for M2a's two buffers.
const M2A_WGSL: &str = r#"
struct Instance {
    transform: mat4x4<f32>,
}
@group(0) @binding(0) var<storage, read> instances: array<Instance>;
@group(0) @binding(1) var<storage, read> generations: array<u32>;
"#;

/// The WGSL the (future, M3) shaders will declare for M2b's mesh-metadata,
/// cluster-node, and slot-mirror buffers (`src/gpu/assets.rs`).
// naga's `Layouter` computes address-space-agnostic base layout — these
// structs are scalar-only precisely so uniform/storage divergence cannot
// bite; the `var<storage>` declarations make the intended address space
// explicit.
const M2B_WGSL: &str = r#"
struct MeshMetadata {
    vertex_offset: u32, index_offset: u32, index_count: u32, base_vertex: i32,
    material_index: u32, lod_count: u32,
    lod_d0: f32, lod_d1: f32, lod_d2: f32, lod_d3: f32,
    aabb_cx: f32, aabb_cy: f32, aabb_cz: f32,
    cluster_table_offset: u32,
    aabb_ex: f32, aabb_ey: f32, aabb_ez: f32,
    meshlet_count: u32,
}
struct ClusterNode {
    meshlet_offset: u32, meshlet_count: u32, parent_error: f32, self_error: f32,
    group_id: u32, child_offset: u32, child_count: u32, padding: u32,
    bs_x: f32, bs_y: f32, bs_z: f32, bs_w: f32,
}
@group(0) @binding(2) var<storage, read> mesh_meta: array<MeshMetadata>;
@group(0) @binding(3) var<storage, read> clusters: array<ClusterNode>;
@group(0) @binding(4) var<storage, read> slot_mirror: array<u32>;
"#;

/// The WGSL M3-α's shaders will declare for the instance-info mirror
/// (`src/spatial.rs::InstanceInfo`, C5 amendment — cull's token→mesh link).
const M3A_WGSL: &str = r#"
struct InstanceInfo {
    mesh_index: u32,
    flags: u32,
}
@group(0) @binding(5) var<storage, read> instance_info: array<InstanceInfo>;
"#;

/// The WGSL M3-α's shaders will declare for the meshlet buffer
/// (`src/gpu/assets.rs::MeshletEntry`, C5 amendment / punch-list R12).
const M3A_MESHLET_WGSL: &str = r#"
struct MeshletEntry {
    sphere_x: f32, sphere_y: f32, sphere_z: f32, sphere_radius: f32,
    cone_packed: u32, data_offset: u32, counts_packed: u32, reserved: u32,
}
@group(0) @binding(6) var<storage, read> meshlets: array<MeshletEntry>;
"#;

/// Reflect (size, [(member_name, offset)]) for a named struct in WGSL source.
fn wgsl_struct_layout(src: &str, name: &str) -> (u32, Vec<(String, u32)>) {
    let module = naga::front::wgsl::parse_str(src).expect("valid WGSL");
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("layout");
    let (handle, ty) = module
        .types
        .iter()
        .find(|(_, t)| t.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("struct {name} not found"));
    let naga::TypeInner::Struct { members, .. } = &ty.inner else {
        panic!("{name} is not a struct");
    };
    let size = layouter[handle].size;
    let offsets = members
        .iter()
        .map(|m| (m.name.clone().unwrap_or_default(), m.offset))
        .collect();
    (size, offsets)
}

#[test]
fn test3_instance_struct_is_byte_exact() {
    let (size, members) = wgsl_struct_layout(M2A_WGSL, "Instance");
    // Host element: [f32; 16], 64 bytes, transform at offset 0 (C5).
    assert_eq!(size, 64, "WGSL Instance size == size_of::<[f32; 16]>()");
    assert_eq!(size as usize, std::mem::size_of::<[f32; 16]>());
    assert_eq!(members, vec![("transform".to_string(), 0)]);
}

#[test]
fn test3_generation_element_is_u32() {
    // array<u32> element: 4 bytes, matching HandleRegistry::generations().
    let module = naga::front::wgsl::parse_str(M2A_WGSL).expect("valid WGSL");
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("layout");
    let (handle, _) = module
        .types
        .iter()
        .find(|(_, t)| matches!(t.inner, naga::TypeInner::Scalar(s) if s == naga::Scalar::U32))
        .expect("u32 type present");
    assert_eq!(layouter[handle].size, 4);
    assert_eq!(layouter[handle].size as usize, std::mem::size_of::<u32>());
}

#[test]
fn test3_mesh_metadata_struct_is_byte_exact() {
    let (size, members) = wgsl_struct_layout(M2B_WGSL, "MeshMetadata");
    // Host element: `pulsar_scenedb::gpu::MeshMetadata`, 72 bytes (C5/§6.1).
    assert_eq!(size, 72, "WGSL MeshMetadata size == size_of::<MeshMetadata>()");
    assert_eq!(size as usize, std::mem::size_of::<pulsar_scenedb::gpu::MeshMetadata>());
    assert_eq!(
        members,
        vec![
            ("vertex_offset".to_string(), 0),
            ("index_offset".to_string(), 4),
            ("index_count".to_string(), 8),
            ("base_vertex".to_string(), 12),
            ("material_index".to_string(), 16),
            ("lod_count".to_string(), 20),
            ("lod_d0".to_string(), 24),
            ("lod_d1".to_string(), 28),
            ("lod_d2".to_string(), 32),
            ("lod_d3".to_string(), 36),
            ("aabb_cx".to_string(), 40),
            ("aabb_cy".to_string(), 44),
            ("aabb_cz".to_string(), 48),
            ("cluster_table_offset".to_string(), 52),
            ("aabb_ex".to_string(), 56),
            ("aabb_ey".to_string(), 60),
            ("aabb_ez".to_string(), 64),
            ("meshlet_count".to_string(), 68),
        ]
    );
}

#[test]
fn test3_cluster_node_struct_is_byte_exact() {
    let (size, members) = wgsl_struct_layout(M2B_WGSL, "ClusterNode");
    // Host element: `pulsar_scenedb::gpu::ClusterNode`, 48 bytes (C5).
    assert_eq!(size, 48, "WGSL ClusterNode size == size_of::<ClusterNode>()");
    assert_eq!(size as usize, std::mem::size_of::<pulsar_scenedb::gpu::ClusterNode>());
    assert_eq!(
        members,
        vec![
            ("meshlet_offset".to_string(), 0),
            ("meshlet_count".to_string(), 4),
            ("parent_error".to_string(), 8),
            ("self_error".to_string(), 12),
            ("group_id".to_string(), 16),
            ("child_offset".to_string(), 20),
            ("child_count".to_string(), 24),
            ("padding".to_string(), 28),
            ("bs_x".to_string(), 32),
            ("bs_y".to_string(), 36),
            ("bs_z".to_string(), 40),
            ("bs_w".to_string(), 44),
        ]
    );
}

/// M3-α T4 (C5 amendment): host `InstanceInfo` (`spatial.rs`) vs naga
/// reflection of the WGSL struct shaders will bind it as, byte-exact —
/// mirrors `test3_instance_struct_is_byte_exact`'s shape for the new column.
#[test]
fn test_instance_info_struct_is_byte_exact() {
    let (size, members) = wgsl_struct_layout(M3A_WGSL, "InstanceInfo");
    assert_eq!(size, 8, "WGSL InstanceInfo size == size_of::<InstanceInfo>()");
    assert_eq!(size as usize, std::mem::size_of::<pulsar_scenedb::InstanceInfo>());
    assert_eq!(members, vec![("mesh_index".to_string(), 0), ("flags".to_string(), 4)]);
}

/// M3-α T6 (C5 amendment / R12): host `MeshletEntry` (`gpu/assets.rs`) vs
/// naga reflection of the WGSL struct shaders will bind it as, byte-exact —
/// mirrors `test3_cluster_node_struct_is_byte_exact`'s shape for the new
/// meshlet buffer.
#[test]
fn test_meshlet_entry_struct_is_byte_exact() {
    let (size, members) = wgsl_struct_layout(M3A_MESHLET_WGSL, "MeshletEntry");
    assert_eq!(size, 32, "WGSL MeshletEntry size == size_of::<MeshletEntry>()");
    assert_eq!(size as usize, std::mem::size_of::<pulsar_scenedb::gpu::MeshletEntry>());
    assert_eq!(
        members,
        vec![
            ("sphere_x".to_string(), 0),
            ("sphere_y".to_string(), 4),
            ("sphere_z".to_string(), 8),
            ("sphere_radius".to_string(), 12),
            ("cone_packed".to_string(), 16),
            ("data_offset".to_string(), 20),
            ("counts_packed".to_string(), 24),
            ("reserved".to_string(), 28),
        ]
    );
}

#[test]
fn test3_slot_mirror_element_is_u32() {
    // array<u32> element: 4 bytes, matching the slot-mirror buffer's host
    // element (a plain `u32`), mirroring `test3_generation_element_is_u32`.
    let module = naga::front::wgsl::parse_str(M2B_WGSL).expect("valid WGSL");
    let mut layouter = naga::proc::Layouter::default();
    layouter.update(module.to_ctx()).expect("layout");
    let (handle, _) = module
        .types
        .iter()
        .find(|(_, t)| matches!(t.inner, naga::TypeInner::Scalar(s) if s == naga::Scalar::U32))
        .expect("u32 type present");
    assert_eq!(layouter[handle].size, 4);
    assert_eq!(layouter[handle].size as usize, std::mem::size_of::<u32>());
}
