// Water hitbox rows' placement: a body bounded by the meshes its owner holds.

// A body's world bounds: the AABB of the bounding spheres of every mesh its
// owner holds (attached and enabled), placed by the owner's transform. `w` of
// `lo` is 1 when the owner holds any.
struct BodyBounds {
    lo: vec4<f32>,
    hi: vec3<f32>,
}

fn body_bounds(row: u32) -> BodyBounds {
    let body = owners[row];
    let t = transforms[body.owner_index];
    let r = object_rotation(t);
    let scale = object_scale(t);
    let position = placed_position(t);
    let largest_scale = max(abs(scale.x), max(abs(scale.y), abs(scale.z)));
    var lo = vec3<f32>(3.0e38);
    var hi = vec3<f32>(-3.0e38);
    var found = 0.0;
    let instances = min(arrayLength(&mesh_bounds), arrayLength(&owners));
    for (var i = 0u; i < instances; i++) {
        let sphere = mesh_bounds[i];
        let mesh = owners[i];
        if sphere.w <= 0.0 || mesh.enabled == 0u || mesh.owner_index != body.owner_index
            || mesh.owner_generation != body.owner_generation {
            continue;
        }
        let center = position + r * (sphere.xyz * scale);
        let radius = sphere.w * largest_scale;
        lo = min(lo, center - vec3<f32>(radius));
        hi = max(hi, center + vec3<f32>(radius));
        found = 1.0;
    }
    return BodyBounds(vec4<f32>(lo, found), hi);
}

// A body whose owner holds no meshes has no bounds: not placed.
fn placement_placed(row: u32) -> bool {
    return body_bounds(row).lo.w != 0.0;
}

// A water hitbox (`helio_pass_water_sim::GpuWaterHitbox`, 20 words): the body's
// new bounds are its `body_bounds`; its old bounds an empty box at their
// centre (the simulation replaces them with where the body was last applied
// while the row holds the same body); `params` the source's edge softness and
// strength and the body's identity (source row + 1).
fn placement_write_row(row: u32, slot: u32, output: u32) -> bool {
    let bounds = body_bounds(row);
    let lo = bounds.lo.xyz;
    let hi = bounds.hi;
    let center = (lo + hi) * 0.5;
    let source = source_base(row);
    for (var i = 0u; i < 3u; i++) {
        rows_out[output + i] = bitcast<u32>(center[i]);
        rows_out[output + 4u + i] = bitcast<u32>(center[i]);
        rows_out[output + 8u + i] = bitcast<u32>(lo[i]);
        rows_out[output + 12u + i] = bitcast<u32>(hi[i]);
    }
    rows_out[output + 16u] = sources[source];
    rows_out[output + 17u] = sources[source + 1u];
    rows_out[output + 18u] = bitcast<u32>(f32(row + 1u));
    return true;
}
