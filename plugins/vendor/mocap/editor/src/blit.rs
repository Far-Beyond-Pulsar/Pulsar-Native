//! Camera frames onto a WGPU surface: the first live camera feed in the
//! engine's UI.
//!
//! A frame is uploaded into a texture on the surface's own device and drawn
//! letterboxed into the surface's back buffer; the surface is then published
//! to the compositor. The viewport calls [`FrameBlit::upload`] only for a frame
//! newer than the last it saw, and [`FrameBlit::draw`] whenever it repaints.

use mocap_capture::{PixelFormat, VideoFrame};

const SHADER: &str = r#"
struct Params {
    rect: vec4<f32>,
    flags: vec4<f32>,
};
@group(0) @binding(0) var video: texture_2d<f32>;
@group(0) @binding(1) var video_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[index];
    var out: VertexOut;
    out.position = vec4<f32>(mix(params.rect.x, params.rect.z, c.x), mix(params.rect.y, params.rect.w, c.y), 0.0, 1.0);
    out.uv = vec2<f32>(select(c.x, 1.0 - c.x, params.flags.x > 0.5), c.y);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(video, video_sampler, in.uv).rgb, 1.0);
}
"#;

/// Background behind the letterbox bars.
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.04, g: 0.045, b: 0.05, a: 1.0 };

/// The NDC rectangle `(left, top, right, bottom)` that fits an image of
/// `image` pixels inside a target of `target` pixels, centred, keeping its
/// aspect ratio.
pub fn fit_rect(image: (u32, u32), target: (u32, u32)) -> [f32; 4] {
    if image.0 == 0 || image.1 == 0 || target.0 == 0 || target.1 == 0 {
        return [0.0; 4];
    }
    let image_aspect = image.0 as f32 / image.1 as f32;
    let target_aspect = target.0 as f32 / target.1 as f32;
    let (half_w, half_h) = if image_aspect > target_aspect {
        (1.0, target_aspect / image_aspect)
    } else {
        (image_aspect / target_aspect, 1.0)
    };
    [-half_w, half_h, half_w, -half_h]
}

struct VideoTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

/// Draws the newest camera frame into a surface.
pub struct FrameBlit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    video: Option<VideoTexture>,
}

impl FrameBlit {
    /// Builds the pipeline for a surface of `target_format` on `device`.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mocap video blit"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mocap video blit layout"),
            entries: &[
                texture_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mocap video blit pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mocap video blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mocap video sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mocap video params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, sampler, params, video: None }
    }

    /// Size of the uploaded frame, `None` before the first.
    pub fn video_size(&self) -> Option<(u32, u32)> {
        self.video.as_ref().map(|v| (v.width, v.height))
    }

    /// Uploads `frame`, recreating the texture when its size or layout changed.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &VideoFrame) {
        if !frame.is_well_formed() || frame.width == 0 || frame.height == 0 {
            return;
        }
        let format = match frame.format {
            PixelFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
            PixelFormat::Bgra8 => wgpu::TextureFormat::Bgra8Unorm,
        };
        let stale = self
            .video
            .as_ref()
            .is_none_or(|v| v.width != frame.width || v.height != frame.height || v.format != format);
        if stale {
            self.video = Some(self.create_texture(device, frame.width, frame.height, format));
        }
        let Some(video) = &self.video else { return };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &video.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(frame.width * 4),
                rows_per_image: Some(frame.height),
            },
            wgpu::Extent3d { width: frame.width, height: frame.height, depth_or_array_layers: 1 },
        );
    }

    fn create_texture(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> VideoTexture {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mocap video"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mocap video bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
            ],
        });
        VideoTexture { texture, bind_group, width, height, format }
    }

    /// Draws the video, letterboxed (and mirrored when asked), into `target`
    /// of `size` pixels; with no video yet, only the background. Returns the
    /// submission to publish the surface with.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        size: (u32, u32),
        mirror: bool,
    ) -> wgpu::SubmissionIndex {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mocap video blit"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mocap video blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(BACKGROUND), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(video) = &self.video {
                let rect = fit_rect((video.width, video.height), size);
                let flags = [f32::from(u8::from(mirror)), 0.0, 0.0, 0.0];
                let params: [f32; 8] = [rect[0], rect[1], rect[2], rect[3], flags[0], flags[1], flags[2], flags[3]];
                queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&params));
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &video.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        queue.submit(Some(encoder.finish()))
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::fit_rect;

    #[test]
    fn a_wide_image_in_a_square_target_letterboxes_top_and_bottom() {
        let r = fit_rect((1920, 1080), (800, 800));
        assert_eq!((r[0], r[2]), (-1.0, 1.0));
        assert!((r[1] - 0.5625).abs() < 1e-4 && (r[3] + 0.5625).abs() < 1e-4);
    }

    #[test]
    fn a_tall_image_in_a_wide_target_pillarboxes() {
        let r = fit_rect((600, 800), (1600, 800));
        assert_eq!((r[1], r[3]), (1.0, -1.0));
        assert!((r[2] - 0.375).abs() < 1e-4);
    }

    #[test]
    fn an_empty_size_draws_nothing() {
        assert_eq!(fit_rect((0, 0), (10, 10)), [0.0; 4]);
        assert_eq!(fit_rect((10, 10), (0, 5)), [0.0; 4]);
    }
}
