//! Scene setup - spawns 3D objects, cameras, and lights

use bevy::prelude::*;
use bevy::core_pipeline::tonemapping::Tonemapping;
use crate::subsystems::render::bevy_renderer::core::{MainCamera, GameObjectId, SharedTexturesResource};

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

    // Scene objects - Unreal-style first person template level
    tracing::debug!("[BEVY] 🎨 Spawning default level objects...");
    
    // Floor plane - large ground surface
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(20.0, 0.1, 20.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.3, 0.3),
            metallic: 0.0,
            perceptual_roughness: 0.8,
            reflectance: 0.1,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.5, 0.0),
        GameObjectId(1),
    ));
    tracing::debug!("[BEVY] ✅ Floor plane spawned");

    // Center cube - focal point (red)
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.8, 0.3, 0.3),
            metallic: 0.2,
            perceptual_roughness: 0.5,
            reflectance: 0.3,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.5, 0.0).with_rotation(Quat::from_rotation_y(0.785)), // 45 degrees
        GameObjectId(2),
    ));
    tracing::debug!("[BEVY] ✅ Center cube spawned");

    // Left sphere - metallic blue
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(0.5))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.5, 0.8),
            metallic: 0.8,
            perceptual_roughness: 0.2,
            reflectance: 0.6,
            ..default()
        })),
        Transform::from_xyz(-3.0, 1.0, 0.0),
        GameObjectId(3),
    ));
    tracing::debug!("[BEVY] ✅ Left sphere spawned");

    // Right cylinder - green
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(0.5, 2.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.8, 0.4),
            metallic: 0.1,
            perceptual_roughness: 0.6,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_xyz(3.0, 1.0, 0.0),
        GameObjectId(4),
    ));
    tracing::debug!("[BEVY] ✅ Right cylinder spawned");

    // Back wall
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(8.0, 4.0, 0.5))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.5, 0.5, 0.5),
            metallic: 0.0,
            perceptual_roughness: 0.9,
            reflectance: 0.1,
            ..default()
        })),
        Transform::from_xyz(0.0, 2.0, -5.0),
        GameObjectId(5),
    ));
    tracing::debug!("[BEVY] ✅ Back wall spawned");

    // Decorative cube 1 (left front)
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(0.6, 0.6, 0.6))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.7, 0.4, 0.6),
            metallic: 0.5,
            perceptual_roughness: 0.4,
            reflectance: 0.4,
            ..default()
        })),
        Transform::from_xyz(-5.0, 0.3, 2.0).with_rotation(Quat::from_rotation_y(0.524)), // 30 degrees
        GameObjectId(6),
    ));

    // Decorative cube 2 (left back)
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(0.6, 0.6, 0.6))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.9, 0.7, 0.3),
            metallic: 0.6,
            perceptual_roughness: 0.3,
            reflectance: 0.5,
            ..default()
        })),
        Transform::from_xyz(-4.0, 0.3, 3.0).with_rotation(Quat::from_rotation_y(-0.262)), // -15 degrees
        GameObjectId(7),
    ));

    // Decorative cube 3 (right front)
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(0.6, 0.6, 0.6))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.4, 0.7, 0.8),
            metallic: 0.7,
            perceptual_roughness: 0.25,
            reflectance: 0.6,
            ..default()
        })),
        Transform::from_xyz(5.0, 0.3, 2.0).with_rotation(Quat::from_rotation_y(-0.524)), // -30 degrees
        GameObjectId(8),
    ));

    // Decorative cube 4 (right back)
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(0.6, 0.6, 0.6))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.5, 0.8, 0.5),
            metallic: 0.4,
            perceptual_roughness: 0.5,
            reflectance: 0.3,
            ..default()
        })),
        Transform::from_xyz(4.0, 0.3, 3.0).with_rotation(Quat::from_rotation_y(0.262)), // 15 degrees
        GameObjectId(9),
    ));

    // Platform left
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.2, 0.5, 1.2))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.35, 0.4),
            metallic: 0.1,
            perceptual_roughness: 0.7,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_xyz(-2.0, 0.5, 4.0),
        GameObjectId(10),
    ));

    // Platform right
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.2, 0.5, 1.2))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.35, 0.4),
            metallic: 0.1,
            perceptual_roughness: 0.7,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_xyz(2.0, 0.5, 4.0),
        GameObjectId(11),
    ));
    tracing::debug!("[BEVY] ✅ All decorative objects spawned");

    // Primary directional light (sun)
    commands.spawn((
        DirectionalLight {
            color: Color::WHITE,
            illuminance: 25000.0, // Bright sunlight
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    
    // Fill light (softer, from opposite side)
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(0.9, 0.95, 1.0), // Slightly blue fill
            illuminance: 8000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(-4.0, 6.0, -4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    
    // Ambient light for overall scene brightness
    commands.insert_resource(AmbientLight {
        color: Color::WHITE,
        brightness: 500.0, // Subtle ambient
        affects_lightmapped_meshes: true,
    });
    
    tracing::debug!("[BEVY] ✅ PBR lighting enabled with 2 directional lights + ambient");

    tracing::debug!("[BEVY] ✅ Scene ready!");
    tracing::debug!("[BEVY] 🎨 Default level loaded - Unreal-style first person template");
    tracing::debug!("[BEVY] 🏗️  11 static objects spawned");
    tracing::debug!("[BEVY] 💡 PBR lighting with 2-point lighting + ambient");
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
