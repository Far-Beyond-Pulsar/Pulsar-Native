//! Material bindings for cutout-aware depth passes. Shares the G-buffer ABI
//! and device-specific texture binding representation; the cut-out test is
//! the materials' (`material_cut_out`, `helio_mats::MATERIAL_SURFACE_SNIPPET`),
//! and without the materials plugin the stand-in is bound (nothing cut out).
use helio_core::{BufferKey, PassContext};

pub struct DepthMaterialBindings {
    pub layout: wgpu::BindGroupLayout,
    pub group: Option<wgpu::BindGroup>,
    config: helio_mats::MaterialBindingConfig,
    key: Option<(wgpu::Buffer, wgpu::Buffer, u64)>,
    stand_in: Option<helio_mats::MaterialStandIn>,
}

impl DepthMaterialBindings {
    pub fn new(device: &wgpu::Device) -> Self {
        let config = helio_mats::MaterialBindingConfig::for_device(device);
        Self {
            layout: crate::create_material_bgl(device, config),
            group: None,
            config,
            key: None,
            stand_in: None,
        }
    }

    pub fn shader_source(&self, geometry: &str) -> String {
        let source = format!(
            "{}\n{}",
            include_str!("../shaders/depth_material.wgsl"),
            geometry
        );
        let source = helio_core::shader::resolve_with(&source, &helio_mats::MATERIAL_SNIPPETS)
            .replace("256>", &format!("{}>", self.config.max_textures));
        if self.config.uses_binding_arrays() {
            source
        } else {
            helio_mats::apply_webgpu_material_bindings(&source, self.config.max_textures)
        }
    }

    /// Rebuild on material-pool replacement or texture publication. The return
    /// value invalidates cached shadows after an asynchronous opacity upload.
    pub fn update(&mut self, ctx: &PassContext) -> bool {
        let stand_in = self
            .stand_in
            .get_or_insert_with(|| helio_mats::MaterialStandIn::new(&ctx.device, ctx.queue));
        let materials = ctx
            .scene_buffers
            .get(BufferKey::of("materials"))
            .map_or(stand_in.rows(), |handle| &handle.buffer);
        let textures = ctx
            .registry
            .get::<helio_mats::MaterialTextureBindings<'_>>(helio_mats::material_textures())
            .unwrap_or_else(|| stand_in.textures());
        let key = (
            materials.clone(),
            textures.material_textures.clone(),
            textures.version,
        );
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: materials.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: textures.material_textures.as_entire_binding(),
            },
        ];
        self.config.append_bind_group_entries(
            &mut entries,
            2,
            textures.texture_views,
            textures.samplers,
        );
        self.group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Depth material bindings"),
            layout: &self.layout,
            entries: &entries,
        }));
        self.key = Some(key);
        true
    }
}
