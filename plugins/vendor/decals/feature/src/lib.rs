pub mod feature;
pub mod scene_rows;

pub use feature::DecalFeature;

use helio_core::graph::ResourceBuilder;
use helio_core::{PassContext, PrepareContext, RenderPass, Result as HelioResult};
use pulsar_scenedb::gpu::{world_mirror::DEFAULT_AUTO_REGISTER_CAPACITY, BufferKey};

pub mod components;
pub mod gpu_types;
pub use components::DecalRow;
pub use components::DECALS;
pub use gpu_types::*;

/// Fixed capacity for the `"decals"` SceneDB buffer, matching
/// the light list's initial capacity's reasoning exactly: kept equal to
/// the packed-layout auto-register capacity so every entity within it is
/// covered, not just an arbitrary prefix. A row with no live entity behind
/// it is `Zeroable`-default -- `decal_collect.wgsl`'s `opa <= 0.0` gate
/// (driven by `color.a == 0`) makes that row a guaranteed no-op even though
/// its degenerate all-zero `transform` would otherwise mis-classify every
/// pixel as "inside" the decal box.
pub const MAX_DECALS: u32 = DEFAULT_AUTO_REGISTER_CAPACITY;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DecalGlobals {
    decal_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

pub struct DecalPass {
    material_binding: helio_mats::MaterialBindingConfig,
    collect_pipeline: wgpu::ComputePipeline,
    apply_pipeline: wgpu::ComputePipeline,
    bgl_collect: wgpu::BindGroupLayout,
    bgl_apply: wgpu::BindGroupLayout,
    bgl_textures: wgpu::BindGroupLayout,
    bg_collect: helio_core::CachedBindGroup,
    bg_apply: helio_core::CachedBindGroup,
    bg_textures: Option<wgpu::BindGroup>,
    bg_textures_version: Option<u64>,
    globals_buf: wgpu::Buffer,
    temp_albedo: Option<(wgpu::Texture, wgpu::TextureView)>,
    temp_normal: Option<(wgpu::Texture, wgpu::TextureView)>,
    temp_orm: Option<(wgpu::Texture, wgpu::TextureView)>,
    temp_emissive: Option<(wgpu::Texture, wgpu::TextureView)>,
    last_w: u32,
    last_h: u32,
    /// Cached in `prepare()`, read back in `execute()` -- see `prepare()`'s
    /// doc for why this isn't just `ctx.scene.decal_count`.
    decal_count: u32,
    /// Bound in place of `"decals"` when no `DecalRow` has ever been
    /// inserted -- SceneDB is the only decal source this pass reads.
    /// `decal_count` is 0 whenever this is bound, so it's never dereferenced.
    fallback_decals: wgpu::Buffer,
}

impl DecalPass {
    /// Decal textures now come from the scene's bindless table (bound per-frame
    /// from the explicit material-texture frame binding, so this pass owns no
    /// texture table state of its own.
    pub fn new(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _camera_buf: &wgpu::Buffer,
        _w: u32,
        _h: u32,
    ) -> Self {
        let material_binding = helio_mats::MaterialBindingConfig::for_device(device);
        let collect_src = decal_collect_source(material_binding);
        let collect_mod = helio_core::shader::module(device, "Decal Collect", &collect_src);
        let apply_mod = helio_core::shader::module(
            device,
            "Decal Apply",
            helio_core::include_wgsl!("../shaders/decal_apply.wgsl"),
        );
        let fallback_decals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Decal Fallback Decals"),
            size: std::mem::size_of::<crate::GpuDecal>() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("DecalGlobals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // BGL collect: camera(0) + globals(1) + decals(2) + Hi-Z mip 0(3) + gbuf(4-7) + temp_out(8-11)
        let bgl_collect = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Decal Collect BGL"),
            entries: &[
                bgl_entry_buf(0, wgpu::BufferBindingType::Storage { read_only: true }),
                bgl_entry_buf(1, wgpu::BufferBindingType::Uniform),
                bgl_entry_buf(2, wgpu::BufferBindingType::Storage { read_only: true }),
                bgl_entry_tex(3, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex(4, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex(5, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex(6, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex(7, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex_storage(
                    8,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba8Unorm,
                ),
                bgl_entry_tex_storage(
                    9,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba16Float,
                ),
                bgl_entry_tex_storage(
                    10,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba8Unorm,
                ),
                bgl_entry_tex_storage(
                    11,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba16Float,
                ),
            ],
        });
        let bgl_textures = create_decal_texture_bgl(device, material_binding);
        let collect_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Decal Collect PL"),
            bind_group_layouts: &[Some(&bgl_collect), Some(&bgl_textures)],
            immediate_size: 0,
        });
        let collect_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Decal Collect"),
                layout: Some(&collect_pl),
                module: &collect_mod,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        // BGL apply: camera(0) + globals(1) + decals(2) + temp_in(3-6) + gbuf_out(7-10 storage)
        let bgl_apply = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Decal Apply BGL"),
            entries: &[
                bgl_entry_buf(0, wgpu::BufferBindingType::Storage { read_only: true }),
                bgl_entry_buf(1, wgpu::BufferBindingType::Uniform),
                bgl_entry_buf(2, wgpu::BufferBindingType::Storage { read_only: true }),
                bgl_entry_tex(3, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex(4, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex(5, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex(6, wgpu::TextureSampleType::Float { filterable: false }),
                bgl_entry_tex_storage(
                    7,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba8Unorm,
                ),
                bgl_entry_tex_storage(
                    8,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba16Float,
                ),
                bgl_entry_tex_storage(
                    9,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba8Unorm,
                ),
                bgl_entry_tex_storage(
                    10,
                    wgpu::StorageTextureAccess::WriteOnly,
                    wgpu::TextureFormat::Rgba16Float,
                ),
            ],
        });
        let apply_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Decal Apply PL"),
            bind_group_layouts: &[Some(&bgl_apply)],
            immediate_size: 0,
        });
        let apply_pipeline = helio_core::shader::create_compute_pipeline(
            &device,
            wgpu::ComputePipelineDescriptor {
                label: Some("Decal Apply"),
                layout: Some(&apply_pl),
                module: &apply_mod,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            },
        );

        Self {
            material_binding,
            collect_pipeline,
            apply_pipeline,
            bgl_collect,
            bgl_apply,
            bgl_textures,
            bg_collect: Default::default(),
            bg_apply: Default::default(),
            bg_textures: None,
            bg_textures_version: None,
            globals_buf,
            temp_albedo: None,
            temp_normal: None,
            temp_orm: None,
            temp_emissive: None,
            last_w: 0,
            last_h: 0,
            decal_count: 0,
            fallback_decals,
        }
    }

    fn ensure_temp(&mut self, w: u32, h: u32, device: &wgpu::Device) {
        if w == self.last_w && h == self.last_h && self.temp_albedo.is_some() {
            return;
        }
        self.last_w = w;
        self.last_h = h;
        let size = wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        };
        self.temp_albedo = Some(make_temp(
            device,
            wgpu::TextureFormat::Rgba8Unorm,
            "DecalTemp_Albedo",
            size,
        ));
        self.temp_normal = Some(make_temp(
            device,
            wgpu::TextureFormat::Rgba16Float,
            "DecalTemp_Normal",
            size,
        ));
        self.temp_orm = Some(make_temp(
            device,
            wgpu::TextureFormat::Rgba8Unorm,
            "DecalTemp_ORM",
            size,
        ));
        self.temp_emissive = Some(make_temp(
            device,
            wgpu::TextureFormat::Rgba16Float,
            "DecalTemp_Emissive",
            size,
        ));
        self.bg_collect.clear();
        self.bg_apply.clear();
    }
}

fn make_temp(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
    size: wgpu::Extent3d,
) -> (wgpu::Texture, wgpu::TextureView) {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let v = t.create_view(&Default::default());
    (t, v)
}

/// Decal collect shader source, resized to this platform's bindless table.
///
/// The WGSL is written against the 256-entry native table; on wasm the arrays are
/// rewritten to individual bindings (baseline WebGPU has no `binding_array`), and
/// elsewhere the declared length is resized to match the selected material tier — the BGL and
/// the shader must agree exactly or `create_bind_group` fails validation.
fn decal_collect_source(material_binding: helio_mats::MaterialBindingConfig) -> String {
    // Rewritten below, so fetched as text (hot reload applies the rewrite to
    // the edited file) rather than handed to `module` as an `include_wgsl!`.
    let text = helio_core::shader::source_text(
        "Decal Collect",
        helio_core::include_wgsl!("../shaders/decal_collect.wgsl"),
    );
    let src: &str = &text;
    if material_binding.uses_binding_arrays() {
        src.replace(
            "binding_array<texture_2d<f32>, 256>",
            &format!(
                "binding_array<texture_2d<f32>, {}>",
                material_binding.max_textures
            ),
        )
        .replace(
            "binding_array<sampler, 256>",
            &format!("binding_array<sampler, {}>", material_binding.max_textures),
        )
    } else {
        helio_mats::apply_webgpu_decal_bindings(src, material_binding.max_textures)
    }
}

/// BGL for group 1: the scene's bindless texture table, shared with the GBuffer pass.
fn create_decal_texture_bgl(
    device: &wgpu::Device,
    material_binding: helio_mats::MaterialBindingConfig,
) -> wgpu::BindGroupLayout {
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();
    material_binding.append_layout_entries(&mut entries, 0, wgpu::ShaderStages::COMPUTE);
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Decal Textures BGL"),
        entries: &entries,
    })
}

fn bgl_entry_buf(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
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
fn bgl_entry_tex(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
fn bgl_entry_tex_storage(
    binding: u32,
    access: wgpu::StorageTextureAccess,
    format: wgpu::TextureFormat,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

impl RenderPass for DecalPass {
    fn scene_buffers(&self) -> &'static [&'static str] {
        &["decals"]
    }

    fn name(&self) -> &'static str {
        "DecalApply"
    }

    /// A member while some `"decals"` row is live. Hosts register the column
    /// up front, so the buffer's presence alone would run both full-screen
    /// passes over empty rows, which only copy the G-buffer unchanged.
    fn is_member(&mut self, scene: &mut helio_core::SceneContent<'_>) -> bool {
        scene.holds_rows("decals")
    }

    fn on_leave(&mut self, _device: &wgpu::Device) {
        // The full-resolution temporaries come back with the next decal.
        self.temp_albedo = None;
        self.temp_normal = None;
        self.temp_orm = None;
        self.temp_emissive = None;
        self.last_w = 0;
        self.last_h = 0;
    }
    fn declare_resources(&self, builder: &mut ResourceBuilder) {
        builder.read(helio_frame_api::gbuffer::GROUP);
        builder.read(helio_frame_api::depth::HIZ);
        builder.read(helio_mats::MATERIAL_TEXTURES);
    }
    fn publish<'a>(&self, _: &mut helio_core::ResourceRegistry<'a>) {}
    fn render_pass_descriptor<'a>(
        &'a self,
        _: &'a wgpu::TextureView,
        _: &'a wgpu::TextureView,
        _: &'a helio_core::ResourceRegistry<'a>,
    ) -> Option<wgpu::RenderPassDescriptor<'a>> {
        None
    }
    fn prepare(&mut self, ctx: &PrepareContext) -> HelioResult<()> {
        // SceneDB is the only decal source: `"decals"` is a fixed-capacity
        // buffer (`MAX_DECALS`), resolved fresh from the mirror by key every
        // frame. 0 when no `DecalRow` has ever been inserted; there is
        // no Helio-owned decal buffer to fall back to. The graph runs the pass
        // only while some row is live (`is_member`).
        let decals = ctx.scene_buffers.get(BufferKey::of("decals"));
        self.decal_count = if decals.is_some() { MAX_DECALS } else { 0 };
        ctx.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&DecalGlobals {
                decal_count: self.decal_count,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            }),
        );
        Ok(())
    }

    fn execute(&mut self, ctx: &mut PassContext) -> HelioResult<()> {
        if self.decal_count == 0 {
            return Ok(());
        }
        let gb = match ctx.registry.read::<helio_core::ViewGroup<'_, 4>>(
            helio_core::ResourceKey::new(helio_frame_api::gbuffer::GROUP),
            self.name(),
        ) {
            Some(g) => g,
            None => return Ok(()),
        };
        let depth_view = match ctx.registry.read::<&wgpu::TextureView>(
            helio_core::ResourceKey::new(helio_frame_api::depth::HIZ),
            self.name(),
        ) {
            Some(v) => v,
            None => return Ok(()),
        };
        // The materials' texture table (a decal's textures are in it),
        // published every frame by the materials plugin; without it no
        // decal has a texture to draw.
        let material_textures = match ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures())
        {
            Some(m) => m,
            None => return Ok(()),
        };
        self.ensure_temp(ctx.width, ctx.height, ctx.device);
        let decals_buf = ctx
            .scene_buffers
            .get(BufferKey::of("decals"))
            .map(|handle| &handle.buffer)
            .unwrap_or(&self.fallback_decals);
        let (_, ta) = self.temp_albedo.as_ref().unwrap();
        let (_, tn) = self.temp_normal.as_ref().unwrap();
        let (_, to) = self.temp_orm.as_ref().unwrap();
        let (_, te) = self.temp_emissive.as_ref().unwrap();

        // Rebuilt whenever anything it binds changes (helio_core::CachedBindGroup).
        self.bg_collect.get_or_create(
            ctx.device,
            &wgpu::BindGroupDescriptor {
                label: Some("Decal Collect BG"),
                layout: &self.bgl_collect,
                entries: &[
                    bind_buf(0, ctx.camera),
                    bind_buf(1, &self.globals_buf),
                    bind_buf(2, decals_buf),
                    bind_tex(3, depth_view),
                    bind_tex(4, gb.views[0]),
                    bind_tex(5, gb.views[1]),
                    bind_tex(6, gb.views[2]),
                    bind_tex(7, gb.views[3]),
                    bind_tex(8, ta),
                    bind_tex(9, tn),
                    bind_tex(10, to),
                    bind_tex(11, te),
                ],
            },
        );

        // Rebuild the texture table only when the scene's texture set changes.
        let tex_version = material_textures.version;
        if self.bg_textures_version != Some(tex_version) || self.bg_textures.is_none() {
            self.bg_textures = Some(build_texture_bind_group(
                ctx.device,
                &self.bgl_textures,
                &material_textures,
                self.material_binding,
            ));
            self.bg_textures_version = Some(tex_version);
        }

        {
            let mut cmds = ctx.graphics_cmds();
            let mut cp = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("DecalCollect"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.collect_pipeline);
            cp.set_bind_group(0, self.bg_collect.get().unwrap(), &[]);
            cp.set_bind_group(1, self.bg_textures.as_ref().unwrap(), &[]);
            cp.dispatch_workgroups(ctx.width.div_ceil(16), ctx.height.div_ceil(16), 1);
        }

        self.bg_apply.get_or_create(
            ctx.device,
            &wgpu::BindGroupDescriptor {
                label: Some("Decal Apply BG"),
                layout: &self.bgl_apply,
                entries: &[
                    bind_buf(0, ctx.camera),
                    bind_buf(1, &self.globals_buf),
                    bind_buf(2, decals_buf),
                    bind_tex(3, ta),
                    bind_tex(4, tn),
                    bind_tex(5, to),
                    bind_tex(6, te),
                    bind_tex(7, gb.views[0]),
                    bind_tex(8, gb.views[1]),
                    bind_tex(9, gb.views[2]),
                    bind_tex(10, gb.views[3]),
                ],
            },
        );

        {
            let mut cmds = ctx.graphics_cmds();
            let mut cp = cmds.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("DecalApply"),
                timestamp_writes: None,
            });
            cp.set_pipeline(&self.apply_pipeline);
            cp.set_bind_group(0, self.bg_apply.get().unwrap(), &[]);
            cp.dispatch_workgroups(ctx.width.div_ceil(16), ctx.height.div_ceil(16), 1);
        }

        Ok(())
    }

    fn writes(&self) -> &'static [&'static str] {
        &[helio_frame_api::gbuffer::GROUP]
    }
}

/// Bind the scene's bindless texture table for group 1.
fn build_texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    textures: &helio_mats::MaterialTextureBindings,
    material_binding: helio_mats::MaterialBindingConfig,
) -> wgpu::BindGroup {
    let mut entries: Vec<wgpu::BindGroupEntry> = Vec::new();
    material_binding.append_bind_group_entries(
        &mut entries,
        0,
        textures.texture_views,
        textures.samplers,
    );
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Decal Textures BG"),
        layout,
        entries: &entries,
    })
}

fn bind_buf<'a>(binding: u32, buf: &'a wgpu::Buffer) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: buf.as_entire_binding(),
    }
}
fn bind_tex<'a>(binding: u32, view: &'a wgpu::TextureView) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::DecalPass;
    use naga::back::glsl;

    /// The wasm fixup rewrites the shader by matching exact source strings, so a
    /// harmless-looking edit to the binding-array declarations or the sampling
    /// call would silently leave `binding_array` in the WebGPU source and only
    /// fail at shader-compile time in a browser. Pin the coupling here.
    #[test]
    fn webgpu_fixup_rewrites_every_binding_array_in_the_real_shader() {
        let src = helio_core::shader::resolve(include_str!("../shaders/decal_collect.wgsl"));
        let fixed = helio_mats::apply_webgpu_decal_bindings(
            &src,
            helio_mats::MAX_MATERIAL_TEXTURES.min(16),
        );

        assert!(
            !fixed.contains("binding_array"),
            "decal_collect.wgsl still declares a binding_array after the WebGPU fixup",
        );
        assert!(
            !fixed.contains("enable wgpu_binding_array"),
            "decal_collect.wgsl still enables the wgpu-only binding_array extension",
        );
        assert!(
            fixed.contains("case 0u: { return textureSampleLevel(scene_texture_0"),
            "the decal sampling call was not rewritten into a per-slot switch",
        );
    }

    #[test]
    fn collect_shader_translates_portable_depth_to_gles() {
        let src = helio_core::shader::resolve(include_str!("../shaders/decal_collect.wgsl"));
        let src = helio_mats::apply_webgpu_decal_bindings(&src, 1);
        let module = naga::front::wgsl::parse_str(&src)
            .expect("Decal Collect WGSL must parse after baseline binding expansion");
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .subgroup_stages(naga::valid::ShaderStages::all())
        .subgroup_operations(naga::valid::SubgroupOperationSet::all())
        .validate(&module)
        .expect("Decal Collect WGSL must validate");

        let mut output = String::new();
        glsl::Writer::new(
            &mut output,
            &module,
            &info,
            &glsl::Options::default(),
            &glsl::PipelineOptions {
                shader_stage: naga::ShaderStage::Compute,
                entry_point: "cs_main".into(),
                multiview: None,
            },
            naga::proc::BoundsCheckPolicies::default(),
        )
        .expect("Decal Collect must lower to GLES")
        .write()
        .expect("Decal Collect must emit GLES");

        assert!(output.contains("void main()"));
        assert!(output.contains("texelFetch"));
        assert!(
            !output.contains("sampler2DShadow"),
            "decal depth must lower as an ordinary R32Float texture"
        );
    }

    async fn compile_on_available_backends() -> usize {
        // Every backend, GL included, on purpose: this is the only device-creating
        // test in its binary, so no device creation races its GL threads' exit
        // (see `helio_core::wgpu_instance_descriptor`).
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapters = instance.enumerate_adapters(wgpu::Backends::all()).await;

        for adapter in &adapters {
            let info = adapter.get_info();
            let backend = format!("{:?}", info.backend);
            let required_features = adapter.features() & helio_mats::BINDLESS_MATERIAL_FEATURES;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Decal Portability Test Device"),
                    required_features,
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .unwrap_or_else(|error| panic!("{backend} adapter must create a device: {error}"));
            device.on_uncaptured_error(Arc::new(move |error| {
                panic!("Decal {backend} validation error: {error:?}");
            }));

            let camera = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Decal Portability Camera"),
                size: 512,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            let _pass = DecalPass::new(&device, &queue, &camera, 1280, 720);
        }

        adapters.len()
    }

    #[test]
    fn pipelines_compile_on_every_available_backend() {
        if pollster::block_on(compile_on_available_backends()) == 0 {
            eprintln!("skipping Decal portability test: no GPU adapter available");
        }
    }
}
