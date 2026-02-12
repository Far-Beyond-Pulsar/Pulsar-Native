//! Gizmo rendering for Helio
//! Provides translate/rotate/scale gizmos for level editor

use glam::{Vec3, Mat4};
use helio_core::{Mesh, PackedVertex};

/// Generate a simple arrow mesh for translate gizmo
pub fn create_arrow_mesh(length: f32, shaft_radius: f32, head_radius: f32, head_length: f32) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    
    // Shaft (cylinder)
    let segments = 8;
    let shaft_length = length - head_length;
    
    for i in 0..=segments {
        let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
        let x = angle.cos() * shaft_radius;
        let z = angle.sin() * shaft_radius;
        let normal = [x / shaft_radius, 0.0, z / shaft_radius];
        
        // Bottom
        vertices.push(PackedVertex::new([x, 0.0, z], normal));
        
        // Top
        vertices.push(PackedVertex::new([x, shaft_length, z], normal));
    }
    
    // Generate shaft indices
    for i in 0..segments {
        let base = i * 2;
        indices.push(base as u32);
        indices.push((base + 2) as u32);
        indices.push((base + 1) as u32);
        
        indices.push((base + 1) as u32);
        indices.push((base + 2) as u32);
        indices.push((base + 3) as u32);
    }
    
    // Arrowhead (cone)
    let head_base_idx = vertices.len() as u32;
    let head_start_y = shaft_length;
    
    for i in 0..=segments {
        let angle = (i as f32 / segments as f32) * std::f32::consts::TAU;
        let x = angle.cos() * head_radius;
        let z = angle.sin() * head_radius;
        let normal = [x / head_radius, 0.5, z / head_radius];
        
        vertices.push(PackedVertex::new([x, head_start_y, z], normal));
    }
    
    // Cone tip
    let tip_idx = vertices.len() as u32;
    vertices.push(PackedVertex::new([0.0, length, 0.0], [0.0, 1.0, 0.0]));
    
    // Generate cone indices
    for i in 0..segments {
        indices.push(head_base_idx + i as u32);
        indices.push(tip_idx);
        indices.push(head_base_idx + (i + 1) as u32);
    }
    
    Mesh { vertices, indices }
}

/// Generate a torus mesh for rotate gizmo
pub fn create_torus_mesh(major_radius: f32, minor_radius: f32, major_segments: usize, minor_segments: usize) -> Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    
    for i in 0..=major_segments {
        let u = (i as f32 / major_segments as f32) * std::f32::consts::TAU;
        let cos_u = u.cos();
        let sin_u = u.sin();
        
        for j in 0..=minor_segments {
            let v = (j as f32 / minor_segments as f32) * std::f32::consts::TAU;
            let cos_v = v.cos();
            let sin_v = v.sin();
            
            let x = (major_radius + minor_radius * cos_v) * cos_u;
            let y = minor_radius * sin_v;
            let z = (major_radius + minor_radius * cos_v) * sin_u;
            
            let nx = cos_v * cos_u;
            let ny = sin_v;
            let nz = cos_v * sin_u;
            
            vertices.push(PackedVertex::new([x, y, z], [nx, ny, nz]));
        }
    }
    
    // Generate indices
    for i in 0..major_segments {
        for j in 0..minor_segments {
            let a = (i * (minor_segments + 1) + j) as u32;
            let b = a + (minor_segments + 1) as u32;
            let c = a + 1;
            let d = b + 1;
            
            indices.push(a);
            indices.push(b);
            indices.push(c);
            
            indices.push(c);
            indices.push(b);
            indices.push(d);
        }
    }
    
    Mesh { vertices, indices }
}

/// Gizmo type matching the one from bevy_renderer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoType {
    None,
    Translate,
    Rotate,
    Scale,
}

/// Gizmo axis
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoAxis {
    X,
    Y,
    Z,
    None,
}

impl GizmoAxis {
    pub fn color(&self) -> Vec3 {
        match self {
            GizmoAxis::X => Vec3::new(1.0, 0.0, 0.0), // Red
            GizmoAxis::Y => Vec3::new(0.0, 1.0, 0.0), // Green
            GizmoAxis::Z => Vec3::new(0.0, 0.0, 1.0), // Blue
            GizmoAxis::None => Vec3::new(0.8, 0.8, 0.8), // Gray
        }
    }
}

/// Create transform for a gizmo arrow along a specific axis
pub fn create_gizmo_arrow_transform(position: Vec3, axis: GizmoAxis, scale: f32) -> Mat4 {
    let translation = Mat4::from_translation(position);
    
    let rotation = match axis {
        GizmoAxis::X => Mat4::from_rotation_z(-std::f32::consts::FRAC_PI_2),
        GizmoAxis::Y => Mat4::IDENTITY,
        GizmoAxis::Z => Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
        GizmoAxis::None => Mat4::IDENTITY,
    };
    
    let scale_mat = Mat4::from_scale(Vec3::splat(scale));
    
    translation * rotation * scale_mat
}

/// Create transform for a rotation torus along a specific axis
pub fn create_gizmo_torus_transform(position: Vec3, axis: GizmoAxis, scale: f32) -> Mat4 {
    let translation = Mat4::from_translation(position);
    
    let rotation = match axis {
        GizmoAxis::X => Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
        GizmoAxis::Y => Mat4::IDENTITY,
        GizmoAxis::Z => Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2),
        GizmoAxis::None => Mat4::IDENTITY,
    };
    
    let scale_mat = Mat4::from_scale(Vec3::splat(scale));
    
    translation * rotation * scale_mat
}
