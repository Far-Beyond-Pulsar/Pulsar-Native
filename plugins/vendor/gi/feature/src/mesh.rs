//! The scene's meshes as a tracer of the probes' rays (`gi_mesh_trace.wgsl`,
//! README.md "Tracers"): hardware ray queries against the frame's TLAS
//! (`RenderEnvironment::tlas`), shaded from its instances' surfaces
//! (`helio_mesh_api::RayInstance`, `ray_instances`; proxies' triangles from
//! `ray_proxy_vertices` / `ray_proxy_indices`). Cut-outs' candidates are
//! alpha-tested against the scene's `materials` and material texture
//! table. It runs after the other tracers and before
//! [`crate::GiResolvePass`]; on a frame without a TLAS or its rows it
//! leaves the hits as they are.

use crate::{
    buffer, storage, uniform, FRAME_BYTES, GI_FRAME, GI_HITS, GI_LIGHT, GI_RAYS, GI_SUN, MAX_RAYS,
};
use bytemuck::{Pod, Zeroable};
use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, ResourceKey, Result as HelioResult};
use helio_pass_shadow_matrix::{
    SUN_OCCLUSION_FRAME, SUN_OCCLUSION_FRAME_BYTES, SUN_OCCLUSION_MAPS, SUN_OCCLUSION_PAGES,
    SUN_OCCLUSION_SNIPPET,
};
use helio_frame_api::lighting::sun_transmittance::{self, SunTransmittance};

/// The scene's point and spot lights a hit chooses from: the first so many
/// live ones (`gi_gather_lights`).
pub const MAX_LIGHTS: u32 = 256;
/// Bytes of the gathered lights: a count, then 64 bytes a light.
const LOCAL_LIGHTS_BYTES: u64 = 16 + 64 * MAX_LIGHTS as u64;

/// `MeshTrace` (`gi_mesh_trace.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuTrace {
    eye: [f32; 4],
    info: [u32; 4],
}

/// `gi_mesh_trace.wgsl` with the cut-outs' alpha evaluator
/// (`helio_mats::vg_alpha`, group 1) in front.
pub(crate) fn mesh_trace_source(binding: helio_mats::MaterialBindingConfig) -> String {
    use helio_mats::vg_alpha;
    format!(
        "{}{}\n{}\n{}",
        vg_alpha::module_header(binding),
        vg_alpha::bindings_wgsl(binding, 1, 0),
        vg_alpha::WGSL,
        include_str!("../shaders/gi_mesh_trace.wgsl")
    )
}

/// The meshes' tracer (module docs).
pub struct GiMeshTracePass {
    layout: wgpu::BindGroupLayout,
    /// Group 1: material rows and texture table (`helio_mats::vg_alpha`).
    materials_layout: wgpu::BindGroupLayout,
    binding: helio_mats::MaterialBindingConfig,
    /// The scene's, by what it was made of; and one for none (no
    /// cut-out's row: every candidate is its surface).
    materials: Option<((wgpu::Buffer, wgpu::Buffer, u64), wgpu::BindGroup)>,
    no_materials: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
    gather_layout: wgpu::BindGroupLayout,
    gather: wgpu::ComputePipeline,
    trace: wgpu::Buffer,
    local_lights: wgpu::Buffer,
    /// No terrain: its occlusion frame says none (a page table of none).
    no_occlusion: (wgpu::Buffer, wgpu::TextureView, wgpu::Buffer),
    /// The sun's transmittance a medium published when this pass was built,
    /// if any; and what stands in for none (a 1x1 map, a zeroed frame).
    sun_transmittance: Option<SunTransmittance>,
    no_sun_transmittance: (wgpu::TextureView, wgpu::Buffer),
    no_lights: wgpu::Buffer,
    /// No proxy pool: a binding for each of its buffers.
    no_proxies: wgpu::Buffer,
}

/// Whether `device` can trace the probes' rays against meshes (hardware ray
/// queries and a TLAS).
pub fn mesh_rays_supported(device: &wgpu::Device) -> bool {
    device
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
        && device.limits().max_tlas_instance_count > 0
}

impl GiMeshTracePass {
    /// The tracer, on a device with hardware ray queries (none without:
    /// the probes then see only the other tracers' geometry).
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        if !mesh_rays_supported(device) {
            return None;
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GI mesh rays"),
            entries: &[
                uniform(0, FRAME_BYTES),
                storage(1, true),
                storage(2, false),
                storage(3, true),
                storage(4, false),
                uniform(5, std::mem::size_of::<GpuTrace>() as u64),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::AccelerationStructure {
                        vertex_return: false,
                    },
                    count: None,
                },
                storage(7, true),
                storage(8, true),
                storage(9, true),
                uniform(11, SUN_OCCLUSION_FRAME_BYTES),
                wgpu::BindGroupLayoutEntry {
                    binding: 12,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                storage(13, false),
                storage(14, true),
                storage(15, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 16,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                uniform(17, sun_transmittance::FRAME_BYTES),
                storage(18, true),
            ],
        });
        let gather_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GI mesh lights"),
            entries: &[
                uniform(5, std::mem::size_of::<GpuTrace>() as u64),
                storage(10, true),
                storage(13, false),
            ],
        });
        let binding = helio_mats::MaterialBindingConfig::for_device(device);
        let materials_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GI mesh rays materials"),
            entries: &helio_mats::vg_alpha::layout_entries(binding, 0, wgpu::ShaderStages::COMPUTE),
        });
        let module = helio_core::shader::module_with(
            device,
            "GI mesh rays",
            &mesh_trace_source(binding),
            &[
                crate::GI_COMMON_SNIPPET,
                SUN_OCCLUSION_SNIPPET,
                sun_transmittance::SNIPPET,
            ],
        );
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("GI mesh rays"),
            bind_group_layouts: &[Some(&layout), Some(&materials_layout)],
            immediate_size: 0,
        });
        let pipeline = helio_core::shader::create_compute_pipeline(
            device,
            wgpu::ComputePipelineDescriptor {
                label: Some("gi_mesh_trace"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some("gi_mesh_trace"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let gather_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GI mesh lights"),
                bind_group_layouts: &[Some(&gather_layout)],
                immediate_size: 0,
            });
        let gather = helio_core::shader::create_compute_pipeline(
            device,
            wgpu::ComputePipelineDescriptor {
                label: Some("gi_gather_lights"),
                layout: Some(&gather_pipeline_layout),
                module: &module,
                entry_point: Some("gi_gather_lights"),
                compilation_options: Default::default(),
                cache: None,
            },
        );
        let maps = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("GI no sun occlusion"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: helio_pass_shadow_matrix::SUN_OCCLUSION_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let maps = maps.create_view(&Default::default());
        let uniform_usage = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        // No material table: rows shorter than one material (each row reads
        // as no row: opaque) and a white texture in every slot.
        let no_rows = buffer(
            device,
            "GI mesh no materials",
            16,
            wgpu::BufferUsages::STORAGE,
        );
        let white = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("GI mesh no material texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let white = white.create_view(&Default::default());
        let sampler = device.create_sampler(&Default::default());
        let views = vec![&white; binding.max_textures];
        let samplers = vec![&sampler; binding.max_textures];
        let no_materials = Self::materials_group(
            device,
            &materials_layout,
            binding,
            &no_rows,
            helio_mats::MaterialTextureBindings {
                material_textures: &no_rows,
                texture_views: &views,
                samplers: &samplers,
                version: 0,
            },
        );
        Some(Self {
            layout,
            materials_layout,
            binding,
            materials: None,
            no_materials,
            pipeline,
            gather_layout,
            gather,
            local_lights: buffer(
                device,
                "GI mesh local lights",
                LOCAL_LIGHTS_BYTES,
                wgpu::BufferUsages::STORAGE,
            ),
            trace: buffer(
                device,
                "GI mesh trace",
                std::mem::size_of::<GpuTrace>() as u64,
                uniform_usage,
            ),
            sun_transmittance: None,
            no_sun_transmittance: sun_transmittance::stand_in(device),
            no_occlusion: (
                buffer(
                    device,
                    "GI no sun occlusion",
                    SUN_OCCLUSION_FRAME_BYTES,
                    uniform_usage,
                ),
                maps,
                buffer(
                    device,
                    "GI no sun occlusion pages",
                    16,
                    wgpu::BufferUsages::STORAGE,
                ),
            ),
            no_lights: buffer(
                device,
                "GI mesh no lights",
                128,
                wgpu::BufferUsages::STORAGE,
            ),
            no_proxies: buffer(
                device,
                "GI mesh no proxies",
                16,
                wgpu::BufferUsages::STORAGE,
            ),
        })
    }

    /// Lights the hits through the sun's transmittance `sun` published
    /// (none: the sun reaches every hit).
    pub fn with_sun_transmittance(mut self, sun: Option<SunTransmittance>) -> Self {
        self.sun_transmittance = sun;
        self
    }

    fn materials_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        binding: helio_mats::MaterialBindingConfig,
        rows: &wgpu::Buffer,
        textures: helio_mats::MaterialTextureBindings<'_>,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GI mesh rays materials"),
            layout,
            entries: &helio_mats::vg_alpha::bind_entries(binding, 0, rows, textures),
        })
    }

    /// The bindings of a material table for [`Self::record`]: `rows` the
    /// scene's `materials` (`GpuMaterial` rows), `textures` its texture
    /// metadata and table (as many slots as `MaterialBindingConfig::for_device`).
    pub fn materials(
        &self,
        device: &wgpu::Device,
        rows: &wgpu::Buffer,
        textures: helio_mats::MaterialTextureBindings<'_>,
    ) -> wgpu::BindGroup {
        Self::materials_group(device, &self.materials_layout, self.binding, rows, textures)
    }

    /// Records the meshes' hits for this frame's rays: `gi` is GiPass's
    /// frame, rays, hits, sun and hits' light; `eye` the eye in the TLAS's
    /// frame; `instances`, `vertices` and `indices` the TLAS's rows and the
    /// scene's mesh pools; `proxies` the proxy pool's vertices and indices
    /// (`helio_core::RayProxies`), if any; `materials` the material table
    /// cut-outs are tested against ([`Self::materials`]), if any; `lights`
    /// the scene's (its first `count` rows);
    /// `occlusion` the sun's occlusion (frame, atlas, pages), if published;
    /// `transmittance` the sun's transmittance map and its frame, if
    /// published.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cmds: &mut helio_core::CommandRecorder<'_>,
        [frame, rays, hits, sun, light]: [&wgpu::Buffer; 5],
        tlas: &wgpu::Tlas,
        eye: [f32; 3],
        [instances, vertices, indices]: [&wgpu::Buffer; 3],
        proxies: Option<(&wgpu::Buffer, &wgpu::Buffer)>,
        materials: Option<&wgpu::BindGroup>,
        lights: Option<(&wgpu::Buffer, u32)>,
        occlusion: Option<(&wgpu::Buffer, &wgpu::TextureView, &wgpu::Buffer)>,
        transmittance: Option<(&wgpu::TextureView, &wgpu::Buffer)>,
        seed: u32,
    ) {
        let (cloud_map, cloud_frame) = transmittance.unwrap_or((
            &self.no_sun_transmittance.0,
            &self.no_sun_transmittance.1,
        ));
        let (lights, count) = lights.unwrap_or((&self.no_lights, 0));
        let count = count.min((lights.size() / 128) as u32);
        let (occlusion_frame, occlusion_maps, occlusion_pages) = occlusion.unwrap_or((
            &self.no_occlusion.0,
            &self.no_occlusion.1,
            &self.no_occlusion.2,
        ));
        let (proxy_vertices, proxy_indices) =
            proxies.unwrap_or((&self.no_proxies, &self.no_proxies));
        let materials = materials.unwrap_or(&self.no_materials);
        let trace = GpuTrace {
            eye: [eye[0], eye[1], eye[2], 0.0],
            info: [count, seed, 0, 0],
        };
        helio_core::upload::write_buffer(queue, &self.trace, 0, bytemuck::bytes_of(&trace));
        fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }
        }
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GI mesh rays"),
            layout: &self.layout,
            entries: &[
                entry(0, frame),
                entry(1, rays),
                entry(2, hits),
                entry(3, sun),
                entry(4, light),
                entry(5, &self.trace),
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::AccelerationStructure(tlas),
                },
                entry(7, instances),
                entry(8, vertices),
                entry(9, indices),
                entry(11, occlusion_frame),
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::TextureView(occlusion_maps),
                },
                entry(13, &self.local_lights),
                entry(14, proxy_vertices),
                entry(15, proxy_indices),
                wgpu::BindGroupEntry {
                    binding: 16,
                    resource: wgpu::BindingResource::TextureView(cloud_map),
                },
                entry(17, cloud_frame),
                entry(18, occlusion_pages),
            ],
        });
        let gather = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GI mesh lights"),
            layout: &self.gather_layout,
            entries: &[
                entry(5, &self.trace),
                entry(10, lights),
                entry(13, &self.local_lights),
            ],
        });
        let mut pass = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("GI mesh rays"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, Some(&gather), &[]);
        pass.set_pipeline(&self.gather);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_bind_group(0, Some(&group), &[]);
        pass.set_bind_group(1, Some(materials), &[]);
        pass.set_pipeline(&self.pipeline);
        // Every ray slot: those past this frame's count return at once.
        pass.dispatch_workgroups(MAX_RAYS.div_ceil(64), 1, 1);
    }
}

impl RenderPass for GiMeshTracePass {
    fn name(&self) -> &'static str {
        "GIMeshRays"
    }

    fn reads(&self) -> &'static [&'static str] {
        const READS: [&str; 12] = [
            GI_FRAME,
            GI_RAYS,
            GI_HITS,
            GI_SUN,
            GI_LIGHT,
            "render_environment",
            helio_core::RAY_INSTANCES,
            helio_core::RAY_PROXY_VERTICES,
            helio_core::RAY_PROXY_INDICES,
            SUN_OCCLUSION_MAPS,
            SUN_OCCLUSION_PAGES,
            SUN_OCCLUSION_FRAME,
        ];
        let mut reads = READS.to_vec();
        if let Some(sun) = self.sun_transmittance {
            reads.extend([sun.map, sun.frame]);
        }
        helio_frame_api::static_names(&reads)
    }

    fn writes(&self) -> &'static [&'static str] {
        &[GI_HITS, GI_LIGHT]
    }

    fn declare_resources(&self, _builder: &mut ResourceBuilder) {}

    fn render_pass_descriptor<'a>(
        &'a self,
        _target: &'a wgpu::TextureView,
        _depth: &'a wgpu::TextureView,
        _resources: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }

    fn prepare(&mut self, _ctx: &PrepareContext) -> HelioResult<()> {
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        let binding = |name: &str| ctx.registry.buffer_binding(name);
        let (Some(frame), Some(rays), Some(hits), Some(sun), Some(light)) = (
            binding(GI_FRAME),
            binding(GI_RAYS),
            binding(GI_HITS),
            binding(GI_SUN),
            binding(GI_LIGHT),
        ) else {
            return Ok(());
        };
        let Some(tlas) = ctx
            .registry
            .get::<helio_core::RenderEnvironment>(helio_core::resource_keys::render_environment())
            .and_then(|e| e.tlas)
        else {
            return Ok(());
        };
        let Some(instances) = ctx
            .registry
            .get::<&wgpu::Buffer>(ResourceKey::new(helio_core::RAY_INSTANCES))
        else {
            return Ok(());
        };
        let pool = |name: &'static str| {
            ctx.scene_buffers
                .get(helio_core::BufferKey::of(name))
                .map(|h| h.buffer.clone())
        };
        let (Some(vertices), Some(indices)) =
            (pool("builtin_mesh_vertex"), pool("builtin_mesh_index"))
        else {
            return Ok(());
        };
        let lights = pool("scene_lights");
        let proxy = |name: &'static str| ctx.registry.get::<&wgpu::Buffer>(ResourceKey::new(name));
        let proxies =
            proxy(helio_core::RAY_PROXY_VERTICES).zip(proxy(helio_core::RAY_PROXY_INDICES));
        // The material table, its bind group kept while it stays the same.
        let rows = pool("materials");
        let textures = ctx.registry.get::<helio_mats::MaterialTextureBindings<'_>>(
            helio_mats::material_textures(),
        );
        match (rows, textures) {
            (Some(rows), Some(textures)) => {
                let key = (rows, textures.material_textures.clone(), textures.version);
                if self.materials.as_ref().is_none_or(|(k, _)| *k != key) {
                    let group = Self::materials_group(
                        ctx.device,
                        &self.materials_layout,
                        self.binding,
                        &key.0,
                        textures,
                    );
                    self.materials = Some((key, group));
                }
            }
            _ => self.materials = None,
        }
        let occlusion = match (
            binding(SUN_OCCLUSION_FRAME),
            ctx.registry.texture_binding(SUN_OCCLUSION_MAPS),
            binding(SUN_OCCLUSION_PAGES),
        ) {
            (Some(frame), Some(maps), Some(pages)) => Some((frame, maps, pages)),
            _ => None,
        };
        let transmittance = self.sun_transmittance.and_then(|published| {
            ctx.registry
                .texture_binding(published.map)
                .zip(binding(published.frame))
        });
        let camera = ctx.camera_data;
        let eye = [
            camera.position_near[0],
            camera.position_near[1],
            camera.position_near[2],
        ];
        let seed = (ctx.frame_num as u32).wrapping_mul(0x9e37_79b9);
        let mut cmds = ctx.graphics_cmds();
        let materials = self.materials.as_ref().map(|(_, group)| group);
        self.record(
            ctx.device,
            ctx.queue,
            &mut cmds,
            [frame, rays, hits, sun, light],
            tlas,
            eye,
            [instances, &vertices, &indices],
            proxies,
            materials,
            lights.as_ref().map(|l| (l, u32::MAX)),
            occlusion,
            transmittance,
            seed,
        );
        Ok(())
    }
}
