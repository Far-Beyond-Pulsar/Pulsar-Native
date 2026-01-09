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

    // Arena level with platforms, ramps, hallways, and varied geometry
    tracing::debug!("[BEVY] 🎨 Spawning arena level...");
    let mut id = 1;
    
    // === GROUND FLOOR ===
    // Main floor
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(40.0, 0.2, 40.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.25, 0.25, 0.28),
            metallic: 0.1,
            perceptual_roughness: 0.85,
            reflectance: 0.15,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.1, 0.0),
        GameObjectId(id),
    ));
    id += 1;

    // === CENTRAL STRUCTURE ===
    // Central tower base
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(2.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.4, 0.35, 0.45),
            metallic: 0.3,
            perceptual_roughness: 0.6,
            reflectance: 0.3,
            ..default()
        })),
        Transform::from_xyz(0.0, 0.5, 0.0),
        GameObjectId(id),
    ));
    id += 1;

    // Central tower mid
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(1.5, 2.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.5, 0.4, 0.5),
            metallic: 0.4,
            perceptual_roughness: 0.5,
            reflectance: 0.4,
            ..default()
        })),
        Transform::from_xyz(0.0, 2.0, 0.0),
        GameObjectId(id),
    ));
    id += 1;

    // Central tower top platform
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(4.0, 0.3, 4.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.6, 0.45, 0.55),
            metallic: 0.5,
            perceptual_roughness: 0.4,
            reflectance: 0.5,
            ..default()
        })),
        Transform::from_xyz(0.0, 3.15, 0.0),
        GameObjectId(id),
    ));
    id += 1;

    // === CORNER PLATFORMS (4 corners) ===
    let platform_positions = [
        (-10.0, -10.0), (10.0, -10.0), (-10.0, 10.0), (10.0, 10.0)
    ];
    let platform_colors = [
        Color::srgb(0.7, 0.3, 0.3),  // Red
        Color::srgb(0.3, 0.5, 0.8),  // Blue
        Color::srgb(0.3, 0.7, 0.4),  // Green
        Color::srgb(0.8, 0.7, 0.3),  // Yellow
    ];

    for (i, &(x, z)) in platform_positions.iter().enumerate() {
        // Platform base
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(5.0, 0.4, 5.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: platform_colors[i],
                metallic: 0.3,
                perceptual_roughness: 0.6,
                reflectance: 0.3,
                ..default()
            })),
            Transform::from_xyz(x, 1.5, z),
            GameObjectId(id),
        ));
        id += 1;

        // Support pillar
        commands.spawn((
            Mesh3d(meshes.add(Cylinder::new(0.8, 3.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.35, 0.35, 0.4),
                metallic: 0.2,
                perceptual_roughness: 0.7,
                reflectance: 0.2,
                ..default()
            })),
            Transform::from_xyz(x, 0.0, z),
            GameObjectId(id),
        ));
        id += 1;
    }

    // === RAMPS (connecting center to corners) ===
    let ramp_configs = [
        (-5.0, 0.75, -5.0, 0.785),   // NW
        (5.0, 0.75, -5.0, -0.785),   // NE
        (-5.0, 0.75, 5.0, 2.356),    // SW
        (5.0, 0.75, 5.0, -2.356),    // SE
    ];

    for (i, &(x, y, z, rot_y)) in ramp_configs.iter().enumerate() {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(6.0, 0.2, 2.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.4, 0.4, 0.45),
                metallic: 0.2,
                perceptual_roughness: 0.75,
                reflectance: 0.25,
                ..default()
            })),
            Transform::from_xyz(x, y, z)
                .with_rotation(Quat::from_rotation_y(rot_y) * Quat::from_rotation_z(0.3)),
            GameObjectId(id),
        ));
        id += 1;
    }

    // === HALLWAYS (N, S, E, W) ===
    // North hallway
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(3.0, 2.5, 8.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.35, 0.4),
            metallic: 0.15,
            perceptual_roughness: 0.8,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_xyz(0.0, 1.25, -15.0),
        GameObjectId(id),
    ));
    id += 1;

    // South hallway
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(3.0, 2.5, 8.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.35, 0.4),
            metallic: 0.15,
            perceptual_roughness: 0.8,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_xyz(0.0, 1.25, 15.0),
        GameObjectId(id),
    ));
    id += 1;

    // East hallway
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(8.0, 2.5, 3.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.35, 0.4),
            metallic: 0.15,
            perceptual_roughness: 0.8,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_xyz(15.0, 1.25, 0.0),
        GameObjectId(id),
    ));
    id += 1;

    // West hallway
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(8.0, 2.5, 3.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.35, 0.4),
            metallic: 0.15,
            perceptual_roughness: 0.8,
            reflectance: 0.2,
            ..default()
        })),
        Transform::from_xyz(-15.0, 1.25, 0.0),
        GameObjectId(id),
    ));
    id += 1;

    // === CYLINDER PILLARS (decorative around arena) ===
    let pillar_positions = [
        (-15.0, -15.0), (0.0, -15.0), (15.0, -15.0),
        (-15.0, 0.0), (15.0, 0.0),
        (-15.0, 15.0), (0.0, 15.0), (15.0, 15.0),
    ];

    for &(x, z) in pillar_positions.iter() {
        commands.spawn((
            Mesh3d(meshes.add(Cylinder::new(0.6, 4.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.45, 0.5),
                metallic: 0.6,
                perceptual_roughness: 0.3,
                reflectance: 0.6,
                ..default()
            })),
            Transform::from_xyz(x, 2.0, z),
            GameObjectId(id),
        ));
        id += 1;
    }

    // === SMALL ELEVATED PLATFORMS ===
    let small_platform_configs = [
        (-5.0, 3.0, 0.0, 0.0),
        (5.0, 3.0, 0.0, 0.0),
        (0.0, 3.0, -5.0, 0.0),
        (0.0, 3.0, 5.0, 0.0),
    ];

    for &(x, y, z, _) in small_platform_configs.iter() {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(2.0, 0.2, 2.0))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.6, 0.5, 0.6),
                metallic: 0.4,
                perceptual_roughness: 0.5,
                reflectance: 0.4,
                ..default()
            })),
            Transform::from_xyz(x, y, z),
            GameObjectId(id),
        ));
        id += 1;

        // Support beam
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(0.3, 6.0, 0.3))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.4, 0.4, 0.45),
                metallic: 0.3,
                perceptual_roughness: 0.6,
                reflectance: 0.3,
                ..default()
            })),
            Transform::from_xyz(x, 0.0, z),
            GameObjectId(id),
        ));
        id += 1;
    }

    // === DECORATIVE GEOMETRY ===
    // Floating cubes (various sizes and rotations)
    let cube_configs = [
        (-7.0, 4.0, -7.0, 0.5, 0.8),
        (7.0, 4.5, -7.0, 0.7, 0.7),
        (-7.0, 4.2, 7.0, 0.6, 0.9),
        (7.0, 4.8, 7.0, 0.8, 0.6),
    ];

    for &(x, y, z, size, rot) in cube_configs.iter() {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(size, size, size))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.7, 0.5, 0.7),
                metallic: 0.7,
                perceptual_roughness: 0.3,
                reflectance: 0.7,
                ..default()
            })),
            Transform::from_xyz(x, y, z).with_rotation(Quat::from_rotation_y(rot)),
            GameObjectId(id),
        ));
        id += 1;
    }

    // Spheres (various positions)
    let sphere_configs = [
        (-12.0, 5.0, 0.0, 0.5),
        (12.0, 5.0, 0.0, 0.5),
        (0.0, 5.5, -12.0, 0.6),
        (0.0, 5.5, 12.0, 0.6),
    ];

    for &(x, y, z, radius) in sphere_configs.iter() {
        commands.spawn((
            Mesh3d(meshes.add(Sphere::new(radius))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.8, 0.6, 0.3),
                metallic: 0.9,
                perceptual_roughness: 0.1,
                reflectance: 0.9,
                ..default()
            })),
            Transform::from_xyz(x, y, z),
            GameObjectId(id),
        ));
        id += 1;
    }

    // === PERIMETER WALLS ===
    let wall_positions = [
        (0.0, 2.0, -19.5, 40.0, 4.0, 0.5),   // North
        (0.0, 2.0, 19.5, 40.0, 4.0, 0.5),    // South
        (-19.5, 2.0, 0.0, 0.5, 4.0, 40.0),   // West
        (19.5, 2.0, 0.0, 0.5, 4.0, 40.0),    // East
    ];

    for &(x, y, z, w, h, d) in wall_positions.iter() {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(w, h, d))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.3, 0.3, 0.35),
                metallic: 0.1,
                perceptual_roughness: 0.9,
                reflectance: 0.1,
                ..default()
            })),
            Transform::from_xyz(x, y, z),
            GameObjectId(id),
        ));
        id += 1;
    }

    tracing::debug!("[BEVY] ✅ Arena level created with {} objects", id - 1);

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
    tracing::debug!("[BEVY] 🏟️  Complex arena level loaded");
    tracing::debug!("[BEVY] 🏗️  Multiple platforms, ramps, hallways, and decorative geometry");
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
