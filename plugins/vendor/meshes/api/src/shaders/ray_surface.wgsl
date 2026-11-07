// The meshes' surfaces as rays see them (helio-mesh-api `RAY_SURFACE_SNIPPET`,
// `//!use mesh_ray_surface`): the instance rows the meshes publish beside the
// frame's TLAS, the mesh pools' and the proxy pool's vertex decode, and a hit
// triangle's fetch. The reader declares the storage these read, under these
// names:
//   instances: array<RayInstance>   (`helio_mesh_api::RAY_INSTANCES`)
//   vertices: array<u32>            (`helio_mesh_api::MESH_VERTICES`)
//   indices: array<u32>             (`helio_mesh_api::MESH_INDICES`)
//   proxy_vertices: array<f32>      (`helio_mesh_api::RAY_PROXY_VERTICES`)
//   proxy_indices: array<u32>       (`helio_mesh_api::RAY_PROXY_INDICES`)

// `helio_mesh_api::RayInstance`, by TLAS instance index.
struct RayInstance {
    first_index: u32,
    base_vertex: u32,
    flags: u32,
    material: u32,
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    transmission: vec4<f32>,
}

// `helio_mesh_api::RAY_INSTANCE_PROXY`: the instance's triangles are in the
// proxy pool, positions and UVs only (a virtual-geometry asset's coarse cut).
const RAY_INSTANCE_PROXY: u32 = 1u;
// `helio_mesh_api::RAY_INSTANCE_MASKED`: a cut-out, its material alpha-tested.
const RAY_INSTANCE_MASKED: u32 = 2u;
// `helio_mesh_api::RAY_INSTANCE_TWO_SIDED`: both faces are outside.
const RAY_INSTANCE_TWO_SIDED: u32 = 4u;
// `helio_mesh_api::PackedVertex` in `u32` words: position (words 0-2), UV0
// (4-5), the snorm8x4 normal (8).
const MESH_VERTEX_WORDS: u32 = 10u;
// A proxy pool vertex in floats (`helio_mesh_api::RAY_PROXY_VERTEX_BYTES`):
// position, then UV0.
const RAY_PROXY_FLOATS: u32 = 5u;

fn mesh_vertex_position(v: u32) -> vec3<f32> {
    let at = v * MESH_VERTEX_WORDS;
    return vec3<f32>(bitcast<f32>(vertices[at]), bitcast<f32>(vertices[at + 1u]), bitcast<f32>(vertices[at + 2u]));
}

fn mesh_vertex_uv(v: u32) -> vec2<f32> {
    let at = v * MESH_VERTEX_WORDS + 4u;
    return vec2<f32>(bitcast<f32>(vertices[at]), bitcast<f32>(vertices[at + 1u]));
}

fn mesh_vertex_normal(v: u32) -> vec3<f32> {
    return unpack4x8snorm(vertices[v * MESH_VERTEX_WORDS + 8u]).xyz;
}

fn ray_proxy_position(v: u32) -> vec3<f32> {
    let at = RAY_PROXY_FLOATS * v;
    return vec3<f32>(proxy_vertices[at], proxy_vertices[at + 1u], proxy_vertices[at + 2u]);
}

fn ray_proxy_uv(v: u32) -> vec2<f32> {
    let at = RAY_PROXY_FLOATS * v + 3u;
    return vec2<f32>(proxy_vertices[at], proxy_vertices[at + 1u]);
}

// A hit's triangle, in object space: its corners and UV0s, its pool
// vertices, and whether it is a proxy's (no vertex normals).
struct RayTriangle {
    corners: array<vec3<f32>, 3>,
    uv: array<vec2<f32>, 3>,
    vertices: vec3<u32>,
    proxy: bool,
}

// Triangle `primitive` of `instance` (a hit's `primitive_index`), from the
// mesh pools or the proxy pool.
fn ray_triangle(instance: RayInstance, primitive: u32) -> RayTriangle {
    let base = instance.first_index + 3u * primitive;
    var t: RayTriangle;
    t.proxy = (instance.flags & RAY_INSTANCE_PROXY) != 0u;
    if t.proxy {
        t.vertices = vec3<u32>(proxy_indices[base], proxy_indices[base + 1u], proxy_indices[base + 2u]) + instance.base_vertex;
        t.corners = array<vec3<f32>, 3>(ray_proxy_position(t.vertices.x), ray_proxy_position(t.vertices.y), ray_proxy_position(t.vertices.z));
        t.uv = array<vec2<f32>, 3>(ray_proxy_uv(t.vertices.x), ray_proxy_uv(t.vertices.y), ray_proxy_uv(t.vertices.z));
    } else {
        t.vertices = vec3<u32>(indices[base], indices[base + 1u], indices[base + 2u]) + instance.base_vertex;
        t.corners = array<vec3<f32>, 3>(mesh_vertex_position(t.vertices.x), mesh_vertex_position(t.vertices.y), mesh_vertex_position(t.vertices.z));
        t.uv = array<vec2<f32>, 3>(mesh_vertex_uv(t.vertices.x), mesh_vertex_uv(t.vertices.y), mesh_vertex_uv(t.vertices.z));
    }
    return t;
}

// The side the mesh calls outside at barycentrics `b` (object space, not
// normalized): its vertex normals blended, or a proxy's winding (its face).
fn ray_triangle_outside(t: RayTriangle, b: vec3<f32>) -> vec3<f32> {
    if t.proxy {
        return cross(t.corners[1] - t.corners[0], t.corners[2] - t.corners[0]);
    }
    return mesh_vertex_normal(t.vertices.x) * b.x + mesh_vertex_normal(t.vertices.y) * b.y + mesh_vertex_normal(t.vertices.z) * b.z;
}
