use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helio::{
    MaterialId, MeshId, MeshUpload, Scene, VoxelMode, VoxelVolumeDescriptor, VoxelVolumeId,
    VOXEL_TERRAIN_GRID_DIM,
};
use helio_voxel_core::GpuVoxelMaterial;
use helio_voxel_core::BRICK_SIZE;

/// Cache of GPU-uploaded mesh geometry, keyed by the resolved asset path.
///
/// Registered as a subsystem by both the game loader and editor contexts.
/// Components check this cache before loading and uploading mesh files.
pub struct MeshCache {
    pub upload_cache: HashMap<String, (MeshId, MaterialId)>,
}

impl MeshCache {
    pub fn new() -> Self {
        Self {
            upload_cache: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<(MeshId, MaterialId)> {
        self.upload_cache.get(key).copied()
    }

    pub fn insert(&mut self, key: String, ids: (MeshId, MaterialId)) {
        self.upload_cache.insert(key, ids);
    }
}

/// Per-object-instance scene cache, keyed by scene-object ID.
///
/// Tracks which scene objects exist per component instance so that
/// the editor can update transforms in-place instead of deleting and
/// re-inserting every frame (which would cascade-free meshes/materials
/// in the helio scene).
pub struct SceneObjectCache {
    /// scene_object_id → (ObjectId, mesh_asset_path)
    pub map: HashMap<String, (helio::ObjectId, String)>,
}

impl SceneObjectCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get(&self, scene_id: &str) -> Option<(helio::ObjectId, &str)> {
        self.map
            .get(scene_id)
            .map(|(id, path)| (*id, path.as_str()))
    }

    pub fn insert(&mut self, scene_id: String, obj_id: helio::ObjectId, mesh_asset: String) {
        self.map.insert(scene_id, (obj_id, mesh_asset));
    }

    pub fn remove(&mut self, scene_id: &str) -> Option<(helio::ObjectId, String)> {
        self.map.remove(scene_id)
    }
}

/// The engine's built-in assets — resolved at compile time so embedded
/// primitives (SM_Cube, SM_Sphere, etc.) are always available.
const ENGINE_ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets");

macro_rules! prim_bytes {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/meshes/primitives/",
            $name,
            ".fbx"
        ))
    };
}

fn embedded_primitive(stem: &str) -> Option<&'static [u8]> {
    match stem {
        "SM_Cube" => Some(prim_bytes!("SM_Cube")),
        "SM_Sphere" => Some(prim_bytes!("SM_Sphere")),
        "SM_Cylinder" => Some(prim_bytes!("SM_Cylinder")),
        "SM_Plane" => Some(prim_bytes!("SM_Plane")),
        "SM_Torus" => Some(prim_bytes!("SM_Torus")),
        _ => None,
    }
}

/// Resolve an asset path to an existing file on disk.
///
/// Checks (in order):
///  1. absolute path
///  2. project-root-relative
///  3. working-directory-relative
///  4. `cwd/assets/` (editor convention)
///  5. engine built-in assets (embedded primitives dir)
pub fn resolve_asset_path(project_root: &Path, asset: &str) -> PathBuf {
    let norm = asset.replace('\\', "/");
    let p = Path::new(&norm);

    if p.is_absolute() && p.exists() {
        return p.to_path_buf();
    }

    let proj = project_root.join(&norm);
    if proj.exists() {
        return proj;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let cwd_path = cwd.join(&norm);
        if cwd_path.exists() {
            return cwd_path;
        }
        let cwd_assets = cwd.join("assets").join(&norm);
        if cwd_assets.exists() {
            return cwd_assets;
        }
    }

    let engine = Path::new(ENGINE_ASSETS_DIR).join(&norm);
    if engine.exists() {
        return engine;
    }

    proj
}

/// Load a mesh file from disk (or from embedded primitive bytes) into a
/// [`MeshUpload`] payload.
///
/// Components call this when they need to load geometry that hasn't been
/// cached yet.  The `path` should already be resolved to an absolute path
/// (use [`resolve_asset_path`] first if needed).
pub fn load_mesh_upload(path: &Path) -> Option<MeshUpload> {
    let cfg = helio_asset_compat::LoadConfig {
        flip_uv_y: true,
        merge_meshes: false,
        import_scale: glam::Vec3::ONE,
    };

    // Try disk first.
    if path.exists() {
        return helio_asset_compat::load_scene_file_with_config(path, cfg)
            .ok()?
            .meshes
            .into_iter()
            .next()
            .map(|m| MeshUpload {
                vertices: m.vertices,
                indices: m.indices,
            });
    }

    // Fallback: check embedded primitives.
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if let Some(bytes) = embedded_primitive(stem) {
        return helio_asset_compat::load_scene_bytes_with_config(bytes, "fbx", None, cfg)
            .ok()?
            .meshes
            .into_iter()
            .next()
            .map(|m| MeshUpload {
                vertices: m.vertices,
                indices: m.indices,
            });
    }

    None
}

// ── Voxel Terrain ─────────────────────────────────────────────────────────────

pub const VOXEL_GRID_VOLUME: usize = (VOXEL_TERRAIN_GRID_DIM as usize).pow(3);

/// CPU-side 64³ voxel material grid compatible with Helio's raymarch brick pools.
pub struct VoxelGrid {
    pub materials: Vec<u8>,
}

impl VoxelGrid {
    pub fn empty() -> Self {
        Self {
            materials: vec![0; VOXEL_GRID_VOLUME],
        }
    }

    fn idx(x: u32, y: u32, z: u32) -> usize {
        (x + y * VOXEL_TERRAIN_GRID_DIM + z * VOXEL_TERRAIN_GRID_DIM * VOXEL_TERRAIN_GRID_DIM)
            as usize
    }

    pub fn set_voxel(&mut self, x: u32, y: u32, z: u32, material: u8) {
        if x < VOXEL_TERRAIN_GRID_DIM && y < VOXEL_TERRAIN_GRID_DIM && z < VOXEL_TERRAIN_GRID_DIM {
            self.materials[Self::idx(x, y, z)] = material;
        }
    }

    pub fn get_voxel(&self, x: u32, y: u32, z: u32) -> u8 {
        if x < VOXEL_TERRAIN_GRID_DIM && y < VOXEL_TERRAIN_GRID_DIM && z < VOXEL_TERRAIN_GRID_DIM {
            self.materials[Self::idx(x, y, z)]
        } else {
            0
        }
    }

    fn hash(x: i32, y: i32, z: i32, seed: u32) -> f32 {
        let mut h = (x as u32)
            .wrapping_mul(374761393)
            .wrapping_add((y as u32).wrapping_mul(668265263))
            .wrapping_add((z as u32).wrapping_mul(2654435761))
            .wrapping_add(seed.wrapping_mul(2246822519));
        h = (h ^ (h >> 15)).wrapping_mul(2246822519);
        h = (h ^ (h >> 13)).wrapping_mul(3266489917);
        h ^= h >> 16;
        (h as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn smoothstep(t: f32) -> f32 {
        t * t * (3.0 - 2.0 * t)
    }

    fn lerp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * t
    }

    fn value_noise2(x: f32, z: f32, seed: u32) -> f32 {
        let x0 = x.floor() as i32;
        let z0 = z.floor() as i32;
        let sx = Self::smoothstep(x - x0 as f32);
        let sz = Self::smoothstep(z - z0 as f32);
        let n00 = Self::hash(x0, 0, z0, seed);
        let n10 = Self::hash(x0 + 1, 0, z0, seed);
        let n01 = Self::hash(x0, 0, z0 + 1, seed);
        let n11 = Self::hash(x0 + 1, 0, z0 + 1, seed);
        Self::lerp(Self::lerp(n00, n10, sx), Self::lerp(n01, n11, sx), sz)
    }

    fn fbm2(x: f32, z: f32, seed: u32, octaves: u32) -> f32 {
        let mut amp = 0.5;
        let mut freq = 1.0;
        let mut sum = 0.0;
        let mut norm = 0.0;
        for i in 0..octaves {
            sum += Self::value_noise2(x * freq, z * freq, seed.wrapping_add(i * 101)) * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }

    /// Heightmap-based procedural generation (mirrors Helio's VoxelTerrain::generate).
    pub fn generate_heightmap(&mut self, seed: u32) {
        let base_height = VOXEL_TERRAIN_GRID_DIM as f32 * 0.45;
        let amplitude = VOXEL_TERRAIN_GRID_DIM as f32 * 0.22;
        let freq = 1.0 / 18.0;

        for x in 0..VOXEL_TERRAIN_GRID_DIM {
            for z in 0..VOXEL_TERRAIN_GRID_DIM {
                let h = Self::fbm2(x as f32 * freq, z as f32 * freq, seed, 4);
                let terrain_height = base_height + h * amplitude;

                for y in 0..VOXEL_TERRAIN_GRID_DIM {
                    let yf = y as f32;
                    if yf > terrain_height {
                        self.set_voxel(x, y, z, 0); // air
                        continue;
                    }
                    let depth = terrain_height - yf;
                    let mat = if depth < 1.0 {
                        1 // grass
                    } else if depth < 4.0 {
                        2 // dirt
                    } else {
                        3 // stone
                    };
                    self.set_voxel(x, y, z, mat);
                }
            }
        }
    }

    /// Fill the entire grid with a single material (for solid volumes).
    pub fn fill_uniform(&mut self, material: u8) {
        self.materials.fill(material);
    }

    /// Upload the full grid to Helio's raymarch GPU brick/data pools.
    pub fn upload_raymarch(
        &self,
        queue: &wgpu::Queue,
        brick_pool: &wgpu::Buffer,
        data_pool: &wgpu::Buffer,
    ) {
        let bricks_per_axis = VOXEL_TERRAIN_GRID_DIM / BRICK_SIZE;
        for bz in 0..bricks_per_axis {
            for by in 0..bricks_per_axis {
                for bx in 0..bricks_per_axis {
                    let brick_idx =
                        (bz * bricks_per_axis * bricks_per_axis + by * bricks_per_axis + bx)
                            as usize;
                    const WORDS_PER_BRICK: usize =
                        (BRICK_SIZE * BRICK_SIZE * BRICK_SIZE) as usize / 4;
                    let mut brick_words = [0u32; WORDS_PER_BRICK];

                    let mut occupied = false;
                    for lz in 0..BRICK_SIZE {
                        for ly in 0..BRICK_SIZE {
                            for lx in 0..BRICK_SIZE {
                                let gx = bx * BRICK_SIZE + lx;
                                let gy = by * BRICK_SIZE + ly;
                                let gz = bz * BRICK_SIZE + lz;
                                let mat = self.get_voxel(gx, gy, gz);
                                if mat != 0 {
                                    occupied = true;
                                }
                                let linear =
                                    (lz * BRICK_SIZE * BRICK_SIZE + ly * BRICK_SIZE + lx) as usize;
                                let word = linear / 4;
                                let byte_in_word = linear % 4;
                                brick_words[word] |= (mat as u32) << (byte_in_word * 8);
                            }
                        }
                    }

                    let data_offset = brick_idx * WORDS_PER_BRICK;
                    let meta_word = if occupied {
                        (1u32 << 24) | (data_offset as u32)
                    } else {
                        0u32
                    };

                    queue.write_buffer(
                        brick_pool,
                        (brick_idx as u64) * 2 * 4,
                        bytemuck::bytes_of(&meta_word),
                    );
                    queue.write_buffer(
                        data_pool,
                        (data_offset as u64) * 4,
                        bytemuck::cast_slice(&brick_words),
                    );
                }
            }
        }
    }
}

/// Tracks per-scene-object voxel terrain state and bridges into Helio's GPU pools.
pub struct VoxelTerrainCache {
    entries: HashMap<String, TerrainEntry>,
    default_palette: Vec<GpuVoxelMaterial>,
}

pub struct TerrainEntry {
    pub grid: VoxelGrid,
    pub volume_id: Option<VoxelVolumeId>,
    pub dirty: bool,
    pub params_hash: u64,
}

impl VoxelTerrainCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            default_palette: vec![
                GpuVoxelMaterial {
                    color: [0.0, 0.0, 0.0],
                    roughness: 1.0,
                    metalness: 0.0,
                    emissive: 0.0,
                    _pad: [0; 2],
                },
                GpuVoxelMaterial {
                    color: [0.3, 0.7, 0.25],
                    roughness: 0.8,
                    metalness: 0.0,
                    emissive: 0.0,
                    _pad: [0; 2],
                },
                GpuVoxelMaterial {
                    color: [0.45, 0.3, 0.15],
                    roughness: 0.9,
                    metalness: 0.0,
                    emissive: 0.0,
                    _pad: [0; 2],
                },
                GpuVoxelMaterial {
                    color: [0.5, 0.5, 0.52],
                    roughness: 0.85,
                    metalness: 0.0,
                    emissive: 0.0,
                    _pad: [0; 2],
                },
            ],
        }
    }

    pub fn get_or_create(&mut self, scene_id: &str) -> &mut TerrainEntry {
        self.entries
            .entry(scene_id.to_string())
            .or_insert_with(|| TerrainEntry {
                grid: VoxelGrid::empty(),
                volume_id: None,
                dirty: true,
                params_hash: 0,
            })
    }

    /// Remove stale entries (scene objects no longer alive).
    pub fn retain_keys(&mut self, live_keys: &std::collections::HashSet<String>) {
        self.entries.retain(|k, _| live_keys.contains(k));
    }

    /// Remove a single entry.
    pub fn remove(&mut self, scene_id: &str) {
        self.entries.remove(scene_id);
    }

    /// Upload all dirty voxel entries to the GPU, lazily creating voxel volumes.
    pub fn flush(&mut self, scene: &mut Scene, queue: &wgpu::Queue) {
        for entry in self.entries.values_mut() {
            if !entry.dirty {
                continue;
            }

            let _volume_id = match entry.volume_id {
                Some(id) => id,
                None => {
                    let desc = VoxelVolumeDescriptor {
                        voxel_size: 1.0,
                        root_extent: VOXEL_TERRAIN_GRID_DIM as f32,
                        local_to_world: glam::Mat4::IDENTITY,
                        movability: Some(libhelio::Movability::Stationary),
                        mode: Some(VoxelMode::Dynamic),
                        material_palette: self.default_palette.clone(),
                    };
                    match scene.insert_voxel_volume(desc) {
                        Ok(id) => {
                            entry.volume_id = Some(id);
                            id
                        }
                        Err(_) => continue,
                    }
                }
            };

            let gpu = scene.gpu_scene();
            entry
                .grid
                .upload_raymarch(queue, &gpu.voxel_brick_pool, &gpu.voxel_data_pool);
            entry.dirty = false;
        }
    }
}

impl TerrainEntry {
    /// Regenerate procedural heightmap terrain if the generation key changed.
    pub fn sync_procedural(&mut self, seed: u32, params_hash: u64) {
        if self.params_hash == params_hash && !self.dirty {
            return;
        }
        self.grid.generate_heightmap(seed);
        self.params_hash = params_hash;
        self.dirty = true;
    }
}
