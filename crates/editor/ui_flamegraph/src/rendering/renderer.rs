use super::types::{FlamegraphUniforms, GpuSpan, RectInstance};

const PALETTE: [[f32; 4]; 16] = palette_float4();

const fn hsla(h: f64, s: f64, l: f64) -> [f32; 4] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 1.0 / 6.0 {
        (c, x, 0.0)
    } else if h < 2.0 / 6.0 {
        (x, c, 0.0)
    } else if h < 3.0 / 6.0 {
        (0.0, c, x)
    } else if h < 4.0 / 6.0 {
        (0.0, x, c)
    } else if h < 5.0 / 6.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [(r + m) as f32, (g + m) as f32, (b + m) as f32, 1.0]
}

const fn palette_float4() -> [[f32; 4]; 16] {
    [
        hsla(210.0 / 360.0, 0.70, 0.65),
        hsla(30.0 / 360.0, 0.75, 0.65),
        hsla(140.0 / 360.0, 0.65, 0.60),
        hsla(340.0 / 360.0, 0.70, 0.65),
        hsla(270.0 / 360.0, 0.65, 0.65),
        hsla(180.0 / 360.0, 0.60, 0.60),
        hsla(50.0 / 360.0, 0.70, 0.65),
        hsla(10.0 / 360.0, 0.70, 0.65),
        hsla(160.0 / 360.0, 0.65, 0.60),
        hsla(290.0 / 360.0, 0.65, 0.65),
        hsla(195.0 / 360.0, 0.65, 0.65),
        hsla(80.0 / 360.0, 0.60, 0.60),
        hsla(320.0 / 360.0, 0.70, 0.65),
        hsla(40.0 / 360.0, 0.70, 0.65),
        hsla(250.0 / 360.0, 0.65, 0.65),
        hsla(120.0 / 360.0, 0.65, 0.60),
    ]
}

struct Pipe {
    span_pipe: wgpu::RenderPipeline,
    text_pipe: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    uni_buf: wgpu::Buffer,
    pal_buf: wgpu::Buffer,
    pal_written: bool,
    span_buf: wgpu::Buffer,
    span_cap: u64,
    text_buf: wgpu::Buffer,
    text_cap: u64,
    /// Tracking pointer + len of last uploaded span data to skip redundant uploads.
    span_data_ptr: *const u8,
    span_data_len: u64,
}

pub struct FlamegraphRenderer {
    pipe: Option<Pipe>,
}

impl FlamegraphRenderer {
    pub fn new() -> Self {
        Self { pipe: None }
    }

    pub fn render_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        _w: u32,
        _h: u32,
        fmt: wgpu::TextureFormat,
        uniforms: &FlamegraphUniforms,
        spans: &[GpuSpan],
        text: &[RectInstance],
    ) {
        let pipe = self
            .pipe
            .get_or_insert_with(|| Self::create_pipe(device, fmt));

        // One-shot palette upload
        if !pipe.pal_written {
            let pal_bytes = bytemuck::cast_slice::<[f32; 4], u8>(&PALETTE);
            queue.write_buffer(&pipe.pal_buf, 0, pal_bytes);
            pipe.pal_written = true;
        }

        // Upload uniforms
        let uni_bytes = bytemuck::bytes_of(uniforms);
        queue.write_buffer(&pipe.uni_buf, 0, uni_bytes);

        // Span storage buffer — skip upload if data hasn't changed
        let span_count = spans.len() as u32;
        if span_count > 0 {
            let span_bytes = bytemuck::cast_slice(spans);
            let data_ptr = spans.as_ptr() as *const u8;
            let data_len = span_bytes.len() as u64;
            let changed = pipe.span_data_ptr != data_ptr || pipe.span_data_len != data_len;
            if changed {
                Self::ensure_buf(
                    device,
                    &mut pipe.span_buf,
                    &mut pipe.span_cap,
                    span_bytes,
                    wgpu::BufferUsages::STORAGE,
                );
                queue.write_buffer(&pipe.span_buf, 0, span_bytes);
                pipe.span_data_ptr = data_ptr;
                pipe.span_data_len = data_len;
            }
        }

        // Text vertex buffer
        let text_count = text.len() as u32;
        if text_count > 0 {
            let text_bytes = bytemuck::cast_slice(text);
            Self::ensure_buf(
                device,
                &mut pipe.text_buf,
                &mut pipe.text_cap,
                text_bytes,
                wgpu::BufferUsages::VERTEX,
            );
            queue.write_buffer(&pipe.text_buf, 0, text_bytes);
        }

        // Bind group — rebuilt each frame (cheap) in case buffers were resized
        let pal_size = wgpu::BufferSize::new((std::mem::size_of::<[f32; 4]>() * 16) as u64);
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flamegraph_bg"),
            layout: &pipe.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: pipe.uni_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &pipe.span_buf,
                        offset: 0,
                        size: None,
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &pipe.pal_buf,
                        offset: 0,
                        size: pal_size,
                    }),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("flamegraph_encoder"),
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("flamegraph_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_bind_group(0, &bg, &[]);

        // 1. Draw spans via vertex-pulling (no vertex buffer needed)
        if span_count > 0 {
            pass.set_pipeline(&pipe.span_pipe);
            pass.draw(0..6, 0..span_count);
        }

        // 2. Draw text rects (uses vertex attributes)
        if text_count > 0 {
            pass.set_pipeline(&pipe.text_pipe);
            pass.set_vertex_buffer(0, pipe.text_buf.slice(..));
            pass.draw(0..6, 0..text_count);
        }

        drop(pass);
        queue.submit(std::iter::once(encoder.finish()));
    }

    fn ensure_buf(
        device: &wgpu::Device,
        buf: &mut wgpu::Buffer,
        cap: &mut u64,
        data: &[u8],
        usage: wgpu::BufferUsages,
    ) {
        let needed = data.len() as u64;
        if needed > *cap {
            *cap = (needed * 2).max(256);
            *buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: *cap,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
    }

    fn create_pipe(device: &wgpu::Device, fmt: wgpu::TextureFormat) -> Pipe {
        let span_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flamegraph_span"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/flamegraph.wgsl").into()),
        });
        let text_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flamegraph_text"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/flamegraph_text.wgsl").into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flamegraph_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pl_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flamegraph_layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let uni_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flamegraph_uni"),
            size: std::mem::size_of::<FlamegraphUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pal_size = (std::mem::size_of::<[f32; 4]>() * 16) as u64;
        let pal_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flamegraph_palette"),
            size: pal_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let span_init = 16384u64;
        let span_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flamegraph_span_storage"),
            size: span_init,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let text_init = 4096u64;
        let text_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flamegraph_text_vert"),
            size: text_init,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Span pipeline — pure vertex-pulling, no vertex buffer
        let span_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flamegraph_span_pipe"),
            layout: Some(&pl_layout),
            vertex: wgpu::VertexState {
                module: &span_mod,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &span_mod,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: fmt,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Text pipeline — reads from vertex attributes
        let text_vbl = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RectInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2,
                1 => Float32x2,
                2 => Float32x4,
                3 => Uint32,
                4 => Uint32x3,
            ],
        };
        let text_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flamegraph_text_pipe"),
            layout: Some(&pl_layout),
            vertex: wgpu::VertexState {
                module: &text_mod,
                entry_point: Some("vs_main"),
                buffers: &[text_vbl],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &text_mod,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: fmt,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Pipe {
            span_pipe,
            text_pipe,
            bgl,
            uni_buf,
            pal_buf,
            pal_written: false,
            span_buf,
            span_cap: span_init,
            text_buf,
            text_cap: text_init,
            span_data_ptr: std::ptr::null(),
            span_data_len: 0,
        }
    }
}
