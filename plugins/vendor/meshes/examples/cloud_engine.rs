//! Cloud Engine — native Helio example.
//!
//! The artistic preset from `cloud-engine-config_2026-08-23_06-14-45.json` is
//! deliberately compiled into this viewport-only port. There is no control UI:
//! left-drag paints clouds and right-drag erases them. Press F for the standard
//! HelioV flycam (WASD, Space/Shift, IJKL, and mouse look); Escape releases the
//! cursor and returns to painting, then Escape again closes the example.

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use egui_wgpu::{
    Renderer as EguiRenderer, RendererOptions as EguiRendererOptions, ScreenDescriptor,
};
use egui_winit::State as EguiState;
use glam::Vec3;
use microfont::{stamp_text, FHEIGHT};
mod v3_demo_common;
use helio::{Camera, Renderer, RendererBuilder, RendererConfig};
use helio_controls::{FlyCamera, FlyCameraConfig, WinitFlyInput};
use v3_demo_common::{
    cube_mesh, directional_light, make_material, new_scene_db_with_gpu_mirror, scene_db_handle,
    spawn_light, spawn_material, spawn_mesh, spawn_object_with_movability,
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, KeyEvent, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

const VOLUME: (u32, u32, u32) = (96, 48, 96);
// Keep simulation/painting in the original cloudbox but draw a flatter box in the
// viewport for coverage.
const BOUNDS_XZ_EXPAND_FACTOR: f32 = 2.0;
const CLOUD_WORLD_HEIGHT: f32 = 25.0;
const BOUNDS_SIM_MIN_X: f32 = -8.0;
const BOUNDS_SIM_MAX_X: f32 = 8.0;
const BOUNDS_SIM_MIN_Z: f32 = -9.5 * BOUNDS_XZ_EXPAND_FACTOR;
const BOUNDS_SIM_MAX_Z: f32 = 9.5 * BOUNDS_XZ_EXPAND_FACTOR;
const BOUNDS_SIM_MIN_Y: f32 = CLOUD_WORLD_HEIGHT - 2.0;
const BOUNDS_SIM_MAX_Y: f32 = CLOUD_WORLD_HEIGHT + 9.0;

const BOUNDS_SIM_MIN: Vec3 = Vec3::new(
    BOUNDS_SIM_MIN_X * BOUNDS_XZ_EXPAND_FACTOR,
    BOUNDS_SIM_MIN_Y,
    BOUNDS_SIM_MIN_Z,
);
const BOUNDS_SIM_MAX: Vec3 = Vec3::new(
    BOUNDS_SIM_MAX_X * BOUNDS_XZ_EXPAND_FACTOR,
    BOUNDS_SIM_MAX_Y,
    BOUNDS_SIM_MAX_Z,
);

// Rendering and painting must address the same finite volume. The previous
// flattened render band clipped the upper half of the simulation, making
// apparently valid brush hits invisible. Keep the optional scale constant for
// compatibility with old tuning, but use the authoritative simulation bounds.
const BOUNDS_RENDER_MIN: Vec3 = BOUNDS_SIM_MIN;
const BOUNDS_RENDER_MAX: Vec3 = BOUNDS_SIM_MAX;
const SIMULATION_INTERVAL: f32 = 1.0 / 30.0;

// Static version of cloud-engine-config_2026-08-23_06-14-45.json. The saved
// document used Draw mode, whose mutable painted voxels are intentionally not
// part of JSON export. This native port preserves that interaction: it begins
// with an empty editable volume.
const PRESET_POSITION: Vec3 = Vec3::new(-1.1756307, 1.5082186, -6.268016);
const PRESET_YAW: f32 = 0.0;
const PRESET_PITCH: f32 = 0.084796175;
const PRESET_FOV_DEGREES: f32 = 61.0;
const PRESET_SEED: f32 = 19.37;
// A restrained shared daylight cue: each endpoint is reached linearly, then
// the direction reverses, avoiding a visible snap at the loop boundary.
const DAYLIGHT_CYCLE_SECONDS: f32 = 120.0;
const SUN_ELEVATION_MIN_DEGREES: f32 = -6.0;
const SUN_ELEVATION_MAX_DEGREES: f32 = 8.0;
const SUN_AZIMUTH_DEGREES: f32 = 18.0;
const EXPOSURE_MIN: f32 = 1.20;
const EXPOSURE_MAX: f32 = 1.48;
const BRUSH_SIZE_MIN: f32 = 0.035;
const BRUSH_SIZE_MAX: f32 = 0.125;
const BRUSH_DRAIN_PER_SECOND: f32 = 0.016;
const BRUSH_RECHARGE_PER_SECOND: f32 = 0.090;
const WIND_SPEED_RADIANS_PER_SECOND: f32 = std::f32::consts::TAU / 180.0;
const WIND_INITIAL_ANGLE_RADIANS: f32 = 5.0_f32.to_radians();
const WIND_STRENGTH: f32 = 1.95;
const CLOUDBOX_OUTLINE_COLOR: [f32; 4] = [0.16, 0.58, 1.0, 0.92];
const CLOUDBOX_OUTLINE_EPS: f32 = 0.001;
const PERF_MODES: [(f32, f32); 6] = [
    (70.0, 0.0),
    (50.0, 0.0),
    (30.0, 0.0),
    (70.0, 1.0),
    (50.0, 1.0),
    (30.0, 1.0),
];

const PERF_OVERLAY_TEXTURE_WIDTH: u32 = 192;
const PERF_OVERLAY_TEXTURE_HEIGHT: u32 = 32;
const PERF_OVERLAY_SCALE: f32 = 2.0;
const PERF_OVERLAY_MARGIN: f32 = 12.0;

fn brush_sphere_vertices(center: Vec3, radius: Vec3) -> Vec<f32> {
    let mut vertices = Vec::with_capacity(3 * 24 * 3);
    for axis in 0..3 {
        for i in 0..24 {
            let a0 = (i as f32 / 24.0) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / 24.0) * std::f32::consts::TAU;
            let point = |a: f32| {
                let ring = match axis {
                    0 => Vec3::new(0.0, a.cos(), a.sin()),
                    1 => Vec3::new(a.cos(), 0.0, a.sin()),
                    _ => Vec3::new(a.cos(), a.sin(), 0.0),
                };
                center + ring * radius
            };
            for p in [point(a0), point(a1)] {
                vertices.extend_from_slice(&[p.x, p.y, p.z]);
            }
        }
    }
    vertices
}
const CUBE_GRID_SIZE: i32 = 16;
const CUBE_SPACING: f32 = 1.1;
const CUBE_HALF_EXTENT: f32 = 0.45;
// Helio's per-object frustum test uses a bounding sphere, not a half extent.
// A cube needs its half-diagonal here; using 0.45 culled visible corners.
const CUBE_BOUNDING_RADIUS: f32 = CUBE_HALF_EXTENT * 1.732_050_8;
const AXIS_REACH: f32 = 1_024.0;
const AXIS_COMPANION_OFFSET: f32 = 0.025;
const MOON_LOOK_DIRECTIONS: [Vec3; 3] = [
    Vec3::new(0.0, 0.0, 0.893),
    Vec3::new(0.774, 0.0, -0.447),
    Vec3::new(-0.774, 0.0, -0.447),
];
// A broad enough lobe that colours visibly travel between the three moon
// sectors, while a direct look still overwhelmingly selects that moon.
const MOON_LOOK_BLEND_SHARPNESS: f32 = 3.0;
const PERF_OVERLAY_SHADER: &str = r#"
struct Rect { ndc: vec4<f32> }
@group(0) @binding(0) var font_tex: texture_2d<f32>;
@group(0) @binding(1) var font_sampler: sampler;
@group(0) @binding(2) var<uniform> rect: Rect;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[index];
    var out: VertexOut;
    out.position = vec4<f32>(mix(rect.ndc.xy, rect.ndc.zw, corner), 0.0, 1.0);
    out.uv = vec2<f32>(corner.x, 1.0 - corner.y);
    return out;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(font_tex, font_sampler, input.uv);
}
"#;

const SIM_SHADER: &str = include_str!("cloud-engine-webgpu-linux-aligned/shaders/simulate.wgsl");
const RENDER_SHADER: &str = include_str!("cloud-engine-webgpu-linux-aligned/shaders/render.wgsl");
const OUTLINE_SHADER: &str = include_str!("cloud-engine-outline.wgsl");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtPreset {
    Verdant,
    Porcelain,
    Ember,
    Violet,
}

#[derive(Clone, Copy)]
enum PresetMode {
    Static(ArtPreset),
    MoonLook,
}

#[derive(Clone, Copy)]
struct ArtDirection {
    cloud: Vec3,
    shadow: Vec3,
    sky: Vec3,
    moon: Vec3,
    curl: f32,
    ribbon: f32,
    sculpt: f32,
    bands: f32,
    outline: f32,
    moon_size: f32,
    moon_glow: f32,
    grain: f32,
}

impl ArtPreset {
    const fn direction(self) -> ArtDirection {
        match self {
            Self::Verdant => ArtDirection {
                cloud: Vec3::new(0.038, 0.297, 0.287),
                shadow: Vec3::new(0.002, 0.093, 0.086),
                sky: Vec3::new(0.002, 0.05, 0.054),
                moon: Vec3::new(1.0, 0.807, 0.175),
                curl: 1.10,
                ribbon: 0.72,
                sculpt: 0.78,
                bands: 5.0,
                outline: 0.62,
                moon_size: 0.18,
                moon_glow: 1.15,
                grain: 0.20,
            },
            Self::Porcelain => ArtDirection {
                cloud: Vec3::new(0.168, 0.479, 0.584),
                shadow: Vec3::new(0.009, 0.045, 0.095),
                sky: Vec3::new(0.0033, 0.018, 0.06),
                moon: Vec3::new(1.0, 0.871, 0.402),
                curl: 1.28,
                ribbon: 0.86,
                sculpt: 0.84,
                bands: 6.0,
                outline: 0.76,
                moon_size: 0.155,
                moon_glow: 0.92,
                grain: 0.13,
            },
            Self::Ember => ArtDirection {
                cloud: Vec3::new(0.694, 0.258, 0.098),
                shadow: Vec3::new(0.105, 0.022, 0.038),
                sky: Vec3::new(0.026, 0.01, 0.036),
                moon: Vec3::new(1.0, 0.658, 0.15),
                curl: 0.96,
                ribbon: 0.64,
                sculpt: 0.72,
                bands: 4.0,
                outline: 0.54,
                moon_size: 0.21,
                moon_glow: 1.52,
                grain: 0.28,
            },
            Self::Violet => ArtDirection {
                cloud: Vec3::new(0.254, 0.191, 0.571),
                shadow: Vec3::new(0.029, 0.022, 0.078),
                sky: Vec3::new(0.009, 0.009, 0.029),
                moon: Vec3::new(0.922, 0.716, 0.287),
                curl: 1.42,
                ribbon: 0.94,
                sculpt: 0.82,
                bands: 5.0,
                outline: 0.88,
                moon_size: 0.145,
                moon_glow: 1.05,
                grain: 0.24,
            },
        }
    }

    fn parse_arg(name: &str) -> Option<Self> {
        match name {
            "verdant" => Some(Self::Verdant),
            "porcelain" => Some(Self::Porcelain),
            "ember" => Some(Self::Ember),
            "violet" => Some(Self::Violet),
            _ => None,
        }
    }

    fn moon_look_direction(camera_forward: Vec3) -> ArtDirection {
        let horizontal_forward =
            Vec3::new(camera_forward.x, 0.0, camera_forward.z).normalize_or_zero();
        let raw_weights = MOON_LOOK_DIRECTIONS
            .map(|direction| (horizontal_forward.dot(direction) * MOON_LOOK_BLEND_SHARPNESS).exp());
        let total_weight = raw_weights.iter().sum::<f32>().max(f32::EPSILON);
        let weights = raw_weights.map(|weight| weight / total_weight);
        let porcelain = Self::Porcelain.direction();
        let ember = Self::Ember.direction();
        let violet = Self::Violet.direction();
        ArtDirection {
            cloud: porcelain.cloud * weights[0]
                + ember.cloud * weights[1]
                + violet.cloud * weights[2],
            shadow: porcelain.shadow * weights[0]
                + ember.shadow * weights[1]
                + violet.shadow * weights[2],
            sky: porcelain.sky * weights[0] + ember.sky * weights[1] + violet.sky * weights[2],
            moon: porcelain.moon * weights[0] + ember.moon * weights[1] + violet.moon * weights[2],
            curl: porcelain.curl * weights[0] + ember.curl * weights[1] + violet.curl * weights[2],
            ribbon: porcelain.ribbon * weights[0]
                + ember.ribbon * weights[1]
                + violet.ribbon * weights[2],
            sculpt: porcelain.sculpt * weights[0]
                + ember.sculpt * weights[1]
                + violet.sculpt * weights[2],
            bands: porcelain.bands * weights[0]
                + ember.bands * weights[1]
                + violet.bands * weights[2],
            outline: porcelain.outline * weights[0]
                + ember.outline * weights[1]
                + violet.outline * weights[2],
            moon_size: porcelain.moon_size * weights[0]
                + ember.moon_size * weights[1]
                + violet.moon_size * weights[2],
            moon_glow: porcelain.moon_glow * weights[0]
                + ember.moon_glow * weights[1]
                + violet.moon_glow * weights[2],
            grain: porcelain.grain * weights[0]
                + ember.grain * weights[1]
                + violet.grain * weights[2],
        }
    }
}

impl PresetMode {
    fn to_art_direction(self, camera_forward: Vec3) -> ArtDirection {
        match self {
            Self::Static(preset) => preset.direction(),
            Self::MoonLook => ArtPreset::moon_look_direction(camera_forward),
        }
    }
}

struct PerfCycleMode {
    steps: u32,
    detail_tier: u32,
}

impl PerfCycleMode {
    const fn from_index(index: u32) -> Self {
        let (steps, tier) = PERF_MODES[(index as usize) % PERF_MODES.len()];
        Self {
            steps: steps as u32,
            detail_tier: tier as u32,
        }
    }
}

struct PerfOverlay {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    pixels: Vec<u32>,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    rect_buffer: wgpu::Buffer,
    frame_accum: f32,
    frame_count: u32,
    last_fps: u32,
}

impl PerfOverlay {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cloud Engine Perf overlay texture"),
            size: wgpu::Extent3d {
                width: PERF_OVERLAY_TEXTURE_WIDTH,
                height: PERF_OVERLAY_TEXTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Cloud Engine Perf overlay sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Cloud Engine Perf overlay BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
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
        let rect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cloud Engine Perf overlay rect"),
            size: std::mem::size_of::<[f32; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Cloud Engine Perf overlay BG"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rect_buffer.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Cloud Engine Perf overlay PL"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Cloud Engine Perf overlay WGSL"),
            source: wgpu::ShaderSource::Wgsl(PERF_OVERLAY_SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Cloud Engine Perf overlay pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let mut overlay = Self {
            _texture: texture,
            _view: view,
            pixels: vec![0; (PERF_OVERLAY_TEXTURE_WIDTH * PERF_OVERLAY_TEXTURE_HEIGHT) as usize],
            pipeline,
            bind_group,
            rect_buffer,
            frame_accum: 0.0,
            frame_count: 0,
            last_fps: 0,
        };
        overlay.update_text(queue, 0, 0);
        overlay
    }

    fn overlay_rect_ndc(width: u32, height: u32) -> [f32; 4] {
        let overlay_w = PERF_OVERLAY_TEXTURE_WIDTH as f32 * PERF_OVERLAY_SCALE;
        let overlay_h = PERF_OVERLAY_TEXTURE_HEIGHT as f32 * PERF_OVERLAY_SCALE;
        let width = width.max(1) as f32;
        let height = height.max(1) as f32;
        let left = width - PERF_OVERLAY_MARGIN - overlay_w;
        let right = width - PERF_OVERLAY_MARGIN;
        let top = height - PERF_OVERLAY_MARGIN - overlay_h;
        let bottom = height - PERF_OVERLAY_MARGIN;
        let left_ndc = left * 2.0 / width - 1.0;
        let right_ndc = right * 2.0 / width - 1.0;
        let top_ndc = -1.0 + 2.0 * top / height;
        let bottom_ndc = -1.0 + 2.0 * bottom / height;
        [left_ndc, top_ndc, right_ndc, bottom_ndc]
    }

    fn is_click_within(
        position: winit::dpi::PhysicalPosition<f64>,
        width: u32,
        height: u32,
    ) -> bool {
        let overlay_w = PERF_OVERLAY_TEXTURE_WIDTH as f64 * PERF_OVERLAY_SCALE as f64;
        let overlay_h = PERF_OVERLAY_TEXTURE_HEIGHT as f64 * PERF_OVERLAY_SCALE as f64;
        let left = f64::from(width).max(1.0) - PERF_OVERLAY_MARGIN as f64 - overlay_w;
        let right = f64::from(width).max(1.0) - PERF_OVERLAY_MARGIN as f64;
        let top = f64::from(height).max(1.0) - PERF_OVERLAY_MARGIN as f64 - overlay_h;
        let bottom = f64::from(height).max(1.0) - PERF_OVERLAY_MARGIN as f64;
        position.x >= left && position.x <= right && position.y >= top && position.y <= bottom
    }

    fn update_text(&mut self, queue: &wgpu::Queue, fps: u32, mode_index: u32) {
        let mode = PerfCycleMode::from_index(mode_index);
        let text = format!(
            "FPS {:>3}  steps {:>2}  tier {}",
            fps, mode.steps, mode.detail_tier
        );
        self.pixels.fill(0);
        stamp_text(
            &mut self.pixels,
            PERF_OVERLAY_TEXTURE_WIDTH as usize,
            PERF_OVERLAY_TEXTURE_HEIGHT as usize,
            4,
            ((PERF_OVERLAY_TEXTURE_HEIGHT as usize - FHEIGHT as usize) / 2) as i32,
            &text,
            u32::MAX,
        )
        .expect("Cloud engine perf overlay texture dimensions");
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&self.pixels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PERF_OVERLAY_TEXTURE_WIDTH * 4),
                rows_per_image: Some(PERF_OVERLAY_TEXTURE_HEIGHT),
            },
            wgpu::Extent3d {
                width: PERF_OVERLAY_TEXTURE_WIDTH,
                height: PERF_OVERLAY_TEXTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
        );
    }

    fn draw(
        &mut self,
        dt: f32,
        mode_index: u32,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        output_size: winit::dpi::PhysicalSize<u32>,
    ) {
        self.frame_accum += dt.max(0.0);
        self.frame_count = self.frame_count.saturating_add(1);
        if self.frame_accum >= 0.25 {
            self.last_fps = (self.frame_count as f32 / self.frame_accum).round() as u32;
            self.frame_accum = 0.0;
            self.frame_count = 0;
            self.update_text(queue, self.last_fps, mode_index);
        }
        let rect = Self::overlay_rect_ndc(output_size.width, output_size.height);
        queue.write_buffer(&self.rect_buffer, 0, bytemuck::cast_slice(&rect));
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Cloud Engine perf overlay"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

#[allow(dead_code)]
fn main() {
    run(false);
}

pub fn run(curve_painter: bool) {
    env_logger::init();
    let mut preset_override = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--preset" {
            if let Some(value) = args.next() {
                preset_override = ArtPreset::parse_arg(value.as_str());
            }
        }
    }

    let start_preset = preset_override
        .map(PresetMode::Static)
        .unwrap_or(PresetMode::MoonLook);

    EventLoop::new()
        .expect("event loop")
        .run_app(&mut App::new(start_preset, curve_painter))
        .expect("event loop error");
}

struct App {
    state: Option<State>,
    art_preset: Arc<Mutex<PresetMode>>,
    curve_painter: bool,
}

impl App {
    fn new(start_preset: PresetMode, curve_painter: bool) -> Self {
        Self {
            state: None,
            art_preset: Arc::new(Mutex::new(start_preset)),
            curve_painter,
        }
    }
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: wgpu::SurfaceConfiguration,
    compute_pipeline: wgpu::ComputePipeline,
    render_pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    sim_uniform: wgpu::Buffer,
    render_uniform: wgpu::Buffer,
    outline_uniform: wgpu::Buffer,
    outline_bind_group: wgpu::BindGroup,
    sim_groups: [wgpu::BindGroup; 2],
    render_groups: [wgpu::BindGroup; 2],
    volume_views: [wgpu::TextureView; 2],
    volume_sampler: wgpu::Sampler,
    scene_sampler: wgpu::Sampler,
    scene_color_texture: wgpu::Texture,
    scene_color_view: wgpu::TextureView,
    _luna_texture: wgpu::Texture,
    luna_view: wgpu::TextureView,
    _moon2_texture: wgpu::Texture,
    moon2_view: wgpu::TextureView,
    luna_sampler: wgpu::Sampler,
    scene_renderer: Renderer,
    outline_vertex_buffer: wgpu::Buffer,
    outline_vertex_count: u32,
    brush_vertex_buffer: wgpu::Buffer,
    brush_vertex_count: u32,
    current_volume: usize,
    camera: FlyCamera,
    input: WinitFlyInput,
    pointer_position: Option<winit::dpi::PhysicalPosition<f64>>,
    brush_center: Vec3,
    brush_active: bool,
    paint_buttons: u8,
    brush_sign: f32,
    brush_size: f32,
    brush_depth: f32,
    auto_brush: AutoBezierBrush,
    curve_painter: bool,
    art_preset: Arc<Mutex<PresetMode>>,
    perf_mode_index: u32,
    last_frame: Instant,
    simulation_accumulator: f32,
    time: f32,
    frame: u32,
    pending_clear: bool,
    perf_overlay: PerfOverlay,
    egui_ctx: egui::Context,
    egui_state: EguiState,
    egui_renderer: EguiRenderer,
    egui_wants_pointer: bool,
    // Kept alive (rather than dropped at the end of setup, its previous
    // behavior) specifically so there's a live World for the inspector
    // agent to re-snapshot every few frames; nothing else in this example
    // mutates it after setup today.
    scene_db: pulsar_scenedb::SceneDb,
    inspector_agent: Option<scenedb_inspector_agent::InlineAgent>,
}

/// A quiet autonomous counterpart to the mouse brush. It only takes over
/// while the user is not painting, and follows one continuous quadratic
/// Bézier path at a time rather than placing independent random stamps.
struct AutoBezierBrush {
    active: bool,
    pause_remaining: f32,
    elapsed: f32,
    duration: f32,
    start: Vec3,
    control: Vec3,
    end: Vec3,
    size: f32,
    random_state: u32,
}

impl AutoBezierBrush {
    fn new() -> Self {
        Self {
            active: false,
            pause_remaining: 0.2,
            elapsed: 0.0,
            duration: 0.7,
            start: Vec3::splat(0.5),
            control: Vec3::splat(0.5),
            end: Vec3::splat(0.5),
            size: 0.06,
            random_state: 0xC10D_C0DE,
        }
    }

    fn random01(&mut self) -> f32 {
        self.random_state ^= self.random_state << 13;
        self.random_state ^= self.random_state >> 17;
        self.random_state ^= self.random_state << 5;
        self.random_state as f32 / u32::MAX as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.random01()
    }

    fn begin_curve(&mut self) {
        self.start = Vec3::new(
            self.range(0.16, 0.84),
            self.range(0.32, 0.68),
            self.range(0.16, 0.84),
        );
        // The requested 10–66% of the cloud-box area becomes the horizontal
        // stroke length; keeping its endpoints inset prevents clipped arcs.
        let length = self.range(0.10, 0.66);
        let angle = self.range(0.0, std::f32::consts::TAU);
        let direction = Vec3::new(angle.cos(), 0.0, angle.sin());
        self.end = (self.start + direction * length).clamp(Vec3::splat(0.08), Vec3::splat(0.92));
        let midpoint = (self.start + self.end) * 0.5;
        let perpendicular = Vec3::new(-direction.z, 0.0, direction.x);
        self.control = (midpoint
            + perpendicular * self.range(-0.22, 0.22)
            + Vec3::Y * self.range(-0.18, 0.18))
        .clamp(Vec3::splat(0.08), Vec3::splat(0.92));
        self.duration = self.range(0.42, 1.10);
        self.size = self.range(0.038, 0.078);
        self.elapsed = 0.0;
        self.active = true;
    }

    fn update(&mut self, dt: f32) {
        if self.active {
            self.elapsed += dt;
            if self.elapsed >= self.duration {
                self.active = false;
                // Soft-RNG cadence requested for the gap before the next curve.
                self.pause_remaining = self.range(0.10, 0.50);
            }
        } else {
            self.pause_remaining -= dt;
            if self.pause_remaining <= 0.0 {
                self.begin_curve();
            }
        }
    }

    fn center(&self) -> Vec3 {
        let t = (self.elapsed / self.duration).clamp(0.0, 1.0);
        let one_minus_t = 1.0 - t;
        self.start * (one_minus_t * one_minus_t)
            + self.control * (2.0 * one_minus_t * t)
            + self.end * (t * t)
    }
}

impl State {
    fn create_scene_color_target(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cloud Engine regular scene color"),
            size: wgpu::Extent3d {
                width: config.width.max(1),
                height: config.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    fn create_rgba_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &'static str,
        bytes: &[u8],
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let image = image::load_from_memory(bytes)
            .expect("Cloud Engine moon PNG")
            .to_rgba8();
        let size = wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size,
        );
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    fn rebuild_render_groups(&mut self) {
        let layout = self.render_pipeline.get_bind_group_layout(0);
        self.render_groups = std::array::from_fn(|volume| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cloud render bindings"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.render_uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&self.volume_views[volume]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.volume_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&self.scene_color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.scene_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&self.luna_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&self.luna_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&self.moon2_view),
                    },
                ],
            })
        });
    }

    fn resize_scene_target(&mut self) {
        let (texture, view) = Self::create_scene_color_target(&self.device, &self.config);
        self.scene_color_texture = texture;
        self.scene_color_view = view;
        self.scene_renderer
            .set_render_size(self.config.width, self.config.height);
        self.rebuild_render_groups();
    }
}

/// The HelioV/Linux world marker: two close parallel lines per world axis make
/// the fixed X/Y/Z frame legible with the debug line renderer's fixed width.
fn add_world_axes(renderer: &mut Renderer) {
    renderer.debug_batch(|debug| {
        debug.line(
            [-AXIS_REACH, 0.0, 0.0],
            [AXIS_REACH, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.5],
        );
        debug.line(
            [-AXIS_REACH, 0.0, AXIS_COMPANION_OFFSET],
            [AXIS_REACH, 0.0, AXIS_COMPANION_OFFSET],
            [1.0, 0.0, 0.0, 0.5],
        );
        debug.line(
            [0.0, -AXIS_REACH, 0.0],
            [0.0, AXIS_REACH, 0.0],
            [0.0, 1.0, 0.0, 0.5],
        );
        debug.line(
            [AXIS_COMPANION_OFFSET, -AXIS_REACH, 0.0],
            [AXIS_COMPANION_OFFSET, AXIS_REACH, 0.0],
            [0.0, 1.0, 0.0, 0.5],
        );
        debug.line(
            [0.0, 0.0, -AXIS_REACH],
            [0.0, 0.0, AXIS_REACH],
            [0.0, 0.4, 1.0, 0.5],
        );
        debug.line(
            [AXIS_COMPANION_OFFSET, 0.0, -AXIS_REACH],
            [AXIS_COMPANION_OFFSET, 0.0, AXIS_REACH],
            [0.0, 0.4, 1.0, 0.5],
        );
    });
}

impl App {
    fn create_state(
        event_loop: &ActiveEventLoop,
        art_preset: Arc<Mutex<PresetMode>>,
        curve_painter: bool,
    ) -> State {
        eprintln!("[CE] create_state start");
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Helio — Cloud Engine")
                        .with_inner_size(winit::dpi::PhysicalSize::new(1600, 900)),
                )
                .expect("create Cloud Engine window"),
        );
        let instance = match std::env::var("HELIO_BACKEND").ok().as_deref() {
            Some("vulkan") => wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                flags: wgpu::InstanceFlags::empty(),
                backend_options: wgpu::BackendOptions::default(),
                display: None,
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            }),
            _ => wgpu::Instance::new(helio_core::wgpu_instance_descriptor()),
        };
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: true,
        }))
        .expect("GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: helio::required_wgpu_features(adapter.features()),
            required_limits: helio::required_wgpu_limits(adapter.limits()),
            experimental_features: helio::required_experimental_features(adapter.features()),
            ..Default::default()
        }))
        .expect("GPU device");
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        eprintln!("[CE] surface configured");

        // Render regular Helio content first. The cloud pass below samples this
        // target, so meshes and future voxel content retain Helio's normal
        // material, lighting, culling, and instancing path.
        let scene_config =
            RendererConfig::new(config.width, config.height, format).with_render_scale(1.0);
        let mut scene_db = new_scene_db_with_gpu_mirror(&device, &queue);
        // The background is composed in the cloud shader. In particular, do
        // not attach Helio's SkyActor here: its analytic sun disc would show
        // through the authored moon textures as an unwanted white rim.
        let graph_scene_db = scene_db_handle(&scene_db);
        let mut scene_renderer = RendererBuilder::new(scene_config.clone(), graph_scene_db.clone())
            .with_pass_build_context(Box::new(helio_mesh_host::presets::deferred))
            .build(
                device.clone(),
                queue.clone(),
                scene_config.width,
                scene_config.height,
                scene_config.surface_format,
            );
        eprintln!("[CE] default graph built");
        // The cloud shader supplies the sky; this target contains only the
        // regular mesh/debug scene to be composited over it.
        scene_renderer.set_clear_color([0.0, 0.0, 0.0, 0.0]);
        eprintln!("[CE] scene renderer configured");
        let cube_material = spawn_material(
            &mut scene_db.world,
            make_material([0.74, 0.80, 0.92, 1.0], 0.58, 0.0, [0.0, 0.0, 0.0], 0.0),
        );
        let cube_mesh = spawn_mesh(
            &mut scene_db.world,
            cube_mesh([0.0, 0.0, 0.0], CUBE_HALF_EXTENT),
        );
        let grid_half = (CUBE_GRID_SIZE - 1) as f32 * CUBE_SPACING * 0.5;
        for x in 0..CUBE_GRID_SIZE {
            for z in 0..CUBE_GRID_SIZE {
                let transform = glam::Mat4::from_translation(Vec3::new(
                    x as f32 * CUBE_SPACING - grid_half,
                    CUBE_HALF_EXTENT,
                    z as f32 * CUBE_SPACING - grid_half,
                ));
                spawn_object_with_movability(
                    &mut scene_db.world,
                    cube_mesh,
                    cube_material,
                    transform,
                    CUBE_BOUNDING_RADIUS,
                    Some(helio::Movability::Static),
                )
                .expect("Cloud Engine cube instance");
            }
        }
        add_world_axes(&mut scene_renderer);
        // Luna is a real directional scene light, independent of the cloud
        // shader's decorative moon, so it will also light later voxel content.
        spawn_light(
            &mut scene_db.world,
            directional_light([0.32, -0.76, 0.57], [0.67, 0.76, 1.0], 3.2),
        );
        let (scene_color_texture, scene_color_view) =
            State::create_scene_color_target(&device, &config);
        let scene_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Cloud Engine scene color sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Both source maps use authored RGBA silhouettes. Moon2's alpha was
        // repaired at asset build time to remove its baked checkerboard.
        let (luna_texture, luna_view) = State::create_rgba_texture(
            &device,
            &queue,
            "Cloud Engine moon1 RGBA",
            include_bytes!("assets/cloud-engine/luna-full-moon-mask.png"),
        );
        let (moon2_texture, moon2_view) = State::create_rgba_texture(
            &device,
            &queue,
            "Cloud Engine moon2 RGBA",
            include_bytes!("assets/cloud-engine/luna-earth-moon2-512.png"),
        );
        let luna_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Cloud Engine celestial sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let sim_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cloud simulation parameters"),
            size: (28 * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let render_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cloud render parameters"),
            size: (68 * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sim_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Cloud simulation WGSL"),
            source: wgpu::ShaderSource::Wgsl(SIM_SHADER.into()),
        });
        let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Cloud raymarch WGSL"),
            source: wgpu::ShaderSource::Wgsl(RENDER_SHADER.into()),
        });
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Cloud density simulation"),
            layout: None,
            module: &sim_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Cloud raymarch"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &render_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &render_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Cloudbox outline WGSL"),
            source: wgpu::ShaderSource::Wgsl(OUTLINE_SHADER.into()),
        });
        let outline_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Cloudbox outline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let outline_vertex_data: Vec<f32> = [
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MAX.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MIN.z,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MAX.z,
        ]
        .to_vec();

        let outline_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cloudbox outline vertices"),
            size: (outline_vertex_data.len() * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let outline_vertex_count = (outline_vertex_data.len() / 3) as u32;

        let initial_brush_world =
            BOUNDS_SIM_MIN + Vec3::new(0.5, 0.45, 0.64) * (BOUNDS_SIM_MAX - BOUNDS_SIM_MIN);
        let initial_brush_data = brush_sphere_vertices(
            initial_brush_world,
            (BOUNDS_SIM_MAX - BOUNDS_SIM_MIN) * BRUSH_SIZE_MAX,
        );
        let brush_vertex_count = (initial_brush_data.len() / 3) as u32;
        let brush_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cloud brush sphere vertices"),
            size: (initial_brush_data.len() * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let outline_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Cloudbox outline uniforms"),
            size: (24 * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let outline_bind_group = {
            let layout = outline_pipeline.get_bind_group_layout(0);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cloudbox outline bindings"),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: outline_uniform.as_entire_binding(),
                }],
            })
        };
        queue.write_buffer(
            &outline_vertex_buffer,
            0,
            bytemuck::cast_slice(&outline_vertex_data),
        );
        queue.write_buffer(
            &brush_vertex_buffer,
            0,
            bytemuck::cast_slice(&initial_brush_data),
        );
        let make_volume = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: VOLUME.0,
                    height: VOLUME.1,
                    depth_or_array_layers: VOLUME.2,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            })
        };
        let volumes = [make_volume("Cloud volume A"), make_volume("Cloud volume B")];
        let views = [
            volumes[0].create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            }),
            volumes[1].create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            }),
        ];
        let volume_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Cloud trilinear sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sim_layout = compute_pipeline.get_bind_group_layout(0);
        let sim_groups = std::array::from_fn(|source| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cloud simulation bindings"),
                layout: &sim_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: sim_uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[source]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&volume_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&views[1 - source]),
                    },
                ],
            })
        });
        let render_layout = render_pipeline.get_bind_group_layout(0);
        let render_groups = std::array::from_fn(|volume| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cloud render bindings"),
                layout: &render_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: render_uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[volume]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&volume_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&scene_color_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&scene_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&luna_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::Sampler(&luna_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&moon2_view),
                    },
                ],
            })
        });
        let perf_overlay = PerfOverlay::new(&device, &queue, config.format);
        let egui_ctx = egui::Context::default();
        let egui_state = EguiState::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let egui_renderer =
            EguiRenderer::new(&device, config.format, EguiRendererOptions::default());
        State {
            window,
            surface,
            device: device.clone(),
            queue: queue.clone(),
            config,
            compute_pipeline,
            render_pipeline,
            sim_uniform,
            render_uniform,
            outline_uniform,
            outline_bind_group,
            sim_groups,
            render_groups,
            volume_views: views,
            volume_sampler,
            scene_sampler,
            scene_color_texture,
            scene_color_view,
            _luna_texture: luna_texture,
            luna_view,
            _moon2_texture: moon2_texture,
            moon2_view,
            luna_sampler,
            scene_renderer,
            outline_pipeline,
            outline_vertex_buffer,
            outline_vertex_count,
            brush_vertex_buffer,
            brush_vertex_count,
            current_volume: 0,
            // The WebGPU source used +Z as forward. Helio's standard flycam
            // uses -Z, so rotate this imported pose by π to preserve its view.
            camera: FlyCamera::new(
                PRESET_POSITION,
                PRESET_YAW + std::f32::consts::PI,
                PRESET_PITCH,
                FlyCameraConfig::default(),
            ),
            input: WinitFlyInput::new(),
            pointer_position: None,
            brush_center: Vec3::new(0.5, 0.45, 0.64),
            brush_active: false,
            paint_buttons: 0,
            brush_sign: 1.0,
            brush_size: BRUSH_SIZE_MAX,
            brush_depth: 0.64,
            auto_brush: AutoBezierBrush::new(),
            curve_painter,
            art_preset,
            perf_mode_index: 0,
            last_frame: Instant::now(),
            simulation_accumulator: 0.0,
            time: 0.0,
            frame: 0,
            pending_clear: true,
            perf_overlay,
            egui_ctx,
            egui_state,
            egui_renderer,
            egui_wants_pointer: false,
            inspector_agent: scenedb_inspector_agent::InlineAgent::maybe_start(),
            scene_db,
        }
    }
}

impl State {
    fn prepare_egui(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> (Vec<egui::ClippedPrimitive>, ScreenDescriptor) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        self.egui_ctx.begin_pass(raw_input);
        egui::Window::new("Cloud Engine Tools")
            .default_pos([12.0, 12.0])
            .default_size([280.0, 420.0])
            .resizable(true)
            .show(&self.egui_ctx, |ui| {
                ui.heading("Cloud Engine");
                ui.label("Paint directly in the 3D volume");
                ui.separator();
                ui.label("Brush");
                ui.add(
                    egui::Slider::new(&mut self.brush_size, BRUSH_SIZE_MIN..=BRUSH_SIZE_MAX)
                        .text("radius"),
                );
                ui.add(egui::Slider::new(&mut self.brush_depth, 0.0..=1.0).text("depth"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.brush_sign, 1.0, "Add");
                    ui.selectable_value(&mut self.brush_sign, -1.0, "Erase");
                });
                ui.checkbox(&mut self.curve_painter, "Autonomous curve painter");
                if ui.button("Clear volume").clicked() {
                    self.pending_clear = true;
                }
                ui.separator();
                ui.label("Mouse: move to position the 3D brush");
                ui.label("Left drag adds density; right drag erases");
                ui.label("F toggles fly camera; Escape releases it");
                ui.separator();
                ui.label(format!(
                    "Brush voxel: {:.3}, {:.3}, {:.3}",
                    self.brush_center.x, self.brush_center.y, self.brush_center.z
                ));
                ui.label(format!(
                    "Simulation: {} Hz",
                    (1.0 / SIMULATION_INTERVAL) as u32
                ));
            });
        let mut full_output = self.egui_ctx.end_pass();
        self.egui_wants_pointer = self.egui_ctx.egui_wants_pointer_input();
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);
        for (id, deltas) in &full_output.textures_delta.set {
            for delta in deltas {
                self.egui_renderer
                    .update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }
        let pixels_per_point = self.window.scale_factor() as f32;
        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point,
        };
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, pixels_per_point);
        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        // The renderer has consumed all texture deltas; clear the container
        // before FullOutput is dropped (egui asserts otherwise).
        full_output.textures_delta.clear();
        (paint_jobs, screen_descriptor)
    }

    fn camera_basis(&self) -> (Vec3, Vec3, Vec3) {
        let basis = self.camera.basis();
        (basis.forward, basis.right, basis.right.cross(basis.forward))
    }

    fn update(&mut self) -> (bool, f32) {
        let dt = self.last_frame.elapsed().as_secs_f32();
        self.last_frame = Instant::now();
        self.time += dt;
        self.simulation_accumulator += dt;
        self.camera.update(self.input.take_input(), dt);
        if self.curve_painter && !self.brush_active {
            self.auto_brush.update(dt);
        }
        let auto_painting = self.curve_painter && !self.brush_active && self.auto_brush.active;
        if self.brush_active {
            self.brush_size = (self.brush_size - BRUSH_DRAIN_PER_SECOND * dt).max(BRUSH_SIZE_MIN);
        } else {
            self.brush_size =
                (self.brush_size + BRUSH_RECHARGE_PER_SECOND * dt).min(BRUSH_SIZE_MAX);
        }
        let painting = self.brush_active || self.paint_buttons != 0;
        let run_simulation = self.pending_clear
            || painting
            || auto_painting
            || self.simulation_accumulator >= SIMULATION_INTERVAL;
        // Whether a paint stroke forces an update every display frame or the
        // idle simulation waits for its 30 Hz cadence, advance by the elapsed
        // simulation time since the last dispatch. Using only this frame's dt
        // in the idle path made the clouds appear slower whenever not painting.
        let simulation_delta = if run_simulation {
            self.simulation_accumulator.min(1.0 / 15.0)
        } else {
            0.0
        };
        if run_simulation {
            self.simulation_accumulator = 0.0;
        }
        self.write_uniforms(simulation_delta, self.pending_clear);
        self.pending_clear = false;
        self.frame = self.frame.wrapping_add(1);
        (run_simulation, dt)
    }

    fn write_uniforms(&self, delta: f32, clear: bool) {
        let perf_mode = PerfCycleMode::from_index(self.perf_mode_index);
        let art_mode = self.art_preset.lock().expect("art preset lock");
        let art = art_mode.to_art_direction(self.camera_basis().0);
        let wind_cycle =
            (self.time * WIND_SPEED_RADIANS_PER_SECOND / std::f32::consts::TAU).rem_euclid(1.0);
        let wind_angle = WIND_INITIAL_ANGLE_RADIANS + wind_cycle * std::f32::consts::TAU;
        let wind = Vec3::new(
            wind_angle.sin() * WIND_STRENGTH,
            0.0,
            wind_angle.cos() * WIND_STRENGTH,
        );
        let painting = self.brush_active || self.paint_buttons != 0;
        let (brush_center, brush_size, brush_active, brush_sign) = if painting {
            (self.brush_center, self.brush_size, true, self.brush_sign)
        } else if self.auto_brush.active {
            (self.auto_brush.center(), self.auto_brush.size, true, 1.0)
        } else {
            (self.brush_center, self.brush_size, false, self.brush_sign)
        };
        let sim: [f32; 28] = [
            delta,
            self.time,
            0.58,
            0.0,
            wind.x,
            wind.y,
            wind.z,
            0.84,
            brush_center.x,
            brush_center.y,
            brush_center.z,
            brush_size,
            0.85,
            brush_active as u8 as f32,
            brush_sign,
            2.0,
            0.15,
            1.41,
            -0.44,
            clear as u8 as f32,
            VOLUME.0 as f32,
            VOLUME.1 as f32,
            VOLUME.2 as f32,
            PRESET_SEED,
            1.0,
            art.curl,
            art.ribbon,
            art.sculpt,
        ];
        self.queue
            .write_buffer(&self.sim_uniform, 0, bytemuck::cast_slice(&sim));
        let (forward, right, up) = self.camera_basis();
        let cycle = (self.time / DAYLIGHT_CYCLE_SECONDS).fract();
        let daylight = if cycle <= 0.5 {
            cycle * 2.0
        } else {
            (1.0 - cycle) * 2.0
        };
        let sun_elevation = (SUN_ELEVATION_MIN_DEGREES
            + (SUN_ELEVATION_MAX_DEGREES - SUN_ELEVATION_MIN_DEGREES) * daylight)
            .to_radians();
        let exposure = EXPOSURE_MIN + (EXPOSURE_MAX - EXPOSURE_MIN) * daylight;
        let sun_azimuth = SUN_AZIMUTH_DEGREES.to_radians();
        let ce = sun_elevation.cos();
        let sun = Vec3::new(
            sun_azimuth.sin() * ce,
            sun_elevation.sin(),
            sun_azimuth.cos() * ce,
        )
        .normalize();
        let render: [f32; 68] = [
            self.config.width as f32,
            self.config.height as f32,
            self.time,
            self.frame as f32,
            self.camera.position().x,
            self.camera.position().y,
            self.camera.position().z,
            (PRESET_FOV_DEGREES.to_radians() * 0.5).tan(),
            forward.x,
            forward.y,
            forward.z,
            exposure,
            right.x,
            right.y,
            right.z,
            perf_mode.steps as f32,
            up.x,
            up.y,
            up.z,
            0.96,
            sun.x,
            sun.y,
            sun.z,
            1.31,
            1.0,
            0.871,
            0.651,
            1.45,
            0.07,
            0.16,
            0.42,
            0.27,
            0.74,
            0.57,
            0.43,
            PRESET_SEED,
            BOUNDS_RENDER_MIN.x,
            BOUNDS_RENDER_MIN.y,
            BOUNDS_RENDER_MIN.z,
            1.32,
            BOUNDS_RENDER_MAX.x,
            BOUNDS_RENDER_MAX.y,
            BOUNDS_RENDER_MAX.z,
            1.48,
            0.58,
            perf_mode.detail_tier as f32,
            0.0,
            0.0,
            1.0,
            art.bands,
            art.outline,
            art.sculpt,
            art.cloud.x,
            art.cloud.y,
            art.cloud.z,
            art.grain,
            art.shadow.x,
            art.shadow.y,
            art.shadow.z,
            art.ribbon,
            art.sky.x,
            art.sky.y,
            art.sky.z,
            art.moon_size,
            art.moon.x,
            art.moon.y,
            art.moon.z,
            art.moon_glow,
        ];
        self.queue
            .write_buffer(&self.render_uniform, 0, bytemuck::cast_slice(&render));

        let (forward, right, up) = self.camera_basis();
        let aspect = self.config.width.max(1) as f32 / self.config.height.max(1) as f32;
        let tan_half_fov = (PRESET_FOV_DEGREES.to_radians() * 0.5).tan();
        let outline: [f32; 24] = [
            self.camera.position().x,
            self.camera.position().y,
            self.camera.position().z,
            0.0,
            right.x,
            right.y,
            right.z,
            0.0,
            up.x,
            up.y,
            up.z,
            0.0,
            forward.x,
            forward.y,
            forward.z,
            0.0,
            1.0 / tan_half_fov,
            1.0 / aspect,
            CLOUDBOX_OUTLINE_EPS,
            0.0,
            CLOUDBOX_OUTLINE_COLOR[0],
            CLOUDBOX_OUTLINE_COLOR[1],
            CLOUDBOX_OUTLINE_COLOR[2],
            CLOUDBOX_OUTLINE_COLOR[3],
        ];
        self.queue
            .write_buffer(&self.outline_uniform, 0, bytemuck::cast_slice(&outline));
        let brush_world = BOUNDS_SIM_MIN + self.brush_center * (BOUNDS_SIM_MAX - BOUNDS_SIM_MIN);
        let brush_vertices = brush_sphere_vertices(
            brush_world,
            (BOUNDS_SIM_MAX - BOUNDS_SIM_MIN) * self.brush_size,
        );
        self.queue.write_buffer(
            &self.brush_vertex_buffer,
            0,
            bytemuck::cast_slice(&brush_vertices),
        );
    }

    /// Converts a viewport cursor coordinate into the editable volume's
    /// normalized brush center, matching the original browser ray/AABB mapping.
    fn update_brush_from_cursor(&mut self, position: winit::dpi::PhysicalPosition<f64>) -> bool {
        let width = self.config.width.max(1) as f32;
        let height = self.config.height.max(1) as f32;
        let x = (position.x as f32 / width) * 2.0 - 1.0;
        let y = 1.0 - (position.y as f32 / height) * 2.0;
        let (forward, right, up) = self.camera_basis();
        let ray = (forward
            + right * x * (width / height) * (PRESET_FOV_DEGREES.to_radians() * 0.5).tan()
            + up * y * (PRESET_FOV_DEGREES.to_radians() * 0.5).tan())
        .normalize();
        let origin = self.camera.position();
        let inverse = ray.recip();
        let t0 = (BOUNDS_RENDER_MIN - origin) * inverse;
        let t1 = (BOUNDS_RENDER_MAX - origin) * inverse;
        let near = t0.min(t1).max_element();
        let far = t0.max(t1).min_element();
        if near > far || far < 0.0 {
            return false;
        }
        // Position the brush inside the actual finite volume. Depth is
        // adjustable in the panel, which makes the spherical cursor useful
        // for painting the front, middle, or back of a cloud.
        let near = near.max(0.0);
        let world = origin + ray * (near + (far - near) * self.brush_depth);
        self.brush_center = ((world - BOUNDS_SIM_MIN) / (BOUNDS_SIM_MAX - BOUNDS_SIM_MIN))
            .clamp(Vec3::splat(0.002), Vec3::splat(0.998));
        true
    }

    fn render(&mut self) {
        // Cheap enough to call every frame, but the inspector UI doesn't
        // need it that often -- throttle to ~10 Hz at 60 fps.
        if let Some(agent) = &mut self.inspector_agent {
            if self.frame % 6 == 0 {
                agent.publish(&self.scene_db.world.telemetry_snapshot());
            }
        }
        // Uploads every row queued since last frame into the GPU-mirrored
        // buffers the renderer actually reads. Without this, CPU-side
        // SceneDB writes are authoritative but invisible to the GPU
        // forever -- a fully black render despite correct scene data. Every
        // v3_demo_common-based example needs this call once per frame; see
        // World::flush_gpu_mirror's own doc.
        v3_demo_common::flush_scene_db(&self.scene_db, &self.queue);

        let (run_simulation, dt) = self.update();
        let (forward, _, up) = self.camera_basis();
        let scene_camera = Camera::perspective_look_at(
            self.camera.position(),
            self.camera.position() + forward,
            up,
            PRESET_FOV_DEGREES.to_radians(),
            self.config.width.max(1) as f32 / self.config.height.max(1) as f32,
            0.1,
            1_000.0,
        );
        if let Err(error) = self
            .scene_renderer
            .render(&scene_camera, &self.scene_color_view)
        {
            log::error!("Cloud Engine scene render failed: {error:?}");
        }
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Cloud frame"),
            });
        let (egui_paint_jobs, egui_screen) = self.prepare_egui(&mut encoder);
        let mut render_volume = self.current_volume;
        let view = output.texture.create_view(&Default::default());
        if run_simulation {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Cloud simulation"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compute_pipeline);
            pass.set_bind_group(0, &self.sim_groups[self.current_volume], &[]);
            pass.dispatch_workgroups(
                VOLUME.0.div_ceil(4),
                VOLUME.1.div_ceil(4),
                VOLUME.2.div_ceil(4),
            );
            render_volume = 1 - self.current_volume;
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Cloud raymarch"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.render_pipeline);
            pass.set_bind_group(0, &self.render_groups[render_volume], &[]);
            pass.draw(0..3, 0..1);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Cloudbox outline"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.outline_pipeline);
            pass.set_bind_group(0, &self.outline_bind_group, &[]);
            pass.set_vertex_buffer(0, self.outline_vertex_buffer.slice(..));
            pass.draw(0..self.outline_vertex_count, 0..1);
            if self.pointer_position.is_some() && !self.egui_wants_pointer {
                pass.set_vertex_buffer(0, self.brush_vertex_buffer.slice(..));
                pass.draw(0..self.brush_vertex_count, 0..1);
            }
        }

        self.perf_overlay.draw(
            dt,
            self.perf_mode_index,
            &mut encoder,
            &self.queue,
            &view,
            winit::dpi::PhysicalSize::new(self.config.width, self.config.height),
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Cloud Engine egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.egui_renderer
                .render(&mut pass, &egui_paint_jobs, &egui_screen);
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(output);
        self.current_volume = render_volume;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            self.state = Some(Self::create_state(
                event_loop,
                self.art_preset.clone(),
                self.curve_painter,
            ));
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let egui_response = state.egui_state.on_window_event(&state.window, &event);
        if egui_response.consumed {
            if let WindowEvent::MouseInput {
                state: ElementState::Released,
                button,
                ..
            } = &event
            {
                match button {
                    MouseButton::Left => state.paint_buttons &= !1,
                    MouseButton::Right => state.paint_buttons &= !2,
                    _ => {}
                }
            }
            state.brush_active = false;
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => state.render(),
            WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                state.config.width = size.width;
                state.config.height = size.height;
                state.surface.configure(&state.device, &state.config);
                state.resize_scene_target();
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if !state.input.cursor_grabbed() {
                    if state.pointer_position.is_some_and(|position| {
                        PerfOverlay::is_click_within(
                            position,
                            state.config.width,
                            state.config.height,
                        )
                    }) {
                        state.perf_mode_index =
                            (state.perf_mode_index + 1) % PERF_MODES.len() as u32;
                        state.brush_active = false;
                        state.perf_overlay.update_text(
                            &state.queue,
                            state.perf_overlay.last_fps,
                            state.perf_mode_index,
                        );
                    } else {
                        state.paint_buttons |= 1;
                        state.brush_sign = 1.0;
                        state.brush_active = state
                            .pointer_position
                            .is_some_and(|position| state.update_brush_from_cursor(position));
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } if !state.input.cursor_grabbed() => {
                state.paint_buttons |= 2;
                state.brush_sign = -1.0;
                state.brush_active = state
                    .pointer_position
                    .is_some_and(|position| state.update_brush_from_cursor(position));
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } if !state.input.cursor_grabbed() => {
                state.paint_buttons &= !1;
                state.brush_active = false;
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Right,
                ..
            } if !state.input.cursor_grabbed() => {
                state.paint_buttons &= !2;
                state.brush_active = false;
            }
            WindowEvent::CursorMoved { position, .. } if !state.input.cursor_grabbed() => {
                state.pointer_position = Some(position);
                let valid = state.update_brush_from_cursor(position);
                state.brush_active = valid && state.paint_buttons != 0;
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(key),
                        state: key_state,
                        repeat,
                        ..
                    },
                ..
            } => {
                if key_state == ElementState::Pressed && !repeat && key == KeyCode::KeyF {
                    state.brush_active = false;
                    if state.input.cursor_grabbed() {
                        state.input.release_cursor(&state.window);
                    } else {
                        state.input.grab_cursor(&state.window);
                    }
                } else if key_state == ElementState::Pressed && !repeat && key == KeyCode::Escape {
                    if !state.input.release_cursor(&state.window) {
                        event_loop.exit();
                    }
                } else if state.input.cursor_grabbed() {
                    state.input.set_key(key, key_state == ElementState::Pressed);
                }
            }
            WindowEvent::Focused(focused) => {
                state.input.set_window_focused(&state.window, focused);
                if !focused {
                    state.brush_active = false;
                }
            }
            _ => {}
        }
    }
    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        if let (Some(state), DeviceEvent::MouseMotion { delta }) = (self.state.as_mut(), event) {
            state.input.add_mouse_motion(delta.0, delta.1);
        }
    }
    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}
