// Virtual geometry streaming: residency lookup and page data helpers (read-only bindings).
//
// Prepend, in order: `helio_vg_format::WGSL`, the bindings from
// `helio_vg_stream::bindings_wgsl(..)`, then this file (then vg_stream_feedback.wgsl in compute).
// The bindings declare:
//   vg_page_pool      array<u32>             page slots, PAGE_BYTES each
//   vg_residency      array<u32>             global page id -> slot | 0xffffffff
//   vg_nodes          array<VgHierarchyNode> all assets' hierarchies, packed
//   vg_group_refs     array<VgGroupRef>      all assets' group refs, packed
//   vg_asset_headers  array<VgAssetHeader>   indexed by asset_slot
//   vg_asset_bases    array<vec4<u32>>       indexed by asset_slot:
//                                            x = node base, y = group-ref base,
//                                            z = global page base, w = ready (1/0)
//
//   vg_asset_quant    array<vec4<f32>>       indexed by asset_slot: xyz = lattice origin, w = step
//                                            (position = origin + f32(q) * step, see vg_decode.wgsl)

// Hierarchy child indices and group-ref indices stored in the data are local
// to the asset; add the asset's bases. GpuGroupRef.page_id is a local page id;
// add the base to get the global page id used by the residency table.

const VG_PAGE_BYTES: u32 = 131072u;
const VG_PAGE_WORDS: u32 = 32768u;
const VG_PAGE_NOT_RESIDENT: u32 = 0xffffffffu;

fn vg_node_base(asset_slot: u32) -> u32 { return vg_asset_bases[asset_slot].x; }
fn vg_group_ref_base(asset_slot: u32) -> u32 { return vg_asset_bases[asset_slot].y; }
fn vg_page_base(asset_slot: u32) -> u32 { return vg_asset_bases[asset_slot].z; }
fn vg_asset_ready(asset_slot: u32) -> bool { return vg_asset_bases[asset_slot].w != 0u; }

/// Global page id of a group ref's page.
fn vg_global_page(asset_slot: u32, group_ref: VgGroupRef) -> u32 {
    return vg_asset_bases[asset_slot].z + group_ref.page_id;
}

/// Pool slot holding the page, or VG_PAGE_NOT_RESIDENT.
fn vg_page_slot(global_page: u32) -> u32 { return vg_residency[global_page]; }
fn vg_page_resident(global_page: u32) -> bool { return vg_residency[global_page] != VG_PAGE_NOT_RESIDENT; }

fn vg_page_u32(slot: u32, byte_offset: u32) -> u32 {
    return vg_page_pool[slot * VG_PAGE_WORDS + (byte_offset >> 2u)];
}

/// Word index into vg_page_pool of a page-relative byte offset.
fn vg_page_word(slot: u32, byte_offset: u32) -> u32 {
    return slot * VG_PAGE_WORDS + (byte_offset >> 2u);
}

/// Cluster `index` of the page in `slot` (PageHeader.clusters_offset is word 4).
fn vg_load_cluster(slot: u32, index: u32) -> VgCluster {
    let w = vg_page_word(slot, vg_page_u32(slot, 16u) + index * 64u);
    var c: VgCluster;
    c.sphere = vec4<f32>(
        bitcast<f32>(vg_page_pool[w]), bitcast<f32>(vg_page_pool[w + 1u]),
        bitcast<f32>(vg_page_pool[w + 2u]), bitcast<f32>(vg_page_pool[w + 3u]));
    c.parent_sphere = vec4<f32>(
        bitcast<f32>(vg_page_pool[w + 4u]), bitcast<f32>(vg_page_pool[w + 5u]),
        bitcast<f32>(vg_page_pool[w + 6u]), bitcast<f32>(vg_page_pool[w + 7u]));
    c.cone = vg_page_pool[w + 8u];
    c.self_error = bitcast<f32>(vg_page_pool[w + 9u]);
    c.parent_error = bitcast<f32>(vg_page_pool[w + 10u]);
    c.counts_material = vg_page_pool[w + 11u];
    c.vertex_offset = vg_page_pool[w + 12u];
    c.index_offset = vg_page_pool[w + 13u];
    c.group_id = vg_page_pool[w + 14u];
    c.flags = vg_page_pool[w + 15u];
    return c;
}

/// Lattice (xyz origin, w step) of an asset's quantized positions.
fn vg_asset_quant_of(asset_slot: u32) -> vec4<f32> { return vg_asset_quant[asset_slot]; }
