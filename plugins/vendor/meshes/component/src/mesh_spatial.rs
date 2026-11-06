//! Content-addressed CPU spatial data for loaded mesh geometry.
//!
//! The asset loader can build this index on its worker before publishing a
//! mesh to SceneDB. Consumers can then raycast the same immutable geometry
//! without scanning every triangle on the interactive path.

use crate::components::MeshGeometryKey;
use glam::Vec3;
use helio_mesh_api::PackedVertex;
use std::{
    collections::{HashMap, VecDeque, hash_map::DefaultHasher},
    hash::Hasher,
    sync::{Arc, Mutex, OnceLock},
};

const MAX_CACHE_ENTRIES: usize = 64;
const MAX_CACHE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Copy)]
struct Triangle([u32; 3]);

struct Node {
    min: Vec3,
    max: Vec3,
    children: [u32; 2],
    triangle_range: [u32; 2],
}

struct MeshBvh {
    triangles: Vec<Triangle>,
    nodes: Vec<Node>,
    estimated_bytes: usize,
}

#[derive(Clone)]
enum CachedSpatialIndex {
    Bvh(Arc<MeshBvh>),
    /// The BVH exceeds the memory budget. Remember this by content key so a
    /// viewport query does not rebuild and discard the same large tree.
    LinearFallback,
}

impl MeshBvh {
    fn build(vertices: &[PackedVertex], indices: &[u32]) -> Self {
        let mut triangles = Vec::new();
        let mut append = |ia: u32, ib: u32, ic: u32| {
            if let (Some(a), Some(b), Some(c)) = (
                vertices.get(ia as usize),
                vertices.get(ib as usize),
                vertices.get(ic as usize),
            ) {
                if [a, b, c]
                    .iter()
                    .all(|v| v.position.iter().all(|x| x.is_finite()))
                {
                    triangles.push(Triangle([ia, ib, ic]));
                }
            }
        };
        if indices.is_empty() {
            for first in (0..vertices.len().saturating_sub(2)).step_by(3) {
                let (Ok(a), Ok(b), Ok(c)) = (
                    u32::try_from(first),
                    u32::try_from(first + 1),
                    u32::try_from(first + 2),
                ) else {
                    break;
                };
                append(a, b, c);
            }
        } else {
            for triangle in indices.chunks_exact(3) {
                append(triangle[0], triangle[1], triangle[2]);
            }
        }

        let mut bvh = Self {
            triangles,
            nodes: Vec::new(),
            estimated_bytes: 0,
        };
        if !bvh.triangles.is_empty() && bvh.triangles.len() <= u32::MAX as usize {
            let count = bvh.triangles.len() as u32;
            bvh.build_node(0, count, vertices);
        } else if bvh.triangles.len() > u32::MAX as usize {
            bvh.triangles.clear();
        }
        bvh.estimated_bytes = bvh
            .triangles
            .capacity()
            .saturating_mul(std::mem::size_of::<Triangle>())
            .saturating_add(
                bvh.nodes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Node>()),
            );
        bvh
    }

    fn build_node(&mut self, start: u32, end: u32, vertices: &[PackedVertex]) -> u32 {
        let (min, max) = triangle_bounds(&self.triangles[start as usize..end as usize], vertices);
        let node_id = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            children: [u32::MAX; 2],
            triangle_range: [start, end],
        });
        if end - start <= 8 {
            return node_id as u32;
        }
        let extent = max - min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let middle = start + (end - start) / 2;
        self.triangles[start as usize..end as usize].select_nth_unstable_by(
            (middle - start) as usize,
            |left, right| {
                let centroid = |triangle: &Triangle| {
                    triangle
                        .0
                        .iter()
                        .filter_map(|index| vertices.get(*index as usize))
                        .map(|vertex| vertex.position[axis])
                        .sum::<f32>()
                        / 3.0
                };
                centroid(left).total_cmp(&centroid(right))
            },
        );
        let left = self.build_node(start, middle, vertices);
        let right = self.build_node(middle, end, vertices);
        self.nodes[node_id].children = [left, right];
        node_id as u32
    }

    fn raycast(&self, vertices: &[PackedVertex], origin: Vec3, direction: Vec3) -> Option<f32> {
        (!self.nodes.is_empty())
            .then(|| self.raycast_node(0, vertices, origin, direction, f32::INFINITY))
            .flatten()
    }

    fn raycast_node(
        &self,
        node_id: u32,
        vertices: &[PackedVertex],
        origin: Vec3,
        direction: Vec3,
        best: f32,
    ) -> Option<f32> {
        let node = self.nodes.get(node_id as usize)?;
        if ray_aabb(origin, direction, node.min, node.max).is_none_or(|entry| entry > best) {
            return None;
        }
        if node.children[0] != u32::MAX {
            let mut nearest = best;
            for child in node.children {
                if let Some(hit) = self.raycast_node(child, vertices, origin, direction, nearest) {
                    nearest = nearest.min(hit);
                }
            }
            return nearest.is_finite().then_some(nearest);
        }
        self.triangles[node.triangle_range[0] as usize..node.triangle_range[1] as usize]
            .iter()
            .filter_map(|triangle| {
                let [a, b, c] = triangle.0;
                Some(
                    [
                        vertices.get(a as usize)?.position,
                        vertices.get(b as usize)?.position,
                        vertices.get(c as usize)?.position,
                    ]
                    .map(Vec3::from_array),
                )
            })
            .filter_map(|triangle| ray_triangle(origin, direction, triangle))
            .min_by(f32::total_cmp)
    }
}

#[derive(Default)]
struct Cache {
    entries: HashMap<MeshGeometryKey, CachedSpatialIndex>,
    lru: VecDeque<MeshGeometryKey>,
    bytes: usize,
}

impl Cache {
    fn get(&mut self, key: MeshGeometryKey) -> Option<CachedSpatialIndex> {
        let value = self.entries.get(&key)?.clone();
        self.lru.retain(|entry| *entry != key);
        self.lru.push_back(key);
        Some(value)
    }

    fn insert(&mut self, key: MeshGeometryKey, index: CachedSpatialIndex) {
        if key == MeshGeometryKey::NONE {
            return;
        }
        let index_bytes = match &index {
            CachedSpatialIndex::Bvh(bvh) => bvh.estimated_bytes,
            CachedSpatialIndex::LinearFallback => 0,
        };
        if index_bytes > MAX_CACHE_BYTES {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(cached_index_bytes(&old));
            self.lru.retain(|entry| *entry != key);
        }
        while self.entries.len() >= MAX_CACHE_ENTRIES
            || self.bytes.saturating_add(index_bytes) > MAX_CACHE_BYTES
        {
            let Some(old_key) = self.lru.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&old_key) {
                self.bytes = self.bytes.saturating_sub(cached_index_bytes(&old));
            }
        }
        self.bytes = self.bytes.saturating_add(index_bytes);
        self.entries.insert(key, index);
        self.lru.push_back(key);
    }
}

fn cached_index_bytes(index: &CachedSpatialIndex) -> usize {
    match index {
        CachedSpatialIndex::Bvh(bvh) => bvh.estimated_bytes,
        CachedSpatialIndex::LinearFallback => 0,
    }
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache::default()))
}

fn cache_key(key: MeshGeometryKey, vertices: &[PackedVertex], indices: &[u32]) -> MeshGeometryKey {
    if key != MeshGeometryKey::NONE {
        return key;
    }
    // Generated meshes sometimes have no assigned content id. Derive an
    // in-process key from the spatially relevant payload so repeated picks
    // reuse the same index; positions and topology fully determine the BVH.
    let mut first = DefaultHasher::new();
    first.write_u64(vertices.len() as u64);
    first.write_u64(indices.len() as u64);
    for vertex in vertices {
        for component in vertex.position {
            first.write_u32(component.to_bits());
        }
    }
    for index in indices {
        first.write_u32(*index);
    }
    let mut second = DefaultHasher::new();
    second.write_u64(0x6d65_7368_7370_6174);
    second.write_u64(vertices.len() as u64);
    for vertex in vertices {
        for component in vertex.position {
            second.write_u32(component.to_bits());
        }
    }
    second.write_u64(indices.len() as u64);
    for index in indices {
        second.write_u32(*index);
    }
    let fallback = (u128::from(first.finish()) << 64) | u128::from(second.finish());
    MeshGeometryKey(fallback.max(1))
}

/// Build and retain a mesh's BVH under its content identity.
///
/// Call this from asset-loading workers before publishing loaded geometry.
/// Returns false for an empty/invalid mesh or an index too large for the
/// bounded cache. A `MeshGeometryKey::NONE` key is replaced with a content
/// fingerprint so generated geometry can also be retained.
pub fn prewarm_mesh_bvh(key: MeshGeometryKey, vertices: &[PackedVertex], indices: &[u32]) -> bool {
    let key = cache_key(key, vertices, indices);
    let mut spatial_cache = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(index) = spatial_cache.get(key) {
        return matches!(index, CachedSpatialIndex::Bvh(_));
    }
    drop(spatial_cache);
    let triangle_count = if indices.is_empty() {
        vertices.len() / 3
    } else {
        indices.len() / 3
    };
    if estimated_bvh_bytes(triangle_count) > MAX_CACHE_BYTES {
        cache()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(key, CachedSpatialIndex::LinearFallback);
        return false;
    }
    let bvh = Arc::new(MeshBvh::build(vertices, indices));
    if bvh.triangles.is_empty() {
        return false;
    }
    let fits = bvh.estimated_bytes <= MAX_CACHE_BYTES;
    cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(
            key,
            if fits {
                CachedSpatialIndex::Bvh(bvh)
            } else {
                CachedSpatialIndex::LinearFallback
            },
        );
    fits
}

fn estimated_bvh_bytes(triangle_count: usize) -> usize {
    if triangle_count == 0 {
        return 0;
    }
    // Each leaf holds at most eight triangles; each split adds one internal
    // node. This upper bound avoids building a huge tree only to discard it.
    let max_nodes = triangle_count
        .div_ceil(8)
        .saturating_mul(2)
        .saturating_sub(1);
    triangle_count
        .saturating_mul(std::mem::size_of::<Triangle>())
        .saturating_add(max_nodes.saturating_mul(std::mem::size_of::<Node>()))
}

/// Return the nearest forward ray hit in mesh-local units.
///
/// `direction` need not be normalized; the returned parameter satisfies
/// `origin + direction * t`. For a transformed unit world ray, this preserves
/// the world-distance parameter when the ray is inverse-transformed locally.
pub fn raycast_mesh(
    key: MeshGeometryKey,
    vertices: &[PackedVertex],
    indices: &[u32],
    origin: Vec3,
    direction: Vec3,
) -> Option<f32> {
    if direction.length_squared() <= f32::EPSILON || !direction.is_finite() {
        return None;
    }
    let key = cache_key(key, vertices, indices);
    let cached = if key != MeshGeometryKey::NONE {
        cache()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(key)
    } else {
        None
    };
    match cached {
        Some(CachedSpatialIndex::Bvh(bvh)) => bvh.raycast(vertices, origin, direction),
        Some(CachedSpatialIndex::LinearFallback) => {
            raycast_linear(vertices, indices, origin, direction)
        }
        None => {
            let bvh = Arc::new(MeshBvh::build(vertices, indices));
            if key != MeshGeometryKey::NONE {
                let fits = bvh.estimated_bytes <= MAX_CACHE_BYTES;
                cache()
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .insert(
                        key,
                        if fits {
                            CachedSpatialIndex::Bvh(bvh.clone())
                        } else {
                            CachedSpatialIndex::LinearFallback
                        },
                    );
                if !fits {
                    return raycast_linear(vertices, indices, origin, direction);
                }
            }
            bvh.raycast(vertices, origin, direction)
        }
    }
}

fn triangle_bounds(triangles: &[Triangle], vertices: &[PackedVertex]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for triangle in triangles {
        for index in triangle.0 {
            if let Some(vertex) = vertices.get(index as usize) {
                let point = Vec3::from_array(vertex.position);
                min = min.min(point);
                max = max.max(point);
            }
        }
    }
    (min, max)
}

fn raycast_linear(
    vertices: &[PackedVertex],
    indices: &[u32],
    origin: Vec3,
    direction: Vec3,
) -> Option<f32> {
    let mut nearest = f32::INFINITY;
    let mut test = |a: u32, b: u32, c: u32| {
        let (Some(a), Some(b), Some(c)) = (
            vertices.get(a as usize),
            vertices.get(b as usize),
            vertices.get(c as usize),
        ) else {
            return;
        };
        let triangle = [a, b, c].map(|vertex| Vec3::from_array(vertex.position));
        if let Some(hit) = ray_triangle(origin, direction, triangle) {
            nearest = nearest.min(hit);
        }
    };
    if indices.is_empty() {
        for first in (0..vertices.len().saturating_sub(2)).step_by(3) {
            let (Ok(a), Ok(b), Ok(c)) = (
                u32::try_from(first),
                u32::try_from(first + 1),
                u32::try_from(first + 2),
            ) else {
                break;
            };
            test(a, b, c);
        }
    } else {
        for [a, b, c] in indices
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        {
            test(a, b, c);
        }
    }
    nearest.is_finite().then_some(nearest)
}

fn ray_aabb(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let mut near = 0.0_f32;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        if direction[axis].abs() < f32::EPSILON {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let inverse = direction[axis].recip();
        let first = (min[axis] - origin[axis]) * inverse;
        let second = (max[axis] - origin[axis]) * inverse;
        near = near.max(first.min(second));
        far = far.min(first.max(second));
        if near > far {
            return None;
        }
    }
    far.is_finite().then_some(near)
}

fn ray_triangle(origin: Vec3, direction: Vec3, [a, b, c]: [Vec3; 3]) -> Option<f32> {
    let edge1 = b - a;
    let edge2 = c - a;
    let p = direction.cross(edge2);
    let determinant = edge1.dot(p);
    if !determinant.is_finite() || determinant.abs() < 1e-8 {
        return None;
    }
    let inverse = determinant.recip();
    let from_a = origin - a;
    let u = from_a.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = from_a.cross(edge1);
    let v = direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = edge2.dot(q) * inverse;
    (distance >= 0.0 && distance.is_finite()).then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_mesh() -> Vec<PackedVertex> {
        [
            [-1.0, -1.0, 0.0],
            [1.0, -1.0, 0.0],
            [0.0, 1.0, 0.0],
            [-1.0, -1.0, -2.0],
            [1.0, -1.0, -2.0],
            [0.0, 1.0, -2.0],
        ]
        .into_iter()
        .map(|position| {
            PackedVertex::from_components(position, [0.0, 0.0, 1.0], [0.0; 2], [1.0, 0.0, 0.0], 1.0)
        })
        .collect()
    }

    #[test]
    fn indexed_bvh_hits_nearest_triangle_and_rejects_empty_bounds_space() {
        let vertices = triangle_mesh();
        let indices = [0, 1, 2, 3, 4, 5];
        let key = MeshGeometryKey(0x70501);
        assert!(prewarm_mesh_bvh(key, &vertices, &indices));
        assert_eq!(
            raycast_mesh(key, &vertices, &indices, Vec3::new(0.0, 0.0, 3.0), -Vec3::Z),
            Some(3.0)
        );
        assert_eq!(
            raycast_mesh(
                key,
                &vertices,
                &indices,
                Vec3::new(0.95, 0.95, 3.0),
                -Vec3::Z
            ),
            None
        );
    }

    #[test]
    fn memory_bounded_linear_fallback_keeps_exact_triangle_semantics() {
        let vertices = triangle_mesh();
        let indices = [0, 1, 2, 3, 4, 5];
        assert_eq!(
            raycast_linear(&vertices, &indices, Vec3::new(0.0, 0.0, 3.0), -Vec3::Z),
            Some(3.0)
        );
        assert_eq!(
            raycast_linear(&vertices, &indices, Vec3::new(0.95, 0.95, 3.0), -Vec3::Z),
            None
        );
    }

    #[test]
    fn unindexed_meshes_and_back_faces_are_pickable() {
        let vertices = triangle_mesh();
        assert!(prewarm_mesh_bvh(MeshGeometryKey::NONE, &vertices[..3], &[]));
        assert_eq!(
            raycast_mesh(
                MeshGeometryKey::NONE,
                &vertices[..3],
                &[],
                Vec3::new(0.0, 0.0, -1.0),
                Vec3::Z
            ),
            Some(1.0)
        );
    }
}
