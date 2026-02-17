//! Helio Skies: Volumetric Atmosphere Rendering Feature
//!
//! Provides modular volumetric atmosphere rendering including:
//! - Sky dome with Rayleigh scattering
//! - Atmospheric scattering (blue sky, sun halos)
//! - Volumetric clouds (3D noise-based raymarch)
//! - Volumetric fog (height/distance-based)
//! - Configurable quality levels

use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use glam::{Vec3, Mat4};
use std::sync::Arc;
use std::ptr;

// ─── Configuration Types ─────────────────────────────────────────────────────

/// Quality preset for atmosphere rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityLevel {
    Low,     // 16 cloud samples, 4 scatter samples
    Medium,  // 32 cloud samples, 8 scatter samples
    High,    // 64 cloud samples, 16 scatter samples
    Ultra,   // 128 cloud samples, 32 scatter samples
}

impl QualityLevel {
    pub fn cloud_samples(&self) -> u32 {
        match self {
            QualityLevel::Low => 16,
            QualityLevel::Medium => 32,
            QualityLevel::High => 64,
            QualityLevel::Ultra => 128,
        }
    }
    
    pub fn scatter_samples(&self) -> u32 {
        match self {
            QualityLevel::Low => 4,
            QualityLevel::Medium => 8,
            QualityLevel::High => 16,
            QualityLevel::Ultra => 32,
        }
    }
}

impl Default for QualityLevel {
    fn default() -> Self {
        QualityLevel::Medium
    }
}

/// Component flags for enabling/disabling features
#[derive(Debug, Clone, Copy)]
pub struct ComponentFlags {
    pub sky: bool,
    pub atmosphere: bool,
    pub clouds: bool,
    pub fog: bool,
}

impl Default for ComponentFlags {
    fn default() -> Self {
        Self {
            sky: true,
            atmosphere: true,
            clouds: false,
            fog: false,
        }
    }
}

/// Atmospheric scattering parameters (physically-based)
#[derive(Debug, Clone, Copy)]
pub struct AtmosphereParameters {
    /// Planet radius in km (Earth = 6371.0)
    pub planet_radius: f32,
    /// Atmosphere thickness in km (Earth = 60.0)
    pub atmosphere_thickness: f32,
    /// Rayleigh scattering coefficient (RGB wavelength-dependent)
    pub rayleigh_coefficient: [f32; 3],
    /// Mie scattering coefficient (mostly white/yellow)
    pub mie_coefficient: f32,
    /// Rayleigh scale height in km
    pub rayleigh_scale_height: f32,
    /// Mie scale height in km
    pub mie_scale_height: f32,
    /// Sun intensity multiplier
    pub sun_intensity: f32,
}

impl Default for AtmosphereParameters {
    fn default() -> Self {
        Self {
            planet_radius: 6371.0,
            atmosphere_thickness: 60.0,
            // Blue sky wavelength scattering (λ^-4 for 650nm, 510nm, 475nm)
            rayleigh_coefficient: [5.8e-6, 13.5e-6, 33.1e-6],
            mie_coefficient: 21e-6,
            rayleigh_scale_height: 8.0,
            mie_scale_height: 1.2,
            sun_intensity: 20.0,
        }
    }
}

/// Cloud parameters
#[derive(Debug, Clone, Copy)]
pub struct CloudParameters {
    /// Cloud layer base altitude (km)
    pub base_altitude: f32,
    /// Cloud layer thickness (km)
    pub thickness: f32,
    /// Cloud coverage (0.0 = none, 1.0 = full)
    pub coverage: f32,
    /// Cloud density multiplier
    pub density: f32,
    /// Wind offset for animation
    pub wind_offset: [f32; 2],
}

impl Default for CloudParameters {
    fn default() -> Self {
        Self {
            base_altitude: 1.5,
            thickness: 2.0,
            coverage: 0.5,
            density: 1.0,
            wind_offset: [0.0, 0.0],
        }
    }
}

/// Fog parameters
#[derive(Debug, Clone, Copy)]
pub struct FogParameters {
    /// Fog color (RGB)
    pub color: [f32; 3],
    /// Fog density at sea level
    pub density: f32,
    /// Height falloff rate (exponential)
    pub height_falloff: f32,
    /// Maximum fog distance
    pub max_distance: f32,
}

impl Default for FogParameters {
    fn default() -> Self {
        Self {
            color: [0.7, 0.8, 0.9],
            density: 0.001,
            height_falloff: 0.2,
            max_distance: 1000.0,
        }
    }
}

/// Complete Helio Skies configuration
#[derive(Debug, Clone)]
pub struct HelioSkiesConfig {
    pub quality: QualityLevel,
    pub components: ComponentFlags,
    pub atmosphere: AtmosphereParameters,
    pub clouds: CloudParameters,
    pub fog: FogParameters,
    /// Sun direction (normalized)
    pub sun_direction: Vec3,
}

impl Default for HelioSkiesConfig {
    fn default() -> Self {
        Self {
            quality: QualityLevel::default(),
            components: ComponentFlags::default(),
            atmosphere: AtmosphereParameters::default(),
            clouds: CloudParameters::default(),
            fog: FogParameters::default(),
            sun_direction: Vec3::new(0.3, 0.7, 0.5).normalize(),
        }
    }
}

// ─── Shader Uniforms ─────────────────────────────────────────────────────────

/// Sky dome vertex data
#[derive(blade_macros::Vertex, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct SkyVertex {
    position: [f32; 3],
}

/// Camera uniforms for sky rendering
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SkyCameraUniforms {
    view_proj: [[f32; 4]; 4],
    camera_position: [f32; 3],
    _pad1: f32,
    sun_direction: [f32; 3],
    _pad2: f32,
}

#[derive(blade_macros::ShaderData)]
struct SkyCameraData {
    camera: SkyCameraUniforms,
}

/// Atmosphere scattering uniforms
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AtmosphereUniforms {
    planet_radius: f32,
    atmosphere_radius: f32,
    rayleigh_coefficient: [f32; 3],
    _pad1: f32,
    mie_coefficient: f32,
    rayleigh_scale_height: f32,
    mie_scale_height: f32,
    sun_intensity: f32,
    scatter_samples: u32,
    _pad2: [u32; 3],
}

#[derive(blade_macros::ShaderData)]
struct AtmosphereData {
    atmosphere: AtmosphereUniforms,
}

// ─── Main Renderer ───────────────────────────────────────────────────────────

pub struct HelioSkiesRenderer {
    enabled: bool,
    config: HelioSkiesConfig,
    
    // Sky dome resources
    sky_pipeline: Option<gpu::RenderPipeline>,
    sky_vertex_buffer: Option<gpu::Buffer>,
    sky_index_buffer: Option<gpu::Buffer>,
    sky_index_count: u32,
}

impl HelioSkiesRenderer {
    pub fn new(config: HelioSkiesConfig) -> Self {
        tracing::info!("[HELIO SKIES] Creating atmosphere renderer");
        Self {
            enabled: true,
            config,
            sky_pipeline: None,
            sky_vertex_buffer: None,
            sky_index_buffer: None,
            sky_index_count: 0,
        }
    }
    
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    
    pub fn get_config(&self) -> &HelioSkiesConfig {
        &self.config
    }
    
    pub fn set_config(&mut self, config: HelioSkiesConfig) {
        self.config = config;
    }
    
    /// Initialize GPU resources
    pub fn init(&mut self, context: &Arc<gpu::Context>, color_format: gpu::TextureFormat, depth_format: gpu::TextureFormat) {
        tracing::info!("[HELIO SKIES] Initializing atmosphere renderer");
        
        if self.config.components.sky {
            self.init_sky_dome(context, color_format, depth_format);
        }
        
        tracing::info!("[HELIO SKIES] ✅ Initialized");
    }
    
    /// Initialize sky dome rendering
    fn init_sky_dome(&mut self, context: &Arc<gpu::Context>, color_format: gpu::TextureFormat, depth_format: gpu::TextureFormat) {
        tracing::info!("[HELIO SKIES] Creating sky dome");
        
        // Create simple sky dome mesh (inverted icosphere)
        let (vertices, indices) = Self::create_sky_dome_mesh();
        self.sky_index_count = indices.len() as u32;
        
        tracing::info!("[HELIO SKIES] Sky mesh: {} vertices, {} indices", vertices.len(), indices.len());
        
        // Create vertex buffer
        let vbuf = context.create_buffer(gpu::BufferDesc {
            name: "helio_skies_sky_vertices",
            size: (vertices.len() * std::mem::size_of::<SkyVertex>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(vertices.as_ptr(), vbuf.data() as *mut SkyVertex, vertices.len());
        }
        context.sync_buffer(vbuf);
        
        // Create index buffer
        let ibuf = context.create_buffer(gpu::BufferDesc {
            name: "helio_skies_sky_indices",
            size: (indices.len() * std::mem::size_of::<u32>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(indices.as_ptr(), ibuf.data() as *mut u32, indices.len());
        }
        context.sync_buffer(ibuf);
        
        self.sky_vertex_buffer = Some(vbuf);
        self.sky_index_buffer = Some(ibuf);
        
        // Create sky pipeline
        let camera_layout = <SkyCameraData as gpu::ShaderData>::layout();
        
        let shader_source = include_str!("shaders/skies_sky.wgsl");
        let shader = context.create_shader(gpu::ShaderDesc { source: shader_source });
        
        let pipeline = context.create_render_pipeline(gpu::RenderPipelineDesc {
            name: "helio_skies_sky",
            data_layouts: &[&camera_layout],
            vertex: shader.at("vs_main"),
            vertex_fetches: &[gpu::VertexFetchState {
                layout: &<SkyVertex as gpu::Vertex>::layout(),
                instanced: false,
            }],
            primitive: gpu::PrimitiveState {
                topology: gpu::PrimitiveTopology::TriangleList,
                front_face: gpu::FrontFace::Ccw,
                cull_mode: None, // TEST: Disable culling to see both sides
                ..Default::default()
            },
            depth_stencil: Some(gpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: false, // Sky is background
                depth_compare: gpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            fragment: Some(shader.at("fs_main")),
            color_targets: &[gpu::ColorTargetState {
                format: color_format,
                blend: None, // Sky replaces background
                write_mask: gpu::ColorWrites::default(),
            }],
            multisample_state: gpu::MultisampleState::default(),
        });
        
        self.sky_pipeline = Some(pipeline);
        tracing::info!("[HELIO SKIES] ✅ Sky dome created");
    }
    
    /// Create sky dome mesh (simple subdivided cube mapped to sphere)
    fn create_sky_dome_mesh() -> (Vec<SkyVertex>, Vec<u32>) {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        
        // Create a simple icosphere (subdivided icosahedron)
        let t = (1.0 + 5.0_f32.sqrt()) / 2.0;
        let scale = 1000.0; // Large dome
        
        // Initial icosahedron vertices
        let base_vertices = [
            Vec3::new(-1.0, t, 0.0),
            Vec3::new(1.0, t, 0.0),
            Vec3::new(-1.0, -t, 0.0),
            Vec3::new(1.0, -t, 0.0),
            Vec3::new(0.0, -1.0, t),
            Vec3::new(0.0, 1.0, t),
            Vec3::new(0.0, -1.0, -t),
            Vec3::new(0.0, 1.0, -t),
            Vec3::new(t, 0.0, -1.0),
            Vec3::new(t, 0.0, 1.0),
            Vec3::new(-t, 0.0, -1.0),
            Vec3::new(-t, 0.0, 1.0),
        ];
        
        // Normalize and scale vertices
        for v in &base_vertices {
            let normalized = v.normalize() * scale;
            vertices.push(SkyVertex {
                position: normalized.to_array(),
            });
        }
        
        // Icosahedron faces (20 triangles)
        let faces = [
            // 5 faces around point 0
            [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
            // 5 adjacent faces
            [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
            // 5 faces around point 3
            [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
            // 5 adjacent faces
            [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
        ];
        
        for face in &faces {
            indices.push(face[0]);
            indices.push(face[1]);
            indices.push(face[2]);
        }
        
        (vertices, indices)
    }
    
    /// Render atmosphere effects
    pub fn render(
        &self,
        encoder: &mut gpu::CommandEncoder,
        target_view: gpu::TextureView,
        depth_view: gpu::TextureView,
        view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
    ) {
        if !self.enabled {
            tracing::trace!("[HELIO SKIES] Skipped - disabled");
            return;
        }
        
        tracing::trace!("[HELIO SKIES] Rendering atmosphere (sky={}, atm={}, clouds={}, fog={})", 
            self.config.components.sky,
            self.config.components.atmosphere,
            self.config.components.clouds,
            self.config.components.fog
        );
        
        // Render sky dome
        if self.config.components.sky {
            tracing::trace!("[HELIO SKIES] Calling render_sky...");
            self.render_sky(encoder, target_view, depth_view, view_proj, camera_pos);
            tracing::trace!("[HELIO SKIES] render_sky returned");
        } else {
            tracing::trace!("[HELIO SKIES] Sky disabled, skipping");
        }
        
        // TODO: Add atmospheric scattering, clouds, fog
    }
    
    fn render_sky(
        &self,
        encoder: &mut gpu::CommandEncoder,
        target_view: gpu::TextureView,
        depth_view: gpu::TextureView,
        view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
    ) {
        tracing::trace!("[HELIO SKIES] render_sky START");
        
        let pipeline = match &self.sky_pipeline {
            Some(p) => {
                tracing::trace!("[HELIO SKIES] Pipeline OK");
                p
            },
            None => {
                tracing::error!("[HELIO SKIES] Pipeline is None!");
                return;
            }
        };
        
        let (vbuf, ibuf) = match (self.sky_vertex_buffer, self.sky_index_buffer) {
            (Some(v), Some(i)) => {
                tracing::trace!("[HELIO SKIES] Buffers OK");
                (v, i)
            },
            _ => {
                tracing::error!("[HELIO SKIES] Buffers are None!");
                return;
            }
        };
        
        tracing::trace!("[HELIO SKIES] Creating camera data");
        let camera_data = SkyCameraData {
            camera: SkyCameraUniforms {
                view_proj,
                camera_position: camera_pos,
                _pad1: 0.0,
                sun_direction: self.config.sun_direction.to_array(),
                _pad2: 0.0,
            },
        };
        
        tracing::trace!("[HELIO SKIES] Creating render pass");
        let mut pass = encoder.render(
            "helio_skies_sky",
            gpu::RenderTargetSet {
                colors: &[gpu::RenderTarget {
                    view: target_view,
                    init_op: gpu::InitOp::Load,
                    finish_op: gpu::FinishOp::Store,
                }],
                depth_stencil: Some(gpu::RenderTarget {
                    view: depth_view,
                    init_op: gpu::InitOp::Load,
                    finish_op: gpu::FinishOp::Store,
                }),
            },
        );
        
        tracing::trace!("[HELIO SKIES] Drawing {} indices", self.sky_index_count);
        let mut rc = pass.with(pipeline);
        rc.bind(0, &camera_data);
        rc.bind_vertex(0, vbuf.into());
        rc.draw_indexed(ibuf.into(), gpu::IndexType::U32, self.sky_index_count, 0, 0, 1);
        
        tracing::trace!("[HELIO SKIES] Dropping render command");
        drop(rc);
        drop(pass);
        
        // TEST: Also try to write yellow to the entire buffer using a fullscreen clear
        tracing::trace!("[HELIO SKIES] TEST: Creating yellow clear pass");
        let mut clear_pass = encoder.render(
            "test_yellow_clear",
            gpu::RenderTargetSet {
                colors: &[gpu::RenderTarget {
                    view: target_view,
                    init_op: gpu::InitOp::Load, // Load existing, we'll overwrite with geometry
                    finish_op: gpu::FinishOp::Store,
                }],
                depth_stencil: None,
            },
        );
        drop(clear_pass);
        
        tracing::trace!("[HELIO SKIES] render_sky COMPLETE");
    }
    
    /// Cleanup GPU resources
    pub fn cleanup(&mut self, context: &Arc<gpu::Context>) {
        tracing::info!("[HELIO SKIES] Cleaning up");
        
        if let Some(mut p) = self.sky_pipeline.take() {
            context.destroy_render_pipeline(&mut p);
        }
        if let Some(b) = self.sky_vertex_buffer.take() {
            context.destroy_buffer(b);
        }
        if let Some(b) = self.sky_index_buffer.take() {
            context.destroy_buffer(b);
        }
        
        tracing::info!("[HELIO SKIES] ✅ Cleanup complete");
    }
}
