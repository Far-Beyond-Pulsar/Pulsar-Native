//! Material bindings for cutout-aware depth passes. Shares the G-buffer ABI
//! and device-specific texture binding representation.
use helio_core::{BufferKey, PassContext};

pub struct DepthMaterialBindings {
    pub layout: wgpu::BindGroupLayout,
    pub group: Option<wgpu::BindGroup>,
    config: helio_mats::MaterialBindingConfig,
    key: Option<(wgpu::Buffer, wgpu::Buffer, u64)>,
}

impl DepthMaterialBindings {
    pub fn new(device: &wgpu::Device) -> Self {
        let config = helio_mats::MaterialBindingConfig::for_device(device);
        Self {
            layout: crate::create_material_bgl(device, config),
            group: None,
            config,
            key: None,
        }
    }

    pub fn shader_source(&self, geometry: &str) -> String {
        let source = format!(
            "{}\n{}",
            include_str!("../shaders/depth_material.wgsl"),
            geometry
        )
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
        let Some(materials) = ctx.scene_buffers.get(BufferKey::of("materials")) else {
            self.group = None;
            self.key = None;
            return false;
        };
        let Some(textures) = ctx
            .registry
            .read::<helio_mats::MaterialTextureBindings<'_>>(
                helio_mats::material_textures(),
                "Depth materials",
            )
        else {
            self.group = None;
            self.key = None;
            return false;
        };
        let key = (
            materials.buffer.clone(),
            textures.material_textures.clone(),
            textures.version,
        );
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: materials.buffer.as_entire_binding(),
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
