//! Rendering mode systems - apply view modes and debug visualizations
//!
//! This module contains systems that apply rendering settings to the scene:
//! - Wireframe mode
//! - Unlit mode
//! - Debug visualizations
//! - Grid visibility

use bevy::prelude::*;
use bevy::pbr::wireframe::{Wireframe, WireframeConfig};
use bevy::pbr::MeshMaterial3d;
use crate::subsystems::render::bevy_renderer::core::{RenderingSettings, ViewMode, DebugVisualization, GameObjectId};

/// Apply rendering mode changes based on settings
/// This system runs after sync_rendering_settings_system to apply the new settings
pub fn apply_rendering_modes_system(
    settings: Res<RenderingSettings>,
    mut wireframe_config: ResMut<WireframeConfig>,
    mut commands: Commands,
    // Query all 3D mesh entities (entities with a Mesh3d component)
    mesh_entities: Query<Entity, With<Mesh3d>>,
    // Track which entities already have wireframe component
    wireframe_entities: Query<Entity, With<Wireframe>>,
) {
    // Only update when settings change (Bevy's change detection)
    if !settings.is_changed() {
        return;
    }

    // WIREFRAME MODE
    match settings.view_mode {
        ViewMode::Wireframe => {
            // Enable global wireframe
            wireframe_config.global = true;

            // Add Wireframe component to all mesh entities that don't have it
            for entity in mesh_entities.iter() {
                if !wireframe_entities.contains(entity) {
                    commands.entity(entity).insert(Wireframe);
                }
            }
        }
        ViewMode::WireframeOnShaded => {
            // Enable global wireframe overlay
            wireframe_config.global = true;
        }
        _ => {
            // Disable global wireframe for other modes
            wireframe_config.global = false;

            // Remove Wireframe component from entities (unless it's wireframe mode)
            if settings.wireframe_enabled {
                // User toggled wireframe overlay - keep global wireframe
                wireframe_config.global = true;
            } else {
                // Remove wireframe components
                for entity in wireframe_entities.iter() {
                    commands.entity(entity).remove::<Wireframe>();
                }
            }
        }
    }

    // WIREFRAME OVERLAY (independent of view mode)
    if settings.wireframe_enabled && settings.view_mode != ViewMode::Wireframe {
        wireframe_config.global = true;
    }

    // DEBUG VISUALIZATIONS
    // These will need custom shaders/materials - for now just log when they're enabled
    static mut LAST_DEBUG_VIZ: Option<DebugVisualization> = None;
    unsafe {
        if LAST_DEBUG_VIZ.is_none() || LAST_DEBUG_VIZ.as_ref() != Some(&settings.debug_visualization) {
            match settings.debug_visualization {
                DebugVisualization::None => {},
                DebugVisualization::ShaderComplexity => {
                    println!("[RENDERING] 🎨 Shader complexity visualization enabled (requires custom shader)");
                }
                DebugVisualization::LightComplexity => {
                    println!("[RENDERING] 💡 Light complexity visualization enabled (requires custom shader)");
                }
                DebugVisualization::Overdraw => {
                    println!("[RENDERING] 📊 Overdraw visualization enabled (requires custom shader)");
                }
                DebugVisualization::TriangleDensity => {
                    println!("[RENDERING] 🔺 Triangle density visualization enabled (requires custom shader)");
                }
                DebugVisualization::LODVisualization => {
                    println!("[RENDERING] 🎯 LOD visualization enabled (requires custom shader)");
                }
            }
            LAST_DEBUG_VIZ = Some(settings.debug_visualization);
        }
    }
}

/// Apply lighting settings
/// Disables/enables lights based on lighting_enabled setting
pub fn apply_lighting_settings_system(
    settings: Res<RenderingSettings>,
    mut lights: Query<&mut Visibility, With<PointLight>>,
    mut dir_lights: Query<&mut Visibility, (With<DirectionalLight>, Without<PointLight>)>,
) {
    if !settings.is_changed() {
        return;
    }

    let visibility = if settings.lighting_enabled || settings.view_mode == ViewMode::Lit {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };

    // Update all lights
    for mut vis in lights.iter_mut() {
        *vis = visibility;
    }
    for mut vis in dir_lights.iter_mut() {
        *vis = visibility;
    }
}
