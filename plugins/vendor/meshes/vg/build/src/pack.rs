//! `.vgeo` writer: cluster encoding, page packing, hierarchy, container.

use bytemuck::bytes_of;
use helio_vg_format::*;
use rayon::prelude::*;

use crate::dag::{Dag, NONE};
use crate::file::*;
use crate::math::*;
use crate::mesh::BuildError;

/// Builder knobs. Everything that changes the output bytes lives here so
/// "same input + same options => same bytes" is checkable.
/// Most pages an asset may pin as its always-resident root set (2 MiB).
pub const MAX_ROOT_PAGES: u32 = 16;

#[derive(Clone, Debug)]
pub struct BuildOptions {
    /// Content id of the source geometry (xxh3-128, low then high); stored
    /// verbatim in the header and used by callers as the cache key.
    pub content_id: [u64; 2],
    /// How many of the coarsest DAG levels must be resident before the asset
    /// is VG-active (`GpuAssetHeader::root_page_count`). The root level alone
    /// already guarantees a hole-free cut; more levels give a nicer first
    /// image. Clamped to at least 1.
    pub resident_levels: u32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            content_id: [0; 2],
            resident_levels: 2,
        }
    }
}

#[derive(Default)]
struct Enc {
    tris: u32,
    verts: u32,
    pos: Vec<u8>,
    attr: Vec<u8>,
    idx: Vec<u8>,
    cone: u32,
}

impl Enc {
    fn bytes(&self) -> usize {
        std::mem::size_of::<GpuCluster>() + self.pos.len() + self.attr.len() + self.idx.len()
    }
}

fn put_bits(words: &mut [u32], bitpos: usize, value: u32, nbits: u32) {
    if nbits == 0 {
        return;
    }
    let (w, o) = (bitpos / 32, (bitpos % 32) as u32);
    words[w] |= value << o;
    if o + nbits > 32 {
        words[w + 1] |= value >> (32 - o);
    }
}

/// Backface cone (`cutoff = cos(half-angle)`) with the widening the format requires: the axis is quantized
/// first, the half-angle is measured against the *quantized* axis, then padded
/// for the shader's own rounding and the cos cutoff is rounded down (a wider cone
/// only ever culls less).
fn cone_for(dag: &Dag, tris: &[[u32; 3]]) -> u32 {
    let mut normals = Vec::with_capacity(tris.len());
    let mut sum = [0.0f32; 3];
    for t in tris {
        let (a, b, c) = (
            dag.vs.pos[t[0] as usize],
            dag.vs.pos[t[1] as usize],
            dag.vs.pos[t[2] as usize],
        );
        let n = normalize(cross(sub(b, a), sub(c, a)));
        if n != [0.0; 3] {
            sum = add(sum, n);
            normals.push(n);
        }
    }
    let disabled = pack_cone([0.0, 0.0, 1.0], 1.0);
    let axis = normalize(sum);
    if normals.is_empty() || axis == [0.0; 3] {
        return disabled;
    }
    let (qa, _) = unpack_cone(pack_cone(axis, 0.0));
    let qa = normalize(qa);
    if qa == [0.0; 3] {
        return disabled;
    }
    let mindp = normals.iter().map(|&n| dot(n, qa)).fold(1.0f32, f32::min);
    let alpha = mindp.clamp(-1.0, 1.0).acos() + 0.02;
    if mindp <= 0.05 || alpha >= std::f32::consts::FRAC_PI_2 - 0.02 {
        return disabled;
    }
    // cutoff = cos(half-angle), rounded DOWN (smaller cos = wider cone = culls less).
    let cutoff = ((alpha.cos() * 127.0).floor() / 127.0).clamp(0.0, 1.0);
    pack_cone(axis, cutoff)
}

struct Lattice {
    origin: [f32; 3],
    step: f32,
}

impl Lattice {
    fn quantize(&self, p: V3) -> [u32; 3] {
        let mut q = [0u32; 3];
        for i in 0..3 {
            let v = ((p[i] as f64 - self.origin[i] as f64) / self.step as f64).round();
            q[i] = v.clamp(0.0, LATTICE_MAX as f64) as u32;
        }
        q
    }
}

fn encode_cluster(dag: &Dag, ci: usize, attrs: &[u8], stride: usize, lat: &Lattice) -> Enc {
    let c = &dag.clusters[ci];
    let mut verts: Vec<u32> = Vec::new();
    let mut idx = Vec::with_capacity(c.tris.len() * 3 + 3);
    for t in &c.tris {
        for &v in t {
            let li = match verts.iter().position(|&x| x == v) {
                Some(p) => p,
                None => {
                    verts.push(v);
                    verts.len() - 1
                }
            };
            idx.push(li as u8);
        }
    }
    debug_assert!(verts.len() <= CLUSTER_MAX_VERTS as usize);
    while idx.len() % 4 != 0 {
        idx.push(0);
    }

    let q: Vec<[u32; 3]> = verts
        .iter()
        .map(|&v| lat.quantize(dag.vs.pos[v as usize]))
        .collect();
    let mut lo = [u32::MAX; 3];
    let mut hi = [0u32; 3];
    for p in &q {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let bits: Vec<u32> = (0..3)
        .map(|i| 32 - (hi[i] - lo[i]).leading_zeros())
        .collect();
    let per_vert = (bits[0] + bits[1] + bits[2]) as usize;
    let stream_words = (per_vert * q.len() + 31) / 32;
    let mut words = vec![0u32; POS_BLOCK_HEADER_WORDS + stream_words];
    words[0..3].copy_from_slice(&lo);
    words[3] = bits[0] | (bits[1] << 8) | (bits[2] << 16);
    // words[4] (attribute offset) is patched once the page layout is known.
    let mut bitpos = 0;
    for p in &q {
        for i in 0..3 {
            put_bits(
                &mut words[POS_BLOCK_HEADER_WORDS..],
                bitpos,
                p[i] - lo[i],
                bits[i],
            );
            bitpos += bits[i] as usize;
        }
    }

    let mut attr = Vec::with_capacity(verts.len() * stride + 3);
    for &v in &verts {
        let s = dag.vs.attr[v as usize] as usize * stride;
        attr.extend_from_slice(&attrs[s..s + stride]);
    }
    while attr.len() % 4 != 0 {
        attr.push(0);
    }

    Enc {
        tris: c.tris.len() as u32,
        verts: verts.len() as u32,
        pos: bytemuck::cast_slice(&words).to_vec(),
        attr,
        idx,
        cone: cone_for(dag, &c.tris),
    }
}

struct Item {
    id: u32,
    sphere: [f32; 4],
    err: f32,
}

fn build_node(items: &[Item], nodes: &mut Vec<GpuHierarchyNode>) -> (u32, [f32; 4], f32) {
    let idx = nodes.len() as u32;
    nodes.push(GpuHierarchyNode::default());
    let mut node = GpuHierarchyNode::default();
    let n = items.len();
    let parts = n.min(NODE_FANOUT);
    let mut sphere: Option<[f32; 4]> = None;
    let mut max_err = 0.0f32;
    for p in 0..parts {
        let chunk = &items[p * n / parts..(p + 1) * n / parts];
        let (child, s, e) = if chunk.len() == 1 {
            (NODE_LEAF_BIT | chunk[0].id, chunk[0].sphere, chunk[0].err)
        } else {
            build_node(chunk, nodes)
        };
        node.children[p] = child;
        node.spheres[p] = s;
        node.max_parent_error[p] = e;
        sphere = Some(match sphere {
            Some(a) => sphere_union(a, s),
            None => s,
        });
        max_err = max_err.max(e);
    }
    nodes[idx as usize] = node;
    (idx, sphere.unwrap(), max_err)
}

fn align(v: usize, a: usize) -> usize {
    (v + a - 1) / a * a
}

pub fn write_vgeo(
    dag: &Dag,
    attrs: &[u8],
    stride: usize,
    opts: &BuildOptions,
) -> Result<Vec<u8>, BuildError> {
    // ── lattice ────────────────────────────────────────────────────────────
    let used = dag
        .clusters
        .iter()
        .flat_map(|c| c.tris.iter().flatten())
        .map(|&v| dag.vs.pos[v as usize]);
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in used {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let ext = (0..3).map(|i| (hi[i] - lo[i]) as f64).fold(0.0, f64::max);
    let step = if ext > 0.0 {
        (ext * (1.0 + 1e-6) / LATTICE_MAX as f64) as f32
    } else {
        1.0
    };
    let lat = Lattice { origin: lo, step };

    // ── encode clusters ────────────────────────────────────────────────────
    let mut enc: Vec<Enc> = (0..dag.clusters.len())
        .into_par_iter()
        .map(|ci| encode_cluster(dag, ci, attrs, stride, &lat))
        .collect();

    // ── page packing ───────────────────────────────────────────────────────
    // Coarsest level first so root/coarse pages are a resident prefix; within
    // a level, Morton order keeps spatially close groups in the same page so a
    // view-dependent request touches few pages.
    let frame = morton_frame(
        dag.groups
            .iter()
            .map(|g| [g.sphere[0], g.sphere[1], g.sphere[2]]),
    );
    let mut order: Vec<usize> = (0..dag.groups.len()).collect();
    let key = |g: usize| {
        let s = dag.groups[g].sphere;
        (
            std::cmp::Reverse(dag.groups[g].level),
            morton3([s[0], s[1], s[2]], frame),
            g,
        )
    };
    order.sort_by_key(|&g| key(g));

    let page_payload = PAGE_BYTES as usize - std::mem::size_of::<PageHeader>();
    let mut pages: Vec<Vec<usize>> = Vec::new();
    let mut used_bytes = usize::MAX;
    for &g in &order {
        let sz: usize = dag.groups[g]
            .clusters
            .iter()
            .map(|&c| enc[c as usize].bytes())
            .sum();
        if sz > page_payload {
            return Err(BuildError::GroupTooLarge);
        }
        if used_bytes == usize::MAX || used_bytes + sz > page_payload {
            pages.push(Vec::new());
            used_bytes = 0;
        }
        pages.last_mut().unwrap().push(g);
        used_bytes += sz;
    }
    let level_count = dag
        .groups
        .iter()
        .filter(|g| g.is_root)
        .map(|g| g.level)
        .max()
        .unwrap_or(0)
        + 1;

    // Group-ref ids follow page order.
    let mut group_id = vec![NONE; dag.groups.len()];
    let mut refs: Vec<GpuGroupRef> = Vec::with_capacity(dag.groups.len());
    let mut dir: Vec<PageDirEntry> = Vec::with_capacity(pages.len());
    let mut cluster_total = 0u32;
    let mut group_page = vec![0u32; dag.groups.len()];

    // Assign every group its ref id first: a cluster stores the ref of the
    // group that PRODUCED it, which lives on a later (finer) page.
    for (pid, groups) in pages.iter().enumerate() {
        let mut first = 0usize;
        for &g in groups {
            group_id[g] = refs.len() as u32;
            group_page[g] = pid as u32;
            refs.push(GpuGroupRef::new(
                pid as u32,
                first as u32,
                dag.groups[g].clusters.len() as u32,
            ));
            first += dag.groups[g].clusters.len();
        }
    }
    // Page dependencies: a page may only be resident if the pages holding the
    // PARENTS of its clusters are (parents = clusters produced by the groups
    // whose members live here; they are members of their consumer groups).
    let mut deps: Vec<std::collections::BTreeSet<u32>> = vec![Default::default(); pages.len()];
    for (g, grp) in dag.groups.iter().enumerate() {
        for &p in &grp.produced {
            let consumer = dag.clusters[p as usize].consumer;
            if consumer != NONE && group_page[consumer as usize] != group_page[g] {
                deps[group_page[g] as usize].insert(group_page[consumer as usize]);
            }
        }
    }

    // ── hierarchy over groups ──────────────────────────────────────────────
    let mut items: Vec<(u64, Item)> = dag
        .groups
        .iter()
        .enumerate()
        .map(|(g, grp)| {
            let s = grp.sphere;
            (
                morton3([s[0], s[1], s[2]], frame),
                Item {
                    id: group_id[g],
                    sphere: s,
                    err: grp.error,
                },
            )
        })
        .collect();
    items.sort_by_key(|(m, it)| (*m, it.id));
    let items: Vec<Item> = items.into_iter().map(|x| x.1).collect();
    let mut nodes = Vec::new();
    build_node(&items, &mut nodes);

    // Pages that must be resident: everything in the top `resident_levels`.
    // Bounded by a page budget: take as many top levels as fit in
    // `MAX_ROOT_PAGES` (at most `resident_levels`); the root level alone must fit.
    let top = level_count - 1;
    let pages_for = |levels: u32| -> u32 {
        let min_resident = top.saturating_sub(levels - 1);
        dag.groups
            .iter()
            .enumerate()
            .filter(|(_, g)| g.level >= min_resident)
            .map(|(g, _)| group_page[g] + 1)
            .max()
            .unwrap_or(1)
    };
    let root_page_count = pages_for(1);
    if root_page_count > MAX_ROOT_PAGES {
        return Err(BuildError::RootTooLarge {
            pages: root_page_count,
        });
    }
    let mut root_page_count = root_page_count;
    for levels in 2..=opts.resident_levels.max(1).min(level_count) {
        let p = pages_for(levels);
        if p > MAX_ROOT_PAGES {
            break;
        }
        root_page_count = p;
    }

    // ── container ──────────────────────────────────────────────────────────
    let hierarchy_offset = align(
        std::mem::size_of::<VgeoHeader>() + std::mem::size_of::<VgeoMeta>(),
        16,
    );
    let group_refs_offset =
        hierarchy_offset + nodes.len() * std::mem::size_of::<GpuHierarchyNode>();
    let page_dir_offset = group_refs_offset + refs.len() * std::mem::size_of::<GpuGroupRef>();
    let page_deps_offset = align(
        page_dir_offset + pages.len() * std::mem::size_of::<PageDirEntry>(),
        4,
    );
    let mut deps_csr: Vec<u32> = Vec::with_capacity(pages.len() + 1);
    let mut deps_flat: Vec<u32> = Vec::new();
    for d in &deps {
        deps_csr.push(deps_flat.len() as u32);
        deps_flat.extend(d.iter().copied());
    }
    deps_csr.push(deps_flat.len() as u32);
    let deps_end = page_deps_offset + (deps_csr.len() + deps_flat.len()) * 4;
    let pages_offset = align(deps_end, 4096);
    let total = pages_offset + pages.len() * PAGE_BYTES as usize;
    if total > u32::MAX as usize {
        return Err(BuildError::FileTooLarge);
    }
    let mut out = vec![0u8; total];
    for (pid, groups) in pages.iter().enumerate() {
        // Page-local cluster order: groups in packing order, each group's
        // clusters contiguous (that is what a GpuGroupRef addresses).
        let mut members: Vec<usize> = Vec::new();
        for &g in groups {
            members.extend(dag.groups[g].clusters.iter().map(|&c| c as usize));
        }
        let n = members.len();
        let clusters_off = std::mem::size_of::<PageHeader>();
        let pos_off = clusters_off + n * std::mem::size_of::<GpuCluster>();
        let attr_off = pos_off + members.iter().map(|&c| enc[c].pos.len()).sum::<usize>();
        let idx_off = attr_off + members.iter().map(|&c| enc[c].attr.len()).sum::<usize>();
        let end = idx_off + members.iter().map(|&c| enc[c].idx.len()).sum::<usize>();
        debug_assert!(end <= PAGE_BYTES as usize);

        let page = &mut out[pages_offset + pid * PAGE_BYTES as usize..][..PAGE_BYTES as usize];
        let hdr = PageHeader {
            magic: PAGE_MAGIC,
            version: FORMAT_VERSION,
            page_id: pid as u32,
            cluster_count: n as u32,
            clusters_offset: clusters_off as u32,
            position_stream_offset: pos_off as u32,
            attribute_stream_offset: attr_off as u32,
            index_stream_offset: idx_off as u32,
        };
        page[..clusters_off].copy_from_slice(bytes_of(&hdr));
        let (mut po, mut ao, mut io) = (pos_off, attr_off, idx_off);
        let (mut min_l, mut max_l) = (u32::MAX, 0u32);
        for (slot, &ci) in members.iter().enumerate() {
            let e = std::mem::take(&mut enc[ci]); // free scratch as pages are written
            let c = &dag.clusters[ci];
            let mut pos = e.pos.clone();
            pos[16..20].copy_from_slice(&(ao as u32).to_le_bytes());
            page[po..po + pos.len()].copy_from_slice(&pos);
            page[ao..ao + e.attr.len()].copy_from_slice(&e.attr);
            page[io..io + e.idx.len()].copy_from_slice(&e.idx);
            let mut flags = 0;
            if c.consumer != NONE && dag.groups[c.consumer as usize].is_root {
                flags |= GpuCluster::FLAG_ROOT;
            }
            if c.level == 0 {
                flags |= GpuCluster::FLAG_LEAF;
            }
            let rec = GpuCluster {
                sphere: c.sphere,
                parent_sphere: c.parent_sphere,
                cone: e.cone,
                self_error: c.self_error,
                parent_error: c.parent_error,
                counts_material: GpuCluster::pack_counts(e.tris, e.verts, c.mat),
                vertex_offset: po as u32,
                index_offset: io as u32,
                group_id: if c.producer == NONE {
                    NODE_INVALID
                } else {
                    group_id[c.producer as usize]
                },
                flags,
            };
            let at = clusters_off + slot * std::mem::size_of::<GpuCluster>();
            page[at..at + 64].copy_from_slice(bytes_of(&rec));
            po += pos.len();
            ao += e.attr.len();
            io += e.idx.len();
            min_l = min_l.min(c.level);
            max_l = max_l.max(c.level);
        }
        cluster_total += n as u32;
        dir.push(PageDirEntry {
            file_offset: 0,
            cluster_count: n as u32,
            min_level: min_l,
            max_level: max_l,
        });
    }

    for (i, d) in dir.iter_mut().enumerate() {
        d.file_offset = (pages_offset + i * PAGE_BYTES as usize) as u32;
    }

    let (lod_bounds, lod_error_max, lod_table) = lod_summary(dag);
    let header = VgeoHeader {
        magic: VGEO_MAGIC,
        version: FORMAT_VERSION,
        content_id: opts.content_id,
        asset: GpuAssetHeader {
            root_node: 0,
            node_count: nodes.len() as u32,
            group_count: refs.len() as u32,
            page_count: pages.len() as u32,
            cluster_count: cluster_total,
            root_page_count,
            triangle_count: dag.source_tris as u32,
            flags: crate::COOK_REVISION << 16,
            bounds: lod_bounds,
            lod_error_max: lod_error_max,
            lod_table: lod_table,
            _reserved: [0; 3],
        },
        hierarchy_offset: hierarchy_offset as u32,
        group_refs_offset: group_refs_offset as u32,
        page_dir_offset: page_dir_offset as u32,
        pages_offset: pages_offset as u32,
    };
    let meta = VgeoMeta {
        origin: lat.origin,
        step: lat.step,
        attribute_stride: stride as u32,
        lattice_bits: LATTICE_BITS,
        level_count,
        page_deps_offset: page_deps_offset as u32,
    };

    out[..VGEO_META_OFFSET].copy_from_slice(bytes_of(&header));
    out[VGEO_META_OFFSET..VGEO_META_OFFSET + 32].copy_from_slice(bytes_of(&meta));
    out[hierarchy_offset..group_refs_offset].copy_from_slice(bytemuck::cast_slice(&nodes));
    out[group_refs_offset..page_dir_offset].copy_from_slice(bytemuck::cast_slice(&refs));
    out[page_dir_offset..page_dir_offset + dir.len() * 16]
        .copy_from_slice(bytemuck::cast_slice(&dir));
    out[page_deps_offset..page_deps_offset + deps_csr.len() * 4]
        .copy_from_slice(bytemuck::cast_slice(&deps_csr));
    let fo = page_deps_offset + deps_csr.len() * 4;
    out[fo..fo + deps_flat.len() * 4].copy_from_slice(bytemuck::cast_slice(&deps_flat));
    Ok(out)
}

/// Bounding sphere of the roots, the largest stored error and the
/// [`LodTable`] of a DAG: triangles of the uniform cut at each tolerance
/// (`self_error <= e < parent_error`, the rule the cull applies).
fn lod_summary(dag: &Dag) -> ([f32; 4], f32, [u32; helio_vg_format::LOD_TABLE_WORDS]) {
    let mut sphere: Option<[f32; 4]> = None;
    let mut e_max = 0.0f32;
    for c in &dag.clusters {
        if c.parent_error.is_infinite() {
            sphere = Some(match sphere {
                Some(s) => crate::math::sphere_union(s, c.sphere),
                None => c.sphere,
            });
        }
        if c.self_error.is_finite() {
            e_max = e_max.max(c.self_error);
        }
    }
    let mut q = [0u8; helio_vg_format::LOD_TABLE_ENTRIES];
    if e_max > 0.0 {
        for (k, v) in q.iter_mut().enumerate() {
            let e = e_max * (-(k as f32) * 0.5).exp2();
            let tris: u64 = dag
                .clusters
                .iter()
                .filter(|c| c.self_error <= e && c.parent_error > e)
                .map(|c| c.tris.len() as u64)
                .sum();
            *v = helio_vg_format::LodTable::quantize(tris as f64);
        }
    }
    let words = helio_vg_format::LodTable {
        error_max: e_max,
        q,
    }
    .words();
    (sphere.unwrap_or([0.0; 4]), e_max, words)
}
