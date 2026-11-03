//! A [`ChunkWorld`]'s volumes (`docs/chunk-octree.md#volumes`): each placed
//! world's generation (its terrain program's pipelines, footprint pool,
//! features and edits), residency state and draw parameters, beside the
//! chunk pool, planner and tracer they share; and drawing's layouts and
//! pipelines, built for each world form a volume brings.

use super::*;
use crate::volume::VolumeTransform;

/// A volume of a [`ChunkWorld`]: its index, the `volume` of its node keys
/// (`NodeKey::table_key` keeps it in bits 16 and up of the chunk table's
/// keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VolumeId(pub u16);

/// The most volumes a world holds at once (chunk keys keep 15 bits of it).
pub const MAX_VOLUMES: usize = 4096;

/// Generation pipelines of one terrain program and world form
/// (`generation_source`), shared by the volumes that have them.
pub(super) struct GenPipelines {
    pub tile_nodes: wgpu::ComputePipeline,
    pub tile_columns: wgpu::ComputePipeline,
    pub tile_caves: wgpu::ComputePipeline,
    pub tile_volumes: wgpu::ComputePipeline,
    pub tile_slope_nodes: wgpu::ComputePipeline,
    pub tile_slope_heights: wgpu::ComputePipeline,
    pub tile_slopes: wgpu::ComputePipeline,
    pub feature_block_quads: wgpu::ComputePipeline,
    pub feature_block_nodes: wgpu::ComputePipeline,
    pub feature_block_candidates: wgpu::ComputePipeline,
    pub tile_features: wgpu::ComputePipeline,
    pub chunk_columns: wgpu::ComputePipeline,
    pub chunk_cells: wgpu::ComputePipeline,
    pub chunk_build: wgpu::ComputePipeline,
    pub evict: wgpu::ComputePipeline,
    /// Kept chunks: packing evicted edited chunks, restoring them.
    pub pack: wgpu::ComputePipeline,
    pub restore: wgpu::ComputePipeline,
}

impl GenPipelines {
    fn new(
        device: &wgpu::Device,
        source: &str,
        gen_layout: &wgpu::PipelineLayout,
        keep_layout: &wgpu::PipelineLayout,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk generation"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let compute = |layout: &wgpu::PipelineLayout, entry: &str| {
            helio_core::shader::create_compute_pipeline(
                device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(entry),
                    layout: Some(layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        Self {
            tile_nodes: compute(gen_layout, "tile_nodes_main"),
            tile_columns: compute(gen_layout, "tile_columns_main"),
            tile_caves: compute(gen_layout, "tile_caves_main"),
            tile_volumes: compute(gen_layout, "tile_volumes_main"),
            tile_slope_nodes: compute(gen_layout, "tile_slope_nodes_main"),
            tile_slope_heights: compute(gen_layout, "tile_slope_heights_main"),
            tile_slopes: compute(gen_layout, "tile_slopes_main"),
            feature_block_quads: compute(gen_layout, "feature_block_quads_main"),
            feature_block_nodes: compute(gen_layout, "feature_block_nodes_main"),
            feature_block_candidates: compute(gen_layout, "feature_block_candidates_main"),
            tile_features: compute(gen_layout, "tile_features_main"),
            chunk_columns: compute(gen_layout, "chunk_columns_main"),
            chunk_cells: compute(gen_layout, "chunk_cells_main"),
            chunk_build: compute(gen_layout, "chunk_build_main"),
            evict: compute(gen_layout, "evict"),
            pack: compute(keep_layout, "pack_main"),
            restore: compute(keep_layout, "restore_main"),
        }
    }
}

/// A volume's camera and sun views' parameters (`DrawParams`): each its
/// own buffer, as a frame's writes all land before its passes run.
pub(super) struct VolumeViews {
    pub camera: wgpu::Buffer,
    pub sun: wgpu::Buffer,
    /// The camera's view again for the passes in which it lights another
    /// volume's points (`ChunkWorld::draw`: shadows across volumes over its
    /// shadow's rectangle, GI rays, foliage probes).
    pub cross: wgpu::Buffer,
}

/// One placed world of a [`ChunkWorld`].
pub(super) struct Volume {
    /// The host's name for it (`PickHit::volume`).
    pub tag: u64,
    pub index: u16,
    pub transform: VolumeTransform,
    /// The transform each eye's last draw placed it with (its hits' motion
    /// from there), and where the eye was in its frame at the last update,
    /// with the transform then and when (its motion relative to the eye).
    pub drawn_transform: [Option<VolumeTransform>; 2],
    pub last_local_eye: Option<(DVec3, VolumeTransform, std::time::Instant)>,
    pub grid: Grid,
    /// Its solid cells' bounding radius about its origin (none: unbounded).
    pub bounds: Option<f64>,
    /// Base cells per face edge, the first exact level ([`exact_level`]).
    pub face_cells: u32,
    pub exact_level: u32,
    /// Height over the datum above which nothing is solid (terrain,
    /// overhangs, edits), with a margin: rays rising past it end.
    pub ceiling: f64,
    /// Height over the datum below which nothing is seen (caves, digs),
    /// less a margin: where the sun's occlusion views end.
    pub floor: f64,
    /// What Solid and Buried nodes not added by a brush show where a ray
    /// meets them (`Planet::rock_material`).
    pub rock: u8,
    /// The world's height range in base layers (the roots' estimate).
    pub height_layers: (i32, i32),
    /// What its packs depend on (`kept_chunks`).
    pub fingerprint: u64,
    pub gen: std::sync::Arc<GenPipelines>,
    pub world: wgpu::Buffer,
    pub terrain: wgpu::Buffer,
    /// Its terrain program's tables (`terrain_tables`: its planet's maps)
    /// and regions (`terrain_regions`: its streams), kept resident for the
    /// footprints it generates, and each footprint's region keys.
    pub terrain_tables: wgpu::Buffer,
    pub regions: crate::engine::RegionPool,
    pub region_key_cache: FxHashMap<Footprint, std::sync::Arc<[[u32; 2]]>>,
    /// Its planet's volume layer (whose cave networks read ground columns
    /// beside a footprint: `region_keys`).
    pub volume_layer: Option<crate::volumes::VolumeLayer>,
    /// The volume layer's constants (`crate::volumes::VolumeConstants`).
    pub volumes: wgpu::Buffer,
    pub materials: wgpu::Buffer,
    pub views: VolumeViews,
    /// Its footprint pool: columns, cave primitives
    /// (`crate::volumes::CAVE_SLOT_WORDS` a footprint), material slopes,
    /// feature placements and candidate blocks.
    pub tile_data: wgpu::Buffer,
    pub tile_cave_data: wgpu::Buffer,
    pub tile_slope_data: wgpu::Buffer,
    pub feature_set: wgpu::Buffer,
    pub tile_placements: wgpu::Buffer,
    pub feature_blocks: wgpu::Buffer,
    pub blocks: candidates::CandidateBlocks,
    pub features: crate::features::CompiledFeatures,
    pub has_features: bool,
    pub pool: SlotPool<Footprint>,
    pub pool_measured: Vec<Option<Quarters>>,
    /// The footprints its pool holds at most, and each one's placement
    /// bytes.
    pub pool_max: usize,
    pub placement_stride: u64,
    /// The edits classification and generation read, shared with the
    /// planner; the version it has not seen yet; the brushes' buffer and
    /// the faces it has room for; baked bricks on the GPU; which edits.
    pub edits: std::sync::Arc<EditView>,
    pub edits_unsent: Option<std::sync::Arc<EditView>>,
    pub brushes: wgpu::Buffer,
    pub brush_capacity: usize,
    pub brick_cache: BrickCache,
    pub edit_state: EditState,
    /// Nodes to generate again: under brushes that came or went (first,
    /// coarsest first), drawn ones the frame they change; under baked
    /// edits that changed, as budget allows.
    pub urgent: FxHashSet<NodeKey>,
    /// Urgent nodes' distances from the eye when first ranked.
    pub urgent_distance: FxHashMap<NodeKey, f64>,
    pub stale: FxHashSet<NodeKey>,
    /// Events for its next plan, its latest plan and what it was made for.
    pub events: Vec<residency::Event>,
    pub plan: residency::Plan,
    pub planned_for: Option<residency::PlannedFor>,
    /// Nodes its plan wants and does not have (settling), and whether
    /// anything structural happened since it was planned.
    pub last_requests: usize,
}

impl Volume {
    /// The eye's position in the volume's frame.
    pub fn local_eye(&self, eye: DVec3) -> DVec3 {
        self.transform.to_local(eye)
    }

    /// Whether anything waits to be generated for it.
    pub fn wants_generation(&self) -> bool {
        !self.urgent.is_empty() || !self.stale.is_empty() || !self.plan.requests.is_empty()
    }

    /// The terrain regions footprint `fp`'s generation reads (near
    /// levels' feature candidates read the ground at level 0, cave networks
    /// their reference ground beside it), kept.
    pub fn region_keys(&mut self, fp: Footprint) -> std::sync::Arc<[[u32; 2]]> {
        if let Some(keys) = self.region_key_cache.get(&fp) {
            return keys.clone();
        }
        if self.region_key_cache.len() >= 1 << 16 {
            self.region_key_cache.clear();
        }
        let read = if self.has_features
            && fp.level < crate::features::NEAR_LEVELS
            && self.features.near(fp.level) > 0
        {
            0
        } else {
            fp.level
        };
        let up = fp.level - read;
        let mut keys = self.regions.keys(
            &self.grid,
            fp.face,
            read,
            [fp.cx * CHUNK << up, ((fp.cx * CHUNK + CHUNK) << up) - 1],
            [fp.cy * CHUNK << up, ((fp.cy * CHUNK + CHUNK) << up) - 1],
        );
        // Its cave networks' reference ground (`tile_caves_main`).
        let base = |c: i32| [c * CHUNK << fp.level, ((c * CHUNK + CHUNK) << fp.level) - 1];
        let ground = self.volume_layer.as_ref().and_then(|v| {
            let (i, j) = v.ground_columns(fp.level, base(fp.cx), base(fp.cy))?;
            Some((v.reference_level(), i, j))
        });
        if let Some((reference, i, j)) = ground {
            keys.extend(self.regions.keys(&self.grid, fp.face, reference, i, j));
            keys.sort_unstable();
            keys.dedup();
        }
        let keys: std::sync::Arc<[[u32; 2]]> = keys.into();
        self.region_key_cache.insert(fp, keys.clone());
        keys
    }
}

/// Drawing's bind group layouts, the same for every world form.
pub(super) struct DrawLayouts {
    pub draw: wgpu::BindGroupLayout,
    pub trace: wgpu::BindGroupLayout,
    pub seed: wgpu::BindGroupLayout,
    pub resolve: wgpu::BindGroupLayout,
    /// The cull writes the draw list the draw layout reads: its own layouts.
    pub cull: [wgpu::BindGroupLayout; 2],
    pub shadow_sample: wgpu::BindGroupLayout,
    pub sky_sample: wgpu::BindGroupLayout,
    pub shadow: wgpu::BindGroupLayout,
    pub shadow_fallback: wgpu::BindGroupLayout,
    pub local: wgpu::BindGroupLayout,
    pub temporal: wgpu::BindGroupLayout,
    pub filter: wgpu::BindGroupLayout,
    pub resolve_shadows: wgpu::BindGroupLayout,
    pub local_denoise: [wgpu::BindGroupLayout; 3],
    pub gi: wgpu::BindGroupLayout,
    pub ground: wgpu::BindGroupLayout,
    pub feedback: wgpu::BindGroupLayout,
}

impl DrawLayouts {
    pub fn new(device: &wgpu::Device) -> Self {
        let c = wgpu::ShaderStages::COMPUTE;
        let all =
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
        let draw = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk draw"),
            entries: &[
                uniform(0, all),
                storage(1, true, all),
                storage(2, true, all),
                storage(3, true, all),
                storage(4, true, all),
                storage(5, true, all),
                storage(6, true, all),
                uniform(7, all),
                storage(8, true, all),
                storage(9, false, c),
                storage(10, true, all),
                storage(11, true, all),
                storage(12, true, all),
                storage(13, false, c),
                storage(14, true, all),
                storage(15, true, all),
                storage(16, true, all),
                storage(17, true, all),
                storage(18, true, all),
                storage(19, true, all),
            ],
        });
        let trace = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk trace"),
            entries: &[
                texture_entry(0, wgpu::TextureSampleType::Uint, c),
                storage_texture(1, wgpu::TextureFormat::Rg32Uint),
                texture_entry(25, wgpu::TextureSampleType::Float { filterable: true }, c),
                sampler_entry(26, c),
                storage_texture(2, wgpu::TextureFormat::Rgba8Unorm),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: c,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadWrite,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: c,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::ReadWrite,
                        format: wgpu::TextureFormat::R32Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                // The meshes' depth: rays end at it.
                texture_entry(27, wgpu::TextureSampleType::Depth, c),
            ],
        });
        let seed = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk seed exits"),
            entries: &[
                texture_entry(
                    5,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::ShaderStages::FRAGMENT,
                ),
                texture_entry(
                    4,
                    wgpu::TextureSampleType::Float { filterable: false },
                    wgpu::ShaderStages::FRAGMENT,
                ),
                storage(22, true, wgpu::ShaderStages::VERTEX),
                // The meshes' depth: boxes behind it are marked hidden.
                texture_entry(
                    27,
                    wgpu::TextureSampleType::Depth,
                    wgpu::ShaderStages::FRAGMENT,
                ),
            ],
        });
        let cull = [
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk cull frames"),
                entries: &[
                    uniform(0, c),
                    storage(1, true, c),
                    storage(2, true, c),
                    storage(11, true, c),
                ],
            }),
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk cull"),
                entries: &[
                    storage(17, true, c),
                    storage(18, false, c),
                    storage(19, false, c),
                    storage(20, false, c),
                    storage(21, false, c),
                ],
            }),
        ];
        let f = wgpu::ShaderStages::FRAGMENT;
        let resolve = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chunk depth resolve"),
            entries: &[
                texture_entry(4, wgpu::TextureSampleType::Float { filterable: false }, f),
                texture_entry(7, wgpu::TextureSampleType::Uint, f),
                texture_entry(14, wgpu::TextureSampleType::Float { filterable: false }, f),
                texture_entry(28, wgpu::TextureSampleType::Float { filterable: false }, f),
                texture_entry(25, wgpu::TextureSampleType::Float { filterable: true }, f),
                sampler_entry(26, f),
            ],
        });
        let hits = [
            texture_entry(4, wgpu::TextureSampleType::Float { filterable: false }, c),
            texture_entry(7, wgpu::TextureSampleType::Uint, c),
        ];
        let scene_depth = texture_entry(27, wgpu::TextureSampleType::Depth, c);
        let float = |binding| {
            texture_entry(
                binding,
                wgpu::TextureSampleType::Float { filterable: false },
                c,
            )
        };
        let read = |binding| storage(binding, true, c);
        let write = |binding| storage(binding, false, c);
        Self {
            shadow_sample: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk sun samples"),
                entries: &[
                    hits[0],
                    hits[1],
                    storage_texture(9, wgpu::TextureFormat::Rgba32Uint),
                ],
            }),
            shadow: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk shadow"),
                entries: &[
                    hits[0],
                    hits[1],
                    write(41),
                    texture_entry(10, wgpu::TextureSampleType::Uint, c),
                    write(11),
                    write(12),
                    storage_texture(13, wgpu::TextureFormat::Rgba16Float),
                    float(16),
                    scene_depth,
                    uniform(62, c),
                    sun_occlusion_maps_entry(63),
                    storage_texture(64, wgpu::TextureFormat::Rgba8Unorm),
                    read(65),
                    write(66),
                ],
            }),
            sky_sample: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk sky samples"),
                entries: &[
                    hits[0],
                    hits[1],
                    storage_texture(15, wgpu::TextureFormat::Rg32Float),
                ],
            }),
            // The fallbacks' walks: their workgroups are the dispatch's.
            shadow_fallback: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk shadow fallbacks"),
                entries: &[hits[0], hits[1], write(41), write(11), scene_depth],
            }),
            // Local lights' terrain shadows: the hits' normals, the scene's
            // lights and tile lists, a light mask a pixel.
            local: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk local shadows"),
                entries: &[
                    hits[0],
                    hits[1],
                    scene_depth,
                    float(29),
                    write(30),
                    read(31),
                    read(32),
                    read(33),
                    write(34),
                ],
            }),
            // The denoiser: the hits, their normals and sky samples, the
            // walked sun, the history (the last frame's read, this frame's
            // written).
            temporal: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk shadow history"),
                entries: &[
                    hits[0],
                    hits[1],
                    float(29),
                    read(40),
                    read(42),
                    read(43),
                    read(54),
                    write(44),
                    write(45),
                    write(55),
                    write(48),
                ],
            }),
            filter: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk shadow filter"),
                entries: &[hits[0], read(46), read(47), write(48)],
            }),
            resolve_shadows: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk shadow resolve"),
                entries: &[
                    hits[0],
                    hits[1],
                    float(29),
                    read(40),
                    read(46),
                    read(47),
                    storage_texture(2, wgpu::TextureFormat::Rgba8Unorm),
                    storage_texture(8, wgpu::TextureFormat::Rgba16Float),
                    storage_texture(49, wgpu::TextureFormat::R32Float),
                ],
            }),
            local_denoise: [
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("chunk local light history"),
                    entries: &[
                        hits[0],
                        hits[1],
                        float(29),
                        read(40),
                        read(43),
                        write(30),
                        read(31),
                        read(32),
                        read(33),
                        read(51),
                        write(52),
                        write(53),
                        read(57),
                    ],
                }),
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("chunk local light filter"),
                    entries: &[hits[0], read(47), write(52), write(53)],
                }),
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("chunk local light gate"),
                    entries: &[hits[0], read(31), write(56)],
                }),
            ],
            // GI probe rays: the frame, rays, hits, the probes' cells, the
            // sun, the sun's occlusion (frame, maps).
            gi: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk GI rays"),
                entries: &[
                    uniform(35, c),
                    read(36),
                    write(37),
                    read(38),
                    read(39),
                    uniform(62, c),
                    sun_occlusion_maps_entry(63),
                    read(65),
                ],
            }),
            // Foliage ground probes: the probe frame, queries, hits, the
            // tiles.
            ground: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk ground probes"),
                entries: &[uniform(58, c), read(59), write(60), read(61)],
            }),
            // Ray reports: the hits of a pixel of each 2x2, as the nodes
            // they want.
            feedback: device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("chunk feedback"),
                entries: &[hits[0], hits[1], write(24)],
            }),
            draw,
            trace,
            seed,
            resolve,
            cull,
        }
    }
}

impl Volume {
    /// Its footprint pool doubled (up to its most): columns, caves, slopes
    /// and placements copied into buffers twice the size (submitted at
    /// once, before the update records anything), the new slots free.
    pub fn grow_footprints(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let size = (self.pool.len() * 2).min(self.pool_max);
        let n = size as u64;
        let mut encoder = device.create_command_encoder(&Default::default());
        let e = &mut encoder;
        self.tile_data = regrown(
            device,
            e,
            &self.tile_data,
            "chunk footprint columns",
            n * TILE_COLUMNS * 16,
        );
        self.tile_cave_data = regrown(
            device,
            e,
            &self.tile_cave_data,
            "chunk footprint caves",
            n * CAVE_SLOT_BYTES,
        );
        self.tile_slope_data = regrown(
            device,
            e,
            &self.tile_slope_data,
            "chunk footprint slopes",
            n * SLOPE_STRIDE * 4,
        );
        self.tile_placements = regrown(
            device,
            e,
            &self.tile_placements,
            "chunk footprint placements",
            (n * self.placement_stride).max(8),
        );
        queue.submit([encoder.finish()]);
        self.pool.grow(size);
        self.pool_measured.resize(size, None);
    }
}

/// Drawing's pipelines for one world form (`draw_source`: spheres and
/// planes compile apart).
pub(super) struct DrawPipelines {
    pub seed_outside: wgpu::RenderPipeline,
    pub seed_inside: wgpu::RenderPipeline,
    /// The draw list built on the GPU (`cull_bin`, `cull_offsets`,
    /// `cull_scatter`).
    pub cull_bin: wgpu::ComputePipeline,
    pub cull_offsets: wgpu::ComputePipeline,
    pub cull_scatter: wgpu::ComputePipeline,
    /// The trace of the camera's rays (from the eye), of orthographic
    /// views' (`trace_ortho`: the sun's occlusion), and of the camera's
    /// counting each ray's steps (`trace_heat`: debug mode 32).
    pub trace: wgpu::ComputePipeline,
    pub trace_ortho: wgpu::ComputePipeline,
    pub trace_heat: wgpu::ComputePipeline,
    pub shadow_sample: wgpu::ComputePipeline,
    /// `shadow_sample` and `sky_sample` for hits of exact levels.
    pub shadow_sample_far: wgpu::ComputePipeline,
    pub sky_sample: wgpu::ComputePipeline,
    pub sky_sample_far: wgpu::ComputePipeline,
    pub shadow: wgpu::ComputePipeline,
    pub shadow_fallback: wgpu::ComputePipeline,
    pub local_sample: wgpu::ComputePipeline,
    pub local_sample_far: wgpu::ComputePipeline,
    pub local_shadow: wgpu::ComputePipeline,
    pub local_shadow_far: wgpu::ComputePipeline,
    /// The shadows' denoiser (`chunk_draw.wgsl` 4c): the sun's and the
    /// sky's history, two filter passes, their resolve; the local lights'
    /// history, a filter pass.
    pub temporal: wgpu::ComputePipeline,
    pub filter: [wgpu::ComputePipeline; 2],
    pub resolve_shadows: wgpu::ComputePipeline,
    pub local_gate: wgpu::ComputePipeline,
    pub local_temporal: wgpu::ComputePipeline,
    pub local_filter: wgpu::ComputePipeline,
    /// The GI probes' rays and foliage ground probes (the primary
    /// volume's, from where they are).
    pub gi_trace: wgpu::ComputePipeline,
    pub ground_probe: wgpu::ComputePipeline,
    /// The passes of a volume lighting the others' points (`cross`), built
    /// once more than one volume is held (`ensure_cross`): their walks are
    /// long kernels to compile, which a world of one volume never runs.
    pub cross: Option<CrossPipelines>,
    module: wgpu::ShaderModule,
    pub feedback: wgpu::ComputePipeline,
    pub publish: wgpu::RenderPipeline,
    /// Opens rects of an orthographic view's exits (`trace_view`'s `open`).
    pub exits_open: wgpu::RenderPipeline,
    /// The publish with the host's scene materials (devices with texture
    /// binding arrays: `SceneState`'s layout).
    pub publish_scene: Option<wgpu::RenderPipeline>,
}

impl DrawPipelines {
    /// The pipelines of `source` (a form's `draw_source`); with the scene
    /// materials' layout and table capacity, the textured publish too.
    pub fn new(
        device: &wgpu::Device,
        layouts: &DrawLayouts,
        source: &str,
        scene: Option<(&wgpu::BindGroupLayout, u32, &wgpu::BindGroupLayout)>,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chunk draw"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = |second: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layouts.draw), Some(second)],
                immediate_size: 0,
            })
        };
        let seed_layout = layout(&layouts.seed);
        let depth = |compare: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let seed = |vertex: &str, fragment: &str| {
            helio_core::shader::create_render_pipeline(
                device,
                wgpu::RenderPipelineDescriptor {
                    label: Some(fragment),
                    layout: Some(&seed_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some(vertex),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: Some(wgpu::Face::Back),
                        front_face: wgpu::FrontFace::Ccw,
                        ..Default::default()
                    },
                    // The nearest box wins (`seed_box`).
                    depth_stencil: depth(helio_core::depth::NEARER),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some(fragment),
                        compilation_options: Default::default(),
                        targets: &[Some(SEED_FORMAT.into())],
                    }),
                    multiview_mask: None,
                    cache: None,
                },
            )
        };
        let cull_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chunk cull"),
            bind_group_layouts: &[Some(&layouts.cull[0]), Some(&layouts.cull[1])],
            immediate_size: 0,
        });
        let compute = |label: &str, layout: &wgpu::PipelineLayout, entry: &str, heat: bool| {
            helio_core::shader::create_compute_pipeline(
                device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: if heat { &[("STEP_HEAT", 1.0)] } else { &[] },
                        ..Default::default()
                    },
                    cache: None,
                },
            )
        };
        // Kernels that fill their workgroup tiles before reading them: no
        // zeroing first (naga zeroes workgroup memory from one invocation
        // while the others wait, longer than the kernels' own work).
        let tiled = |label: &str, layout: &wgpu::PipelineLayout, entry: &str| {
            helio_core::shader::create_compute_pipeline(
                device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        zero_initialize_workgroup_memory: false,
                        ..Default::default()
                    },
                    cache: None,
                },
            )
        };
        let trace_layout = layout(&layouts.trace);
        let sample_layout = layout(&layouts.shadow_sample);
        let sky_layout = layout(&layouts.sky_sample);
        let local_layout = layout(&layouts.local);
        let filter_layout = layout(&layouts.filter);
        let gbuffer_targets = crate::engine::GBUFFER_FORMATS.map(|format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        });
        let publish = helio_core::shader::create_render_pipeline(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("chunk gbuffer"),
                layout: Some(&layout(&layouts.resolve)),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("resolve_vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                // `shadow` decided which hits show against the meshes' depth.
                depth_stencil: depth(wgpu::CompareFunction::Always),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("publish"),
                    compilation_options: Default::default(),
                    targets: &gbuffer_targets,
                }),
                multiview_mask: None,
                cache: None,
            },
        );
        let exits_open = helio_core::shader::create_render_pipeline(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("chunk exits open"),
                layout: Some(
                    &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: None,
                        bind_group_layouts: &[],
                        immediate_size: 0,
                    }),
                ),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("resolve_vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("exits_open"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::R32Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            },
        );
        // The publish again, textured by the host's scene materials, where
        // the device has texture binding arrays.
        let publish_scene = scene.map(|(textures, capacity, shore_params)| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("chunk draw (scene materials)"),
                source: wgpu::ShaderSource::Wgsl(scene::scene_source(source, capacity).into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[
                    Some(&layouts.draw),
                    Some(&layouts.resolve),
                    Some(textures),
                    Some(shore_params),
                ],
                immediate_size: 0,
            });
            helio_core::shader::create_render_pipeline(
                device,
                wgpu::RenderPipelineDescriptor {
                    label: Some("chunk gbuffer (scene materials)"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("resolve_vertex"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: Default::default(),
                    depth_stencil: depth(wgpu::CompareFunction::Always),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("publish"),
                        compilation_options: Default::default(),
                        targets: &gbuffer_targets,
                    }),
                    multiview_mask: None,
                    cache: None,
                },
            )
        });
        Self {
            seed_outside: seed("seed_vertex", "seed_fragment"),
            seed_inside: seed("seed_vertex_inside", "seed_fragment_inside"),
            cull_bin: compute("cull_bin", &cull_layout, "cull_bin", false),
            cull_offsets: compute("cull_offsets", &cull_layout, "cull_offsets", false),
            cull_scatter: compute("cull_scatter", &cull_layout, "cull_scatter", false),
            trace: compute("chunk trace", &trace_layout, "trace", false),
            trace_ortho: compute("chunk ortho trace", &trace_layout, "trace_ortho", false),
            trace_heat: compute("chunk heat trace", &trace_layout, "trace", true),
            shadow_sample: compute("chunk sun samples", &sample_layout, "shadow_sample", false),
            shadow_sample_far: compute(
                "chunk far sun samples",
                &sample_layout,
                "shadow_sample_far",
                false,
            ),
            sky_sample: compute("chunk sky samples", &sky_layout, "sky_sample", false),
            sky_sample_far: compute(
                "chunk far sky samples",
                &sky_layout,
                "sky_sample_far",
                false,
            ),
            shadow: compute("chunk shadow", &layout(&layouts.shadow), "shadow", false),
            shadow_fallback: compute(
                "chunk shadow fallbacks",
                &layout(&layouts.shadow_fallback),
                "shadow_fallback",
                false,
            ),
            local_sample: compute(
                "chunk local light samples",
                &local_layout,
                "local_sample",
                false,
            ),
            local_sample_far: compute(
                "chunk far local light samples",
                &local_layout,
                "local_sample_far",
                false,
            ),
            local_shadow: compute("chunk local shadows", &local_layout, "local_shadow", false),
            local_shadow_far: compute(
                "chunk far local shadows",
                &local_layout,
                "local_shadow_far",
                false,
            ),
            temporal: tiled(
                "chunk shadow history",
                &layout(&layouts.temporal),
                "temporal",
            ),
            filter: [
                tiled("chunk shadow filter", &filter_layout, "filter_first"),
                tiled("chunk shadow filter", &filter_layout, "filter_second"),
            ],
            resolve_shadows: tiled(
                "chunk shadow resolve",
                &layout(&layouts.resolve_shadows),
                "resolve",
            ),
            local_gate: compute(
                "chunk local light gate",
                &layout(&layouts.local_denoise[2]),
                "local_gate",
                false,
            ),
            local_temporal: tiled(
                "chunk local light history",
                &layout(&layouts.local_denoise[0]),
                "local_temporal",
            ),
            local_filter: compute(
                "chunk local light filter",
                &layout(&layouts.local_denoise[1]),
                "local_filter",
                false,
            ),
            gi_trace: compute("chunk GI rays", &layout(&layouts.gi), "gi_trace", false),
            ground_probe: compute(
                "chunk ground probes",
                &layout(&layouts.ground),
                "ground_probe",
                false,
            ),
            feedback: tiled("chunk feedback", &layout(&layouts.feedback), "feedback"),
            publish,
            exits_open,
            publish_scene,
            cross: None,
            module,
        }
    }

    /// The passes that light other volumes' points, built once.
    pub fn ensure_cross(&mut self, device: &wgpu::Device, layouts: &DrawLayouts) {
        if self.cross.is_some() {
            return;
        }
        let compute = |label: &str, second: &wgpu::BindGroupLayout, entry: &str| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layouts.draw), Some(second)],
                immediate_size: 0,
            });
            helio_core::shader::create_compute_pipeline(
                device,
                wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    module: &self.module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    cache: None,
                },
            )
        };
        self.cross = Some(CrossPipelines {
            shadow: compute("chunk shadows across volumes", &layouts.shadow_fallback, "shadow_cross"),
            gi_trace: compute("chunk GI rays across volumes", &layouts.gi, "gi_trace_cross"),
            ground_probe: compute(
                "chunk ground probes across volumes",
                &layouts.ground,
                "ground_probe_cross",
            ),
        });
    }
}

/// A volume's passes lighting the other volumes' points: its sun shadow on
/// their hits and the meshes, the GI probes' rays and foliage's probes
/// entering it (`ChunkWorld::cross_volumes`).
pub(super) struct CrossPipelines {
    pub shadow: wgpu::ComputePipeline,
    pub gi_trace: wgpu::ComputePipeline,
    pub ground_probe: wgpu::ComputePipeline,
}

/// Footprints a world of `grid` has at every level (its pool never needs
/// more slots).
fn footprints_of(grid: &Grid) -> u64 {
    (0..grid.levels())
        .map(|l| {
            let n = residency::tiles_at(grid, l) as u64;
            n * n * grid.faces().len() as u64
        })
        .sum()
}

impl ChunkWorld {
    /// Add `planet` to the world placed by `transform`, named `tag` (what
    /// its hits report): its terrain program's generation pipelines
    /// (shared with volumes of the same program and form), its footprint
    /// pool (no larger than the footprints it has), its edits, and its
    /// planner. Its chunks share the world's pool.
    pub fn add_volume(
        &mut self,
        tag: u64,
        planet: &Planet,
        transform: VolumeTransform,
    ) -> VolumeId {
        let index = self
            .volumes
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.volumes.len());
        assert!(
            index < MAX_VOLUMES,
            "a world holds {MAX_VOLUMES} volumes at most"
        );
        if index == self.volumes.len() {
            self.volumes.push(None);
        }
        let index16 = index as u16;
        let device = self.device.clone();
        let queue = self.queue.clone();
        let grid = *planet.grid();
        let program = planet.field().program();
        let plane = grid.is_plane();
        // Drawing's pipelines for its form.
        let form = usize::from(plane);
        if self.pipelines[form].is_none() {
            let source = draw_source(plane, self.counting);
            self.pipelines[form] = Some(DrawPipelines::new(
                &device,
                &self.layouts,
                &source,
                self.scene
                    .as_ref()
                    .map(|s| (&s.layout, s.capacity, &s.shore_params_layout)),
            ));
        }
        let gen_source = generation_source(plane, &program, self.wide64);
        let source_key = crate::record::fnv64(gen_source.as_bytes());
        let gen = match self
            .gen_cache
            .get(&source_key)
            .and_then(std::sync::Weak::upgrade)
        {
            Some(gen) => gen,
            None => {
                let gen = std::sync::Arc::new(GenPipelines::new(
                    &device,
                    &gen_source,
                    &self.gen_pipeline_layout,
                    &self.keep_pipeline_layout,
                ));
                self.gen_cache
                    .insert(source_key, std::sync::Arc::downgrade(&gen));
                gen
            }
        };
        // What generated packs depend on: the generation shaders, the
        // terrain's and volume layer's constants and the world
        // (`kept_chunks`).
        let volume_constants = crate::engine::volume_constants(planet);
        let fingerprint = {
            let mut bytes =
                format!("chunk packs {KEPT_FORMAT} {DETAIL_LEVELS} {}", gen_source).into_bytes();
            bytes.extend_from_slice(&terrain_bytes(&program));
            bytes.extend_from_slice(bytemuck::cast_slice(&program.tables));
            bytes.extend_from_slice(bytemuck::bytes_of(&volume_constants));
            bytes.extend_from_slice(bytemuck::bytes_of(&WorldGpu::new(planet)));
            crate::record::fnv64(&bytes)
        };
        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let s = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let limits = device.limits();
        let binding = u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        let features = planet.features();
        let placement_stride = features.stride as u64 * 8;
        // A pool of two batches' footprints, growing up to the capacity's
        // (at least two batches'), no more than the world has.
        let pool_max = (self.capacity.footprints as u64)
            .min(binding / (TILE_COLUMNS * 16))
            .min(binding / placement_stride.max(1))
            .max(2 * TILE_BATCH as u64)
            .min(footprints_of(&grid).max(1)) as usize;
        let pool_size = pool_max.min(2 * TILE_BATCH);
        let g = s | wgpu::BufferUsages::COPY_SRC;
        // Brushes kept analytic: the large ones, then the recent ones.
        let edits = planet.edits();
        let view = std::sync::Arc::new(
            edit_view(
                planet,
                0,
                None,
                std::sync::Arc::new(classify::BrickIndex::new(&grid, &edits.baked, index16)),
            )
            .0,
        );
        let (brushes, brush_capacity) = brush_buffer(&device, &queue, &view.faces);
        let (materials, detail, shape) =
            crate::engine::material_table(&planet.field().appearance());
        let params = |label: &str| {
            buffer(
                label,
                std::mem::size_of::<DrawParams>() as u64,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            )
        };
        // The terrain's heights, its features standing over and sunk under
        // them.
        let height_layers = (
            (planet.min_terrain_height() / grid.voxel_size()).floor() as i32
                - 64
                - planet.features().below,
            (planet.max_terrain_height() / grid.voxel_size()).ceil() as i32
                + 64
                + planet.features().above,
        );
        // The batch's two-rate nodes, shared (one volume generates a
        // frame): the tiles', the scratch nodes, the tiles' slope lattice
        // rings, the block jobs' node quads, and the 8 a column's node words
        // read past the last, for the largest program.
        let node_bytes = (2 * TILE_BATCH as u64 * u64::from(crate::engine::TILE_NODES.pow(2))
            + u64::from(crate::engine::NODE_SCRATCH)
            + if features.is_empty() {
                0
            } else {
                4 * (candidates::BLOCK_JOBS * candidates::BLOCK_READS) as u64
            }
            + 8)
            * u64::from(crate::engine::node_words(&program))
            * 4;
        if node_bytes > self.tile_node_data.size() {
            self.tile_node_data =
                buffer("chunk tile nodes", node_bytes, wgpu::BufferUsages::STORAGE);
        }
        // Its planner from the next update (with that update's settings).
        self.unplanned.push(index16);
        self.motion_version += 1;
        self.drawn_set.set_top_level(index16, grid.levels() - 1);
        self.volumes[index] = Some(Volume {
            tag,
            index: index16,
            transform,
            drawn_transform: [None; 2],
            last_local_eye: None,
            grid,
            bounds: crate::volume::bounding_radius(planet),
            face_cells: grid.cells() as u32,
            exact_level: exact_level(&grid),
            ceiling: planet.outer_radius() - grid.radius() + CEILING_MARGIN,
            floor: planet.inner_radius() - grid.radius() - CEILING_MARGIN,
            rock: planet.rock_material() as u8,
            height_layers,
            fingerprint,
            gen,
            world: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk world"),
                contents: bytemuck::bytes_of(&WorldGpu::new(planet)),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
            terrain: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk terrain"),
                contents: &terrain_bytes(&program),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
            terrain_tables: crate::engine::tables_buffer(&device, &program),
            regions: crate::engine::RegionPool::new(&device, &program, REGION_CAPACITY),
            region_key_cache: Default::default(),
            volume_layer: planet.volumes().cloned(),
            volumes: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk volumes"),
                contents: bytemuck::bytes_of(&volume_constants),
                usage: wgpu::BufferUsages::UNIFORM,
            }),
            materials: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk materials"),
                contents: bytemuck::bytes_of(&ChunkMaterials {
                    materials,
                    detail,
                    shape,
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }),
            views: VolumeViews {
                camera: params("chunk draw params"),
                sun: params("chunk sun params"),
                cross: params("chunk cross params"),
            },
            tile_data: buffer(
                "chunk footprint columns",
                pool_size as u64 * TILE_COLUMNS * 16,
                g,
            ),
            tile_cave_data: buffer(
                "chunk footprint caves",
                pool_size as u64 * CAVE_SLOT_BYTES,
                g,
            ),
            tile_slope_data: buffer(
                "chunk footprint slopes",
                pool_size as u64 * SLOPE_STRIDE * 4,
                g,
            ),
            feature_set: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk features"),
                contents: bytemuck::cast_slice(&features.gpu_words()),
                usage: wgpu::BufferUsages::STORAGE,
            }),
            tile_placements: buffer(
                "chunk footprint placements",
                (pool_size as u64 * placement_stride).max(8),
                g,
            ),
            feature_blocks: buffer(
                "chunk feature candidate blocks",
                candidates::words(features) as u64 * 4,
                s,
            ),
            blocks: candidates::CandidateBlocks::new(),
            features: features.clone(),
            has_features: !features.is_empty(),
            pool: SlotPool::new(pool_size),
            pool_measured: vec![None; pool_size],
            pool_max,
            placement_stride,
            edits: view,
            edits_unsent: None,
            brushes,
            brush_capacity,
            brick_cache: BrickCache::new(
                &device,
                self.capacity.bricks.min((binding / BRICK_BYTES) as usize) as u32,
            ),
            edit_state: EditState::of(edits),
            urgent: FxHashSet::default(),
            urgent_distance: FxHashMap::default(),
            stale: FxHashSet::default(),
            events: Vec::new(),
            plan: Default::default(),
            planned_for: None,
            last_requests: 1,
        });
        if self.primary.is_none() {
            self.primary = Some(index16);
        }
        // More than one volume: each form's passes lighting the others.
        if self.volumes.iter().flatten().count() > 1 {
            for pipelines in self.pipelines.iter_mut().flatten() {
                pipelines.ensure_cross(&self.device, &self.layouts);
            }
        }
        VolumeId(index16)
    }

    /// Remove volume `id`: its chunks go back to the pool, its nodes leave
    /// the cut, and what was kept of it is dropped.
    pub fn remove_volume(&mut self, id: VolumeId) {
        let Some(vol) = self
            .volumes
            .get_mut(usize::from(id.0))
            .and_then(Option::take)
        else {
            return;
        };
        if !self.unplanned.contains(&vol.index) {
            self.worker.remove_volume(vol.index);
        }
        self.unplanned.retain(|&u| u != vol.index);
        self.motion_version += 1;
        let keys: Vec<NodeKey> = self
            .nodes
            .keys()
            .filter(|k| k.volume == vol.index)
            .copied()
            .collect();
        for key in keys {
            if let Some(old) = self.remove_resident(key) {
                self.dropped.push(old.slot);
            }
        }
        // Its nodes leave the cut the next update.
        let drawn: Vec<NodeKey> = self.drawn_set.nodes_of(vol.index);
        self.removed_drawn.extend(drawn);
        self.drop_kept_where(|k| k.volume == vol.index);
        self.to_pack.retain(|k| k.volume != vol.index);
        if self.primary == Some(vol.index) {
            self.primary = self.volumes.iter().flatten().map(|v| v.index).next();
        }
        self.ground_placed.remove(&vol.index);
    }

    /// Place volume `id` by `transform` (from the next draw; its hits' motion
    /// since the last draw is that between them).
    pub fn set_transform(&mut self, id: VolumeId, transform: VolumeTransform) {
        if let Some(vol) = self.volume_mut(id) {
            vol.transform = transform;
        }
    }

    /// Where volume `id` is.
    pub fn transform(&self, id: VolumeId) -> Option<VolumeTransform> {
        self.volume(id).map(|v| v.transform)
    }

    /// The volumes held, in index order: id and tag.
    pub fn volume_ids(&self) -> Vec<(VolumeId, u64)> {
        self.volumes
            .iter()
            .flatten()
            .map(|v| (VolumeId(v.index), v.tag))
            .collect()
    }

    /// Each volume's residency (diagnostics): its id and tag, resident
    /// chunks, drawn chunks, and nodes its plan waits for.
    pub fn volume_stats(&self) -> Vec<(VolumeId, u64, usize, usize, usize)> {
        self.volumes
            .iter()
            .flatten()
            .map(|v| {
                let resident = self.nodes.keys().filter(|k| k.volume == v.index).count();
                let drawn = self
                    .drawn_set
                    .list
                    .iter()
                    .filter(|c| (self.slot_keys[c.slot as usize][0] >> 16) as u16 == v.index)
                    .count();
                (VolumeId(v.index), v.tag, resident, drawn, v.plan.waiting)
            })
            .collect()
    }

    /// The volume foliage tiles and GI probes are placed in (in f64, its
    /// cells): the one whose surface is nearest the eye (`follow_eye`), the
    /// first added before an update has seen the eye. The others light
    /// them where they reach (`ChunkWorld::draw`, `probe_ground`).
    pub fn primary(&self) -> Option<VolumeId> {
        self.primary.map(VolumeId)
    }

    /// The primary volume for an eye at `eye` (world): the volume whose
    /// ground the eye is nearest (the distance from its terrain's height
    /// range, `height_layers`; past a plane's edges, from them too), kept
    /// until another's is nearer by half (no switching back and forth
    /// between two about as near; an eye in two volumes' ranges keeps the
    /// one it had).
    pub(super) fn follow_eye(&mut self, eye: DVec3) {
        let surface = |v: &Volume| {
            let local = v.transform.to_local(eye);
            let h = v.grid.height(local);
            let voxel = v.grid.voxel_size();
            let (lo, hi) = (
                f64::from(v.height_layers.0) * voxel,
                f64::from(v.height_layers.1) * voxel,
            );
            let height = (lo - h).max(h - hi).max(0.0);
            if v.grid.shape() != crate::grid::Shape::Plane {
                return height;
            }
            let half = f64::from(v.grid.cells()) * v.grid.voxel_size() * 0.5;
            let out = (local.x.abs() - half).max(0.0).hypot((local.z.abs() - half).max(0.0));
            height.hypot(out)
        };
        let Some((best, nearest)) = self
            .volumes
            .iter()
            .flatten()
            .map(|v| (v.index, surface(v)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            return;
        };
        let current = self
            .primary
            .and_then(|p| self.volumes[usize::from(p)].as_ref())
            .map(surface);
        if current.is_none_or(|c| nearest < 0.5 * c) {
            self.primary = Some(best);
        }
    }

    pub(super) fn volume(&self, id: VolumeId) -> Option<&Volume> {
        self.volumes.get(usize::from(id.0)).and_then(Option::as_ref)
    }

    pub(super) fn volume_mut(&mut self, id: VolumeId) -> Option<&mut Volume> {
        self.volumes
            .get_mut(usize::from(id.0))
            .and_then(Option::as_mut)
    }

    /// Volume `index` taken out to work on it beside the world's shared
    /// state ([`Self::put_volume`] puts it back).
    pub(super) fn take_volume(&mut self, index: u16) -> Option<Volume> {
        self.volumes
            .get_mut(usize::from(index))
            .and_then(Option::take)
    }

    pub(super) fn put_volume(&mut self, vol: Volume) {
        let at = usize::from(vol.index);
        self.volumes[at] = Some(vol);
    }

    /// Indices of the volumes held.
    pub(super) fn volume_indices(&self) -> Vec<u16> {
        self.volumes.iter().flatten().map(|v| v.index).collect()
    }
}
