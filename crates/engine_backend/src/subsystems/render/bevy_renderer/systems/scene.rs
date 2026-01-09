//! Scene setup - spawns 3D objects, cameras, and lights

use bevy::prelude::*;
use bevy::core_pipeline::tonemapping::Tonemapping;
use crate::subsystems::render::bevy_renderer::core::{MainCamera, GameObjectId, SharedTexturesResource};
use std::path::Path;

/// Setup 3D scene - runs AFTER DXGI textures are created
pub fn setup_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    shared_textures: Res<SharedTexturesResource>,
) {
    tracing::debug!("[BEVY] 🎬 Setting up scene...");

    // Get the shared textures to determine which buffer to render to
    let textures = match shared_textures.0.lock().ok().and_then(|l| l.as_ref().cloned()) {
        Some(t) => t,
        None => {
            tracing::debug!("[BEVY] ❌ No render targets available");
            return;
        }
    };
    
    // Get the WRITE buffer index (this is where the camera will render)
    let write_index = textures.write_index.load(std::sync::atomic::Ordering::Acquire);
    let render_target = textures.textures[write_index].clone();
    
    tracing::debug!("[BEVY] ✅ Got render target handles");
    tracing::debug!("[BEVY] 📍 Initial write_index={}, read_index={}", 
             write_index, 
             textures.read_index.load(std::sync::atomic::Ordering::Acquire));
    tracing::debug!("[BEVY] 🎯 Camera will initially render to buffer {} (asset ID: {:?})", 
             write_index, render_target.id());

    // Camera rendering to shared DXGI texture with TONEMAPPING DISABLED
    tracing::debug!("[BEVY] 📹 Creating camera targeting shared texture");
    commands.spawn((
        Camera3d::default(),
        Camera {
            target: bevy::camera::RenderTarget::Image(render_target.into()),
            clear_color: bevy::prelude::ClearColorConfig::Custom(Color::srgb(0.2, 0.2, 0.3)), // Dark blue-grey background
            ..default()
        },
        Transform::from_xyz(-3.0, 3.0, 6.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
        Tonemapping::None, // CRITICAL: Disable tonemapping for proper color reproduction
        MainCamera,
    ));
    tracing::debug!("[BEVY] ✅ Camera spawned with tonemapping DISABLED - double-buffering enabled!");
    tracing::debug!("[BEVY] 🔄 Camera renders to write buffer, GPUI reads from read buffer");

    // Try to load default level file from the project directory
    let project_dir = engine_state::get_project_path()
        .unwrap_or("C:\\Users\\redst\\OneDrive\\Documents\\Pulsar_Projects\\blank_project");
    
    println!("[PULSAR_SCENE DEBUG] ========================================");
    println!("[PULSAR_SCENE DEBUG] Project dir from engine_state::get_project_path(): {:?}", engine_state::get_project_path());
    println!("[PULSAR_SCENE DEBUG] Using project_dir: {:?}", project_dir);
    
    let level_path = Path::new(&project_dir).join("scenes").join("default.level");
    println!("[PULSAR_SCENE DEBUG] Level path: {:?}", level_path);
    println!("[PULSAR_SCENE DEBUG] Level path exists: {}", level_path.exists());
    println!("[PULSAR_SCENE DEBUG] ========================================");
    
    tracing::debug!("[BEVY] 🔍 Checking for level file at {:?}", level_path);
    let mut id = 1;
    
    if level_path.exists() {
        println!("[PULSAR_SCENE DEBUG] Level file EXISTS! Attempting to read...");
        tracing::debug!("[BEVY] 📂 Loading level from: {:?}", level_path);
        match std::fs::read_to_string(&level_path) {
            Ok(content) => {
                println!("[BEVY DEBUG] Successfully read file, {} bytes", content.len());
                println!("[BEVY DEBUG] File content preview (first 200 chars): {:?}", &content.chars().take(200).collect::<String>());
                match serde_json::from_str::<LevelData>(&content) {
                    Ok(level) => {
                        println!("[BEVY DEBUG] ✅ JSON parsing successful! Found {} objects", level.game_objects.len());
                        tracing::debug!("[BEVY] ✅ Level file parsed successfully");
                        spawn_level_objects(&mut commands, &mut meshes, &mut materials, &level, &mut id);
                        tracing::debug!("[BEVY] ✅ Level loaded with {} objects", id - 1);
                    }
                    Err(e) => {
                        println!("[BEVY DEBUG] ❌ JSON parsing FAILED: {}", e);
                        tracing::warn!("[BEVY] ⚠️ Failed to parse level file: {}", e);
                        spawn_fallback_scene(&mut commands, &mut meshes, &mut materials, &mut id);
                    }
                }
            }
            Err(e) => {
                println!("[BEVY DEBUG] ❌ Failed to read file: {}", e);
                tracing::warn!("[BEVY] ⚠️ Failed to read level file: {}", e);
                spawn_fallback_scene(&mut commands, &mut meshes, &mut materials, &mut id);
            }
        }
    } else {
        println!("[BEVY DEBUG] Level file DOES NOT EXIST - using fallback");
        tracing::debug!("[BEVY] 📂 No level file found at {:?}, using fallback scene", level_path);
        spawn_fallback_scene(&mut commands, &mut meshes, &mut materials, &mut id);
    }
    
    // Directional light with shadows enabled
    commands.spawn((
        DirectionalLight {
            color: Color::WHITE,
            illuminance: 20000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    
    // Ambient light
    commands.insert_resource(AmbientLight {
        color: Color::WHITE,
        brightness: 300.0,
        affects_lightmapped_meshes: true,
    });
    
    tracing::debug!("[BEVY] ✅ Scene ready!");
}

fn spawn_fallback_scene(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    id: &mut u32,
) {
    tracing::debug!("[BEVY] 🎨 Spawning fallback scene (basic cube)...");
    
    // Ground plane
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(10.0, 0.1, 10.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.3, 0.3),
            metallic: 0.0,
            perceptual_roughness: 0.8,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.0, 0.0),
        GameObjectId((*id).into()),
    ));
    *id += 1;
    
    // Center cube
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.3, 0.3),
            metallic: 0.2,
            perceptual_roughness: 0.5,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.6, 0.0).with_rotation(Quat::from_rotation_y(0.785)),
        GameObjectId((*id).into()),
    ));
    *id += 1;

    tracing::debug!("[BEVY] ✅ Fallback scene created with {} objects", *id - 1);
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct LevelData {
    #[serde(default)]
    name: Option<String>,
    game_objects: Vec<GameObject>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GameObject {
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    name: Option<String>,
    mesh_type: MeshType,
    transform: GameObjectTransform,
    #[serde(default)]
    material: Option<MaterialData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GameObjectTransform {
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

#[derive(Debug, Serialize, Deserialize)]
struct MaterialData {
    #[serde(default)]
    color: Option<Vec<f32>>,  // Support both RGB [r,g,b] and RGBA [r,g,b,a]
    #[serde(default)]
    metallic: Option<f32>,
    #[serde(default)]
    roughness: Option<f32>,
    #[serde(default)]
    emissive: Option<[f32; 3]>,
    #[serde(default)]
    alpha_mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
enum MeshType {
    Cube,
    Sphere,
    Cylinder,
    Plane,
}

fn spawn_level_objects(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    level: &LevelData,
    id: &mut u32,
) {
    for obj in &level.game_objects {
        let scale = &obj.transform.scale;
        let mesh = match obj.mesh_type {
            MeshType::Cube => meshes.add(Cuboid::new(scale[0], scale[1], scale[2])),
            MeshType::Sphere => meshes.add(Sphere::new(scale[0]).mesh().ico(5).unwrap()),
            MeshType::Cylinder => meshes.add(Cylinder::new(scale[0], scale[1])),
            MeshType::Plane => meshes.add(Cuboid::new(scale[0], 0.1, scale[2])),
        };

        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            obj.transform.rotation[0].to_radians(),
            obj.transform.rotation[1].to_radians(),
            obj.transform.rotation[2].to_radians(),
        );

        // Extract material data with defaults
        let material_data = obj.material.as_ref();
        
        // Handle color (support both RGB and RGBA)
        let color = if let Some(color_vec) = material_data.and_then(|m| m.color.as_ref()) {
            if color_vec.len() >= 3 {
                [color_vec[0], color_vec[1], color_vec[2]]
            } else {
                [0.8, 0.8, 0.8]
            }
        } else {
            [0.8, 0.8, 0.8]
        };
        
        let metallic = material_data
            .and_then(|m| m.metallic)
            .unwrap_or(0.0);
        let roughness = material_data
            .and_then(|m| m.roughness)
            .unwrap_or(0.5);
        let emissive = material_data
            .and_then(|m| m.emissive)
            .unwrap_or([0.0, 0.0, 0.0]);

        let mut material = StandardMaterial {
            base_color: Color::srgb(color[0], color[1], color[2]),
            metallic,
            perceptual_roughness: roughness,
            emissive: Color::srgb(emissive[0], emissive[1], emissive[2]).into(),
            ..default()
        };

        // Handle alpha mode if specified
        if let Some(alpha_mode_str) = material_data.and_then(|m| m.alpha_mode.as_ref()) {
            material.alpha_mode = match alpha_mode_str.as_str() {
                "Opaque" => bevy::prelude::AlphaMode::Opaque,
                "Mask" => bevy::prelude::AlphaMode::Mask(0.5),
                "Blend" => bevy::prelude::AlphaMode::Blend,
                _ => bevy::prelude::AlphaMode::Opaque,
            };
        }

        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(materials.add(material)),
            bevy::prelude::Transform::from_translation(Vec3::new(
                obj.transform.position[0], 
                obj.transform.position[1], 
                obj.transform.position[2]
            ))
                .with_rotation(rotation)
                .with_scale(Vec3::ONE),
            GameObjectId((*id).into()),
        ));
        *id += 1;
    }
}

/// System to swap render target buffers for double buffering
/// This runs AFTER rendering to ensure the camera always renders to the write buffer
/// while GPUI reads from the read buffer
pub fn swap_render_buffers_system(
    shared_textures: Res<SharedTexturesResource>,
    mut camera_query: Query<&mut Camera, With<MainCamera>>,
) {
    // Get the shared textures
    let textures = match shared_textures.0.lock().ok().and_then(|l| l.as_ref().cloned()) {
        Some(t) => t,
        None => return,
    };

    // Swap the buffer indices atomically
    let old_write = textures.write_index.load(std::sync::atomic::Ordering::Acquire);
    let old_read = textures.read_index.load(std::sync::atomic::Ordering::Acquire);
    
    // Swap: write becomes read, read becomes write
    textures.write_index.store(old_read, std::sync::atomic::Ordering::Release);
    textures.read_index.store(old_write, std::sync::atomic::Ordering::Release);
    
    let new_write = textures.write_index.load(std::sync::atomic::Ordering::Acquire);
    
    // Increment frame counter
    textures.frame_number.fetch_add(1, std::sync::atomic::Ordering::Release);
    
    // Update camera target to render to the new write buffer
    for mut camera in camera_query.iter_mut() {
        let new_target_handle = textures.textures[new_write].clone();
        camera.target = bevy::camera::RenderTarget::Image(new_target_handle.into());
        
        // Log every 120 frames (once per second at 120 FPS)
        static FRAME_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let frame = FRAME_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

// Debug system to track rendering
pub fn debug_rendering_system(
    _query: Query<&Camera, With<MainCamera>>,
    mut _counter: Local<u32>,
) {
    // Any debug info can be printed here
}
