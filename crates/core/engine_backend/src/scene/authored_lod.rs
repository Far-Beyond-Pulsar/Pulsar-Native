//! Per-view selection for authored mesh LOD.
//!
//! This only resolves policy. The selected asset id is kept as data, not as a
//! strong SceneDB asset reference; render demand and geometry residency are
//! handled by the view's LOD asset bridge.

use helio_core::{
    BufferHandle, BufferKey, SceneDerivation, SceneDerivationContext, SceneDerivationOutput,
};
use pulsar_scenedb::AssetId;
use std::collections::HashSet;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;

const OWNER_ROW_BYTES: u64 = 12;
const TRANSFORM_ROW_BYTES: u64 = 48;
const HANDLE_ROW_BYTES: u64 = 8;
const LEVEL_ROW_BYTES: u64 = 24;
const SELECTION_ROW_BYTES: u64 = 28;
const OBJECT_ROW_BYTES: u64 = 252;
const ASSET_ROW_BYTES: u64 = 48;
const SECTION_ROW_BYTES: u64 = 8;
const WORKGROUP: u32 = 64;

pub const LOD_SELECTIONS_KEY: BufferKey = BufferKey::of("authored_lod_selections");
/// Generic per-object-entity "skip authored LOD" flags published by whatever
/// system draws those objects' detail itself (virtual geometry). Optional.
const LOD_SUPPRESSED_KEY: BufferKey = BufferKey::of("lod_suppressed");
const LOD_ASSETS_KEY: BufferKey = BufferKey::of("authored_lod_assets");
const LOD_SECTIONS_KEY: BufferKey = BufferKey::of("authored_lod_sections");
const OBJECTS_KEY: BufferKey = BufferKey::of("static_objects");
const OBJECT_SECTIONS_KEY: BufferKey = BufferKey::of("static_mesh_draw_sections::handles");
const OBJECTS_STRUCTURE_KEY: BufferKey = BufferKey::of("scene_objects_structure");
const VERTEX_POOL_KEY: BufferKey = BufferKey::of("builtin_mesh_vertex");
const INDEX_POOL_KEY: BufferKey = BufferKey::of("builtin_mesh_index");

/// Source buffers emitted by SceneDB and the transform derivation.
#[derive(Clone, Copy, Debug)]
pub struct AuthoredLodKeys {
    pub owners: BufferKey,
    pub generations: BufferKey,
    pub hidden: BufferKey,
    pub transforms: BufferKey,
    pub level_handles: BufferKey,
    pub levels: BufferKey,
    pub camera: BufferKey,
}

/// GPU output for one authored LOD component entity. Rows are keyed by the
/// component entity index so the delayed residency bridge can consume them
/// without scanning `World`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LodSelectionRow {
    pub level_index: u32,
    pub valid: u32,
    pub mesh_asset: [u32; 4],
    pub camera_distance: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct AssetGeometryRow {
    mesh_asset: [u32; 4],
    vertex_offset: u32,
    index_offset: u32,
    section_offset: u32,
    section_count: u32,
    bounds_local: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct AssetSectionRow {
    first_index: u32,
    index_count: u32,
}

pub struct AuthoredLodSelection {
    keys: AuthoredLodKeys,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    uniforms: wgpu::Buffer,
    apply_uniforms: wgpu::Buffer,
    output: Option<wgpu::Buffer>,
    output_rows: u32,
    output_epoch: u64,
    output_generation: u64,
    last_signature: Option<u64>,
    pending_readback: Option<PendingReadback>,
    requested_assets: HashSet<AssetId>,
    active_assets: HashMap<AssetId, ActiveLodAsset>,
    mirror: Option<pulsar_scenedb::gpu::GpuMirrorHandle>,
    apply_pipeline: wgpu::ComputePipeline,
    apply_layout: wgpu::BindGroupLayout,
    patched_objects: Option<wgpu::Buffer>,
    patched_object_bytes: u64,
    patched_object_epoch: u64,
    patched_object_generation: u64,
    last_apply_signature: Option<u64>,
    asset_table: Option<wgpu::Buffer>,
    asset_table_rows: u32,
    asset_table_epoch: u64,
    asset_table_generation: u64,
    section_table: Option<wgpu::Buffer>,
    section_table_rows: u32,
    section_table_epoch: u64,
    section_table_generation: u64,
    last_asset_table_signature: Option<u64>,
    lod_selection_signature: Option<u64>,
    lod_selection_generation: u64,
    last_structure_signature: Option<u64>,
    output_structure_generation: u64,
    /// Bound in place of `lod_suppressed` while nothing publishes it (zeroed).
    no_suppression: wgpu::Buffer,
}

struct ActiveLodAsset {
    _mesh: Arc<helio_component::components::LoadedMesh>,
    _gpu: helio_component::components::LoadedMeshGpuLease,
}

struct PendingReadback {
    buffer: wgpu::Buffer,
    rows: u32,
    start_mapping: Option<Sender<Result<(), wgpu::BufferAsyncError>>>,
    mapped: Receiver<Result<(), wgpu::BufferAsyncError>>,
}

impl AuthoredLodSelection {
    pub fn new(
        device: &wgpu::Device,
        keys: AuthoredLodKeys,
        mirror: Option<pulsar_scenedb::gpu::GpuMirrorHandle>,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Authored LOD Selection Layout"),
            entries: &[
                layout_entry(0, wgpu::BufferBindingType::Uniform),
                layout_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(3, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(4, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(5, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(6, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(7, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(8, wgpu::BufferBindingType::Storage { read_only: false }),
                layout_entry(9, wgpu::BufferBindingType::Storage { read_only: true }),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Authored LOD Selection"),
            source: wgpu::ShaderSource::Wgsl(include_str!("authored_lod.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Authored LOD Selection Pipeline Layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Authored LOD Selection"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs_select_authored_lod"),
            compilation_options: Default::default(),
            cache: None,
        });
        let apply_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Authored LOD Apply Layout"),
            entries: &[
                layout_entry(0, wgpu::BufferBindingType::Uniform),
                layout_entry(1, wgpu::BufferBindingType::Storage { read_only: false }),
                layout_entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(3, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(4, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(5, wgpu::BufferBindingType::Storage { read_only: true }),
                layout_entry(6, wgpu::BufferBindingType::Storage { read_only: true }),
            ],
        });
        let apply_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Authored LOD Apply"),
            source: wgpu::ShaderSource::Wgsl(include_str!("authored_lod_apply.wgsl").into()),
        });
        let apply_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Authored LOD Apply Pipeline Layout"),
            bind_group_layouts: &[Some(&apply_layout)],
            immediate_size: 0,
        });
        let apply_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Authored LOD Apply"),
            layout: Some(&apply_pipeline_layout),
            module: &apply_module,
            entry_point: Some("cs_apply_authored_lod"),
            compilation_options: Default::default(),
            cache: None,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Authored LOD Selection Uniforms"),
            size: std::mem::size_of::<SelectionUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let apply_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Authored LOD Apply Uniforms"),
            size: std::mem::size_of::<ApplyUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            keys,
            layout,
            pipeline,
            uniforms,
            apply_uniforms,
            output: None,
            output_rows: 0,
            output_epoch: 0,
            output_generation: 0,
            last_signature: None,
            pending_readback: None,
            requested_assets: HashSet::new(),
            active_assets: HashMap::new(),
            mirror,
            apply_pipeline,
            apply_layout,
            patched_objects: None,
            patched_object_bytes: 0,
            patched_object_epoch: 0,
            patched_object_generation: 0,
            last_apply_signature: None,
            asset_table: None,
            asset_table_rows: 0,
            asset_table_epoch: 0,
            asset_table_generation: 0,
            section_table: None,
            section_table_rows: 0,
            section_table_epoch: 0,
            section_table_generation: 0,
            last_asset_table_signature: None,
            lod_selection_signature: None,
            lod_selection_generation: 0,
            last_structure_signature: None,
            output_structure_generation: 0,
            no_suppression: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Authored LOD No Suppression"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
        }
    }

    fn ensure_output(&mut self, device: &wgpu::Device, rows: u32) -> bool {
        if self.output.is_some() && rows <= self.output_rows {
            return false;
        }
        self.output_rows = rows.max(1).next_power_of_two();
        self.output = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Authored LOD Selections"),
            size: u64::from(self.output_rows) * SELECTION_ROW_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        self.output_epoch = self.output_epoch.wrapping_add(1).max(1);
        true
    }

    fn refresh_asset_tables(&mut self, ctx: &SceneDerivationContext<'_>) -> bool {
        let mut ids = self.active_assets.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable_by_key(|id| id.0);
        let mut assets = Vec::with_capacity(ids.len());
        let mut sections = Vec::new();
        for id in ids {
            let Some(active) = self.active_assets.get(&id) else {
                continue;
            };
            let vertex = active._gpu.vertex_range();
            let index = active._gpu.index_range();
            let section_offset = sections.len() as u32;
            if active._mesh.upload.sections.is_empty() {
                if index.count > 0 {
                    sections.push(AssetSectionRow {
                        first_index: 0,
                        index_count: index.count,
                    });
                }
            } else {
                sections.extend(active._mesh.upload.sections.iter().map(|section| AssetSectionRow {
                    first_index: section.first_index,
                    index_count: section.index_count,
                }));
            }
            let section_count = sections.len() as u32 - section_offset;
            assets.push(AssetGeometryRow {
                mesh_asset: asset_words(id),
                vertex_offset: vertex.offset,
                index_offset: index.offset,
                section_offset,
                section_count,
                bounds_local: active._mesh.bounds_local,
            });
        }
        let asset_bytes = bytemuck::cast_slice(&assets);
        let section_bytes = bytemuck::cast_slice(&sections);
        let mut sig = signature(&[]);
        for bytes in [asset_bytes, section_bytes] {
            for byte in bytes {
                sig ^= u64::from(*byte);
                sig = sig.wrapping_mul(0x0100_0000_01b3);
            }
        }
        sig ^= assets.len() as u64;
        sig = sig.wrapping_mul(0x0100_0000_01b3);
        sig ^= sections.len() as u64;
        if self.last_asset_table_signature == Some(sig) {
            return false;
        }
        self.last_asset_table_signature = Some(sig);
        self.asset_table_rows = assets.len().max(1) as u32;
        self.section_table_rows = sections.len().max(1) as u32;
        self.asset_table = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Authored LOD Asset Geometry"),
            size: u64::from(self.asset_table_rows) * ASSET_ROW_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.section_table = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Authored LOD Asset Sections"),
            size: u64::from(self.section_table_rows) * SECTION_ROW_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        if let Some(buffer) = &self.asset_table {
            if assets.is_empty() {
                ctx.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&AssetGeometryRow::default()));
            } else {
                ctx.queue.write_buffer(buffer, 0, asset_bytes);
            }
        }
        if let Some(buffer) = &self.section_table {
            if sections.is_empty() {
                ctx.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&AssetSectionRow::default()));
            } else {
                ctx.queue.write_buffer(buffer, 0, section_bytes);
            }
        }
        self.asset_table_epoch = self.asset_table_epoch.wrapping_add(1).max(1);
        self.section_table_epoch = self.section_table_epoch.wrapping_add(1).max(1);
        self.asset_table_generation = self.asset_table_generation.wrapping_add(1).max(1);
        self.section_table_generation = self.section_table_generation.wrapping_add(1).max(1);
        true
    }

    fn append_geometry_pool_outputs(&self, buffers: &mut Vec<(BufferKey, BufferHandle)>) {
        let Some(mirror) = self.mirror.as_ref() else {
            return;
        };
        for key in [VERTEX_POOL_KEY, INDEX_POOL_KEY] {
            let Some(handle) = mirror.store().resolve_buffer_handle(key) else {
                continue;
            };
            buffers.push((
                key,
                BufferHandle {
                    buffer: handle.buffer,
                    epoch: handle.epoch,
                    row_bytes: handle.row_bytes,
                    content_generation: handle.content_generation,
                },
            ));
        }
    }

    fn apply_lod_to_objects(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        encoder: &mut wgpu::CommandEncoder,
        objects: &BufferHandle,
        owners: &BufferHandle,
        section_handles: &BufferHandle,
        selections: &BufferHandle,
        assets: &BufferHandle,
        sections: &BufferHandle,
    ) -> Option<(BufferHandle, bool)> {
        if !valid_row_size(objects, OBJECT_ROW_BYTES)
            || !valid_row_size(owners, OWNER_ROW_BYTES)
            || !valid_row_size(section_handles, HANDLE_ROW_BYTES)
            || !valid_row_size(selections, SELECTION_ROW_BYTES)
            || !valid_row_size(assets, ASSET_ROW_BYTES)
            || !valid_row_size(sections, SECTION_ROW_BYTES)
        {
            return None;
        }
        let source_bytes = objects.buffer.size();
        let rows = (source_bytes / OBJECT_ROW_BYTES).min(u64::from(u32::MAX)) as u32;
        if rows == 0 {
            return None;
        }
        let grew = self.patched_objects.is_none() || source_bytes > self.patched_object_bytes;
        if grew {
            self.patched_object_bytes = source_bytes.next_power_of_two();
            self.patched_objects = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Authored LOD Static Objects"),
                size: self.patched_object_bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
            self.patched_object_epoch = self.patched_object_epoch.wrapping_add(1).max(1);
        }
        let output = self.patched_objects.as_ref().expect("patched object output");
        let run_signature = signature(&[
            objects,
            owners,
            section_handles,
            selections,
            assets,
            sections,
        ]);
        let should_run = grew || self.last_apply_signature != Some(run_signature);
        if should_run {
            self.last_apply_signature = Some(run_signature);
            self.patched_object_generation = self.patched_object_generation.wrapping_add(1).max(1);
            if grew {
                encoder.clear_buffer(output, 0, None);
            }
            encoder.copy_buffer_to_buffer(&objects.buffer, 0, output, 0, source_bytes);
            let uniforms = ApplyUniforms {
                object_rows: rows,
                _pad: [0; 3],
            };
            ctx.queue.write_buffer(&self.apply_uniforms, 0, bytemuck::bytes_of(&uniforms));
            let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Authored LOD Apply Bind Group"),
                layout: &self.apply_layout,
                entries: &[
                    entry(0, &self.apply_uniforms),
                    entry(1, output),
                    entry(2, &owners.buffer),
                    entry(3, &section_handles.buffer),
                    entry(4, &selections.buffer),
                    entry(5, &assets.buffer),
                    entry(6, &sections.buffer),
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Authored LOD Apply"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.apply_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(rows.div_ceil(WORKGROUP), 1, 1);
        }
        Some((
            BufferHandle {
                buffer: output.clone(),
                epoch: self.patched_object_epoch,
                row_bytes: OBJECT_ROW_BYTES,
                content_generation: self.patched_object_generation,
            },
            should_run,
        ))
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SelectionUniforms {
    component_rows: u32,
    add_origin_to_camera: u32,
    _pad: [u32; 2],
    origin_hi: [f32; 4],
    origin_lo: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ApplyUniforms {
    object_rows: u32,
    _pad: [u32; 3],
}

impl SceneDerivation for AuthoredLodSelection {
    fn name(&self) -> &'static str {
        "AuthoredLodSelection"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        self.poll_asset_demand();
        self.refresh_resident_assets();
        let projection = ctx.inputs;
        let owners = projection.get(self.keys.owners);
        let generations = projection.get(self.keys.generations);
        let hidden = projection.get(self.keys.hidden);
        let transforms = projection.get(self.keys.transforms);
        let handles = projection.get(self.keys.level_handles);
        let levels = projection.get(self.keys.levels);
        let camera = projection.get(self.keys.camera);
        let component_rows = handles.map_or(0, |h| (h.buffer.size() / HANDLE_ROW_BYTES) as u32);
        let owner_rows = owners.map_or(0, |h| (h.buffer.size() / OWNER_ROW_BYTES) as u32);
        let grew = self.ensure_output(ctx.device, owner_rows);
        let output = self.output.as_ref().expect("LOD output allocated").clone();
        if grew {
            encoder.clear_buffer(&output, 0, None);
            self.output_generation = self.output_generation.wrapping_add(1).max(1);
        }
        let mut recorded = grew;
        let input_rows = (
            owners,
            generations,
            hidden,
            transforms,
            handles,
            levels,
            camera,
        );
        if let (
            Some(owners),
            Some(generations),
            Some(hidden),
            Some(transforms),
            Some(handles),
            Some(levels),
            Some(camera),
        ) = input_rows
        {
            let valid = valid_row_size(owners, OWNER_ROW_BYTES)
                && valid_row_size(generations, 4)
                && valid_row_size(hidden, 4)
                && valid_row_size(transforms, TRANSFORM_ROW_BYTES)
                && valid_row_size(handles, HANDLE_ROW_BYTES)
                && valid_row_size(levels, LEVEL_ROW_BYTES)
                && valid_row_size(
                    camera,
                    std::mem::size_of::<helio_core::GpuCameraUniforms>() as u64,
                );
            if valid {
                let mut input_signature = signature(&[
                    owners,
                    generations,
                    hidden,
                    transforms,
                    handles,
                    levels,
                    camera,
                ]);
                // Another system taking over objects' detail changes which
                // owners may select a level.
                let suppressed = projection.get(LOD_SUPPRESSED_KEY);
                if let Some(suppressed) = suppressed {
                    input_signature ^= signature(&[suppressed]).rotate_left(21);
                    input_signature = input_signature.wrapping_mul(0x0100_0000_01b3);
                }
                // A moved frame origin moves the eye in the camera rows.
                if let Some(origin) = ctx.world_origin {
                    for bits in [origin.x.to_bits(), origin.y.to_bits(), origin.z.to_bits()] {
                        input_signature ^= bits;
                        input_signature = input_signature.wrapping_mul(0x0100_0000_01b3);
                    }
                }
                let should_run = component_rows > 0
                    && (grew || self.last_signature != Some(input_signature));
                if should_run {
                    self.last_signature = Some(input_signature);
                    self.output_generation = self.output_generation.wrapping_add(1).max(1);
                    encoder.clear_buffer(&output, 0, None);
                    // The camera rows are relative to the frame's world
                    // origin. Transform rows already made origin-relative
                    // upstream compare with them directly; world rows need
                    // the origin added back to the eye.
                    let relative = projection.contains(BufferKey::of("transforms_origin_relative"));
                    let camera_origin = if relative { None } else { ctx.world_origin };
                    let (origin_hi, origin_lo) = origin_pair(camera_origin);
                    let uniforms = SelectionUniforms {
                        component_rows,
                        add_origin_to_camera: u32::from(camera_origin.is_some()),
                        _pad: [0; 2],
                        origin_hi,
                        origin_lo,
                    };
                    ctx.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
                    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Authored LOD Selection Bind Group"),
                        layout: &self.layout,
                        entries: &[
                            entry(0, &self.uniforms),
                            entry(1, &owners.buffer),
                            entry(2, &generations.buffer),
                            entry(3, &hidden.buffer),
                            entry(4, &transforms.buffer),
                            entry(5, &handles.buffer),
                            entry(6, &levels.buffer),
                            entry(7, &camera.buffer),
                            entry(8, &output),
                            entry(
                                9,
                                suppressed.map_or(&self.no_suppression, |handle| &handle.buffer),
                            ),
                        ],
                    });
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Authored LOD Selection"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.dispatch_workgroups(component_rows.div_ceil(WORKGROUP), 1, 1);
                    drop(pass);
                    recorded = true;

                    if self.pending_readback.is_none() {
                        let bytes = u64::from(self.output_rows) * SELECTION_ROW_BYTES;
                        let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("Authored LOD Demand Readback"),
                            size: bytes,
                            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, bytes);
                        let (start_mapping, mapped) = mpsc::channel();
                        self.pending_readback = Some(PendingReadback {
                            buffer: staging,
                            rows: self.output_rows,
                            start_mapping: Some(start_mapping),
                            mapped,
                        });
                    }
                }
            }
        }

        recorded |= self.refresh_asset_tables(ctx);
        let selection = BufferHandle {
            buffer: output,
            epoch: self.output_epoch,
            row_bytes: SELECTION_ROW_BYTES,
            content_generation: self.output_generation,
        };
        let asset_table = table_handle(
            self.asset_table.as_ref().expect("asset table allocated"),
            self.asset_table_epoch,
            ASSET_ROW_BYTES,
            self.asset_table_generation,
        );
        let section_table = table_handle(
            self.section_table.as_ref().expect("section table allocated"),
            self.section_table_epoch,
            SECTION_ROW_BYTES,
            self.section_table_generation,
        );
        let mut buffers = vec![
            (LOD_SELECTIONS_KEY, selection.clone()),
            (LOD_ASSETS_KEY, asset_table.clone()),
            (LOD_SECTIONS_KEY, section_table.clone()),
        ];
        self.append_geometry_pool_outputs(&mut buffers);
        if let (Some(objects), Some(owners), Some(section_handles)) = (
            projection.get(OBJECTS_KEY),
            projection.get(self.keys.owners),
            projection.get(OBJECT_SECTIONS_KEY),
        ) {
            if let Some((patched, apply_recorded)) = self.apply_lod_to_objects(
                ctx,
                encoder,
                objects,
                owners,
                section_handles,
                &selection,
                &asset_table,
                &section_table,
            ) {
                buffers.push((OBJECTS_KEY, patched));
                recorded |= apply_recorded;
                if let Some(source_structure) = projection.get(OBJECTS_STRUCTURE_KEY) {
                    let structure_signature = signature(&[
                        source_structure,
                        &asset_table,
                        &section_table,
                    ]) ^ self.lod_selection_generation.rotate_left(37);
                    if self.last_structure_signature != Some(structure_signature) {
                        self.last_structure_signature = Some(structure_signature);
                        self.output_structure_generation = self
                            .output_structure_generation
                            .wrapping_add(1)
                            .max(1);
                    }
                    buffers.push((
                        OBJECTS_STRUCTURE_KEY,
                        BufferHandle {
                            buffer: source_structure.buffer.clone(),
                            epoch: source_structure.epoch,
                            row_bytes: source_structure.row_bytes,
                            content_generation: self.output_structure_generation,
                        },
                    ));
                }
            }
        }
        SceneDerivationOutput {
            buffers,
            flushed: Vec::new(),
            recorded,
        }
    }

    fn after_submit(&mut self) {
        let Some(pending) = &mut self.pending_readback else {
            return;
        };
        let Some(sender) = pending.start_mapping.take() else {
            return;
        };
        pending
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
    }
}

impl AuthoredLodSelection {
    fn refresh_resident_assets(&mut self) {
        let Some(mirror) = self.mirror.as_ref() else {
            return;
        };
        self.active_assets
            .retain(|asset, active| {
                self.requested_assets.contains(asset)
                    && helio_component::components::mesh_asset_content_version(*asset)
                        == Some(active._mesh.content_id)
            });
        for asset in self.requested_assets.iter().copied() {
            if self.active_assets.contains_key(&asset) {
                continue;
            }
            let Some(residency) = helio_component::components::mesh_asset_residency(asset) else {
                continue;
            };
            if !residency.is_resident() {
                continue;
            }
            let Ok(mesh) = residency.get() else {
                continue;
            };
            match helio_component::components::acquire_mesh_asset_gpu_lease(
                mirror,
                asset,
                &mesh,
            ) {
                Ok(gpu) => {
                    self.active_assets
                        .insert(asset, ActiveLodAsset { _mesh: mesh, _gpu: gpu });
                }
                Err(error) => tracing::warn!(?asset, %error, "authored LOD GPU residency failed; drawing the base mesh"),
            }
        }
    }

    fn poll_asset_demand(&mut self) {
        let Some(pending) = self.pending_readback.take() else {
            return;
        };
        match pending.mapped.try_recv() {
            Ok(Ok(())) => {
                let view = pending
                    .buffer
                    .slice(..)
                    .get_mapped_range()
                    .expect("mapped LOD demand range");
                let rows = view
                    .chunks_exact(SELECTION_ROW_BYTES as usize)
                    .take(pending.rows as usize)
                    .map(bytemuck::pod_read_unaligned::<LodSelectionRow>)
                    .filter(|row| row.valid != 0)
                    .map(|row| asset_id(row.mesh_asset))
                    .filter(|asset| !asset.is_zero())
                    .collect::<HashSet<_>>();
                let selection_signature = view
                    .chunks_exact(SELECTION_ROW_BYTES as usize)
                    .take(pending.rows as usize)
                    .map(bytemuck::pod_read_unaligned::<LodSelectionRow>)
                    .fold(0xcbf2_9ce4_8422_2325u64, |mut hash, row| {
                        for word in [
                            row.valid,
                            row.level_index,
                            row.mesh_asset[0],
                            row.mesh_asset[1],
                            row.mesh_asset[2],
                            row.mesh_asset[3],
                        ] {
                            hash ^= u64::from(word);
                            hash = hash.wrapping_mul(0x0100_0000_01b3);
                        }
                        hash
                    });
                drop(view);
                pending.buffer.unmap();
                for asset in rows.difference(&self.requested_assets).copied() {
                    // Prefetch starts on SceneDB's asset loader workers. The
                    // SceneDB residency cache still owns the RAM budget; this
                    // view retains only asset identities, never every payload.
                    drop(helio_component::components::prefetch_mesh_asset(asset));
                }
                self.requested_assets = rows;
                if self.lod_selection_signature != Some(selection_signature) {
                    self.lod_selection_signature = Some(selection_signature);
                    self.lod_selection_generation =
                        self.lod_selection_generation.wrapping_add(1).max(1);
                }
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "authored LOD demand readback failed");
            }
            Err(TryRecvError::Empty) => {
                self.pending_readback = Some(pending);
            }
            Err(TryRecvError::Disconnected) => {}
        }
    }
}

fn layout_entry(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn valid_row_size(handle: &BufferHandle, expected: u64) -> bool {
    handle.row_bytes == 0 || handle.row_bytes == expected
}

fn signature(handles: &[&BufferHandle]) -> u64 {
    handles
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |mut hash, handle| {
            for value in [
                handle.epoch,
                handle.content_generation,
                handle.buffer.size(),
            ] {
                hash ^= value;
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
            hash
        })
}

fn origin_pair(origin: Option<glam::DVec3>) -> ([f32; 4], [f32; 4]) {
    let origin = origin.unwrap_or(glam::DVec3::ZERO);
    let hi = origin.as_vec3();
    let lo = (origin - hi.as_dvec3()).as_vec3();
    (hi.extend(0.0).to_array(), lo.extend(0.0).to_array())
}

fn asset_id(words: [u32; 4]) -> AssetId {
    AssetId(
        u128::from(words[0])
            | (u128::from(words[1]) << 32)
            | (u128::from(words[2]) << 64)
            | (u128::from(words[3]) << 96),
    )
}

fn asset_words(asset: AssetId) -> [u32; 4] {
    let bytes = asset.0.to_le_bytes();
    std::array::from_fn(|word| {
        let offset = word * 4;
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
    })
}

fn table_handle(
    buffer: &wgpu::Buffer,
    epoch: u64,
    row_bytes: u64,
    content_generation: u64,
) -> BufferHandle {
    BufferHandle {
        buffer: buffer.clone(),
        epoch,
        row_bytes,
        content_generation,
    }
}

#[allow(dead_code)]
const _: [(); SELECTION_ROW_BYTES as usize] = [(); std::mem::size_of::<LodSelectionRow>()];
const _: [(); ASSET_ROW_BYTES as usize] = [(); std::mem::size_of::<AssetGeometryRow>()];
const _: [(); SECTION_ROW_BYTES as usize] = [(); std::mem::size_of::<AssetSectionRow>()];

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::{Pod, Zeroable};
    use helio_component::components::LODLevelGpu;
    use helio_core::GpuCameraUniforms;
    use std::sync::Arc;

    fn gpu() -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
        static DEVICE_CREATION: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = DEVICE_CREATION.lock().unwrap_or_else(|p| p.into_inner());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("GPU LOD test requires a local adapter");
        // Selection binds nine buffers; the default limit is eight per stage.
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .expect("GPU LOD test device");
        (Arc::new(device), Arc::new(queue))
    }

    fn upload<T: Pod>(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: BufferKey,
        rows: &[T],
    ) -> BufferHandle {
        let bytes = bytemuck::cast_slice(rows);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Authored LOD test input"),
            size: bytes.len().max(4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        if !bytes.is_empty() {
            queue.write_buffer(&buffer, 0, bytes);
        }
        let row_bytes = if rows.is_empty() {
            0
        } else {
            (bytes.len() / rows.len()) as u64
        };
        let _ = key;
        BufferHandle {
            buffer,
            epoch: 1,
            row_bytes,
            content_generation: 1,
        }
    }

    fn selection(
        derivation: &mut AuthoredLodSelection,
        device: &Arc<wgpu::Device>,
        queue: &Arc<wgpu::Queue>,
        projection: &mut helio_core::SceneBufferProjection,
        world_origin: Option<glam::DVec3>,
    ) -> LodSelectionRow {
        let mut encoder = device.create_command_encoder(&Default::default());
        let output = derivation.derive(
            &SceneDerivationContext {
                device,
                queue,
                inputs: projection,
                world_origin,
            },
            &mut encoder,
        );
        let selected = output
            .buffers
            .into_iter()
            .find(|(key, _)| *key == LOD_SELECTIONS_KEY)
            .expect("selection output")
            .1;
        queue.submit([encoder.finish()]);
        derivation.after_submit();
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Authored LOD selection readback"),
            size: SELECTION_ROW_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&selected.buffer, 0, &staging, 0, SELECTION_ROW_BYTES);
        queue.submit([encoder.finish()]);
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| result.expect("map selection"));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll selection");
        let bytes = slice.get_mapped_range().expect("mapped selection range");
        let row = bytemuck::pod_read_unaligned::<LodSelectionRow>(&bytes);
        drop(bytes);
        staging.unmap();
        row
    }

    #[test]
    fn gpu_selection_changes_with_the_current_view_camera() {
        let (device, queue) = gpu();
        let mut lod = AuthoredLodSelection::new(
            &device,
            AuthoredLodKeys {
                owners: BufferKey::of("owners"),
                generations: BufferKey::of("generations"),
                hidden: BufferKey::of("hidden"),
                transforms: BufferKey::of("transforms"),
                level_handles: BufferKey::of("handles"),
                levels: BufferKey::of("levels"),
                camera: BufferKey::of("camera"),
            },
            None,
        );
        let mut camera = GpuCameraUniforms::zeroed();
        camera.position_near = [0.0, 0.0, 0.0, 0.1];
        let keys = lod.keys;
        let mut projection = helio_core::SceneBufferProjection::from_handles([
            (
                keys.owners,
                upload(&device, &queue, keys.owners, &[[0u32, 7, 1]]),
            ),
            (
                keys.generations,
                upload(&device, &queue, keys.generations, &[7u32]),
            ),
            (keys.hidden, upload(&device, &queue, keys.hidden, &[0u32])),
            (
                keys.transforms,
                upload(
                    &device,
                    &queue,
                    keys.transforms,
                    &[[
                        0.0f32, 0.0, 100.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0,
                    ]],
                ),
            ),
            (
                keys.level_handles,
                upload(&device, &queue, keys.level_handles, &[[0u32, 2]]),
            ),
            (
                keys.levels,
                upload(
                    &device,
                    &queue,
                    keys.levels,
                    &[
                        LODLevelGpu {
                            distance_threshold: 0.0,
                            lod_bias: 0.0,
                            mesh_asset: [1, 0, 0, 0],
                        },
                        LODLevelGpu {
                            distance_threshold: 50.0,
                            lod_bias: 0.0,
                            mesh_asset: [2, 0, 0, 0],
                        },
                    ],
                ),
            ),
            (keys.camera, upload(&device, &queue, keys.camera, &[camera])),
        ]);

        let far = selection(&mut lod, &device, &queue, &mut projection, None);
        assert_eq!(far.level_index, 1);
        assert_eq!(far.mesh_asset, [2, 0, 0, 0]);
        assert_eq!(far.camera_distance, 100.0);

        camera.position_near = [0.0, 0.0, 100.0, 0.1];
        let camera_handle = upload(&device, &queue, keys.camera, &[camera]);
        projection.insert(
            keys.camera,
            BufferHandle {
                content_generation: 2,
                ..camera_handle
            },
        );
        let near = selection(&mut lod, &device, &queue, &mut projection, None);
        assert_eq!(near.level_index, 0);
        assert_eq!(near.mesh_asset, [1, 0, 0, 0]);
        assert_eq!(near.camera_distance, 0.0);

        // Another system (virtual geometry) takes over object 0's detail: no
        // authored level is selected for it until the flag clears.
        projection.insert(
            LOD_SUPPRESSED_KEY,
            upload(&device, &queue, LOD_SUPPRESSED_KEY, &[1u32, 0]),
        );
        let suppressed = selection(&mut lod, &device, &queue, &mut projection, None);
        assert_eq!(suppressed.valid, 0);
        projection.insert(
            LOD_SUPPRESSED_KEY,
            BufferHandle {
                content_generation: 2,
                ..upload(&device, &queue, LOD_SUPPRESSED_KEY, &[0u32; 2])
            },
        );
        let released = selection(&mut lod, &device, &queue, &mut projection, None);
        assert_eq!((released.valid, released.level_index), (1, 0));

        // A camera-relative frame: the camera rows hold the eye relative to
        // the frame's world origin. The eye at the world origin again, with
        // the frame's origin 60 m away, measures 100 m to the object, both
        // from world-space transform rows and from origin-relative ones.
        let origin = glam::DVec3::new(0.0, 0.0, 60.0);
        camera.position_near = [0.0, 0.0, -60.0, 0.1];
        projection.insert(
            keys.camera,
            BufferHandle {
                content_generation: 3,
                ..upload(&device, &queue, keys.camera, &[camera])
            },
        );
        let world_rows = selection(&mut lod, &device, &queue, &mut projection, Some(origin));
        assert_eq!((world_rows.level_index, world_rows.camera_distance), (1, 100.0));
        projection.insert(
            keys.transforms,
            BufferHandle {
                content_generation: 2,
                ..upload(
                    &device,
                    &queue,
                    keys.transforms,
                    &[[
                        0.0f32, 0.0, 40.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0,
                    ]],
                )
            },
        );
        projection.insert(
            BufferKey::of("transforms_origin_relative"),
            upload(&device, &queue, BufferKey::of("transforms_origin_relative"), &[0u32]),
        );
        let relative_rows = selection(&mut lod, &device, &queue, &mut projection, Some(origin));
        assert_eq!((relative_rows.level_index, relative_rows.camera_distance), (1, 100.0));
    }
}
