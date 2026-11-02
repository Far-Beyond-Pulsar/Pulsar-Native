//! The virtual-geometry scene derivation (plan:
//! `.agents/plans/virtual-geometry.md` section 3 items 1, 3 and 7).
//!
//! # Why it lives here
//!
//! A [`SceneDerivation`] is the sanctioned way to add derived scene data
//! without touching `helio-core` or the scene join. This one has to know the
//! row layout of [`VirtualGeometryComponent`](crate::components::VirtualGeometryComponent)
//! (`vg_instance_overrides`) and the scene join's `static_objects` rows;
//! `helio-default-passes` does not depend on this crate, and the passes
//! (`helio-pass-vg-*`) must never depend on it. Keeping the derivation next to
//! its component means the component's GPU ABI has exactly one reader. The
//! passes see only the output buffers below, by [`BufferKey`].
//!
//! # What it does
//!
//! Each frame any input changed (and never otherwise) it joins, on the GPU,
//!
//! * every static-mesh instance the scene join placed (`static_objects`,
//!   `static_mesh_draw_sections::handles`, `builtin_mesh_vertex::handles`),
//! * the optional [`VirtualGeometryComponent`](crate::components::VirtualGeometryComponent)
//!   of the instance's owner object (`vg_instance_overrides` rows, linked to
//!   owners through `component_owners`), and
//! * asset readiness from [`VG_ASSET_READY_KEY`],
//!
//! and publishes:
//!
//! | key | rows | content |
//! |---|---|---|
//! | [`VG_INSTANCES_KEY`] `"vg_instances"` | 16 B [`GpuVgInstance`](helio_vg_format::GpuVgInstance) | one row per **VG-active** instance, compacted in static-mesh-instance order. Unused tail rows are zero: read [`VG_INSTANCE_COUNT_KEY`] |
//! | [`VG_INSTANCE_COUNT_KEY`] `"vg_instance_count"` | one 16 B row | word 0 = number of rows in `vg_instances` |
//! | [`LOD_SUPPRESSED_KEY`] `"lod_suppressed"` | `u32` per object entity index | non-zero = the authored-LOD selection must skip this object |
//!
//! # VG-active
//!
//! An instance is VG-active iff virtual geometry is globally on
//! ([`VgInstanceControl::set_enabled`]), the scene join placed it, its owner's
//! `VirtualGeometryComponent` (if any) is `enabled`, and its mesh's asset is
//! ready. Active rows carry `INST_ENABLED | INST_LOD_SUPPRESS`, the authored
//! override bits (shadow caster, force root only, tier lock), `INST_CONE_CULL`
//! when the instance transform is a rotation, translation and a single
//! non-zero uniform positive scale (cone culling is invalid under non-uniform
//! scale or mirroring), the override's `lod_error_scale` (1 when absent) and
//! `object_row`: the row in the scene join's `static_objects` of the
//! instance's **first section**; its `section_handles[i].count` sections
//! occupy `object_row .. object_row + count`.
//!
//! # `lod_suppressed`
//!
//! Plan item 7: the authored LOD and virtual geometry are alternatives. The
//! derivation neither knows nor names the authored-LOD types: it publishes a
//! generic per-object flag buffer and the authored-LOD selection honours the
//! key. The buffer is indexed by the **object entity index** (the
//! `owner_index` of `component_owners` rows), the same index space as
//! `object_transforms`. An object with several mesh instances is suppressed
//! if any of them is VG-active. It is published every frame (all zero when
//! nothing is VG-active), so a consumer never sees stale data.
//!
//! Install this derivation after the scene join and before the authored-LOD
//! selection.
//!
//! # `vg_asset_ready` (published by `helio-vg-stream`)
//!
//! Sorted ascending by `vertex_offset`, 16-byte [`VgAssetReadyRow`] rows,
//! keyed by the **interned vertex range** (`builtin_mesh_vertex::handles`
//! offset) of the mesh asset: the one identity a static-mesh instance carries
//! on the GPU. Rows past the last asset are [`VgAssetReadyRow::PADDING`]
//! (`vertex_offset == u32::MAX`, so they sort last). `asset_slot` is the
//! slot in the stream's asset table that [`GpuVgInstance::asset_slot`]
//! indexes; [`VG_ASSET_READY_FLAG_READY`] is set once the asset's `.vgeo` root
//! pages and hierarchy are resident.
//!
//! [`GpuVgInstance::asset_slot`]: helio_vg_format::GpuVgInstance

use helio_core::{
    BufferHandle, BufferKey, SceneDerivation, SceneDerivationContext, SceneDerivationOutput,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use helio_vg_format::{
    GpuVgInstance, VgAssetReadyRow, INST_LOD_SUPPRESS, VG_ASSET_READY_FLAG_READY,
};

/// The VG instance rows (`GpuVgInstance`, 16 B each), one per VG-active
/// instance. See the module doc.
pub const VG_INSTANCES_KEY: BufferKey = BufferKey::of("vg_instances");
/// One 16-byte row; word 0 is the number of valid rows in [`VG_INSTANCES_KEY`].
pub const VG_INSTANCE_COUNT_KEY: BufferKey = BufferKey::of("vg_instance_count");
/// Generic authored-LOD suppression flags, one `u32` per object entity index.
pub const LOD_SUPPRESSED_KEY: BufferKey = BufferKey::of("lod_suppressed");
/// One `u32` per `static_objects` row: non-zero = the row belongs to a VG-active
/// instance, so the classic main view must not draw it (the object batch honours
/// the key generically; shadows keep using the classic geometry as the proxy).
pub const CLASSIC_SUPPRESSED_KEY: BufferKey = BufferKey::of("classic_suppressed");
/// Asset readiness rows published by `helio-vg-stream`. See the module doc.
pub const VG_ASSET_READY_KEY: BufferKey = BufferKey::of("vg_asset_ready");


pub const VG_INSTANCE_ROW_BYTES: u64 = 16;
pub const VG_INSTANCE_COUNT_ROW_BYTES: u64 = 16;
pub const LOD_SUPPRESSED_ROW_BYTES: u64 = 4;
pub const CLASSIC_SUPPRESSED_ROW_BYTES: u64 = 4;
pub const VG_ASSET_READY_ROW_BYTES: u64 = 16;
const OWNER_ROW_BYTES: u64 = 12;
const GENERATION_ROW_BYTES: u64 = 4;
const HANDLE_ROW_BYTES: u64 = 8;
const OVERRIDE_ROW_BYTES: u64 = 8;
/// A scene-join `static_objects` row (`StaticObjectComponent`).
const OBJECT_ROW_BYTES: u64 = 252;
const WORKGROUP: u32 = 64;


const _: () = assert!(std::mem::size_of::<VgAssetReadyRow>() as u64 == VG_ASSET_READY_ROW_BYTES);
const _: () = assert!(std::mem::size_of::<GpuVgInstance>() as u64 == VG_INSTANCE_ROW_BYTES);
const _: () = assert!(
    std::mem::size_of::<crate::components::VgOverrideGpu>() as u64 == OVERRIDE_ROW_BYTES
);

/// Where the frontend's authored rows live.
#[derive(Clone, Copy, Debug)]
pub struct VgInstanceKeys {
    /// Component -> owner links, keyed by the component entity.
    pub owners: BufferKey,
    /// Entity generations.
    pub generations: BufferKey,
    /// `VirtualGeometryComponent` handle table (`vg_instance_overrides::handles`).
    pub override_handles: BufferKey,
    /// `VirtualGeometryComponent` row pool (`vg_instance_overrides`).
    pub overrides: BufferKey,
    /// Static-mesh instances' section ranges (`static_mesh_draw_sections::handles`).
    pub section_handles: BufferKey,
    /// Static-mesh instances' interned vertex ranges (`builtin_mesh_vertex::handles`).
    pub vertex_handles: BufferKey,
}

impl VgInstanceKeys {
    /// The keys the editor's SceneDB publishes (the same names
    /// `engine_backend::scene::helio_bridge` hands the scene join).
    pub const fn standard() -> Self {
        Self {
            owners: BufferKey::of("component_owners"),
            generations: helio_core::ENTITY_GENERATIONS_KEY,
            override_handles: BufferKey::of("vg_instance_overrides::handles"),
            overrides: BufferKey::of("vg_instance_overrides"),
            section_handles: BufferKey::of("static_mesh_draw_sections::handles"),
            vertex_handles: BufferKey::of("builtin_mesh_vertex::handles"),
        }
    }
}

/// The scene join's published object rows (`StaticObjectComponent`).
const OBJECTS_KEY: BufferKey = BufferKey::of("static_objects");

/// Runtime switch for the derivation, from `VirtualGeometrySettings`: when
/// off, no instance is VG-active and nothing is suppressed. Cloneable; hold
/// one outside the renderer and flip it as the setting changes.
#[derive(Clone, Debug)]
pub struct VgInstanceControl(Arc<AtomicBool>);

impl VgInstanceControl {
    pub fn set_enabled(&self, enabled: bool) {
        self.0.store(enabled, Ordering::Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// The shader as compiled: the shared VG declarations, then the derivation.
pub fn shader_source() -> String {
    format!(
        "{}\n{}",
        helio_vg_format::WGSL,
        include_str!("virtual_geometry_instances.wgsl")
    )
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    instance_rows: u32,
    override_rows: u32,
    flags: u32,
    _pad: u32,
}

/// A buffer the derivation owns and publishes.
pub(crate) struct Output {
    pub(crate) label: &'static str,
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) rows: u32,
    pub(crate) row_bytes: u64,
    pub(crate) epoch: u64,
    pub(crate) content_generation: u64,
}

impl Output {
    pub(crate) fn new(device: &wgpu::Device, label: &'static str, row_bytes: u64) -> Self {
        Self {
            label,
            buffer: Self::allocate(device, label, row_bytes, 1),
            rows: 1,
            row_bytes,
            epoch: 1,
            content_generation: 1,
        }
    }

    pub(crate) fn allocate(device: &wgpu::Device, label: &str, row_bytes: u64, rows: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: row_bytes * u64::from(rows.max(1)),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    /// Hold at least `rows` rows (power-of-two growth), returning whether the
    /// buffer was replaced.
    pub(crate) fn ensure(&mut self, device: &wgpu::Device, rows: u32) -> bool {
        if rows <= self.rows {
            return false;
        }
        let rows = rows.next_power_of_two();
        self.buffer = Self::allocate(device, self.label, self.row_bytes, rows);
        self.rows = rows;
        self.epoch += 1;
        true
    }

    pub(crate) fn handle(&self) -> BufferHandle {
        BufferHandle {
            buffer: self.buffer.clone(),
            epoch: self.epoch,
            row_bytes: self.row_bytes,
            content_generation: self.content_generation,
        }
    }
}

/// The virtual-geometry scene derivation. See the module doc.
pub struct VirtualGeometryInstances {
    keys: VgInstanceKeys,
    control: VgInstanceControl,
    scatter: wgpu::ComputePipeline,
    classify: wgpu::ComputePipeline,
    compact: wgpu::ComputePipeline,
    uniforms: wgpu::Buffer,
    /// Bound in place of an input that is not registered (yet).
    empty: wgpu::Buffer,
    /// Per object entity: its `VirtualGeometryComponent` row (flags | present bit, scale bits).
    by_owner: Output,
    /// Per static-mesh instance: the candidate row (flags 0 = inactive).
    candidates: Output,
    instances: Output,
    count: Output,
    suppressed: Output,
    classic: Output,
    last_signature: Option<u64>,
    reported: Vec<BufferKey>,
}

impl VirtualGeometryInstances {
    pub fn new(device: &wgpu::Device, keys: VgInstanceKeys) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("VG Instances"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            keys,
            control: VgInstanceControl(Arc::new(AtomicBool::new(true))),
            scatter: pipeline("cs_scatter_overrides"),
            classify: pipeline("cs_classify"),
            compact: pipeline("cs_compact"),
            uniforms: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VG Instances Uniforms"),
                size: std::mem::size_of::<Uniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            empty: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VG Instances Empty Input"),
                size: 256,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            by_owner: Output::new(device, "VG Overrides By Owner", 8),
            candidates: Output::new(device, "VG Instance Candidates", VG_INSTANCE_ROW_BYTES),
            instances: Output::new(device, "VG Instances", VG_INSTANCE_ROW_BYTES),
            count: Output::new(device, "VG Instance Count", VG_INSTANCE_COUNT_ROW_BYTES),
            suppressed: Output::new(device, "LOD Suppressed", LOD_SUPPRESSED_ROW_BYTES),
            classic: Output::new(device, "Classic Suppressed", CLASSIC_SUPPRESSED_ROW_BYTES),
            last_signature: None,
            reported: Vec::new(),
        }
    }

    /// The runtime enable switch (on by default).
    pub fn control(&self) -> VgInstanceControl {
        self.control.clone()
    }

    /// `key`'s buffer when registered with the expected row size. A size
    /// mismatch is reported once and treated as missing.
    fn input<'a>(
        &mut self,
        inputs: &'a helio_core::SceneBufferProjection,
        key: BufferKey,
        row_bytes: u64,
    ) -> Option<&'a BufferHandle> {
        let handle = inputs.get(key)?;
        if handle.row_bytes != 0 && handle.row_bytes != row_bytes {
            if !self.reported.contains(&key) {
                self.reported.push(key);
                tracing::error!(
                    "virtual geometry: buffer {key:?} has {}-byte rows, expected {row_bytes}; \
                     nothing is virtual-geometry active",
                    handle.row_bytes
                );
            }
            return None;
        }
        Some(handle)
    }

    fn outputs(&self) -> Vec<(BufferKey, BufferHandle)> {
        vec![
            (VG_INSTANCES_KEY, self.instances.handle()),
            (VG_INSTANCE_COUNT_KEY, self.count.handle()),
            (LOD_SUPPRESSED_KEY, self.suppressed.handle()),
            (CLASSIC_SUPPRESSED_KEY, self.classic.handle()),
        ]
    }
}

fn rows_of(handle: &BufferHandle, row_bytes: u64) -> u32 {
    (handle.buffer.size() / row_bytes).min(u64::from(u32::MAX)) as u32
}

fn signature(handles: &[Option<&BufferHandle>], extra: &[u64]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    };
    for handle in handles {
        match handle {
            Some(handle) => {
                mix(handle.epoch);
                mix(handle.content_generation);
                mix(handle.buffer.size());
            }
            None => mix(u64::MAX),
        }
    }
    for value in extra {
        mix(*value);
    }
    hash
}

fn bind(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    entries: &[wgpu::BindGroupEntry<'_>],
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("VG Instances"),
        layout: &pipeline.get_bind_group_layout(0),
        entries,
    })
}

fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

impl SceneDerivation for VirtualGeometryInstances {
    fn name(&self) -> &'static str {
        "VirtualGeometryInstances"
    }

    fn derive(
        &mut self,
        ctx: &SceneDerivationContext<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) -> SceneDerivationOutput {
        let keys = self.keys;
        let inputs = ctx.inputs;
        let owners = self.input(inputs, keys.owners, OWNER_ROW_BYTES);
        let generations = self.input(inputs, keys.generations, GENERATION_ROW_BYTES);
        let override_handles = self.input(inputs, keys.override_handles, HANDLE_ROW_BYTES);
        let overrides = self.input(inputs, keys.overrides, OVERRIDE_ROW_BYTES);
        let section_handles = self.input(inputs, keys.section_handles, HANDLE_ROW_BYTES);
        let vertex_handles = self.input(inputs, keys.vertex_handles, HANDLE_ROW_BYTES);
        let objects = self.input(inputs, OBJECTS_KEY, OBJECT_ROW_BYTES);
        let asset_ready = self.input(inputs, VG_ASSET_READY_KEY, VG_ASSET_READY_ROW_BYTES);
        let enabled = self.control.enabled();

        let instance_rows = section_handles.map_or(0, |h| rows_of(h, HANDLE_ROW_BYTES));
        let entity_rows = generations.map_or(0, |h| rows_of(h, GENERATION_ROW_BYTES));
        let override_rows = override_handles.map_or(0, |h| rows_of(h, HANDLE_ROW_BYTES));

        let grew = self.candidates.ensure(ctx.device, instance_rows)
            | self.instances.ensure(ctx.device, instance_rows);
        let grew = grew
            | self.by_owner.ensure(ctx.device, entity_rows)
            | self.suppressed.ensure(ctx.device, entity_rows);
        let object_rows = objects.map_or(0, |h| rows_of(h, OBJECT_ROW_BYTES));
        let grew = grew | self.classic.ensure(ctx.device, object_rows);

        let signature = signature(
            &[
                owners,
                generations,
                override_handles,
                overrides,
                section_handles,
                vertex_handles,
                objects,
                asset_ready,
            ],
            &[
                u64::from(enabled),
                self.candidates.epoch,
                self.instances.epoch,
                self.by_owner.epoch,
                self.suppressed.epoch,
                self.classic.epoch,
            ],
        );
        let mut recorded = false;
        if grew || self.last_signature != Some(signature) {
            helio_core::cpu_scope!("VirtualGeometryInstances");
            self.last_signature = Some(signature);
            recorded = true;
            encoder.clear_buffer(&self.by_owner.buffer, 0, None);
            encoder.clear_buffer(&self.candidates.buffer, 0, None);
            encoder.clear_buffer(&self.instances.buffer, 0, None);
            encoder.clear_buffer(&self.count.buffer, 0, None);
            encoder.clear_buffer(&self.suppressed.buffer, 0, None);
            encoder.clear_buffer(&self.classic.buffer, 0, None);
            if let (
                Some(owners),
                Some(generations),
                Some(section_handles),
                Some(vertex_handles),
                Some(objects),
            ) = (owners, generations, section_handles, vertex_handles, objects)
            {
                let empty = &self.empty;
                let override_handles = override_handles.map_or(empty, |h| &h.buffer);
                let overrides = overrides.map_or(empty, |h| &h.buffer);
                let asset_ready = asset_ready.map_or(empty, |h| &h.buffer);
                ctx.queue.write_buffer(
                    &self.uniforms,
                    0,
                    bytemuck::bytes_of(&Uniforms {
                        instance_rows: instance_rows.min(self.candidates.rows),
                        override_rows,
                        flags: u32::from(enabled),
                        _pad: 0,
                    }),
                );
                let device: &wgpu::Device = ctx.device;
                let scatter = bind(
                    device,
                    &self.scatter,
                    &[
                        entry(0, &self.uniforms),
                        entry(1, &owners.buffer),
                        entry(2, &generations.buffer),
                        entry(3, override_handles),
                        entry(4, overrides),
                        entry(9, &self.by_owner.buffer),
                    ],
                );
                let classify = bind(
                    device,
                    &self.classify,
                    &[
                        entry(0, &self.uniforms),
                        entry(1, &owners.buffer),
                        entry(5, &section_handles.buffer),
                        entry(6, &vertex_handles.buffer),
                        entry(7, &objects.buffer),
                        entry(8, asset_ready),
                        entry(9, &self.by_owner.buffer),
                        entry(10, &self.candidates.buffer),
                        entry(11, &self.suppressed.buffer),
                        entry(14, &self.classic.buffer),
                    ],
                );
                let compact = bind(
                    device,
                    &self.compact,
                    &[
                        entry(0, &self.uniforms),
                        entry(10, &self.candidates.buffer),
                        entry(12, &self.instances.buffer),
                        entry(13, &self.count.buffer),
                    ],
                );
                let mut run = |label: &'static str,
                               pipeline: &wgpu::ComputePipeline,
                               bind_group: &wgpu::BindGroup,
                               workgroups: u32| {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(label),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, bind_group, &[]);
                    pass.dispatch_workgroups(workgroups, 1, 1);
                };
                run(
                    "VG Scatter Overrides",
                    &self.scatter,
                    &scatter,
                    override_rows.div_ceil(WORKGROUP).max(1),
                );
                run(
                    "VG Classify Instances",
                    &self.classify,
                    &classify,
                    instance_rows.div_ceil(WORKGROUP).max(1),
                );
                run("VG Compact Instances", &self.compact, &compact, 1);
            }
            self.instances.content_generation += 1;
            self.count.content_generation += 1;
            self.suppressed.content_generation += 1;
            self.classic.content_generation += 1;
        }
        SceneDerivationOutput {
            buffers: self.outputs(),
            flushed: Vec::new(),
            recorded,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader parses and validates (no device needed), and its structs
    /// have the sizes of the Rust rows.
    #[test]
    fn shader_validates_and_layouts_match() {
        let source = shader_source();
        let module = naga::front::wgsl::parse_str(&source).expect("parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("validate");
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).expect("layout");
        let size_of_struct = |name: &str| {
            let (handle, _) = module
                .types
                .iter()
                .find(|(_, t)| t.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("missing {name}"));
            layouter[handle].size as usize
        };
        assert_eq!(size_of_struct("AssetReady"), std::mem::size_of::<VgAssetReadyRow>());
        assert_eq!(
            size_of_struct("Override"),
            std::mem::size_of::<crate::components::VgOverrideGpu>()
        );
        assert_eq!(size_of_struct("VgInstance"), std::mem::size_of::<GpuVgInstance>());
        assert_eq!(size_of_struct("Owner") as u64, OWNER_ROW_BYTES);
        assert_eq!(size_of_struct("Uniforms"), std::mem::size_of::<Uniforms>());
    }

    #[test]
    fn padding_rows_sort_last_and_never_match() {
        let pad = VgAssetReadyRow::PADDING;
        assert_eq!(pad.vertex_count, 0);
        assert!(pad.vertex_offset > 0x00ff_ffff);
    }

    #[test]
    fn standard_keys_name_the_published_buffers() {
        let keys = VgInstanceKeys::standard();
        assert_eq!(keys.overrides, BufferKey::of("vg_instance_overrides"));
        assert_eq!(keys.override_handles, BufferKey::of("vg_instance_overrides::handles"));
    }
}
