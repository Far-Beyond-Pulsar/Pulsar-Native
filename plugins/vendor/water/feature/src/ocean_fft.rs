//! GPU Tessendorf ocean simulation ported from Ultra-Water's `OceanSim.js`.
//!
//! Four 256² cascades share one 256×1024 atlas during the packed Stockham IFFT.
//! The finished displacement and slope-moment fields live in repeat sampled
//! texture arrays with a complete manually generated mip chain. Keeping this
//! stage independent from volume geometry is intentional: Ultra-Water models
//! one continuous ocean; Helio's authored volume bounds are consumed later by
//! the surface renderer and must not distort the spectrum or its world scale.

use std::f32::consts::PI;
use wgpu::util::DeviceExt;

pub const FFT_SIZE: u32 = 256;
pub const CASCADE_COUNT: usize = 4;
pub const CASCADE_LENGTHS: [f32; CASCADE_COUNT] = [600.0, 85.0, 12.0, 2.1];
const LOG_N: u32 = 8;
const ATLAS_HEIGHT: u32 = FFT_SIZE * CASCADE_COUNT as u32;
const MIP_LEVELS: u32 = LOG_N + 1;
const LOOP_SECONDS: f32 = 300.0;
const GRAVITY: f32 = 9.81;

/// Ultra-Water's authored ocean spectrum. Values are kept at the reference
/// defaults; scene-level controls can be added without changing the solver.
#[derive(Clone, Copy, Debug)]
pub struct OceanSpectrum {
    pub wind_speed: f32,
    pub wind_direction_degrees: f32,
    pub fetch_km: f32,
    pub spread_blend: f32,
    pub wind_swell: f32,
    pub gamma: f32,
    pub short_waves_fade: f32,
    pub capillary_fade: f32,
    pub wave_scale: f32,
    pub swell_scale: f32,
    pub swell_wind_speed: f32,
    pub swell_direction_degrees: f32,
    pub swell_fetch_km: f32,
    pub swell_swell: f32,
    pub choppiness: f32,
    pub foam_recovery: f32,
    pub time_scale: f32,
    pub seed: u32,
}

impl Default for OceanSpectrum {
    fn default() -> Self {
        Self {
            wind_speed: 9.0,
            wind_direction_degrees: 20.0,
            fetch_km: 120.0,
            spread_blend: 0.85,
            wind_swell: 0.25,
            gamma: 3.3,
            short_waves_fade: 0.04,
            capillary_fade: 0.08,
            wave_scale: 1.0,
            swell_scale: 0.7,
            swell_wind_speed: 7.0,
            swell_direction_degrees: -15.0,
            swell_fetch_km: 900.0,
            swell_swell: 1.0,
            choppiness: 0.85,
            foam_recovery: 0.7,
            time_scale: 1.0,
            seed: 1337,
        }
    }
}

impl OceanSpectrum {
    fn sanitized(mut self) -> Self {
        fn finite(value: f32, fallback: f32) -> f32 {
            if value.is_finite() {
                value
            } else {
                fallback
            }
        }
        let defaults = Self::default();
        self.wind_speed = finite(self.wind_speed, defaults.wind_speed).max(0.1);
        self.wind_direction_degrees =
            finite(self.wind_direction_degrees, defaults.wind_direction_degrees).rem_euclid(360.0);
        self.fetch_km = finite(self.fetch_km, defaults.fetch_km).max(0.01);
        self.spread_blend = finite(self.spread_blend, defaults.spread_blend).clamp(0.0, 1.0);
        self.wind_swell = finite(self.wind_swell, defaults.wind_swell).clamp(0.0, 2.0);
        self.gamma = finite(self.gamma, defaults.gamma).max(1.0);
        self.short_waves_fade = finite(self.short_waves_fade, defaults.short_waves_fade).max(0.0);
        self.capillary_fade = finite(self.capillary_fade, defaults.capillary_fade).max(0.0);
        self.wave_scale = finite(self.wave_scale, defaults.wave_scale).max(0.0);
        self.swell_scale = finite(self.swell_scale, defaults.swell_scale).max(0.0);
        self.swell_wind_speed = finite(self.swell_wind_speed, defaults.swell_wind_speed).max(0.1);
        self.swell_direction_degrees = finite(
            self.swell_direction_degrees,
            defaults.swell_direction_degrees,
        )
        .rem_euclid(360.0);
        self.swell_fetch_km = finite(self.swell_fetch_km, defaults.swell_fetch_km).max(0.01);
        self.swell_swell = finite(self.swell_swell, defaults.swell_swell).clamp(0.0, 2.0);
        self.choppiness = finite(self.choppiness, defaults.choppiness).max(0.0);
        self.foam_recovery = finite(self.foam_recovery, defaults.foam_recovery).max(0.0);
        self.time_scale = finite(self.time_scale, defaults.time_scale).max(0.0);
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpectrumUniform {
    spec_a: [[f32; 4]; 2],
    spec_b: [[f32; 4]; 2],
    lengths: [f32; 4],
    cuts: [f32; 4],
    capillary_fade: f32,
    seed: u32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EvolveUniform {
    lengths: [f32; 4],
    time: f32,
    _pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FftUniform {
    sub: u32,
    vertical: u32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct AssembleUniform {
    lambda: f32,
    dt: f32,
    foam_recovery: f32,
    previous_valid: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MipUniform {
    /// The source mip's width and height. The destination is half this size.
    source_size: [u32; 2],
    _pad: [u32; 2],
}

/// Per-patch reduction of the frame's f64 world origin. The camera-relative
/// surface coordinates can then be reduced modulo each independent FFT patch
/// without inheriting Helio's old shared 270 m tiling period.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct OceanPhaseUniform {
    /// x/z origin phase for the 600, 85, 12 and 2.1 m patches.
    phase: [[f32; 4]; CASCADE_COUNT],
}

struct FftAtlases {
    _texture0: wgpu::Texture,
    _texture1: wgpu::Texture,
    view0: wgpu::TextureView,
    view1: wgpu::TextureView,
}

struct OceanOutputs {
    _displacement: wgpu::Texture,
    _slopes: wgpu::Texture,
    pub(crate) displacement_array: wgpu::TextureView,
    pub(crate) slopes_array: wgpu::TextureView,
    displacement_layers: Vec<Vec<wgpu::TextureView>>,
    slope_layers: Vec<Vec<wgpu::TextureView>>,
}

struct FoamHistory {
    _texture: wgpu::Texture,
    pub(crate) array: wgpu::TextureView,
    layers: Vec<wgpu::TextureView>,
}

/// Stateful GPU implementation of Ultra-Water's Fourier ocean.
pub struct OceanFft {
    spectrum: OceanSpectrum,
    spectrum_uniform: wgpu::Buffer,
    phase_uniform: wgpu::Buffer,
    evolve_uniform: wgpu::Buffer,
    evolve_bg: wgpu::BindGroup,
    evolve_pipeline: wgpu::RenderPipeline,
    init_pipeline: wgpu::RenderPipeline,
    init_bg: wgpu::BindGroup,
    fft_uniforms: Vec<wgpu::Buffer>,
    fft_bgs: [Vec<wgpu::BindGroup>; 2],
    fft_pipeline: wgpu::RenderPipeline,
    assemble_uniforms: [wgpu::Buffer; CASCADE_COUNT],
    assemble_bgs: Vec<[[wgpu::BindGroup; 2]; 2]>,
    assemble_pipeline: wgpu::RenderPipeline,
    mip_bgs: Vec<Vec<wgpu::BindGroup>>,
    mip_pipeline: wgpu::RenderPipeline,
    _h0: wgpu::Texture,
    h0_view: wgpu::TextureView,
    ping: FftAtlases,
    pong: FftAtlases,
    outputs: OceanOutputs,
    foam_history: [FoamHistory; 2],
    foam_parity: usize,
    initialized: bool,
    time: f32,
    outputs_valid: bool,
    spectrum_dirty: bool,
    prepared_dt: f32,
}

impl OceanFft {
    pub fn new(device: &wgpu::Device) -> Self {
        let h0 = make_atlas(device, "Ultra Water H0", wgpu::TextureFormat::Rgba32Float);
        let h0_view = h0.create_view(&wgpu::TextureViewDescriptor::default());
        let ping = make_fft_atlases(device, "Ultra Water FFT Ping");
        let pong = make_fft_atlases(device, "Ultra Water FFT Pong");
        let outputs = make_ocean_outputs(device);
        let foam_history = [
            make_foam_history(device, "Ultra Water Foam History A"),
            make_foam_history(device, "Ultra Water Foam History B"),
        ];

        let spectrum = OceanSpectrum::default().sanitized();
        let spectrum_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ultra Water Spectrum Uniform"),
            contents: bytemuck::bytes_of(&spectrum_uniform(spectrum)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let phase_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ultra Water Per Cascade World Phase"),
            contents: bytemuck::bytes_of(&OceanPhaseUniform {
                phase: [[0.0; 4]; CASCADE_COUNT],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let evolve_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ultra Water Evolve Uniform"),
            contents: bytemuck::bytes_of(&EvolveUniform {
                lengths: CASCADE_LENGTHS,
                time: 0.0,
                _pad: [0.0; 3],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let fullscreen = helio_core::shader::module(
            device,
            "Ultra Water FFT Fullscreen VS",
            helio_core::include_wgsl!("../shaders/fullscreen.vert.wgsl"),
        );
        let init_shader = helio_core::shader::module(
            device,
            "Ultra Water Spectrum Init FS",
            helio_core::include_wgsl!("../shaders/ocean_init.wgsl"),
        );
        let evolve_shader = helio_core::shader::module(
            device,
            "Ultra Water Spectrum Evolution FS",
            helio_core::include_wgsl!("../shaders/ocean_evolve.wgsl"),
        );
        let fft_shader = helio_core::shader::module(
            device,
            "Ultra Water Stockham FFT FS",
            helio_core::include_wgsl!("../shaders/ocean_fft.wgsl"),
        );
        let assemble_shader = helio_core::shader::module(
            device,
            "Ultra Water Field Assembly FS",
            helio_core::include_wgsl!("../shaders/ocean_assemble.wgsl"),
        );
        let mip_shader = helio_core::shader::module(
            device,
            "Ultra Water Field Mip FS",
            helio_core::include_wgsl!("../shaders/ocean_mip.wgsl"),
        );

        let texture_entry = |binding, dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: dimension,
                multisampled: false,
            },
            count: None,
        };
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let init_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ultra Water Init BGL"),
            entries: &[uniform_entry(0)],
        });
        let evolve_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ultra Water Evolve BGL"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2),
                uniform_entry(1),
            ],
        });
        let fft_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ultra Water FFT BGL"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2),
                texture_entry(1, wgpu::TextureViewDimension::D2),
                uniform_entry(2),
            ],
        });
        let assemble_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ultra Water Assemble BGL"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2),
                texture_entry(1, wgpu::TextureViewDimension::D2),
                texture_entry(2, wgpu::TextureViewDimension::D2Array),
                uniform_entry(3),
            ],
        });
        let mip_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ultra Water Mip BGL"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2),
                texture_entry(1, wgpu::TextureViewDimension::D2),
                uniform_entry(2),
            ],
        });

        let make_pipeline = |label: &'static str,
                             bgl: &wgpu::BindGroupLayout,
                             shader: &wgpu::ShaderModule,
                             target_count: usize| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            });
            let targets = (0..target_count)
                .map(|_| {
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })
                })
                .collect::<Vec<_>>();
            helio_core::shader::create_render_pipeline(
                device,
                wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &fullscreen,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: shader,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &targets,
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                },
            )
        };

        let init_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ultra Water Init PL"),
            bind_group_layouts: &[Some(&init_bgl)],
            immediate_size: 0,
        });
        let init_pipeline = helio_core::shader::create_render_pipeline(
            device,
            wgpu::RenderPipelineDescriptor {
                label: Some("Ultra Water Spectrum Init"),
                layout: Some(&init_layout),
                vertex: wgpu::VertexState {
                    module: &fullscreen,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &init_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba32Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            },
        );
        let init_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ultra Water Init BG"),
            layout: &init_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: spectrum_uniform.as_entire_binding(),
            }],
        });
        let evolve_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ultra Water Evolve BG"),
            layout: &evolve_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&h0_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: evolve_uniform.as_entire_binding(),
                },
            ],
        });

        let fft_uniforms = (0..2)
            .flat_map(|vertical| (1..=LOG_N).map(move |stage| (vertical, 1 << stage)))
            .map(|(vertical, sub)| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Ultra Water FFT Stage Uniform"),
                    contents: bytemuck::bytes_of(&FftUniform {
                        sub,
                        vertical,
                        _pad: [0; 2],
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
            })
            .collect::<Vec<_>>();
        let fft_bg = |src: &FftAtlases, uniform: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Ultra Water FFT Stage BG"),
                layout: &fft_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src.view0),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&src.view1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            })
        };
        let fft_bgs = [
            fft_uniforms
                .iter()
                .map(|uniform| fft_bg(&ping, uniform))
                .collect(),
            fft_uniforms
                .iter()
                .map(|uniform| fft_bg(&pong, uniform))
                .collect(),
        ];

        let assemble_uniforms = std::array::from_fn(|_| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Ultra Water Assemble Uniform"),
                contents: bytemuck::bytes_of(&AssembleUniform {
                    lambda: spectrum.choppiness,
                    dt: 0.0,
                    foam_recovery: spectrum.foam_recovery,
                    previous_valid: 0.0,
                }),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        });
        let mip_uniforms = (1..MIP_LEVELS)
            .map(|mip| {
                let size = (FFT_SIZE >> (mip - 1)).max(1);
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Ultra Water Mip Uniform"),
                    contents: bytemuck::bytes_of(&MipUniform {
                        source_size: [size, size],
                        _pad: [0; 2],
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                })
            })
            .collect::<Vec<_>>();

        let assemble_bg = |cascade: usize, previous_foam: &FoamHistory, src: &FftAtlases| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Ultra Water Assemble BG"),
                layout: &assemble_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src.view0),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&src.view1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&previous_foam.array),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: assemble_uniforms[cascade].as_entire_binding(),
                    },
                ],
            })
        };
        let assemble_bgs = (0..CASCADE_COUNT)
            .map(|cascade| {
                [
                    [
                        assemble_bg(cascade, &foam_history[0], &ping),
                        assemble_bg(cascade, &foam_history[0], &pong),
                    ],
                    [
                        assemble_bg(cascade, &foam_history[1], &ping),
                        assemble_bg(cascade, &foam_history[1], &pong),
                    ],
                ]
            })
            .collect::<Vec<_>>();
        let mip_bgs = (0..CASCADE_COUNT)
            .map(|cascade| {
                (1..MIP_LEVELS as usize)
                    .map(|mip| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Ultra Water Mip BG"),
                            layout: &mip_bgl,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(
                                        &outputs.displacement_layers[cascade][mip - 1],
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::TextureView(
                                        &outputs.slope_layers[cascade][mip - 1],
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: mip_uniforms[mip - 1].as_entire_binding(),
                                },
                            ],
                        })
                    })
                    .collect()
            })
            .collect();

        let evolve_pipeline =
            make_pipeline("Ultra Water Evolve PL", &evolve_bgl, &evolve_shader, 2);
        let fft_pipeline = make_pipeline("Ultra Water FFT PL", &fft_bgl, &fft_shader, 2);
        let assemble_pipeline = make_pipeline(
            "Ultra Water Assemble PL",
            &assemble_bgl,
            &assemble_shader,
            3,
        );
        let mip_pipeline = make_pipeline("Ultra Water Mip PL", &mip_bgl, &mip_shader, 2);
        Self {
            spectrum,
            spectrum_uniform,
            phase_uniform,
            evolve_uniform,
            evolve_bg,
            evolve_pipeline,
            init_pipeline,
            init_bg,
            fft_uniforms,
            fft_bgs,
            fft_pipeline,
            assemble_uniforms,
            assemble_bgs,
            assemble_pipeline,
            mip_bgs,
            mip_pipeline,
            _h0: h0,
            h0_view,
            ping,
            pong,
            outputs,
            foam_history,
            foam_parity: 0,
            initialized: false,
            time: 0.0,
            outputs_valid: false,
            spectrum_dirty: true,
            prepared_dt: 0.0,
        }
    }

    /// Update the authored spectrum. A change rebuilds H0 once on the next
    /// execute; the fixed random seed makes a rebuild reproducible.
    pub fn set_spectrum(&mut self, spectrum: OceanSpectrum) {
        self.spectrum = spectrum.sanitized();
        self.initialized = false;
        self.spectrum_dirty = true;
    }

    pub fn spectrum(&self) -> OceanSpectrum {
        self.spectrum
    }

    /// Current phase, wrapped to the reference's 300 second loop.
    pub fn time(&self) -> f32 {
        self.time
    }

    /// The four finished, mipmapped fields used by the surface renderer.
    /// Displacement channels are `(lambda*Dx, Dy, lambda*Dz, Jacobian)`;
    /// slope channels are `(sx, sz, sx², sz²)`.
    pub fn displacement_view(&self) -> &wgpu::TextureView {
        &self.outputs.displacement_array
    }

    pub fn slopes_view(&self) -> &wgpu::TextureView {
        &self.outputs.slopes_array
    }

    pub fn phase_binding(&self) -> &wgpu::Buffer {
        &self.phase_uniform
    }

    /// Advance the loop-stable phase and upload the small frame controls before
    /// the graph records this pass's render work.
    pub fn prepare(&mut self, queue: &wgpu::Queue, time_delta: f32, world_origin: glam::DVec3) {
        self.prepared_dt = time_delta.max(0.0) * self.spectrum.time_scale.max(0.0);
        self.time = (self.time + self.prepared_dt) % LOOP_SECONDS;
        if self.spectrum_dirty {
            queue.write_buffer(
                &self.spectrum_uniform,
                0,
                bytemuck::bytes_of(&spectrum_uniform(self.spectrum)),
            );
            self.spectrum_dirty = false;
        }
        queue.write_buffer(
            &self.evolve_uniform,
            0,
            bytemuck::bytes_of(&EvolveUniform {
                lengths: CASCADE_LENGTHS,
                time: self.time,
                _pad: [0.0; 3],
            }),
        );
        let phase = std::array::from_fn(|cascade| {
            let patch = f64::from(CASCADE_LENGTHS[cascade]);
            [
                world_origin.x.rem_euclid(patch) as f32,
                world_origin.z.rem_euclid(patch) as f32,
                0.0,
                0.0,
            ]
        });
        queue.write_buffer(
            &self.phase_uniform,
            0,
            bytemuck::bytes_of(&OceanPhaseUniform { phase }),
        );
        for buffer in &self.assemble_uniforms {
            queue.write_buffer(
                buffer,
                0,
                bytemuck::bytes_of(&AssembleUniform {
                    lambda: self.spectrum.choppiness,
                    dt: self.prepared_dt,
                    foam_recovery: self.spectrum.foam_recovery,
                    previous_valid: if self.outputs_valid { 1.0 } else { 0.0 },
                }),
            );
        }
    }

    /// Encode one complete spectrum/evolution/IFFT/assembly/mip update.
    pub fn execute(&mut self, encoder: &mut helio_core::CommandRecorder<'_>) {
        if !self.initialized {
            let attachments = [Some(color_attachment(&self.h0_view))];
            let mut pass =
                encoder.begin_render_pass(&render_desc("Ultra Water Spectrum Init", &attachments));
            pass.set_pipeline(&self.init_pipeline);
            pass.set_bind_group(0, &self.init_bg, &[]);
            pass.draw(0..6, 0..1);
            self.initialized = true;
        }

        {
            let attachments = [
                Some(color_attachment(&self.ping.view0)),
                Some(color_attachment(&self.ping.view1)),
            ];
            let mut pass =
                encoder.begin_render_pass(&render_desc("Ultra Water Evolve", &attachments));
            pass.set_pipeline(&self.evolve_pipeline);
            pass.set_bind_group(0, &self.evolve_bg, &[]);
            pass.draw(0..6, 0..1);
        }

        let mut src_is_ping = true;
        for (stage_index, _) in self.fft_uniforms.iter().enumerate() {
            let dst = if src_is_ping { &self.pong } else { &self.ping };
            let attachments = [
                Some(color_attachment(&dst.view0)),
                Some(color_attachment(&dst.view1)),
            ];
            let mut pass =
                encoder.begin_render_pass(&render_desc("Ultra Water Stockham Stage", &attachments));
            pass.set_pipeline(&self.fft_pipeline);
            pass.set_bind_group(
                0,
                &self.fft_bgs[if src_is_ping { 0 } else { 1 }][stage_index],
                &[],
            );
            pass.draw(0..6, 0..1);
            src_is_ping = !src_is_ping;
        }

        // Assemble each cascade from the atlas. The previous Jacobian channel
        // supplies the dt-based foam recovery term, matching Ultra-Water.
        for cascade in 0..CASCADE_COUNT {
            let next_foam = self.foam_parity ^ 1;
            let attachments = [
                Some(color_attachment(
                    &self.outputs.displacement_layers[cascade][0],
                )),
                Some(color_attachment(&self.outputs.slope_layers[cascade][0])),
                Some(color_attachment(
                    &self.foam_history[next_foam].layers[cascade],
                )),
            ];
            let mut pass =
                encoder.begin_render_pass(&render_desc("Ultra Water Assemble", &attachments));
            pass.set_pipeline(&self.assemble_pipeline);
            pass.set_bind_group(
                0,
                &self.assemble_bgs[cascade][self.foam_parity][if src_is_ping { 0 } else { 1 }],
                &[],
            );
            // The cascade selector is taken from instance_index.
            pass.draw(0..6, cascade as u32..cascade as u32 + 1);
            drop(pass);
        }
        self.foam_parity ^= 1;
        self.outputs_valid = true;

        self.generate_mips(encoder);
    }

    fn generate_mips(&self, encoder: &mut helio_core::CommandRecorder<'_>) {
        for cascade in 0..CASCADE_COUNT {
            for mip in 1..MIP_LEVELS as usize {
                let attachments = [
                    Some(color_attachment(
                        &self.outputs.displacement_layers[cascade][mip],
                    )),
                    Some(color_attachment(&self.outputs.slope_layers[cascade][mip])),
                ];
                let mut pass =
                    encoder.begin_render_pass(&render_desc("Ultra Water Mip", &attachments));
                pass.set_pipeline(&self.mip_pipeline);
                pass.set_bind_group(0, &self.mip_bgs[cascade][mip - 1], &[]);
                pass.draw(0..6, 0..1);
            }
        }
    }
}

fn spectrum_uniform(p: OceanSpectrum) -> SpectrumUniform {
    fn jonswap_params(wind_speed: f32, fetch_km: f32) -> (f32, f32) {
        let fetch = fetch_km * 1000.0;
        let alpha = 0.076 * ((GRAVITY * fetch) / (wind_speed * wind_speed)).powf(-0.22);
        let peak = 22.0 * ((wind_speed * fetch) / (GRAVITY * GRAVITY)).powf(-0.33);
        (alpha, peak)
    }
    let (alpha, peak) = jonswap_params(p.wind_speed, p.fetch_km);
    let (swell_alpha, swell_peak) = jonswap_params(p.swell_wind_speed, p.swell_fetch_km);
    let to_radians = PI / 180.0;
    SpectrumUniform {
        spec_a: [
            [
                p.wave_scale,
                p.wind_direction_degrees * to_radians,
                p.spread_blend,
                p.wind_swell,
            ],
            [
                p.swell_scale * p.wave_scale,
                p.swell_direction_degrees * to_radians,
                1.0,
                p.swell_swell,
            ],
        ],
        spec_b: [
            [alpha, peak, p.gamma, p.short_waves_fade],
            [swell_alpha, swell_peak, p.gamma, p.short_waves_fade],
        ],
        lengths: CASCADE_LENGTHS,
        cuts: [
            2.0 * PI / CASCADE_LENGTHS[1] * 6.0,
            2.0 * PI / CASCADE_LENGTHS[2] * 6.0,
            2.0 * PI / CASCADE_LENGTHS[3] * 6.0,
            0.0,
        ],
        capillary_fade: p.capillary_fade,
        seed: p.seed,
        _pad: [0; 2],
    }
}

fn make_atlas(device: &wgpu::Device, label: &str, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: FFT_SIZE,
            height: ATLAS_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn make_fft_atlases(device: &wgpu::Device, label: &str) -> FftAtlases {
    let make = |suffix| {
        let texture = make_atlas(
            device,
            &format!("{label} {suffix}"),
            wgpu::TextureFormat::Rgba16Float,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    };
    let (texture0, view0) = make("A");
    let (texture1, view1) = make("B");
    FftAtlases {
        _texture0: texture0,
        _texture1: texture1,
        view0,
        view1,
    }
}

fn make_ocean_outputs(device: &wgpu::Device) -> OceanOutputs {
    let make = |label| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: FFT_SIZE,
                height: FFT_SIZE,
                depth_or_array_layers: CASCADE_COUNT as u32,
            },
            mip_level_count: MIP_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    let displacement = make("Ultra Water Displacement");
    let slopes = make("Ultra Water Slope Moments");
    let array_view = |texture: &wgpu::Texture, label| {
        texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            array_layer_count: Some(CASCADE_COUNT as u32),
            mip_level_count: Some(MIP_LEVELS),
            ..Default::default()
        })
    };
    let layers = |texture: &wgpu::Texture| {
        (0..CASCADE_COUNT)
            .map(|layer| {
                (0..MIP_LEVELS)
                    .map(|mip| {
                        texture.create_view(&wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2),
                            base_mip_level: mip,
                            mip_level_count: Some(1),
                            base_array_layer: layer as u32,
                            array_layer_count: Some(1),
                            ..Default::default()
                        })
                    })
                    .collect()
            })
            .collect()
    };
    let displacement_layers = layers(&displacement);
    let slope_layers = layers(&slopes);
    OceanOutputs {
        displacement_array: array_view(&displacement, "Ultra Water Displacement Array"),
        slopes_array: array_view(&slopes, "Ultra Water Slopes Array"),
        displacement_layers,
        slope_layers,
        _displacement: displacement,
        _slopes: slopes,
    }
}

fn make_foam_history(device: &wgpu::Device, label: &str) -> FoamHistory {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: FFT_SIZE,
            height: FFT_SIZE,
            depth_or_array_layers: CASCADE_COUNT as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let array = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("Ultra Water Foam History Array"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        array_layer_count: Some(CASCADE_COUNT as u32),
        ..Default::default()
    });
    let layers = (0..CASCADE_COUNT)
        .map(|layer| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Ultra Water Foam History Layer"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    FoamHistory {
        _texture: texture,
        array,
        layers,
    }
}

fn color_attachment(view: &wgpu::TextureView) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        resolve_target: None,
        depth_slice: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    }
}

fn render_desc<'a>(
    label: &'a str,
    color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>],
) -> wgpu::RenderPassDescriptor<'a> {
    wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }
}
