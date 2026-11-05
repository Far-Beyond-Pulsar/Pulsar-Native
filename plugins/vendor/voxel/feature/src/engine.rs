//! The host interface (frames, picks, the gbuffer) and the terrain half of
//! the shaders: the terrain program composed with the world helpers, checked
//! against the CPU field.
use crate::planet::Planet;
use crate::terrain::TerrainProgram;
use crate::volume::{VolumeTransform, VoxelVolume};
use bytemuck::{Pod, Zeroable};
use glam::{DVec3, IVec4, Vec3};
use std::sync::{Arc, Mutex};
use wgpu::util::DeviceExt;

/// The voxel volumes a scene shows, as its content publishes them each
/// frame under [`VOXEL_FRAME`] (a Pulsar voxel terrain's runtime, a Helio
/// example host). The pass draws them from the active camera's eye.
#[derive(Clone)]
pub struct PlanetFrame {
    /// The volumes shown: a planet, moons, planes, small free volumes. The
    /// first is the primary one (foliage grows on it, GI probes trace it).
    pub volumes: Vec<VolumeFrame>,
    /// Direction towards the sun (world frame).
    pub sun: Vec3,
}

/// Where scene content publishes its voxel volumes for the voxel feature's
/// pass ([`helio_core::PayloadContract`]).
pub const VOXEL_FRAME: helio_core::PayloadContract<PlanetFrame> =
    helio_core::PayloadContract::new("voxel.frame");

impl PlanetFrame {
    /// A frame of one world at the origin (its own frame is the world's).
    pub fn single(planet: Arc<Planet>, sun: Vec3) -> Self {
        Self {
            volumes: vec![VolumeFrame::new(VoxelVolume::new(
                0,
                planet,
                VolumeTransform::IDENTITY,
            ))],
            sun,
        }
    }

    /// The frame's first volume's world, if any.
    pub fn planet(&self) -> Option<&Arc<Planet>> {
        self.volumes.first().map(|v| &v.volume.planet)
    }
}

/// A volume of a frame: the world and where it is, how it looks, the chunks
/// a save kept of its edits, and what its owner asks of the drawn world.
#[derive(Clone)]
pub struct VolumeFrame {
    pub volume: VoxelVolume,
    /// Chunks a save kept of the volume's edits: a volume built for exactly
    /// those edits restores them by copy instead of generating them (others
    /// are refused, [`crate::chunks::ChunkWorld::load_kept`]).
    pub kept: Option<Arc<crate::chunks::KeptChunks>>,
    /// The owner's art settings (none: the terrain generator's own).
    pub appearance: Option<TerrainAppearance>,
    /// How smooth its surfaces are lit, 0 to 1, over whichever appearance
    /// is shown (`TerrainAppearance::smoothing`).
    pub smoothing: f32,
    /// What the owner asks of the drawn world, and where the answers go.
    pub requests: VolumeRequests,
}

impl VolumeFrame {
    pub fn new(volume: VoxelVolume) -> Self {
        Self {
            volume,
            kept: None,
            appearance: None,
            smoothing: 0.0,
            requests: VolumeRequests::default(),
        }
    }
}

/// A volume owner's questions about the drawn world, published with every
/// frame until answered: each is answered once, into `answers`.
#[derive(Clone, Default)]
pub struct VolumeRequests {
    /// The drawn surface under rays from a camera's eye.
    pub picks: Vec<PickRequest>,
    /// Pack the volume's resident edited chunks for a save (a request id;
    /// a newer id asks again).
    pub keep: Option<u64>,
    /// The owner's inbox.
    pub answers: Option<Arc<VolumeAnswers>>,
}

/// A request for the first terrain hit drawn along a ray from a camera's
/// eye (world frame), answered a frame or two later by the pass whose
/// camera is at that eye ([`VolumeAnswer::Pick`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickRequest {
    pub id: u64,
    pub origin: DVec3,
    pub direction: DVec3,
}

/// The answer to a [`PickRequest`]: the first terrain hit along that
/// pixel's ray as drawn, or `None` (sky, loading, a mesh in front:
/// terrain behind meshes is not traced).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickResult {
    pub id: u64,
    pub hit: Option<PickHit>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickHit {
    /// The volume hit (`VoxelVolume::id`); `cell` is in its frame.
    pub volume: u64,
    /// From the eye along the pixel's ray.
    pub distance: f64,
    /// The level cell drawn there: at level 0 the exact voxel.
    pub cell: crate::grid::Cell,
    pub level: u32,
    /// How the ray entered the cell: `2 axis + 1` stepping up axis (i, j,
    /// k), `2 axis` stepping down it (`Hit::axis` and its side in chunk_draw.wgsl); `None`
    /// when it started inside. See [`crate::Planet::drawn_hit`].
    pub entered: Option<u32>,
    /// The material drawn there: the cell's own where edits set it, else
    /// its leaf's surface or subsurface material.
    pub material: u32,
}

/// What the pass answers a volume's owner.
#[derive(Clone)]
pub enum VolumeAnswer {
    Pick(PickResult),
    /// The resident edited chunks of the world shown, for keep request
    /// `id`: valid for that world's edits
    /// ([`crate::chunks::ChunkWorld::kept_chunks`]).
    Kept {
        id: u64,
        planet: Arc<Planet>,
        chunks: crate::chunks::KeptChunks,
    },
}

/// A volume owner's inbox: the pass pushes answers, the owner takes them.
#[derive(Default)]
pub struct VolumeAnswers(Mutex<Vec<VolumeAnswer>>);

impl VolumeAnswers {
    pub fn push(&self, answer: VolumeAnswer) {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(answer);
    }

    pub fn take(&self) -> Vec<VolumeAnswer> {
        std::mem::take(
            &mut *self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

pub use crate::terrain::{MaterialAppearance, TerrainAppearance, MATERIALS};

/// `VoxelVariation` of chunk_draw.wgsl: how the voxel terrain varies a
/// material over the land (its dry, middle and lush patch tints of the
/// material's base colour; `tints[0][3]` 1: it varies).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct VariationGpu {
    tints: [[f32; 4]; 3],
}

/// The voxel materials of `appearance`: each id's material as an ordinary
/// material row (the materials' [`helio_mats::MaterialRow`]: linear base
/// colour, roughness, emission as the colour times its strength, and two-
/// sided foliage for a material that lets light through), the voxel's own
/// variation of it (patch tints), its detail weights and its shape weights
/// (x: smoothing). sRGB is public, so colours convert once, not per pixel.
pub(crate) fn material_table(
    appearance: &TerrainAppearance,
) -> (
    [helio_mats::MaterialRow; MATERIALS],
    [VariationGpu; MATERIALS],
    [f32; 4],
    [f32; 4],
) {
    let clean = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let linear = |c: [f32; 3]| {
        [
            clean(c[0]).powf(2.2),
            clean(c[1]).powf(2.2),
            clean(c[2]).powf(2.2),
        ]
    };
    let rows = std::array::from_fn(|id| {
        let m = &appearance.materials[id];
        let base = linear([m.colour[0], m.colour[1], m.colour[2]]);
        let emission = if m.emission.is_finite() {
            m.emission.clamp(0.0, 64.0)
        } else {
            0.0
        };
        let row = helio_mats::MaterialRow::new(
            [base[0], base[1], base[2], 1.0],
            clean(m.colour[3]),
            0.0,
            base,
            emission,
        );
        let transmission = clean(m.transmission);
        if transmission > 0.0 {
            row.foliage(transmission)
        } else {
            row
        }
    });
    let variation = std::array::from_fn(|id| {
        let m = &appearance.materials[id];
        let base = linear([m.colour[0], m.colour[1], m.colour[2]]);
        let mut tints = [[0.0; 4]; 3];
        if let Some(patches) = m.patches {
            // A patch colour as a tint of the base colour (a channel the
            // base lacks stays black).
            tints = patches.map(|patch| {
                let patch = linear(patch);
                let tint = |c: usize| {
                    if base[c] > 0.0 {
                        patch[c] / base[c]
                    } else {
                        0.0
                    }
                };
                [tint(0), tint(1), tint(2), 0.0]
            });
            tints[0][3] = 1.0;
        }
        VariationGpu { tints }
    });
    (
        rows,
        variation,
        appearance.detail.map(clean),
        [clean(appearance.smoothing), 0.0, 0.0, 0.0],
    )
}

/// `World` of world.wgsl: the grid mapping.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct WorldGpu {
    /// reference cells, layer thickness (mm), grid cells, level offset.
    grid: [i32; 4],
    /// domain scale (Q24), pad.
    scale: [u32; 4],
    /// sphere domain constants (`Grid::sphere_constants`).
    sphere: [u32; 4],
}

impl WorldGpu {
    pub(crate) fn new(planet: &Planet) -> Self {
        let g = planet.grid();
        Self {
            grid: [
                g.reference_cells(),
                g.layer_mm() as i32,
                g.cells(),
                g.level_offset() as i32,
            ],
            scale: {
                let (inv, shift, layer_q16) = g.volume_constants();
                [g.domain_scale(), inv, shift, layer_q16]
            },
            sphere: g.sphere_constants(),
        }
    }
}

/// The program's tables as `terrain_tables` (binding 40): at least one
/// `vec4<u32>`, a whole number of them.
pub(crate) fn tables_buffer(device: &wgpu::Device, program: &TerrainProgram) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    let mut words = program.tables.to_vec();
    words.resize(words.len().max(4).next_multiple_of(4), 0);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain tables"),
        contents: bytemuck::cast_slice(&words),
        usage: wgpu::BufferUsages::STORAGE,
    })
}

/// Most probes a region lookup makes (`terrain_region`): the table is at
/// most half full.
const REGION_PROBES: usize = 64;

/// A region key's hash (`terrain_region_hash` in WGSL).
fn region_hash(key: [u32; 2]) -> u32 {
    let h = key[0].wrapping_mul(0x9e37_79b1) ^ key[1].wrapping_mul(0x85eb_ca77);
    (h ^ (h >> 15)).wrapping_mul(0x2c1b_3c6d) ^ (h >> 13)
}

/// A program's regions on the GPU (`terrain_regions`, binding 41;
/// [`crate::terrain::TerrainRegions`]): an open-addressed table of the
/// resident keys and the regions' words, which the field makes on the CPU
/// the first time a footprint needs them. Layout (`vec4<u32>`): a head
/// (table mask, capacity, words a region, where regions start in words /
/// 4), the table (key, slot + 1), then the regions.
pub(crate) struct RegionPool {
    source: Option<crate::terrain::Regions>,
    words: usize,
    capacity: usize,
    entries: usize,
    buffer: wgpu::Buffer,
    resident: rustc_hash::FxHashMap<[u32; 2], (u32, u64)>,
    free: Vec<u32>,
    /// Regions made since the last flush, and whether the table changed.
    pending: Vec<(u32, Vec<u32>)>,
    table_dirty: bool,
    /// Regions being made off the frame ([`Self::reserve`]), and those made
    /// and not yet in the pool.
    making: rustc_hash::FxHashSet<[u32; 2]>,
    made: std::sync::Arc<std::sync::Mutex<Vec<([u32; 2], Vec<u32>)>>>,
}

impl RegionPool {
    /// A pool of `capacity` regions of `program`'s (none without regions:
    /// a one-word buffer for the binding).
    pub(crate) fn new(device: &wgpu::Device, program: &TerrainProgram, capacity: usize) -> Self {
        let source = program.regions.clone();
        let capacity = if source.is_some() { capacity.max(1) } else { 0 };
        let words = source
            .as_ref()
            .map_or(4, |r| r.0.words().next_multiple_of(4));
        let entries = (2 * capacity).next_power_of_two();
        let size = 16 * (1 + entries as u64) + 4 * (words * capacity) as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain regions"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        {
            let mut head = buffer
                .slice(..16)
                .get_mapped_range_mut()
                .expect("a mapped region head");
            let head_words = [
                (entries - 1) as u32,
                capacity as u32,
                words as u32,
                (1 + entries) as u32,
            ];
            head.copy_from_slice(bytemuck::cast_slice(&head_words));
        }
        buffer.unmap();
        Self {
            source,
            words,
            capacity,
            entries,
            buffer,
            resident: Default::default(),
            free: (0..capacity as u32).rev().collect(),
            pending: Vec::new(),
            table_dirty: false,
            making: Default::default(),
            made: Default::default(),
        }
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// The regions the program reads over footprint columns `[i0, i1] x
    /// [j0, j1]` of `face` at grid level `level` (none without regions).
    pub(crate) fn keys(
        &self,
        grid: &crate::grid::Grid,
        face: u8,
        level: u32,
        i: [i32; 2],
        j: [i32; 2],
    ) -> Vec<[u32; 2]> {
        let mut out = Vec::new();
        if let Some(r) = &self.source {
            r.0.keys(grid, face, level, i, j, &mut out);
        }
        out
    }

    /// Make room for `n` more regions besides those `frame` keeps,
    /// evicting the longest unused (an eighth of the pool at least:
    /// evictions come in batches, not a table upload a footprint): how many
    /// fit.
    fn room(&mut self, n: usize, frame: u64) -> usize {
        if self.free.len() < n {
            let mut old: Vec<([u32; 2], u32, u64)> = self
                .resident
                .iter()
                .filter(|(_, e)| e.1 < frame)
                .map(|(k, e)| (*k, e.0, e.1))
                .collect();
            old.sort_unstable_by_key(|e| (e.2, e.0));
            for e in old
                .iter()
                .take((n - self.free.len()).max(self.capacity / 8))
            {
                self.resident.remove(&e.0);
                self.free.push(e.1);
            }
            self.table_dirty = true;
        }
        n.min(self.free.len())
    }

    /// Region `key`'s words `w` into a free slot, kept through `frame`.
    fn place(&mut self, key: [u32; 2], w: Vec<u32>, frame: u64) {
        let slot = self.free.pop().expect("a free region slot");
        self.resident.insert(key, (slot, frame));
        self.pending.push((slot, w));
        self.table_dirty = true;
    }

    /// Keep regions `keys` resident through `frame`, making the missing
    /// ones now (on the CPU, in parallel): false when the pool cannot hold
    /// them besides those `frame` keeps (then nothing is made).
    pub(crate) fn reserve_now(&mut self, keys: &[[u32; 2]], frame: u64) -> bool {
        use rayon::prelude::*;
        let Some(source) = self.source.clone() else {
            return true;
        };
        let mut missing = Vec::new();
        for k in keys {
            match self.resident.get_mut(k) {
                Some(e) => e.1 = e.1.max(frame),
                None => missing.push(*k),
            }
        }
        if self.room(missing.len(), frame) < missing.len() {
            return false;
        }
        let words = self.words;
        let made: Vec<Vec<u32>> = missing
            .par_iter()
            .map(|k| {
                let mut w = vec![0u32; words];
                source.0.region(*k, &mut w[..source.0.words()]);
                w
            })
            .collect();
        for (k, w) in missing.into_iter().zip(made) {
            self.place(k, w, frame);
        }
        true
    }

    /// Have regions `keys` made off the frame if they are not resident
    /// (a footprint the frame may reach: its regions come together).
    pub(crate) fn request(&mut self, keys: &[[u32; 2]]) {
        let Some(source) = self.source.clone() else {
            return;
        };
        for &k in keys {
            if self.resident.contains_key(&k) || !self.making.insert(k) {
                continue;
            }
            let (source, made, words) = (source.clone(), self.made.clone(), self.words);
            rayon::spawn(move || {
                let mut w = vec![0u32; words];
                source.0.region(k, &mut w[..source.0.words()]);
                made.lock().unwrap_or_else(|e| e.into_inner()).push((k, w));
            });
        }
    }

    /// Whether regions `keys` are all resident.
    pub(crate) fn resident(&self, keys: &[[u32; 2]]) -> bool {
        self.source.is_none() || keys.iter().all(|k| self.resident.contains_key(k))
    }

    /// Whether regions `keys` are all resident, kept through `frame` if
    /// so. Missing ones are made off the frame (the thread pool) and enter
    /// the pool on a later call: a footprint waits for its regions instead
    /// of the frame making them.
    pub(crate) fn reserve(&mut self, keys: &[[u32; 2]], frame: u64) -> bool {
        let Some(source) = self.source.clone() else {
            return true;
        };
        self.admit(frame);
        let mut ready = true;
        for &k in keys {
            if let Some(e) = self.resident.get_mut(&k) {
                e.1 = e.1.max(frame);
                continue;
            }
            ready = false;
            if self.making.insert(k) {
                let (source, made, words) = (source.clone(), self.made.clone(), self.words);
                rayon::spawn(move || {
                    let mut w = vec![0u32; words];
                    source.0.region(k, &mut w[..source.0.words()]);
                    made.lock().unwrap_or_else(|e| e.into_inner()).push((k, w));
                });
            }
        }
        ready
    }

    /// Regions made off the frame into the pool, as many as fit besides
    /// those `frame` keeps (the rest wait). Each frame admits before it
    /// asks which footprints' regions are resident: a footprint waiting on
    /// its regions is not reserved, so `reserve` alone would never admit
    /// the first ones.
    pub(crate) fn admit(&mut self, frame: u64) {
        let mut done = std::mem::take(&mut *self.made.lock().unwrap_or_else(|e| e.into_inner()));
        if done.is_empty() {
            return;
        }
        let fit = self.room(done.len(), frame);
        let rest = done.split_off(fit);
        for (k, w) in done {
            self.making.remove(&k);
            self.place(k, w, frame);
        }
        self.made
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(rest);
    }

    /// Upload the regions made and the table, if it changed (before the
    /// dispatches that read them).
    pub(crate) fn flush(&mut self, queue: &wgpu::Queue) {
        let base = 16 * (1 + self.entries as u64);
        for (slot, w) in self.pending.drain(..) {
            queue.write_buffer(
                &self.buffer,
                base + 4 * (u64::from(slot) * self.words as u64),
                bytemuck::cast_slice(&w),
            );
        }
        if !self.table_dirty || self.capacity == 0 {
            return;
        }
        let mask = self.entries - 1;
        let mut table = vec![[0u32; 4]; self.entries];
        let mut keys: Vec<_> = self.resident.iter().map(|(k, e)| (*k, e.0)).collect();
        keys.sort_unstable();
        for (k, slot) in keys {
            let mut h = region_hash(k) as usize & mask;
            for probe in 0.. {
                assert!(probe < REGION_PROBES, "region table clustered");
                if table[h][2] == 0 {
                    table[h] = [k[0], k[1], slot + 1, 0];
                    break;
                }
                h = (h + 1) & mask;
            }
        }
        queue.write_buffer(&self.buffer, 16, bytemuck::cast_slice(&table));
        self.table_dirty = false;
    }
}

/// Terrain constants padded to a whole uniform (16-byte multiple).
pub(crate) fn terrain_bytes(program: &TerrainProgram) -> Vec<u8> {
    let mut bytes = program.constants.clone();
    bytes.resize(bytes.len().max(16).next_multiple_of(16), 0);
    bytes
}

/// The chunk shaders ([`crate::chunks::shader_sources`]) for both world
/// forms, with and without 64-bit integers, with the Earth terrain program
/// (shader validation without a device).
pub fn validation_sources() -> Vec<(String, String)> {
    let grid = crate::grid::Grid::new(6_371_000.0, 0.1).expect("Earth grid");
    let field = crate::layers::TerrainLayers::earth()
        .field(&grid, 1)
        .expect("Earth terrain");
    let program = crate::terrain::TerrainField::program(&field);
    [(false, false), (true, false), (false, true), (true, true)]
        .into_iter()
        .flat_map(|(plane, wide64)| {
            let form = format!(
                "{}{}",
                if plane { "plane" } else { "sphere" },
                if wide64 { ", int64" } else { "" }
            );
            crate::chunks::shader_sources(plane, &program, wide64, wide64)
                .into_iter()
                .map(move |(label, source)| (format!("{label} ({form})"), source))
        })
        .collect()
}

/// The noise library with its wide region (`// wide:begin` .. `// wide:end`)
/// replaced by `wide64.wgsl`. The markers are matched without their line
/// ends: checkouts may convert them to CRLF.
fn hardware_wide(noise: &str) -> String {
    let (head, rest) = noise
        .split_once("// wide:begin")
        .expect("noise.wgsl wide region");
    let (_, tail) = rest
        .split_once("// wide:end")
        .expect("noise.wgsl wide region end");
    format!("{head}{}{tail}", include_str!("../shaders/wide64.wgsl"))
}

/// The terrain half of every shader: the noise library, world helpers, the
/// material state and the terrain program with defaults for the parts it
/// leaves out, the column functions ([`column_source`]; `generation`: the
/// generation module's) and the world shape. With `wide64` the
/// noise library's wide products use hardware 64-bit integers
/// (`wide64.wgsl`). The program reads the `world` and `terrain` uniforms
/// and the `terrain_tables` and `terrain_regions` storage, declared here
/// (group 0, bindings 1, 16, 40 and 41); the volume layer reads `volumes`
/// (binding 18).
pub(crate) fn terrain_source(
    plane: bool,
    program: &TerrainProgram,
    wide64: bool,
    generation: bool,
) -> String {
    let noise = helio_voxel_world::wgsl::NOISE;
    let mut s = if wide64 {
        hardware_wide(noise)
    } else {
        String::from(noise)
    };
    s.push_str(helio_voxel_world::wgsl::WORLD);
    s.push_str(include_str!("../shaders/material_state.wgsl"));
    s.push_str(&program.wgsl);
    if !program.wgsl.contains("fn terrain_climate") {
        // No climate (`TerrainField::climate`).
        s.push_str(&format!(
            "fn terrain_climate(p: vec3<i32>, height: i32) -> vec2<i32> {{ return vec2<i32>({}, {}); }}
",
            crate::terrain::DEFAULT_CLIMATE[0],
            crate::terrain::DEFAULT_CLIMATE[1]
        ));
    }
    if !program.wgsl.contains("fn terrain_erosion") {
        // No erosion (`TerrainField::erosion`).
        s.push_str("fn terrain_erosion(p: vec3<i32>) -> vec2<i32> { return vec2<i32>(0, i32(0x80000000u)); }
");
    }
    if !program.wgsl.contains("fn terrain_drainage") {
        // No drainage (`TerrainField::drainage`).
        s.push_str("fn terrain_drainage(p: vec3<i32>) -> vec4<i32> { return vec4<i32>(0, 0, 0x7fffffff, 0); }
");
    }
    if !program.wgsl.contains("fn terrain_cave_gate") {
        // Networks wherever their share allows (`TerrainField::cave_gate`).
        s.push_str("fn terrain_cave_gate(surface: u32, height: i32) -> i32 { return 256; }\n");
    }
    // The volume layer (`docs/volumes.md`), reading the `volumes` uniform.
    s.push_str(helio_voxel_world::wgsl::VOLUMES);
    s.push_str(&column_source(program, generation));
    s.push_str(
        "@group(0) @binding(1) var<uniform> world: World;
@group(0) @binding(16) var<uniform> terrain: TerrainConstants;
@group(0) @binding(40) var<storage, read> terrain_tables: array<vec4<u32>>;
@group(0) @binding(41) var<storage, read> terrain_regions: array<vec4<u32>>;
",
    );
    s.push_str(&format!(
        "// The program's regions (`engine::RegionPool`).
fn terrain_region_hash(key: vec2<u32>) -> u32 {{
    let h = (key.x * 0x9e3779b1u) ^ (key.y * 0x85ebca77u);
    return ((h ^ (h >> 15u)) * 0x2c1b3c6du) ^ (h >> 13u);
}}
fn terrain_region(key: vec2<u32>) -> i32 {{
    let head = terrain_regions[0];
    if head.y == 0u {{ return -1; }}
    var h = terrain_region_hash(key) & head.x;
    for (var probe = 0u; probe < {REGION_PROBES}u; probe++) {{
        let e = terrain_regions[1u + h];
        if e.z == 0u {{ return -1; }}
        if e.x == key.x && e.y == key.y {{ return i32(e.z) - 1; }}
        h = (h + 1u) & head.x;
    }}
    return -1;
}}
fn terrain_region_word(slot: u32, i: u32) -> u32 {{
    let head = terrain_regions[0];
    let at = head.w * 4u + slot * head.z + i;
    return terrain_regions[at >> 2u][at & 3u];
}}
"
    ));
    // World shape: 0 sphere, 1 plane.
    s.push_str(&format!("const SHAPE: u32 = {};\nconst PLANE_FACE: u32 = 2u;\nfn is_plane() -> bool {{ return SHAPE == 1u; }}\n", if plane { "1u" } else { "0u" }));
    s
}

/// Words of a node of `program`'s two-rate field (`TERRAIN_NODE_WORDS`;
/// 0 when the program does not define it).
pub(crate) fn node_words(program: &TerrainProgram) -> u32 {
    program
        .wgsl
        .split("const TERRAIN_NODE_WORDS: u32 = ")
        .nth(1)
        .and_then(|r| r.split('u').next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

/// Columns between a two-rate field's nodes (`landform::NODE_COLUMNS`), and
/// nodes along a tile's side in the tile node buffer: the tile's 5 and a
/// ring past them (columns within 16 of the tile read them too).
pub(crate) const NODE_COLUMNS: i32 = 16;
pub(crate) const TILE_NODES: u32 = 7;

/// Point-mode scratch nodes after a batch's tile nodes in `tile_nodes`: one
/// per lane of `tile_caves_main`'s `HALO_GROUPS` workgroups.
pub(crate) const HALO_GROUPS: u32 = 128;
pub(crate) const NODE_SCRATCH: u32 = HALO_GROUPS * 64;

/// The column functions, each one call site of the program (compilers
/// inline every call, and each copy of a large program is compile time):
/// `field_height(face, i, j, level)` and `generation_column(face, i, j,
/// level, display)` (height, surface word); with `generation`,
/// `generation_tile_column(tile, ..)` for a column of a batch's tile,
/// `generation_halo_column(tile, .., scratch)` for one near it,
/// `generation_tile_nodes` (`tile_nodes_main`) and node grids
/// (`generation_grid_nodes`, `generation_grid_column`).
///
/// The program (its two-rate column, `docs/two-rate-field.md`) reads its
/// macro octaves through `generation_node_word(slot)`: the bilinear blend
/// of the column's four nodes, `floor(sum w_c v_c / 256)` with the high
/// and low bytes summed apart (`landform::lerp4`). Generation reads a
/// batch's tiles' nodes from `tile_nodes`; a column out of its tile's
/// ring evaluates its four into function arrays and leaves the blend in a
/// scratch node. Module-scope arrays would not do: WGSL clears every
/// private variable an entry point can reach at each invocation, and the
/// hot kernels would clear them for nothing.
fn column_source(program: &TerrainProgram, generation: bool) -> String {
    assert!(
        node_words(program) > 0,
        "terrain programs define TERRAIN_NODE_WORDS (the two-rate column, `terrain`)"
    );
    let mut s = String::new();
    s.push_str(&format!(
        "const NODE_COLUMNS: i32 = {NODE_COLUMNS};
const TILE_NODES: u32 = {TILE_NODES}u;

// Bilinear weights (sum 256) of a column `d` columns past its first node.
fn gen_node_weights(d: vec2<i32>) -> vec4<i32> {{
    let u = vec2<i32>(NODE_COLUMNS) - d;
    return vec4<i32>(u.x * u.y, d.x * u.y, u.x * d.y, d.x * d.y);
}}
"
    ));
    // The four nodes evaluated and summed in `hi`/`lo` (then `blend`).
    let point = "    let first = vec2<i32>(i, j) & vec2<i32>(~(NODE_COLUMNS - 1));
    let w = gen_node_weights(vec2<i32>(i, j) - first);
    var hi: array<i32, TERRAIN_NODE_WORDS>;
    var lo: array<i32, TERRAIN_NODE_WORDS>;
    for (var c = 0u; c < 4u; c++) {
        let node = first + vec2<i32>(i32(c & 1u), i32(c >> 1u)) * NODE_COLUMNS;
        terrain_node(domain_point(face, node.x, node.y, level), level + u32(world.grid.w));
        for (var slot = 0u; slot < TERRAIN_NODE_WORDS; slot++) {
            let v = terrain_node_word(slot);
            hi[slot] += (v >> 8u) * w[c];
            lo[slot] += (v & 255) * w[c];
        }
    }
";
    if !generation {
        s.push_str(&format!(
            "// A column's blended node words (`gen_point_nodes`).
var<private> gen_node: array<i32, TERRAIN_NODE_WORDS>;

fn generation_node_word(slot: u32) -> i32 {{
    return gen_node[slot];
}}

fn gen_point_nodes(face: u32, i: i32, j: i32, level: u32) {{
{point}    for (var slot = 0u; slot < TERRAIN_NODE_WORDS; slot++) {{ gen_node[slot] = hi[slot] + (lo[slot] >> 8u); }}
}}

fn generation_column(face: u32, i: i32, j: i32, level: u32, display: bool) -> vec2<i32> {{
    if terrain_has_nodes(level + u32(world.grid.w)) {{ gen_point_nodes(face, i, j, level); }}
    return terrain_column_nodes(domain_point(face, i, j, level), level + u32(world.grid.w), display);
}}

fn field_height(face: u32, i: i32, j: i32, level: u32) -> i32 {{
    return generation_column(face, i, j, level, false).x;
}}
"
        ));
        return s;
    }
    let batch = crate::chunks::TILE_BATCH as u32 * TILE_NODES * TILE_NODES;
    s.push_str(&format!(
        "@group(0) @binding(25) var<storage, read_write> tile_nodes: array<i32>;
// The first scratch node (`NODE_SCRATCH` of them).
const NODE_SCRATCH_AT: u32 = {batch}u;
// The column's first node's first word in `tile_nodes`, the nodes in a
// row of its grid, and its weights.
var<private> gen_node_at: u32 = 0u;
var<private> gen_node_row: u32 = 0u;
var<private> gen_node_w: vec4<i32> = vec4<i32>(0);

fn generation_node_word(slot: u32) -> i32 {{
    let s = TERRAIN_NODE_WORDS;
    let a = gen_node_at + slot;
    let v = vec4<i32>(tile_nodes[a], tile_nodes[a + s], tile_nodes[a + gen_node_row * s], tile_nodes[a + (gen_node_row + 1u) * s]);
    let hi = (v >> vec4<u32>(8u)) * gen_node_w;
    let lo = (v & vec4<i32>(255)) * gen_node_w;
    return hi.x + hi.y + hi.z + hi.w + ((lo.x + lo.y + lo.z + lo.w) >> 8u);
}}

// The column's nodes from tile `tile`'s, when there: a column of the
// tile's level, its first node within the tile's ring.
fn gen_tile_nodes(tile: u32, i: i32, j: i32, level: u32) -> bool {{
    let job = tile_jobs[tile];
    if job.level != level {{ return false; }}
    let first = vec2<i32>(i, j) & vec2<i32>(~(NODE_COLUMNS - 1));
    let n = (first - vec2<i32>(job.cx, job.cy) * 64) / NODE_COLUMNS + 1;
    if any(n < vec2<i32>(0)) || any(n >= vec2<i32>(i32(TILE_NODES) - 1)) {{ return false; }}
    gen_node_at = (tile * TILE_NODES * TILE_NODES + u32(n.y) * TILE_NODES + u32(n.x)) * TERRAIN_NODE_WORDS;
    gen_node_row = TILE_NODES;
    gen_node_w = gen_node_weights(vec2<i32>(i, j) - first);
    return true;
}}

// The column's four nodes evaluated, their blend in scratch node `scratch`.
fn gen_point_nodes(face: u32, i: i32, j: i32, level: u32, scratch: u32) {{
{point}    let at = (NODE_SCRATCH_AT + scratch) * TERRAIN_NODE_WORDS;
    for (var slot = 0u; slot < TERRAIN_NODE_WORDS; slot++) {{ tile_nodes[at + slot] = hi[slot] + (lo[slot] >> 8u); }}
    gen_node_at = at;
    gen_node_w = vec4<i32>(256, 0, 0, 0);
}}

// A column of tile `tile` (within its ring).
fn generation_tile_column(tile: u32, face: u32, i: i32, j: i32, level: u32, display: bool) -> vec2<i32> {{
    if terrain_has_nodes(level + u32(world.grid.w)) {{ _ = gen_tile_nodes(tile, i, j, level); }}
    return terrain_column_nodes(domain_point(face, i, j, level), level + u32(world.grid.w), display);
}}

// A column near tile `tile` (or of another level: features read level-0
// ground): its nodes from the tile's when there, else its own four
// (scratch node `scratch`, one per lane).
fn generation_halo_column(tile: u32, face: u32, i: i32, j: i32, level: u32, display: bool, scratch: u32) -> vec2<i32> {{
    if terrain_has_nodes(level + u32(world.grid.w)) && !gen_tile_nodes(tile, i, j, level) {{ gen_point_nodes(face, i, j, level, scratch); }}
    return terrain_column_nodes(domain_point(face, i, j, level), level + u32(world.grid.w), display);
}}

// Lane `lane`'s node of tile `tile` (cell `(ci, cj)` of `level`): the
// tile's 7x7, from one node spacing before its first column.
fn generation_tile_nodes(tile: u32, face: u32, ci: i32, cj: i32, level: u32, lane: u32) {{
    if lane >= TILE_NODES * TILE_NODES || !terrain_has_nodes(level + u32(world.grid.w)) {{ return; }}
    let n = vec2<i32>(i32(lane % TILE_NODES), i32(lane / TILE_NODES));
    let at = vec2<i32>(ci, cj) * 64 + (n - 1) * NODE_COLUMNS;
    terrain_node(domain_point(face, at.x, at.y, level), level + u32(world.grid.w));
    let base = (tile * TILE_NODES * TILE_NODES + lane) * TERRAIN_NODE_WORDS;
    for (var slot = 0u; slot < TERRAIN_NODE_WORDS; slot++) {{ tile_nodes[base + slot] = terrain_node_word(slot); }}
}}

// Node grids of a level: `side` x `side` nodes from node-aligned column
// `origin`, node (x, y) at node slot `base + y row + x` of `tile_nodes`:
// the material slopes' rings (`tile_slopes_main`: a ring a footprint of
// the batch after the scratch nodes, at most 5x5 in rows of TILE_NODES)
// and the features' candidate block jobs' level-0 node quads after the
// rings (`feature_block_nodes_main`: 2x2 grids, `BLOCK_READS` a job).
const LATTICE_AT: u32 = NODE_SCRATCH_AT + {scratch}u;
const FEATURE_LATTICE_AT: u32 = LATTICE_AT + {rings}u;

// Node `n` of a grid.
fn generation_grid_nodes(base: u32, row: u32, face: u32, origin: vec2<i32>, side: u32, level: u32, n: u32) {{
    if n >= side * side || !terrain_has_nodes(level + u32(world.grid.w)) {{ return; }}
    let c = vec2<u32>(n % side, n / side);
    let at = origin + vec2<i32>(c) * NODE_COLUMNS;
    terrain_node(domain_point(face, at.x, at.y, level), level + u32(world.grid.w));
    let slot = (base + c.y * row + c.x) * TERRAIN_NODE_WORDS;
    for (var w = 0u; w < TERRAIN_NODE_WORDS; w++) {{ tile_nodes[slot + w] = terrain_node_word(w); }}
}}

// A column of a grid (its first node and the one past it inside the grid).
fn generation_grid_column(base: u32, row: u32, origin: vec2<i32>, face: u32, i: i32, j: i32, level: u32, display: bool) -> vec2<i32> {{
    if terrain_has_nodes(level + u32(world.grid.w)) {{
        let first = vec2<i32>(i, j) & vec2<i32>(~(NODE_COLUMNS - 1));
        let n = vec2<u32>((first - origin) / NODE_COLUMNS);
        gen_node_at = (base + n.y * row + n.x) * TERRAIN_NODE_WORDS;
        gen_node_row = row;
        gen_node_w = gen_node_weights(vec2<i32>(i, j) - first);
    }}
    return terrain_column_nodes(domain_point(face, i, j, level), level + u32(world.grid.w), display);
}}
",
        scratch = NODE_SCRATCH,
        rings = batch
    ));
    s
}

/// Compare a world's terrain program on the GPU with its CPU field at
/// `samples` pseudo-random columns (every level, face edges included) and
/// ground-material inputs, and its volume layer (`docs/volumes.md`) at
/// cells near the surface, in cave networks and on cliffs. Generator
/// authors run this in their tests; it returns the first disagreement.
pub fn verify_field(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    planet: &Planet,
    samples: u32,
) -> Result<(), String> {
    let grid = *planet.grid();
    let field = planet.field();
    let mut rng = 0x1234_5678u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let volumes = planet.volumes();
    // The volume's draws come from a sequence of their own, so the
    // columns are the same as without volumes.
    let mut vrng = 0x9E37_79B9_7F4A_7C15u64;
    let mut vnext = || {
        vrng ^= vrng << 13;
        vrng ^= vrng >> 7;
        vrng ^= vrng << 17;
        vrng
    };
    let mut columns = Vec::new();
    let mut volume = Vec::new();
    for s in 0..samples {
        let level = (next() % u64::from(grid.levels())) as u32;
        let cells = (grid.cells() >> level).max(1) as u64;
        let face = if grid.is_plane() {
            i32::from(crate::grid::PLANE_FACE)
        } else {
            (next() % 6) as i32
        };
        let (i, j) = if s % 4 == 0 {
            (
                (next() % 2) as i32 * (cells as i32 - 1),
                (next() % cells) as i32,
            )
        } else {
            ((next() % cells) as i32, (next() % cells) as i32)
        };
        // A layer near the top, and a slope for the cliff shapes (eighths).
        let column = crate::terrain::Column {
            face: face as u8,
            i,
            j,
            level,
        };
        let top = crate::terrain::top_cells(&grid, field.height(column), level);
        columns.push([face, i, j, level as i32]);
        volume.push([(vnext() % 64) as i32, top + (vnext() % 81) as i32 - 40]);
        // Every third sample, a cell of a cave primitive as well, at a
        // level that resolves it.
        let Some(v) = volumes.filter(|v| v.caves() && s % 3 == 1) else {
            continue;
        };
        let regions = (grid.cells() >> v.region_shift()).max(1) as u64;
        for _ in 0..16 {
            let (ri, rj) = ((vnext() % regions) as i32, (vnext() % regions) as i32);
            let network = planet.cave_network(face as u8, ri, rj);
            if network.is_empty() {
                continue;
            }
            let p = network[(vnext() % network.len() as u64) as usize];
            let deepest = v
                .deepest_level(p.flags & crate::volumes::CHAMBER != 0)
                .max(0) as u64;
            let level = (vnext() % (deepest + 1)) as u32;
            let t = (vnext() % 17) as i64;
            let at = |n: usize| p.a[n] + (p.b[n] - p.a[n]) * t / 16;
            let reach = i64::from(p.r) * ((vnext() % 5) as i64 - 2) / 2;
            let spread = i64::from(p.rv) * ((vnext() % 7) as i64 - 3) / 2;
            columns.push([
                face,
                ((at(0) + reach) >> 2 >> level) as i32,
                ((at(1) - reach) >> 2 >> level) as i32,
                level as i32,
            ]);
            volume.push([
                (vnext() % 64) as i32,
                ((at(2) + spread) >> 2 >> level) as i32,
            ]);
            break;
        }
    }
    verify_cells(device, queue, planet, &columns, &volume)
}

/// [`verify_field`] at the given columns (`[face, i, j, level]`), each at
/// a cell near its top.
pub fn verify_columns(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    planet: &Planet,
    columns: &[[i32; 4]],
) -> Result<(), String> {
    let grid = *planet.grid();
    let field = planet.field();
    let mut rng = 0x5bd1_e995u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let volume: Vec<[i32; 2]> = columns
        .iter()
        .map(|c| {
            let level = c[3] as u32;
            let column = crate::terrain::Column {
                face: c[0] as u8,
                i: c[1],
                j: c[2],
                level,
            };
            let top = crate::terrain::top_cells(&grid, field.height(column), level);
            [(next() % 64) as i32, top + (next() % 81) as i32 - 40]
        })
        .collect();
    verify_cells(device, queue, planet, columns, &volume)
}

/// Columns `columns` (`[face, i, j, level]`) and a cell of each (`volume`:
/// `[slope, k]`) on the GPU and the CPU, with ground-material inputs drawn
/// here.
fn verify_cells(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    planet: &Planet,
    columns: &[[i32; 4]],
    volume: &[[i32; 2]],
) -> Result<(), String> {
    let grid = *planet.grid();
    let field = planet.field();
    let program = field.program();
    let mut rng = 0x9e37_79b9u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let (lo, hi) = field.height_range();
    let span = (i64::from(hi) - i64::from(lo) + 1).max(1) as u64;
    let inputs: Vec<IVec4> = columns.iter().map(|c| IVec4::from_array(*c)).collect();
    let extra: Vec<IVec4> = columns
        .iter()
        .map(|_| {
            IVec4::new(
                (i64::from(lo) + (next() % span) as i64) as i32,
                (next() % 40) as i32,
                (next() % 40) as i32,
                (next() % 200_000) as i32 - 100_000,
            )
        })
        .collect();
    let volume: Vec<IVec4> = volume
        .iter()
        .map(|v| IVec4::new(v[0], v[1], 0, 0))
        .collect();
    let samples = inputs.len() as u32;
    let module = helio_core::shader::module(
        device,
        "terrain verification",
        &verify_source(
            grid.is_plane(),
            &program,
            device.features().contains(wgpu::Features::SHADER_INT64),
        ),
    );
    let pipeline = helio_core::shader::create_compute_pipeline(
        &device,
        wgpu::ComputePipelineDescriptor {
            label: Some("terrain verification"),
            layout: None,
            module: &module,
            entry_point: Some("verify"),
            compilation_options: Default::default(),
            cache: None,
        },
    );
    let init = |label, contents: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    };
    let world = init(
        "verify world",
        bytemuck::bytes_of(&WorldGpu::new(planet)),
        wgpu::BufferUsages::UNIFORM,
    );
    let terrain = init(
        "verify terrain",
        &terrain_bytes(&program),
        wgpu::BufferUsages::UNIFORM,
    );
    let volumes_uniform = init(
        "verify volumes",
        bytemuck::bytes_of(&volume_constants(planet)),
        wgpu::BufferUsages::UNIFORM,
    );
    let tables = tables_buffer(device, &program);
    // Every region the columns read, resident at once: theirs and their
    // cave networks' reference ground.
    let keys: Vec<[u32; 2]> = {
        let probe = RegionPool::new(device, &program, 0);
        let mut keys = Vec::new();
        for c in columns {
            let (face, level) = (c[0] as u8, c[3] as u32);
            keys.extend(probe.keys(&grid, face, level, [c[1], c[1]], [c[2], c[2]]));
            let base = |a: i32| [a << level, ((a + 1) << level) - 1];
            if let Some(v) = planet.volumes() {
                if let Some((i, j)) = v.ground_columns(level, base(c[1]), base(c[2])) {
                    keys.extend(probe.keys(&grid, face, v.reference_level(), i, j));
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    };
    let mut regions = RegionPool::new(device, &program, keys.len());
    regions.reserve_now(&keys, 0);
    regions.flush(queue);
    let ins = init(
        "verify inputs",
        bytemuck::cast_slice(&inputs),
        wgpu::BufferUsages::STORAGE,
    );
    let ext = init(
        "verify extra",
        bytemuck::cast_slice(&extra),
        wgpu::BufferUsages::STORAGE,
    );
    let vol = init(
        "verify volume",
        bytemuck::cast_slice(&volume),
        wgpu::BufferUsages::STORAGE,
    );
    let bytes = u64::from(samples) * 16;
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("verify out"),
        size: bytes.max(8),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("verify readback"),
        size: bytes.max(8),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    fn binding(binding: u32, b: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
        wgpu::BindGroupEntry {
            binding,
            resource: b.as_entire_binding(),
        }
    }
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain verification"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            binding(1, &world),
            binding(16, &terrain),
            binding(18, &volumes_uniform),
            binding(20, &ins),
            binding(21, &ext),
            binding(22, &out),
            binding(23, &vol),
            binding(40, &tables),
            binding(41, regions.buffer()),
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(samples.div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out, 0, &read, 0, bytes.max(8));
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    read.slice(..)
        .map_async(wgpu::MapMode::Read, move |r| drop(tx.send(r)));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let data = read
        .slice(..)
        .get_mapped_range()
        .map_err(|e| e.to_string())?;
    let gpu: &[[i32; 4]] = bytemuck::cast_slice(&data[..bytes as usize]);
    for (((a, e), v), g) in inputs.iter().zip(&extra).zip(&volume).zip(gpu) {
        let level = a.w as u32;
        let face = a.x as u8;
        let p = grid.domain_point(face, a.y, a.z, level);
        let column = crate::terrain::Column {
            face,
            i: a.y,
            j: a.z,
            level,
        };
        let height = field.height(column);
        let q = grid.volume_point(face, a.y, a.z, v.y, level);
        let surface = field.surface(column, height) & 0xff;
        let cell = verify_cell(planet, face, a.y, a.z, level, v.x, v.y, height);
        let climate = field.climate(q, e.x);
        let worn = field.erosion(q);
        let drains = field.drainage(q);
        let cpu = [
            height,
            field.ground_material(p, surface, e.x, e.y, e.z, e.w) as i32,
            cell[0] + cell[1] * 2 + cell[2] * 64,
            (q.x ^ q.y ^ q.z)
                .wrapping_add(surface as i32 * 7919)
                .wrapping_add(climate.0.wrapping_mul(524_287))
                .wrapping_add(climate.1.wrapping_mul(8_191))
                .wrapping_add(worn.0.wrapping_mul(3))
                .wrapping_add(worn.1.wrapping_mul(5))
                .wrapping_add(drains[0].wrapping_mul(17))
                .wrapping_add(drains[1].wrapping_mul(19))
                .wrapping_add(drains[2].wrapping_mul(23))
                .wrapping_add(drains[3].wrapping_mul(29)),
        ];
        if *g != cpu {
            return Err(format!(
                "column {a} with inputs {e}, volume {v}: GPU {g:?}, CPU {cpu:?}"
            ));
        }
    }
    Ok(())
}

/// The terrain verification shader of [`verify_field`]: the terrain half
/// and a kernel comparing columns, volume cells and the climate, erosion
/// and drainage at them.
pub(crate) fn verify_source(plane: bool, program: &TerrainProgram, wide64: bool) -> String {
    let kernel = "
@group(0) @binding(20) var<storage, read> verify_in: array<vec4<i32>>;
@group(0) @binding(21) var<storage, read> verify_extra: array<vec4<i32>>;
@group(0) @binding(22) var<storage, read_write> verify_out: array<vec4<i32>>;
@group(0) @binding(23) var<storage, read> verify_volume: array<vec4<i32>>;

// Cell `k` of a column as the volume layer makes it (`Planet::column_kind`
// with the given slope): solid, the primitives over it, the cliff's rise.
fn verify_cell(face: u32, i: i32, j: i32, level: u32, slope: i32, k: i32, height: i32, p: vec3<i32>) -> vec3<i32> {
    let top = top_cells(height, level);
    let cliff = volume_cliff(p, slope, level);
    var solid = k < top;
    if cliff.rise > 0 && k >= top - cliff.cells && k < top + cliff.cells {
        solid = volume_cliff_solid(cliff, volume_point(face, i, j, k, level), height, k, level);
    }
    var covering = 0;
    if volumes_caves() && i32(level) <= cave_deepest_level(true) {
        let half = (1 << level) >> 1u;
        let base = vec2<i32>((i << level) + half, (j << level) + half);
        let origin = vec2<i32>(i << level, j << level);
        let x = 2 << level;
        let reference = u32(volumes.head.z);
        let r = base >> vec2<u32>(volume_region_shift());
        for (var o = 0; o < 3; o++) {
            let region = r - vec2<i32>(select(0, 1, o == 1), select(0, 1, o == 2));
            var ground: array<vec2<i32>, 6>;
            for (var s = 0u; s < VOLUME_SITES; s++) {
                let site = volume_site(face, region.x, region.y, s);
                if site.used {
                    let g = generation_column(face, site.x >> reference, site.y >> reference, reference, reference >= 1u);
                    ground[s] = vec2<i32>(g.x, terrain_cave_gate(u32(g.y) & 0xffu, g.x));
                }
            }
            let net = volume_network(face, region.x, region.y, ground, origin);
            for (var n = 0u; n < net.count; n++) {
                let c = net.prims[n];
                if cave_resolved(c, level) && cave_covers(c, x, x) {
                    covering += 1;
                    if solid && volume_carves(c, x, x, origin, k, level, height) { solid = false; }
                }
            }
        }
    }
    return vec3<i32>(i32(solid), covering, cliff.rise);
}

@compute @workgroup_size(64) fn verify(@builtin(global_invocation_id) id: vec3<u32>) {
    // (The tables and regions stay bound for programs that read neither.)
    if id.x >= arrayLength(&verify_in) || arrayLength(&terrain_tables) == 0u || arrayLength(&terrain_regions) == 0u { return; }
    let a = verify_in[id.x];
    let e = verify_extra[id.x];
    let v = verify_volume[id.x];
    let level = u32(a.w);
    let p = domain_point(u32(a.x), a.y, a.z, level);
    let column = generation_column(u32(a.x), a.y, a.z, level, false);
    let height = column.x;
    let q = volume_point(u32(a.x), a.y, a.z, v.y, level);
    let cell = verify_cell(u32(a.x), a.y, a.z, level, v.x, v.y, height, p);
    let surface = u32(column.y) & 0xffu;
    let climate = terrain_climate(q, e.x);
    let worn = terrain_erosion(q);
    let drains = terrain_drainage(q);
    verify_out[id.x] = vec4<i32>(height, i32(ground_material(p, surface, e.x, e.y, e.z, e.w)),
        cell.x + cell.y * 2 + cell.z * 64, (q.x ^ q.y ^ q.z) + i32(surface) * 7919
            + climate.x * 524287 + climate.y * 8191
            + worn.x * 3 + worn.y * 5 + drains.x * 17 + drains.y * 19 + drains.z * 23 + drains.w * 29);
}
";
    terrain_source(plane, program, wide64, false) + kernel
}

/// [`verify_field`]'s cell: solid, the primitives over it, the cliff's rise.
#[allow(clippy::too_many_arguments)]
fn verify_cell(
    planet: &Planet,
    face: u8,
    i: i32,
    j: i32,
    level: u32,
    slope: i32,
    k: i32,
    height: i32,
) -> [i32; 3] {
    let grid = planet.grid();
    let top = crate::terrain::top_cells(grid, height, level);
    let Some(v) = planet.volumes() else {
        return [i32::from(k < top), 0, 0];
    };
    let cliff = v.cliff(grid.domain_point(face, i, j, level), slope, level);
    let mut solid = k < top;
    if cliff.rise > 0 && k >= top - cliff.cells && k < top + cliff.cells {
        solid = v.cliff_solid(
            &cliff,
            grid.volume_point(face, i, j, k, level),
            height,
            k,
            level,
        );
    }
    let mut covering = 0;
    if v.caves() && level as i32 <= v.deepest_level(true) {
        let half = (1 << level) >> 1;
        let (x, y) = (
            crate::volumes::q2_center(i, level),
            crate::volumes::q2_center(j, level),
        );
        for (ri, rj) in v.owners((i << level) + half, (j << level) + half) {
            for p in planet.cave_network(face, ri, rj).iter() {
                if v.resolved(p, level) && p.covers(x, y, v.roughness()) {
                    covering += 1;
                    if solid && v.carves(p, x, y, k, level, height) {
                        solid = false;
                    }
                }
            }
        }
    }
    [i32::from(solid), covering, cliff.rise]
}

/// A planet's volume constants (`volumes` uniform; zero without volumes).
pub(crate) fn volume_constants(planet: &Planet) -> crate::volumes::VolumeConstants {
    planet.volumes().map(|v| v.constants()).unwrap_or_default()
}

#[cfg(test)]
mod wide_tests {
    #[test]
    fn hardware_wide_replaces_the_region_with_either_line_end() {
        let noise = helio_voxel_world::wgsl::NOISE;
        for text in [
            noise.replace("\r\n", "\n"),
            noise.replace("\r\n", "\n").replace('\n', "\r\n"),
        ] {
            let s = super::hardware_wide(&text);
            assert!(s.contains("u64(a) * u64(b)") && !s.contains("mid_carry"));
        }
    }
}
