use crate::HlfsConfig;

pub(crate) const TILE_SIZE: u32 = 8;
pub(crate) const COARSE_TILE_SIZE: u32 = 64;
pub(crate) const GRID_CAPACITY: u64 = 64;
pub(crate) const COARSE_CAPACITY: u64 = 256;
pub(crate) const VISIBLE_CAPACITY: u64 = 16;
pub(crate) const PROPOSAL_BYTES: u64 = 40;

pub(crate) struct Image {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}
impl Image {
    pub fn new(
        device: &wgpu::Device,
        name: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        storage: bool,
    ) -> Self {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | if storage {
                wgpu::TextureUsages::STORAGE_BINDING
            } else {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST
            };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }
}
pub(crate) struct History {
    pub reservoirs: Image,
    pub lighting: Image,
    pub geometry: Image,
    pub visible: wgpu::Buffer,
}
pub(crate) struct Targets {
    pub depth_bounds: Image,
    pub depth_mips: Vec<wgpu::TextureView>,
    pub raw_lighting: Image,
    pub history: [History; 2],
    pub output: Image,
    pub coarse: wgpu::Buffer,
    pub proposals: wgpu::Buffer,
    pub grid: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    pub sample_width: u32,
    pub sample_height: u32,
}
fn buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}
pub(crate) fn tile_count(width: u32, height: u32, size: u32) -> u64 {
    u64::from(width.max(1).div_ceil(size)) * u64::from(height.max(1).div_ceil(size))
}
impl Targets {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        config: HlfsConfig,
        output: Option<Image>,
    ) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let (sw, sh) = (
            width.div_ceil(config.sample_scale),
            height.div_ceil(config.sample_scale),
        );
        let color = |label| Image::new(device, label, sw, sh, wgpu::TextureFormat::Rg32Uint, true);
        let history = std::array::from_fn(|_| History {
            reservoirs: Image::new(
                device,
                "HLFS weighted reservoir history",
                if config.temporal_resampling { sw } else { 1 },
                if config.temporal_resampling {
                    sh * config.samples_per_pixel
                } else {
                    1
                },
                wgpu::TextureFormat::Rgba32Uint,
                true,
            ),
            lighting: color("HLFS lighting history"),
            geometry: Image::new(
                device,
                "HLFS geometry history",
                sw,
                sh,
                wgpu::TextureFormat::Rg32Uint,
                true,
            ),
            visible: buffer(
                device,
                "HLFS visible light history",
                tile_count(sw, sh, TILE_SIZE) * (3 + VISIBLE_CAPACITY) * 4,
            ),
        });
        let depth_width = width.div_ceil(TILE_SIZE).next_power_of_two();
        let depth_height = height.div_ceil(TILE_SIZE).next_power_of_two();
        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HLFS current depth pyramid"),
            size: wgpu::Extent3d {
                width: depth_width,
                height: depth_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: depth_width.max(depth_height).ilog2() + 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let depth_mips = (0..depth_texture.mip_level_count())
            .map(|mip| {
                depth_texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let depth_bounds = Image {
            view: depth_texture.create_view(&Default::default()),
            texture: depth_texture,
        };
        Self {
            depth_bounds,
            depth_mips,
            raw_lighting: color("HLFS raw lighting"),
            history,
            output: output
                .unwrap_or_else(|| Image::new(device, "HLFS output", width, height, format, false)),
            coarse: buffer(
                device,
                "HLFS coarse light grid",
                tile_count(width, height, COARSE_TILE_SIZE) * (2 + COARSE_CAPACITY / 2) * 4,
            ),
            proposals: buffer(
                device,
                "HLFS tile proposals",
                if config.tile_presampling {
                    (tile_count(width, height, COARSE_TILE_SIZE) * 256 + 1) * PROPOSAL_BYTES
                } else {
                    PROPOSAL_BYTES
                },
            ),
            grid: buffer(
                device,
                "HLFS fine light grid",
                tile_count(width, height, TILE_SIZE) * (2 + GRID_CAPACITY / 2) * 4,
            ),
            width,
            height,
            sample_width: sw,
            sample_height: sh,
        }
    }
}

pub(crate) struct Fallbacks {
    pub empty_lights: wgpu::Buffer,
    /// No GI probes: one pixel's word pair, which the composite ignores.
    pub empty_gi: wgpu::Buffer,
    pub empty_shadow_matrices: wgpu::Buffer,
    pub black: Image,
    /// `black` as a one-layer array: the zero (unfiltered) glass transmittance.
    pub black_array_view: wgpu::TextureView,
    pub lightmap_uv: Image,
    pub _shadow: Image,
    pub shadow_view: wgpu::TextureView,
    pub shadow_sampler: wgpu::Sampler,
    pub linear_sampler: wgpu::Sampler,
    pub _noise: wgpu::Texture,
    pub noise_view: wgpu::TextureView,
}
impl Fallbacks {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let black = Image::new(
            device,
            "HLFS black fallback",
            1,
            1,
            wgpu::TextureFormat::Rgba16Float,
            false,
        );
        let black_array_view = black.texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let lightmap_uv = Image::new(
            device,
            "HLFS missing lightmap UV",
            1,
            1,
            wgpu::TextureFormat::Rgba32Float,
            false,
        );
        queue.write_texture(
            lightmap_uv.texture.as_image_copy(),
            bytemuck::cast_slice(&[-1.0f32, -1.0, 0.0, 0.0]),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(16),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let shadow = Image::new(
            device,
            "HLFS unshadowed fallback",
            1,
            1,
            wgpu::TextureFormat::Depth32Float,
            false,
        );
        let shadow_view = shadow.texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Initialize HLFS fallback shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        queue.submit([encoder.finish()]);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("HLFS shadow sampler"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("HLFS lightmap sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let noise = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HLFS scalar STBN"),
            size: wgpu::Extent3d {
                width: 32,
                height: 32,
                depth_or_array_layers: 32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        queue.write_texture(
            noise.as_image_copy(),
            include_bytes!("../assets/stbn_32x32x32.r8"),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(32),
                rows_per_image: Some(32),
            },
            noise.size(),
        );
        let noise_view = noise.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            empty_gi: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS no GI"),
                size: 8,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            empty_lights: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS empty lights"),
                size: 128,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            empty_shadow_matrices: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("HLFS empty shadow matrices"),
                size: 96,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }),
            black,
            black_array_view,
            lightmap_uv,
            _shadow: shadow,
            shadow_view,
            shadow_sampler,
            linear_sampler,
            _noise: noise,
            noise_view,
        }
    }
}
