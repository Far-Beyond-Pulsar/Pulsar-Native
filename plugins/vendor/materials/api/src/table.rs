//! The material texture table ([`MaterialTextureTable`]) and the stand-in a
//! drawer binds when no materials plugin publishes one ([`MaterialStandIn`]).
//!
//! A table holds one view per slot (a white texture where a slot is empty),
//! the material sampler and the per-material texture metadata; its
//! [`MaterialTextureBindings`] are what every drawer binds
//! (`MaterialBindingConfig::append_bind_group_entries`). The materials'
//! feature (`helio-pass-materials`) keeps one in step with the scene's
//! texture store and publishes it as [`crate::MATERIAL_TEXTURES`].

use crate::{MaterialBindingConfig, MaterialRow, MaterialSampler, MaterialTextureBindings};

/// Words of one `MaterialTextureData` row (seven 48-byte slots and a 16-byte
/// parameter vector, `material_surface.wgsl`).
pub const MATERIAL_TEXTURE_DATA_WORDS: usize = 88;

/// A material texture table: one view per slot, its sampler and metadata.
pub struct MaterialTextureTable {
    _fallback_texture: wgpu::Texture,
    // Boxed (as the slots' views are, in `slots`' heap buffer): their
    // addresses never change when the table moves.
    fallback_view: Box<wgpu::TextureView>,
    sampler: Box<wgpu::Sampler>,
    /// The `MaterialTextureData` sentinel (`params.w = -1`): shaders derive
    /// a material's texture slots from its row (`tex_*`, identity UVs).
    metadata: wgpu::Buffer,
    /// One entry per slot; never resized, so the views' addresses are
    /// stable until an entry is replaced (then `view_refs` is rebuilt).
    slots: Vec<Option<(wgpu::Texture, wgpu::TextureView)>>,
    /// The table's views (a slot's or the fallback), pointing into `self`.
    view_refs: Vec<&'static wgpu::TextureView>,
    sampler_refs: Vec<&'static wgpu::Sampler>,
    version: u64,
}

impl MaterialTextureTable {
    /// An empty table as large as `device` binds ([`MaterialBindingConfig`]),
    /// read with `sampler` (the default sampler without).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: Option<&MaterialSampler>,
    ) -> Self {
        use wgpu::util::DeviceExt;
        let binding = MaterialBindingConfig::for_device(device);
        let fallback_texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("Default Material Texture"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        let fallback_view =
            Box::new(fallback_texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let sampler = Box::new(
            device.create_sampler(&sampler.map(|s| s.0.clone()).unwrap_or(
                wgpu::SamplerDescriptor {
                    label: Some("Default Material Sampler"),
                    ..Default::default()
                },
            )),
        );
        let mut sentinel = [0u32; MATERIAL_TEXTURE_DATA_WORDS];
        sentinel[MATERIAL_TEXTURE_DATA_WORDS - 1] = (-1.0f32).to_bits();
        let metadata = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Material Texture Metadata"),
            contents: bytemuck::cast_slice(&sentinel),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let mut table = Self {
            _fallback_texture: fallback_texture,
            fallback_view,
            sampler,
            metadata,
            slots: vec![None; binding.max_textures],
            view_refs: Vec::with_capacity(binding.max_textures),
            sampler_refs: Vec::with_capacity(binding.max_textures),
            version: 0,
        };
        table.refresh_refs();
        table
    }

    /// Slots the table holds.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Puts `texture` (`None`: the white fallback) in `slot`. Returns whether
    /// the slot changed (the bindings' version then moves); an error for a
    /// texture the table cannot bind (not a filterable, single-layer,
    /// single-sample 2D texture) or a slot past the table.
    pub fn set(
        &mut self,
        device_features: wgpu::Features,
        slot: usize,
        texture: Option<&wgpu::Texture>,
    ) -> Result<bool, String> {
        let Some(cached) = self.slots.get_mut(slot) else {
            return Err(format!(
                "material texture slot {slot} is past the table's {} slots",
                self.slots.len()
            ));
        };
        if cached.as_ref().map(|(texture, _)| texture) == texture {
            return Ok(false);
        }
        if let Some(texture) = texture {
            if texture.dimension() != wgpu::TextureDimension::D2
                || texture.depth_or_array_layers() != 1
                || texture.sample_count() != 1
                || !texture
                    .usage()
                    .contains(wgpu::TextureUsages::TEXTURE_BINDING)
                || texture.format().sample_type(None, Some(device_features))
                    != Some(wgpu::TextureSampleType::Float { filterable: true })
            {
                return Err(format!(
                    "material texture slot {slot} requires a filterable, single-layer, single-sample 2D texture"
                ));
            }
            *cached = Some((texture.clone(), texture.create_view(&Default::default())));
        } else {
            *cached = None;
        }
        self.version = self.version.wrapping_add(1);
        self.refresh_refs();
        Ok(true)
    }

    /// The table as drawers bind it.
    pub fn bindings(&self) -> MaterialTextureBindings<'_> {
        MaterialTextureBindings {
            material_textures: &self.metadata,
            texture_views: &self.view_refs,
            samplers: &self.sampler_refs,
            version: self.version,
        }
    }

    /// Points `view_refs`/`sampler_refs` at this table's current views.
    fn refresh_refs(&mut self) {
        // SAFETY: the views and the sampler are owned by `self` on the heap
        // (the boxes, `slots`' buffer), so they stay where they are when the
        // table moves, and live as long as it does; `slots` is never resized
        // and an entry replaced in `set` calls this again. The `'static`
        // references never leave `self` but as borrows of it (`bindings`).
        let fallback: &'static wgpu::TextureView =
            unsafe { &*(&*self.fallback_view as *const wgpu::TextureView) };
        self.view_refs.clear();
        for entry in &self.slots {
            self.view_refs.push(match entry {
                Some((_, view)) => unsafe { &*(view as *const wgpu::TextureView) },
                None => fallback,
            });
        }
        let sampler: &'static wgpu::Sampler = unsafe { &*(&*self.sampler as *const wgpu::Sampler) };
        self.sampler_refs.clear();
        self.sampler_refs.resize(self.slots.len(), sampler);
    }
}

/// What a drawer binds where no materials plugin publishes the material
/// table: the grey stand-in material (`MaterialRow::STAND_IN`, which the
/// shaders' `material_at` also returns for a row the table lacks) and a
/// texture table of white fallbacks. A kind's drawer keeps one and binds
/// it when `MATERIAL_TEXTURES` is not in the frame or no `materials` rows
/// are; without the materials plugin every surface draws default grey PBR.
pub struct MaterialStandIn {
    table: MaterialTextureTable,
    rows: wgpu::Buffer,
}

impl MaterialStandIn {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        use wgpu::util::DeviceExt;
        let rows = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Stand-in material row"),
            contents: bytemuck::bytes_of(&MaterialRow::STAND_IN),
            usage: wgpu::BufferUsages::STORAGE,
        });
        Self {
            table: MaterialTextureTable::new(device, queue, None),
            rows,
        }
    }

    /// The stand-in texture table (white fallbacks). Its version is
    /// `u64::MAX`, which a published table never reaches, so a drawer
    /// caching bind groups by version rebuilds when it switches.
    pub fn textures(&self) -> MaterialTextureBindings<'_> {
        MaterialTextureBindings {
            version: u64::MAX,
            ..self.table.bindings()
        }
    }

    /// One stand-in material row (the `materials` table's stand-in).
    pub fn rows(&self) -> &wgpu::Buffer {
        &self.rows
    }
}
