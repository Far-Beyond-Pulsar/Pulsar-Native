//! Test 3 (C5): host struct offsets vs naga reflection of the WGSL structs,
//! byte-exact. M2a scope: instance (64 B mat4) + generation (u32/slot). The
//! material/mesh-metadata rows follow their M3/M2b definitions.

/// The WGSL the (future, M3) shaders will declare for M2a's two buffers.
const M2A_WGSL: &str = r#"
struct Instance {
    transform: mat4x4<f32>,
}
@group(0) @binding(0) var<storage, read> instances: array<Instance>;
@group(0) @binding(1) var<storage, read> generations: array<u32>;
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
