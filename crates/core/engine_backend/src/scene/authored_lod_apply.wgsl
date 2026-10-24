struct Owner {
    owner_index: u32,
    owner_generation: u32,
    enabled: u32,
}

struct Handle {
    offset: u32,
    count: u32,
}

struct Selection {
    level_index: u32,
    valid: u32,
    mesh_asset: array<u32, 4>,
    camera_distance: f32,
}

struct AssetGeometry {
    mesh_asset: array<u32, 4>,
    vertex_offset: u32,
    index_offset: u32,
    section_offset: u32,
    section_count: u32,
    bounds_local: vec4<f32>,
}

struct Section {
    first_index: u32,
    index_count: u32,
}

struct StaticObjectRow {
    mesh_slot: u32,
    mesh_generation: u32,
    material_slot: u32,
    material_generation: u32,
    transform: array<array<f32, 4>, 4>,
    prev_transform: array<array<f32, 4>, 4>,
    normal_mat: array<array<f32, 4>, 3>,
    bounds: array<f32, 4>,
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    flags: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    section_index: u32,
}

struct Uniforms {
    object_rows: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read_write> objects: array<StaticObjectRow>;
@group(0) @binding(2) var<storage, read> owners: array<Owner>;
@group(0) @binding(3) var<storage, read> section_handles: array<Handle>;
@group(0) @binding(4) var<storage, read> selections: array<Selection>;
@group(0) @binding(5) var<storage, read> assets: array<AssetGeometry>;
@group(0) @binding(6) var<storage, read> sections: array<Section>;

fn less_id(left: array<u32, 4>, right: array<u32, 4>) -> bool {
    var word = 4u;
    loop {
        if word == 0u { break; }
        word -= 1u;
        if left[word] < right[word] { return true; }
        if left[word] > right[word] { return false; }
    }
    return false;
}

fn equal_id(left: array<u32, 4>, right: array<u32, 4>) -> bool {
    return left[0] == right[0] && left[1] == right[1]
        && left[2] == right[2] && left[3] == right[3];
}

fn find_asset(id: array<u32, 4>) -> u32 {
    var low = 0u;
    var high = arrayLength(&assets);
    loop {
        if low >= high { break; }
        let mid = low + (high - low) / 2u;
        if less_id(assets[mid].mesh_asset, id) {
            low = mid + 1u;
        } else {
            high = mid;
        }
    }
    if low < arrayLength(&assets) && equal_id(assets[low].mesh_asset, id) {
        return low;
    }
    return 0xffffffffu;
}

@compute @workgroup_size(64)
fn cs_apply_authored_lod(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row_index = gid.x;
    if row_index >= u.object_rows || row_index >= arrayLength(&objects) {
        return;
    }
    var object = objects[row_index];
    if object.mesh_generation == 0u || object.mesh_slot >= arrayLength(&owners) {
        return;
    }
    let owner = owners[object.mesh_slot];
    if owner.enabled == 0u || owner.owner_index >= arrayLength(&selections)
        || object.mesh_slot >= arrayLength(&section_handles) {
        return;
    }
    let selection = selections[owner.owner_index];
    if selection.valid == 0u {
        return;
    }
    let asset_index = find_asset(selection.mesh_asset);
    if asset_index == 0xffffffffu {
        return;
    }
    let asset = assets[asset_index];
    let base_sections = section_handles[object.mesh_slot].count;
    if asset.section_count != base_sections || object.section_index >= asset.section_count {
        return;
    }
    let section_index = asset.section_offset + object.section_index;
    if section_index >= arrayLength(&sections) {
        return;
    }
    let section = sections[section_index];
    object.index_count = section.index_count;
    object.first_index = asset.index_offset + section.first_index;
    object.vertex_offset = i32(asset.vertex_offset);
    let transform = object.transform;
    let axis_x = vec3<f32>(transform[0][0], transform[0][1], transform[0][2]);
    let axis_y = vec3<f32>(transform[1][0], transform[1][1], transform[1][2]);
    let axis_z = vec3<f32>(transform[2][0], transform[2][1], transform[2][2]);
    let translation = vec3<f32>(transform[3][0], transform[3][1], transform[3][2]);
    let center = axis_x * asset.bounds_local.x
        + axis_y * asset.bounds_local.y
        + axis_z * asset.bounds_local.z
        + translation;
    let scale = max(length(axis_x), max(length(axis_y), length(axis_z)));
    object.bounds = array<f32, 4>(center.x, center.y, center.z, max(asset.bounds_local.w * scale, 0.0));
    objects[row_index] = object;
}
