//! Sparse asynchronous queries against the rendered Helio water surface.
//!
//! Query work shares the surface shader's displacement/shoreline functions,
//! and is recorded after the FFT, shallow-water interaction, and mip stages.
//! Readback uses three bounded staging generations. A saturated ring drops a
//! request batch instead of waiting for the GPU.

use std::sync::{Arc, Mutex};

use bytemuck::{Pod, Zeroable};

/// The surface query kernel (`water_query.wgsl`), composed into the surface
/// shader (`//!use water_query`).
pub const WATER_QUERY_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!("//!use water_query", "../shaders/water_query.wgsl");

pub const MAX_WATER_SURFACE_QUERIES: usize = 256;
const STAGING_GENERATIONS: usize = 3;
const QUERY_INPUT_BYTES: u64 = 32;
const QUERY_OUTPUT_BYTES: u64 = 48;
const MAX_STAGING_BYTES: u64 = MAX_WATER_SURFACE_QUERIES as u64 * QUERY_OUTPUT_BYTES;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterSurfaceQueryPoint {
    /// Stable application-owned key. It is echoed through readback and is not
    /// a SceneDB row, so row recycling cannot alias a removed body.
    pub body_key: u64,
    pub point_index: u32,
    /// Absolute world coordinates. Converted to Helio's frame-relative
    /// coordinates immediately before upload, preserving precision on large
    /// worlds.
    pub world_position: glam::DVec3,
    /// Index into the current fixed-capacity `water_volumes` GPU buffer.
    pub volume_index: u32,
    /// Same LOD selector used by the surface shader (`cell_size`). Values
    /// below zero are clamped to the finest FFT mip.
    pub cell_size: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterSurfaceQueryResult {
    pub body_key: u64,
    pub point_index: u32,
    pub world_position: glam::DVec3,
    pub surface_height: f64,
    pub normal: glam::Vec3,
    /// Estimated from consecutive water displacements (`surface_position -
    /// query_position`) for this body/point key. Subtracting query-point
    /// movement avoids folding rigid-body translation into water drag. The
    /// estimate still includes spatial variation when a moving floater samples
    /// different parts of the wave; it is zero until a prior sample exists.
    pub surface_velocity: glam::Vec3,
    /// Renderer-owned frame number and the spectral loop's simulation time at
    /// the exact GPU query dispatch (wraps every 300 seconds), not callback
    /// arrival time. It stays fixed when the host water clock pauses.
    pub sampled_frame: u64,
    pub sampled_time: f64,
}

/// Host-side adapter point for the engine's physics runtime. The Helio pass
/// intentionally does not depend on `pulsar_physics`; an adapter converts the
/// results into that crate's public sample contract and publishes its mailbox.
pub trait WaterSurfaceQueryConsumer {
    fn publish_water_surface_queries(
        &mut self,
        sampled_frame: u64,
        sampled_time: f64,
        results: &[WaterSurfaceQueryResult],
    );
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QueryInput {
    position_cell: [f32; 4],
    identity_volume: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QueryOutput {
    surface_position_valid: [f32; 4],
    normal_valid: [f32; 4],
    identity: [u32; 4],
}

type MapSlot = Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>;

struct StagingGeneration {
    staging: wgpu::Buffer,
    map_result: MapSlot,
    pending: Option<PendingBatch>,
}

struct PendingBatch {
    points: Vec<WaterSurfaceQueryPoint>,
    world_origin: glam::DVec3,
    frame: u64,
    time: f64,
    map_started: bool,
    discard: bool,
    removed_bodies: std::collections::HashSet<u64>,
}

pub struct WaterSurfaceQueries {
    input: wgpu::Buffer,
    output: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    generations: Vec<StagingGeneration>,
    next_generation: usize,
    requested: Vec<WaterSurfaceQueryPoint>,
    completed: Vec<WaterSurfaceQueryResult>,
    previous: std::collections::HashMap<(u64, u32), (glam::DVec3, glam::DVec3, f64, u64)>,
    skipped_batches: u64,
}

impl WaterSurfaceQueries {
    pub fn new(device: &wgpu::Device) -> Self {
        let input = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Water Surface Query Inputs"),
            size: MAX_WATER_SURFACE_QUERIES as u64 * QUERY_INPUT_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Water Surface Query GPU Results"),
            size: MAX_STAGING_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let entries = [
            buffer_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
            texture_entry(2, wgpu::TextureViewDimension::D2Array, true),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture_entry(12, wgpu::TextureViewDimension::D2Array, true),
            texture_entry(13, wgpu::TextureViewDimension::D2Array, true),
            buffer_entry(14, wgpu::BufferBindingType::Uniform),
            texture_entry(15, wgpu::TextureViewDimension::D2Array, false),
            buffer_entry(16, wgpu::BufferBindingType::Uniform),
            buffer_entry(21, wgpu::BufferBindingType::Storage { read_only: true }),
            buffer_entry(22, wgpu::BufferBindingType::Storage { read_only: false }),
        ];
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Water Surface Query BGL"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Water Surface Query Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = helio_core::shader::module_with(
            device,
            "Water Surface Query Shader",
            helio_core::include_wgsl!("../shaders/surface.wgsl"),
            &[
                helio_pass_hiz::HIZ_SNIPPET,
                crate::pipeline::WATER_SHORE_SNIPPET,
                helio_frame_api::lighting::sky::ATMOSPHERE_SNIPPET,
                crate::pipeline::WATER_SKY_SNIPPET,
                crate::pipeline::WATER_LIGHTS_SNIPPET,
                WATER_QUERY_SNIPPET,
            ],
        );
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Water Surface Query Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cs_water_query"),
            compilation_options: Default::default(),
            cache: None,
        });
        let generations = (0..STAGING_GENERATIONS)
            .map(|_| StagingGeneration {
                staging: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Water Surface Query Readback Generation"),
                    size: MAX_STAGING_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                map_result: Arc::new(Mutex::new(None)),
                pending: None,
            })
            .collect();
        Self {
            input,
            output,
            bind_group_layout,
            pipeline,
            generations,
            next_generation: 0,
            requested: Vec::new(),
            completed: Vec::new(),
            previous: std::collections::HashMap::new(),
            skipped_batches: 0,
        }
    }

    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    /// Replace the pending request set for the next renderer frame. At most
    /// 256 points enter a dispatch; excess points are ignored deterministically.
    pub fn set_requests(&mut self, points: impl IntoIterator<Item = WaterSurfaceQueryPoint>) {
        self.requested.clear();
        self.requested.extend(
            points
                .into_iter()
                .filter(|point| {
                    point.world_position.is_finite()
                        && point.cell_size.is_finite()
                        && point.volume_index < crate::MAX_SIM_VOLUMES
                })
                .take(MAX_WATER_SURFACE_QUERIES),
        );
    }

    /// Release request and velocity history when an opted-in body is removed.
    pub fn remove_body(&mut self, body_key: u64) {
        self.requested.retain(|point| point.body_key != body_key);
        self.completed.retain(|sample| sample.body_key != body_key);
        self.previous.retain(|(key, _), _| *key != body_key);
        for generation in &mut self.generations {
            if let Some(batch) = generation.pending.as_mut() {
                batch.removed_bodies.insert(body_key);
            }
        }
    }

    /// Poll completed maps and start mapping copies from an already submitted
    /// prior frame. This never waits for a pending GPU map.
    pub fn prepare(&mut self) {
        self.harvest();
        for generation in &mut self.generations {
            let Some(batch) = generation.pending.as_mut() else {
                continue;
            };
            if batch.map_started {
                continue;
            }
            let bytes = batch.points.len() as u64 * QUERY_OUTPUT_BYTES;
            let callback = Arc::clone(&generation.map_result);
            generation
                .staging
                .slice(0..bytes)
                .map_async(wgpu::MapMode::Read, move |result| {
                    *callback.lock().expect("water query map mutex poisoned") = Some(result);
                });
            batch.map_started = true;
        }
    }

    /// Encode the height/normal query after simulation work and copy its
    /// bounded output into the selected staging generation.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world_origin: glam::DVec3,
        frame: u64,
        encoder: &mut helio_core::CommandRecorder<'_>,
        volumes: &wgpu::Buffer,
        ocean_displacement: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        ocean_slopes: &wgpu::TextureView,
        interactions: &wgpu::TextureView,
        ocean_phase: &wgpu::Buffer,
        shore_fields: &wgpu::TextureView,
        shore_params: &wgpu::Buffer,
        time: f64,
    ) {
        if self.requested.is_empty() {
            return;
        }
        let Some(generation_index) = self.find_free_generation() else {
            self.skipped_batches = self.skipped_batches.saturating_add(1);
            self.requested.clear();
            return;
        };
        let points = self.requested.clone();
        let gpu_points: Vec<QueryInput> = points
            .iter()
            .map(|point| {
                let relative = point.world_position - world_origin;
                QueryInput {
                    position_cell: [
                        relative.x as f32,
                        relative.y as f32,
                        relative.z as f32,
                        point.cell_size.max(0.0),
                    ],
                    identity_volume: [
                        point.body_key as u32,
                        (point.body_key >> 32) as u32,
                        point.point_index,
                        point.volume_index,
                    ],
                }
            })
            .collect();
        queue.write_buffer(&self.input, 0, bytemuck::cast_slice(&gpu_points));
        self.requested.clear();
        self.generations[generation_index].pending = Some(PendingBatch {
            points,
            world_origin,
            frame,
            time,
            map_started: false,
            discard: false,
            removed_bodies: std::collections::HashSet::new(),
        });
        self.next_generation = (generation_index + 1) % self.generations.len();
        let pending = self.generations[generation_index]
            .pending
            .as_ref()
            .expect("query batch reserved for this dispatch");
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Water Surface Query BG"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: volumes.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(ocean_displacement),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::TextureView(ocean_slopes),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: wgpu::BindingResource::TextureView(interactions),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: ocean_phase.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 15,
                    resource: wgpu::BindingResource::TextureView(shore_fields),
                },
                wgpu::BindGroupEntry {
                    binding: 16,
                    resource: shore_params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 21,
                    resource: self.input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 22,
                    resource: self.output.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Water Surface Sparse Queries"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups((pending.points.len() as u32).div_ceil(64), 1, 1);
        drop(pass);
        let bytes = pending.points.len() as u64 * QUERY_OUTPUT_BYTES;
        encoder.copy_buffer_to_buffer(
            &self.output,
            0,
            &self.generations[generation_index].staging,
            0,
            bytes,
        );
    }

    /// Drain every query result that has completed without polling/waiting the
    /// GPU. A result's `sampled_frame` remains the dispatch frame.
    pub fn take_completed(&mut self) -> Vec<WaterSurfaceQueryResult> {
        self.harvest();
        std::mem::take(&mut self.completed)
    }

    pub fn skipped_batches(&self) -> u64 {
        self.skipped_batches
    }

    pub fn clear(&mut self) {
        self.requested.clear();
        self.completed.clear();
        self.previous.clear();
        for generation in &mut self.generations {
            if let Some(batch) = generation.pending.as_mut() {
                batch.discard = true;
            }
        }
    }

    fn find_free_generation(&self) -> Option<usize> {
        (0..self.generations.len())
            .map(|offset| (self.next_generation + offset) % self.generations.len())
            .find(|index| self.generations[*index].pending.is_none())
    }

    fn harvest(&mut self) {
        for generation in &mut self.generations {
            let result = generation
                .map_result
                .lock()
                .expect("water query map mutex poisoned")
                .take();
            let Some(result) = result else { continue };
            let pending = generation.pending.take();
            if result.is_ok() {
                if let Some(batch) = pending {
                    let discard = batch.discard;
                    let removed_bodies = batch.removed_bodies;
                    let bytes = batch.points.len() as u64 * QUERY_OUTPUT_BYTES;
                    let Ok(mapped) = generation.staging.slice(0..bytes).get_mapped_range() else {
                        continue;
                    };
                    let outputs: &[QueryOutput] = bytemuck::cast_slice(&mapped);
                    for (point, output) in batch.points.into_iter().zip(outputs.iter().copied()) {
                        if discard
                            || removed_bodies.contains(&point.body_key)
                            || output.identity[3] == 0
                            || output.identity[0] != point.body_key as u32
                            || output.identity[1] != (point.body_key >> 32) as u32
                            || output.identity[2] != point.point_index
                        {
                            continue;
                        }
                        let surface_position = glam::DVec3::new(
                            f64::from(output.surface_position_valid[0]) + batch.world_origin.x,
                            f64::from(output.surface_position_valid[1]) + batch.world_origin.y,
                            f64::from(output.surface_position_valid[2]) + batch.world_origin.z,
                        );
                        if !surface_position.is_finite()
                            || !output.normal_valid[..3]
                                .iter()
                                .all(|value| value.is_finite())
                        {
                            continue;
                        }
                        let key = (point.body_key, point.point_index);
                        let query_position = point.world_position;
                        let prior = self.previous.get(&key).copied();
                        let velocity = prior
                            .and_then(
                                |(
                                    previous_surface_position,
                                    previous_query_position,
                                    previous_time,
                                    previous_frame,
                                )| {
                                    let dt = batch.time - previous_time;
                                    (batch.frame > previous_frame && dt > 1e-5 && dt <= 0.5).then(
                                        || {
                                            let current_displacement =
                                                surface_position - query_position;
                                            let previous_displacement =
                                                previous_surface_position - previous_query_position;
                                            ((current_displacement - previous_displacement) / dt)
                                                .as_vec3()
                                        },
                                    )
                                },
                            )
                            .unwrap_or(glam::Vec3::ZERO);
                        if prior.is_none_or(|(_, _, _, prior_frame)| batch.frame > prior_frame) {
                            self.previous.insert(
                                key,
                                (surface_position, query_position, batch.time, batch.frame),
                            );
                        }
                        self.completed.push(WaterSurfaceQueryResult {
                            body_key: point.body_key,
                            point_index: point.point_index,
                            world_position: surface_position,
                            surface_height: surface_position.y,
                            normal: glam::Vec3::from_array([
                                output.normal_valid[0],
                                output.normal_valid[1],
                                output.normal_valid[2],
                            ])
                            .normalize_or_zero(),
                            surface_velocity: velocity,
                            sampled_frame: batch.frame,
                            sampled_time: batch.time,
                        });
                    }
                    drop(mapped);
                    generation.staging.unmap();
                }
            }
        }
    }
}

fn buffer_entry(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
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

fn texture_entry(
    binding: u32,
    view_dimension: wgpu::TextureViewDimension,
    filterable: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension,
            multisampled: false,
        },
        count: None,
    }
}
