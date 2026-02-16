//! Gizmo rendering module (simplified - just render function, no Feature trait)
//! 
//! Renders transform gizmos as an overlay after the main scene.

use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use crate::scene::{SceneDb, GizmoType, GizmoAxis};
use std::sync::Arc;
use std::ptr;

/// Gizmo vertex with position
#[derive(blade_macros::Vertex, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct GizmoVertex {
    position: [f32; 3],
}

// Camera uniforms for gizmo rendering
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GizmoCameraUniforms {
    view_proj: [[f32; 4]; 4],
    position: [f32; 3],
    _pad: f32,
}

#[derive(blade_macros::ShaderData)]
struct GizmoCameraData {
    camera: GizmoCameraUniforms,
}

// Per-axis gizmo instance data
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GizmoInstanceUniforms {
    world_position: [f32; 3],
    _pad1: f32,
    color: [f32; 4],
    axis_direction: [f32; 3],
    scale: f32,
}

#[derive(blade_macros::ShaderData)]
struct GizmoInstanceData {
    gizmo: GizmoInstanceUniforms,
}

pub struct GizmoRenderer {
    scene_db: Arc<SceneDb>,
    pipeline: Option<gpu::RenderPipeline>,
    arrow_vertex_buffer: Option<gpu::Buffer>,
    arrow_index_buffer: Option<gpu::Buffer>,
    arrow_index_count: u32,
    torus_vertex_buffer: Option<gpu::Buffer>,
    torus_index_buffer: Option<gpu::Buffer>,
    torus_index_count: u32,
    cube_vertex_buffer: Option<gpu::Buffer>,
    cube_index_buffer: Option<gpu::Buffer>,
    cube_index_count: u32,
}

impl GizmoRenderer {
    pub fn new(scene_db: Arc<SceneDb>) -> Self {
        tracing::info!("[GIZMO RENDERER] Creating gizmo renderer");
        Self {
            scene_db,
            pipeline: None,
            arrow_vertex_buffer: None,
            arrow_index_buffer: None,
            arrow_index_count: 0,
            torus_vertex_buffer: None,
            torus_index_buffer: None,
            torus_index_count: 0,
            cube_vertex_buffer: None,
            cube_index_buffer: None,
            cube_index_count: 0,
        }
    }
    
    pub fn init(&mut self, context: &Arc<gpu::Context>, color_format: gpu::TextureFormat, depth_format: gpu::TextureFormat) {
        tracing::info!("[GIZMO RENDERER] Initializing");
        
        // Create simple arrow mesh
        let vertices = vec![
            GizmoVertex { position: [-0.02, 0.0, 0.0] },
            GizmoVertex { position: [0.02, 0.0, 0.0] },
            GizmoVertex { position: [0.02, 0.8, 0.0] },
            GizmoVertex { position: [-0.02, 0.8, 0.0] },
            GizmoVertex { position: [-0.1, 0.8, 0.0] },
            GizmoVertex { position: [0.1, 0.8, 0.0] },
            GizmoVertex { position: [0.0, 1.0, 0.0] },
        ];
        
        let indices: Vec<u32> = vec![0, 1, 2,  0, 2, 3,  4, 5, 6];
        self.arrow_index_count = indices.len() as u32;
        
        // Create vertex buffer
        let vbuf = context.create_buffer(gpu::BufferDesc {
            name: "gizmo_arrow_vertices",
            size: (vertices.len() * std::mem::size_of::<GizmoVertex>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(vertices.as_ptr(), vbuf.data() as *mut GizmoVertex, vertices.len());
        }
        context.sync_buffer(vbuf);
        
        // Create index buffer
        let ibuf = context.create_buffer(gpu::BufferDesc {
            name: "gizmo_arrow_indices",
            size: (indices.len() * std::mem::size_of::<u32>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(indices.as_ptr(), ibuf.data() as *mut u32, indices.len());
        }
        context.sync_buffer(ibuf);
        
        self.arrow_vertex_buffer = Some(vbuf);
        self.arrow_index_buffer = Some(ibuf);
        
        // === CREATE TORUS MESH (for rotate gizmo) ===
        let segments = 32;
        let mut torus_vertices = Vec::new();
        let mut torus_indices = Vec::new();
        let radius = 0.8;
        let thickness = 0.03;
        
        for i in 0..segments {
            let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
            let x = angle.cos() * radius;
            let z = angle.sin() * radius;
            
            // Inner and outer vertices
            let inner_x = angle.cos() * (radius - thickness);
            let inner_z = angle.sin() * (radius - thickness);
            
            torus_vertices.push(GizmoVertex { position: [inner_x, 0.0, inner_z] });
            torus_vertices.push(GizmoVertex { position: [x, 0.0, z] });
        }
        
        for i in 0..segments {
            let next = (i + 1) % segments;
            let base = i * 2;
            let next_base = next * 2;
            
            // Two triangles per segment
            torus_indices.push(base as u32);
            torus_indices.push(next_base as u32);
            torus_indices.push((base + 1) as u32);
            
            torus_indices.push((base + 1) as u32);
            torus_indices.push(next_base as u32);
            torus_indices.push((next_base + 1) as u32);
        }
        
        self.torus_index_count = torus_indices.len() as u32;
        
        let torus_vbuf = context.create_buffer(gpu::BufferDesc {
            name: "gizmo_torus_vertices",
            size: (torus_vertices.len() * std::mem::size_of::<GizmoVertex>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(torus_vertices.as_ptr(), torus_vbuf.data() as *mut GizmoVertex, torus_vertices.len());
        }
        context.sync_buffer(torus_vbuf);
        
        let torus_ibuf = context.create_buffer(gpu::BufferDesc {
            name: "gizmo_torus_indices",
            size: (torus_indices.len() * std::mem::size_of::<u32>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(torus_indices.as_ptr(), torus_ibuf.data() as *mut u32, torus_indices.len());
        }
        context.sync_buffer(torus_ibuf);
        
        self.torus_vertex_buffer = Some(torus_vbuf);
        self.torus_index_buffer = Some(torus_ibuf);
        
        // === CREATE CUBE MESH (for scale gizmo) ===
        let cube_vertices = vec![
            // Cube with 0.15 unit size
            GizmoVertex { position: [-0.075, -0.075, -0.075] },
            GizmoVertex { position: [0.075, -0.075, -0.075] },
            GizmoVertex { position: [0.075, 0.075, -0.075] },
            GizmoVertex { position: [-0.075, 0.075, -0.075] },
            GizmoVertex { position: [-0.075, -0.075, 0.075] },
            GizmoVertex { position: [0.075, -0.075, 0.075] },
            GizmoVertex { position: [0.075, 0.075, 0.075] },
            GizmoVertex { position: [-0.075, 0.075, 0.075] },
        ];
        
        let cube_indices: Vec<u32> = vec![
            // Front face
            0, 1, 2,  0, 2, 3,
            // Back face
            4, 6, 5,  4, 7, 6,
            // Top face
            3, 2, 6,  3, 6, 7,
            // Bottom face
            0, 5, 1,  0, 4, 5,
            // Right face
            1, 5, 6,  1, 6, 2,
            // Left face
            0, 3, 7,  0, 7, 4,
        ];
        
        self.cube_index_count = cube_indices.len() as u32;
        
        let cube_vbuf = context.create_buffer(gpu::BufferDesc {
            name: "gizmo_cube_vertices",
            size: (cube_vertices.len() * std::mem::size_of::<GizmoVertex>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(cube_vertices.as_ptr(), cube_vbuf.data() as *mut GizmoVertex, cube_vertices.len());
        }
        context.sync_buffer(cube_vbuf);
        
        let cube_ibuf = context.create_buffer(gpu::BufferDesc {
            name: "gizmo_cube_indices",
            size: (cube_indices.len() * std::mem::size_of::<u32>()) as u64,
            memory: gpu::Memory::Shared,
        });
        unsafe {
            ptr::copy_nonoverlapping(cube_indices.as_ptr(), cube_ibuf.data() as *mut u32, cube_indices.len());
        }
        context.sync_buffer(cube_ibuf);
        
        self.cube_vertex_buffer = Some(cube_vbuf);
        self.cube_index_buffer = Some(cube_ibuf);
        
        // Create pipeline
        let camera_layout = <GizmoCameraData as gpu::ShaderData>::layout();
        let instance_layout = <GizmoInstanceData as gpu::ShaderData>::layout();
        
        let shader_source = include_str!("shaders/gizmo.wgsl");
        let shader = context.create_shader(gpu::ShaderDesc { source: shader_source });
        
        let pipeline = context.create_render_pipeline(gpu::RenderPipelineDesc {
            name: "gizmo",
            data_layouts: &[&camera_layout, &instance_layout],
            vertex: shader.at("vs_main"),
            vertex_fetches: &[gpu::VertexFetchState {
                layout: &<GizmoVertex as gpu::Vertex>::layout(),
                instanced: false,
            }],
            primitive: gpu::PrimitiveState {
                topology: gpu::PrimitiveTopology::TriangleList,
                front_face: gpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(gpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: false,
                depth_compare: gpu::CompareFunction::Always, // ALWAYS pass depth test for debugging
                stencil: Default::default(),
                bias: Default::default(),
            }),
            fragment: Some(shader.at("fs_main")),
            color_targets: &[gpu::ColorTargetState {
                format: color_format,
                blend: Some(gpu::BlendState::ALPHA_BLENDING),
                write_mask: gpu::ColorWrites::default(),
            }],
            multisample_state: gpu::MultisampleState::default(),
        });
        
        self.pipeline = Some(pipeline);
        tracing::info!("[GIZMO RENDERER] ✅ Initialized");
    }
    
    pub fn render(
        &self,
        encoder: &mut gpu::CommandEncoder,
        target_view: gpu::TextureView,
        depth_view: gpu::TextureView,
        view_proj: [[f32; 4]; 4],
        camera_pos: [f32; 3],
    ) {
        let gizmo_state = self.scene_db.get_gizmo_state();
        
        if gizmo_state.gizmo_type == GizmoType::None {
            return;
        }
        
        let pipeline = match &self.pipeline {
            Some(p) => p,
            _ => {
                tracing::error!("[GIZMO RENDERER] Not initialized - pipeline missing!");
                return;
            }
        };
        
        // Get selected object position
        let selected_id = match self.scene_db.get_selected_id() {
            Some(id) => id,
            None => return,
        };
        
        let entry = match self.scene_db.get_entry(&selected_id) {
            Some(e) => e,
            None => return,
        };
        
        let pos_array = entry.get_position();
        let gizmo_pos = [pos_array[0], pos_array[1], pos_array[2]];
        
        // Calculate scale based on camera distance
        let distance = (Vec3::new(camera_pos[0], camera_pos[1], camera_pos[2]) 
                      - Vec3::new(gizmo_pos[0], gizmo_pos[1], gizmo_pos[2])).length();
        let scale = (distance * 0.15).max(0.5).min(3.0);
        
        let camera_data = GizmoCameraData {
            camera: GizmoCameraUniforms {
                view_proj,
                position: camera_pos,
                _pad: 0.0,
            },
        };
        
        let mut pass = encoder.render(
            "gizmo_overlay",
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
        
        let mut rc = pass.with(pipeline);
        rc.bind(0, &camera_data);
        
        // Select appropriate mesh based on gizmo type
        let (vbuf, ibuf, index_count) = match gizmo_state.gizmo_type {
            GizmoType::Translate => {
                match (self.arrow_vertex_buffer, self.arrow_index_buffer) {
                    (Some(v), Some(i)) => (v, i, self.arrow_index_count),
                    _ => return,
                }
            },
            GizmoType::Rotate => {
                match (self.torus_vertex_buffer, self.torus_index_buffer) {
                    (Some(v), Some(i)) => (v, i, self.torus_index_count),
                    _ => return,
                }
            },
            GizmoType::Scale => {
                match (self.cube_vertex_buffer, self.cube_index_buffer) {
                    (Some(v), Some(i)) => (v, i, self.cube_index_count),
                    _ => return,
                }
            },
            GizmoType::None => return,
        };
        
        // Render each axis
        for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
            let highlighted = gizmo_state.highlighted_axis == Some(axis);
            let axis_scale = if highlighted { scale * 1.2 } else { scale };
            
            let (color, axis_dir) = match axis {
                GizmoAxis::X => ([if highlighted { 1.0 } else { 0.8 }, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
                GizmoAxis::Y => ([0.0, if highlighted { 1.0 } else { 0.8 }, 0.0, 1.0], [0.0, 1.0, 0.0]),
                GizmoAxis::Z => ([0.0, 0.0, if highlighted { 1.0 } else { 0.8 }, 1.0], [0.0, 0.0, 1.0]),
            };
            
            // For scale gizmos, offset the cubes to the end of each axis
            let instance_pos = if gizmo_state.gizmo_type == GizmoType::Scale {
                let offset = Vec3::new(axis_dir[0], axis_dir[1], axis_dir[2]) * scale;
                [gizmo_pos[0] + offset.x, gizmo_pos[1] + offset.y, gizmo_pos[2] + offset.z]
            } else {
                gizmo_pos
            };
            
            let instance_data = GizmoInstanceData {
                gizmo: GizmoInstanceUniforms {
                    world_position: instance_pos,
                    _pad1: 0.0,
                    color,
                    axis_direction: axis_dir,
                    scale: axis_scale,
                },
            };
            
            rc.bind(1, &instance_data);
            rc.bind_vertex(0, vbuf.into());
            rc.draw_indexed(ibuf.into(), gpu::IndexType::U32, index_count, 0, 0, 1);
        }
        
        drop(rc);
        drop(pass);
    }
    
    pub fn cleanup(&mut self, context: &Arc<gpu::Context>) {
        if let Some(mut p) = self.pipeline.take() {
            context.destroy_render_pipeline(&mut p);
        }
        if let Some(b) = self.arrow_vertex_buffer.take() {
            context.destroy_buffer(b);
        }
        if let Some(b) = self.arrow_index_buffer.take() {
            context.destroy_buffer(b);
        }
        if let Some(b) = self.torus_vertex_buffer.take() {
            context.destroy_buffer(b);
        }
        if let Some(b) = self.torus_index_buffer.take() {
            context.destroy_buffer(b);
        }
        if let Some(b) = self.cube_vertex_buffer.take() {
            context.destroy_buffer(b);
        }
        if let Some(b) = self.cube_index_buffer.take() {
            context.destroy_buffer(b);
        }
    }
}
