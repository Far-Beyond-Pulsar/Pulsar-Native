//! The api's WGSL snippets declare the rows as the Rust does: each struct's
//! size and each constant's value, so a reader including the snippet reads
//! the buffers the meshes write.

use helio_mesh_api::*;

/// The storage `mesh_ray_surface` reads, as a reader declares it.
const RAY_SURFACE_READER: &str = "//!use mesh_ray_surface
@group(0) @binding(0) var<storage, read> instances: array<RayInstance>;
@group(0) @binding(1) var<storage, read> vertices: array<u32>;
@group(0) @binding(2) var<storage, read> indices: array<u32>;
@group(0) @binding(3) var<storage, read> proxy_vertices: array<f32>;
@group(0) @binding(4) var<storage, read> proxy_indices: array<u32>;
@compute @workgroup_size(1)
fn main() {
    let t = ray_triangle(instances[0], 0u);
    _ = ray_triangle_outside(t, vec3<f32>(1.0, 0.0, 0.0));
}
";

const DRAW_ROWS_READER: &str = "//!use mesh_draw_rows
@group(0) @binding(0) var<storage, read> instances: array<GpuInstanceData>;
@group(0) @binding(1) var<storage, read> draws: array<GpuDrawCall>;
@group(0) @binding(2) var<storage, read_write> indirect: array<DrawIndexedIndirect>;
@compute @workgroup_size(1)
fn main() {
    _ = instance_coordinate_space(instances[0].flags);
    indirect[0] = DrawIndexedIndirect(draws[0].index_count, 0u, 0u, 0, 0u);
}
";

fn module(reader: &str, snippet: helio_core::shader::ShaderSnippet) -> naga::Module {
    let source = helio_core::shader::resolve_with(reader, &[snippet]);
    let module = naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("{error:?}"));
    module
}

fn struct_size(module: &naga::Module, name: &str) -> usize {
    module
        .types
        .iter()
        .find_map(|(_, ty)| match ty.inner {
            naga::TypeInner::Struct { span, .. } if ty.name.as_deref() == Some(name) => {
                Some(span as usize)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no struct {name}"))
}

fn constant(module: &naga::Module, name: &str) -> u32 {
    let (_, c) = module
        .constants
        .iter()
        .find(|(_, c)| c.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no constant {name}"));
    match module.global_expressions[c.init] {
        naga::Expression::Literal(naga::Literal::U32(v)) => v,
        ref other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn the_ray_surface_rows_match_the_rust() {
    let m = module(RAY_SURFACE_READER, RAY_SURFACE_SNIPPET);
    assert_eq!(
        struct_size(&m, "RayInstance"),
        std::mem::size_of::<RayInstance>()
    );
    assert_eq!(constant(&m, "RAY_INSTANCE_PROXY"), RAY_INSTANCE_PROXY);
    assert_eq!(constant(&m, "RAY_INSTANCE_MASKED"), RAY_INSTANCE_MASKED);
    assert_eq!(
        constant(&m, "RAY_INSTANCE_TWO_SIDED"),
        RAY_INSTANCE_TWO_SIDED
    );
    assert_eq!(
        constant(&m, "MESH_VERTEX_WORDS") as usize * 4,
        std::mem::size_of::<PackedVertex>()
    );
    assert_eq!(
        u64::from(constant(&m, "RAY_PROXY_FLOATS")) * 4,
        RAY_PROXY_VERTEX_BYTES
    );
}

#[test]
fn the_draw_rows_match_the_rust() {
    let m = module(DRAW_ROWS_READER, DRAW_ROWS_SNIPPET);
    assert_eq!(
        struct_size(&m, "GpuInstanceData"),
        std::mem::size_of::<GpuInstanceData>()
    );
    assert_eq!(
        struct_size(&m, "GpuDrawCall"),
        std::mem::size_of::<GpuDrawCall>()
    );
    assert_eq!(
        struct_size(&m, "DrawIndexedIndirect"),
        std::mem::size_of::<DrawIndexedIndirectArgs>()
    );
    for (name, value) in [
        ("INSTANCE_FLAG_CASTS_SHADOW", INSTANCE_FLAG_CASTS_SHADOW),
        (
            "INSTANCE_FLAG_RECEIVES_SHADOW",
            INSTANCE_FLAG_RECEIVES_SHADOW,
        ),
        ("INSTANCE_FLAG_ALWAYS_VISIBLE", INSTANCE_FLAG_ALWAYS_VISIBLE),
        ("INSTANCE_FLAG_MOVABLE", INSTANCE_FLAG_MOVABLE),
        (
            "INSTANCE_FLAG_CLASSIC_SUPPRESSED",
            INSTANCE_FLAG_CLASSIC_SUPPRESSED,
        ),
        (
            "INSTANCE_COORDINATE_SPACE_SHIFT",
            INSTANCE_COORDINATE_SPACE_SHIFT,
        ),
    ] {
        assert_eq!(constant(&m, name), value, "{name}");
    }
}
