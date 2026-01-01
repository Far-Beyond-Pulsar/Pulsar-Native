//! Gizmos - 3D manipulation visuals for the Bevy renderer
//!
//! This module renders interactive 3D gizmos for object transformation in the level editor.
//! Gizmos are only visible in Edit mode and hidden during Play mode.

use bevy::prelude::*;
use std::sync::Arc;
use crate::subsystems::render::bevy_renderer::core::{GameObjectId, Selected, SelectionOutline};

/// Gizmo type - which manipulation tool is active
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoType {
    None,
    Translate,
    Rotate,
    Scale,
}

/// Gizmo axis being manipulated
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoAxis {
    None,
    X,
    Y,
    Z,
}

/// Component marker for gizmo entities
#[derive(Component)]
pub struct GizmoVisual {
    pub gizmo_type: GizmoType,
    pub axis: GizmoAxis,
}

/// Simple transform for sharing between Bevy and GPUI
#[derive(Clone, Copy, Debug)]
pub struct SharedTransform {
    pub position: [f32; 3],
    pub rotation: [f32; 4], // Quaternion [x, y, z, w]
    pub scale: [f32; 3],
}

/// Resource holding gizmo state from the editor
#[derive(Resource, Clone)]
pub struct GizmoStateResource {
    pub gizmo_type: GizmoType,
    pub active_axis: GizmoAxis,
    pub target_position: Vec3,
    pub enabled: bool, // False in Play mode
    pub selected_object_id: Option<String>, // ID of currently selected object (None = no selection, no gizmo)

    // Transform updates from Bevy → GPUI (set by gizmo drag system)
    pub updated_object_id: Option<String>,  // Object that was just moved
    pub updated_transform: Option<SharedTransform>, // New transform after gizmo manipulation
}

impl Default for GizmoStateResource {
    fn default() -> Self {
        Self {
            gizmo_type: GizmoType::Translate,
            active_axis: GizmoAxis::None,
            target_position: Vec3::ZERO,
            enabled: true, // Start enabled (Edit mode)
            selected_object_id: None, // No object selected initially
            updated_object_id: None,
            updated_transform: None,
        }
    }
}

/// System to update gizmo visuals based on selected object
/// Gizmos are ONLY visible when:
/// 1. Editor is in Edit mode (enabled = true)
/// 2. An object is selected (selected_object_id is Some)
/// 3. A gizmo tool is active (gizmo_type != None)
pub fn update_gizmo_visuals(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    gizmo_state: Res<GizmoStateResource>,
    gizmo_query: Query<Entity, With<GizmoVisual>>,
) {
    // ALWAYS clear existing gizmos first
    for entity in gizmo_query.iter() {
        commands.entity(entity).despawn();
    }
    
    // Don't render gizmos if:
    // 1. Disabled (Play mode)
    // 2. No object selected
    // 3. No tool selected
    if !gizmo_state.enabled 
        || gizmo_state.selected_object_id.is_none() 
        || gizmo_state.gizmo_type == GizmoType::None 
    {
        return;
    }
    
    // Render appropriate gizmo type at target position
    match gizmo_state.gizmo_type {
        GizmoType::Translate => {
            spawn_translate_gizmo(&mut commands, &mut meshes, &mut materials, &gizmo_state);
        }
        GizmoType::Rotate => {
            spawn_rotate_gizmo(&mut commands, &mut meshes, &mut materials, &gizmo_state);
        }
        GizmoType::Scale => {
            spawn_scale_gizmo(&mut commands, &mut meshes, &mut materials, &gizmo_state);
        }
        GizmoType::None => {}
    }
}

/// Spawn translate gizmo (3 colored arrows)
fn spawn_translate_gizmo(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    gizmo_state: &GizmoStateResource,
) {
    let arrow_length = 2.0;
    let arrow_radius = 0.05;
    let position = gizmo_state.target_position;
    
    // X axis (Red arrow)
    let x_color = if gizmo_state.active_axis == GizmoAxis::X {
        Color::srgb(1.0, 1.0, 0.0) // Yellow when active
    } else {
        Color::srgb(1.0, 0.2, 0.2) // Red
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Capsule3d::new(arrow_radius, arrow_length))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: x_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position + Vec3::X * arrow_length / 2.0)
            .with_rotation(Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)),
        GizmoVisual {
            gizmo_type: GizmoType::Translate,
            axis: GizmoAxis::X,
        },
    ));
    
    // Y axis (Green arrow)
    let y_color = if gizmo_state.active_axis == GizmoAxis::Y {
        Color::srgb(1.0, 1.0, 0.0) // Yellow when active
    } else {
        Color::srgb(0.2, 1.0, 0.2) // Green
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Capsule3d::new(arrow_radius, arrow_length))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: y_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position + Vec3::Y * arrow_length / 2.0),
        GizmoVisual {
            gizmo_type: GizmoType::Translate,
            axis: GizmoAxis::Y,
        },
    ));
    
    // Z axis (Blue arrow)
    let z_color = if gizmo_state.active_axis == GizmoAxis::Z {
        Color::srgb(1.0, 1.0, 0.0) // Yellow when active
    } else {
        Color::srgb(0.2, 0.5, 1.0) // Blue
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Capsule3d::new(arrow_radius, arrow_length))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: z_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position + Vec3::Z * arrow_length / 2.0)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        GizmoVisual {
            gizmo_type: GizmoType::Translate,
            axis: GizmoAxis::Z,
        },
    ));
}

/// Spawn rotate gizmo (3 colored rings)
fn spawn_rotate_gizmo(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    gizmo_state: &GizmoStateResource,
) {
    let ring_radius = 1.5;
    let ring_thickness = 0.03;
    let position = gizmo_state.target_position;
    
    // X axis ring (Red)
    let x_color = if gizmo_state.active_axis == GizmoAxis::X {
        Color::srgb(1.0, 1.0, 0.0)
    } else {
        Color::srgb(1.0, 0.2, 0.2)
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Torus {
            minor_radius: ring_thickness,
            major_radius: ring_radius,
        })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: x_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        GizmoVisual {
            gizmo_type: GizmoType::Rotate,
            axis: GizmoAxis::X,
        },
    ));
    
    // Y axis ring (Green)
    let y_color = if gizmo_state.active_axis == GizmoAxis::Y {
        Color::srgb(1.0, 1.0, 0.0)
    } else {
        Color::srgb(0.2, 1.0, 0.2)
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Torus {
            minor_radius: ring_thickness,
            major_radius: ring_radius,
        })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: y_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position),
        GizmoVisual {
            gizmo_type: GizmoType::Rotate,
            axis: GizmoAxis::Y,
        },
    ));
    
    // Z axis ring (Blue)
    let z_color = if gizmo_state.active_axis == GizmoAxis::Z {
        Color::srgb(1.0, 1.0, 0.0)
    } else {
        Color::srgb(0.2, 0.5, 1.0)
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Torus {
            minor_radius: ring_thickness,
            major_radius: ring_radius,
        })),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: z_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        GizmoVisual {
            gizmo_type: GizmoType::Rotate,
            axis: GizmoAxis::Z,
        },
    ));
}

/// Spawn scale gizmo (3 colored cubes)
fn spawn_scale_gizmo(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    gizmo_state: &GizmoStateResource,
) {
    let handle_distance = 2.0;
    let handle_size = 0.2;
    let position = gizmo_state.target_position;
    
    // X axis handle (Red cube)
    let x_color = if gizmo_state.active_axis == GizmoAxis::X {
        Color::srgb(1.0, 1.0, 0.0)
    } else {
        Color::srgb(1.0, 0.2, 0.2)
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(handle_size, handle_size, handle_size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: x_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position + Vec3::X * handle_distance),
        GizmoVisual {
            gizmo_type: GizmoType::Scale,
            axis: GizmoAxis::X,
        },
    ));
    
    // Y axis handle (Green cube)
    let y_color = if gizmo_state.active_axis == GizmoAxis::Y {
        Color::srgb(1.0, 1.0, 0.0)
    } else {
        Color::srgb(0.2, 1.0, 0.2)
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(handle_size, handle_size, handle_size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: y_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position + Vec3::Y * handle_distance),
        GizmoVisual {
            gizmo_type: GizmoType::Scale,
            axis: GizmoAxis::Y,
        },
    ));
    
    // Z axis handle (Blue cube)
    let z_color = if gizmo_state.active_axis == GizmoAxis::Z {
        Color::srgb(1.0, 1.0, 0.0)
    } else {
        Color::srgb(0.2, 0.5, 1.0)
    };
    
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(handle_size, handle_size, handle_size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: z_color,
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position + Vec3::Z * handle_distance),
        GizmoVisual {
            gizmo_type: GizmoType::Scale,
            axis: GizmoAxis::Z,
        },
    ));
    
    // Center cube for uniform scaling
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(handle_size * 1.5, handle_size * 1.5, handle_size * 1.5))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.8, 0.8),
            unlit: true,
            alpha_mode: AlphaMode::Opaque,
            ..default()
        })),
        Transform::from_translation(position),
        GizmoVisual {
            gizmo_type: GizmoType::Scale,
            axis: GizmoAxis::None, // Uniform scale
        },
    ));
}

/// System to update selection highlighting
/// Adds/removes outline materials on objects based on selection state
pub fn update_selection_highlighting(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    gizmo_state: Res<GizmoStateResource>,
    // All game objects with their IDs
    game_objects: Query<(Entity, &GameObjectId, Option<&Selected>)>,
    // Objects that have outline materials
    outlined_objects: Query<(Entity, &SelectionOutline)>,
) {
    if !gizmo_state.enabled {
        // In Play mode, clear all selection outlines
        for (entity, _) in outlined_objects.iter() {
            commands.entity(entity).remove::<SelectionOutline>();
            commands.entity(entity).remove::<Selected>();
        }
        return;
    }
    
    // Helper to convert String ID to numeric ID
    // For now, we map known IDs: "red_cube" -> 1, "blue_sphere" -> 2, "gold_sphere" -> 3
    let selected_numeric_id = gizmo_state.selected_object_id.as_ref().and_then(|id| {
        match id.as_str() {
            "red_cube" => Some(1),
            "blue_sphere" => Some(2),
            "gold_sphere" => Some(3),
            _ => {
                // Try to parse as number
                id.parse::<u64>().ok()
            }
        }
    });
    
    // Update selection state on all objects
    for (entity, game_obj_id, has_selected) in game_objects.iter() {
        let should_be_selected = selected_numeric_id == Some(game_obj_id.0);
        
        if should_be_selected && has_selected.is_none() {
            // Add selection marker and outline
            commands.entity(entity).insert(Selected);
            commands.entity(entity).insert(SelectionOutline {
                color: Color::srgb(1.0, 0.7, 0.0), // Orange outline
                width: 0.05,
            });
            tracing::debug!("[BEVY-GIZMO] ✅ Selected object {} (ID: {})", game_obj_id.0, gizmo_state.selected_object_id.as_ref().unwrap_or(&"none".to_string()));
        } else if !should_be_selected && has_selected.is_some() {
            // Remove selection marker and outline
            commands.entity(entity).remove::<Selected>();
            commands.entity(entity).remove::<SelectionOutline>();
        }
    }
}

