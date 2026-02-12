use glam::{Vec3, Mat4};
use helio_feature_procedural_shadows::{LightConfig, LightType};
use helio_render::TransformUniforms;

/// Represents a mesh instance in the scene
#[derive(Clone)]
pub struct MeshInstance {
    pub transform: Mat4,
    pub mesh_type: MeshType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshType {
    Cube,
    Plane,
    Sphere,
}

/// Builder for creating complex game scenes
pub struct SceneBuilder {
    pub meshes: Vec<MeshInstance>,
    pub lights: Vec<LightConfig>,
}

impl SceneBuilder {
    pub fn new() -> Self {
        Self {
            meshes: Vec::new(),
            lights: Vec::new(),
        }
    }

    /// Create a REAL game level - underground research facility (inspired by Portal/Half-Life)
    /// This is a multi-room facility with proper scale, flow, and storytelling
    pub fn build_mixed_environment() -> Self {
        let mut scene = Self::new();

        // === ENTRANCE HALL - First impressive space ===
        scene.build_entrance_hall(Vec3::new(0.0, 0.0, 0.0));

        // === TESTING CHAMBER - Large cubic room with observation windows ===
        scene.build_testing_chamber(Vec3::new(0.0, 0.0, -20.0));

        // === REACTOR CORE - Vertical shaft with platforms ===
        scene.build_reactor_core(Vec3::new(20.0, 0.0, -20.0));

        // === MAINTENANCE CORRIDORS - Connecting passages ===
        scene.build_corridor(Vec3::new(0.0, 0.0, -10.0), 20.0, 0.0); // North-South
        scene.build_corridor(Vec3::new(10.0, 0.0, -20.0), 20.0, 90.0); // East-West

        // === OBSERVATION DECK - Elevated overlook ===
        scene.build_observation_deck(Vec3::new(-15.0, 8.0, -20.0));

        // === LOADING BAY - Large cargo area ===
        scene.build_loading_bay(Vec3::new(20.0, 0.0, 5.0));

        // === EMERGENCY STAIRWELL - Vertical connection ===
        scene.build_stairwell(Vec3::new(-10.0, 0.0, 0.0));

        // === Dramatic facility lighting ===
        scene.add_facility_lighting();

        scene
    }

    /// Entrance hall - impressive first space with high ceiling
    fn build_entrance_hall(&mut self, base_pos: Vec3) {
        let width = 12.0;
        let height = 10.0;
        let depth = 8.0;

        // Main floor
        self.add_plane(base_pos + Vec3::new(0.0, 0.0, 0.0), width, depth);

        // Feature wall with reception desk
        self.add_wall(
            base_pos + Vec3::new(0.0, height / 2.0, depth / 2.0),
            width,
            height,
            0.5,
        );

        // Reception desk
        self.add_cube(base_pos + Vec3::new(0.0, 1.0, depth / 2.0 - 2.0), 6.0, 2.0, 1.5);

        // Pillars (structural)
        for i in 0..4 {
            let x = if i % 2 == 0 { -width / 3.0 } else { width / 3.0 };
            let z = if i < 2 { -depth / 3.0 } else { depth / 3.0 };
            self.add_cube(
                base_pos + Vec3::new(x, height / 2.0, z),
                1.0,
                height,
                1.0,
            );
        }

        // Seating area
        self.add_cube(base_pos + Vec3::new(-4.0, 0.4, 0.0), 1.5, 0.8, 1.5);
        self.add_cube(base_pos + Vec3::new(4.0, 0.4, 0.0), 1.5, 0.8, 1.5);

        // Decorative sphere (company logo?)
        self.add_sphere(base_pos + Vec3::new(0.0, 2.0, -2.0), 1.0);
    }

    /// Testing chamber - large open room like Portal test chambers
    fn build_testing_chamber(&mut self, base_pos: Vec3) {
        let width = 16.0;
        let height = 12.0;
        let depth = 16.0;

        // Floor with grid pattern (using multiple planes)
        for x in -2..3 {
            for z in -2..3 {
                if (x + z) % 2 == 0 {
                    self.add_plane(
                        base_pos + Vec3::new(x as f32 * 3.0, 0.01, z as f32 * 3.0),
                        2.8,
                        2.8,
                    );
                }
            }
        }

        // Walls
        self.add_wall(
            base_pos + Vec3::new(-width / 2.0, height / 2.0, 0.0),
            0.5,
            height,
            depth,
        );
        self.add_wall(
            base_pos + Vec3::new(width / 2.0, height / 2.0, 0.0),
            0.5,
            height,
            depth,
        );
        self.add_wall(
            base_pos + Vec3::new(0.0, height / 2.0, -depth / 2.0),
            width,
            height,
            0.5,
        );
        self.add_wall(
            base_pos + Vec3::new(0.0, height / 2.0, depth / 2.0),
            width,
            height,
            0.5,
        );

        // Testing apparatus in center
        self.add_cube(base_pos + Vec3::new(0.0, 2.0, 0.0), 3.0, 4.0, 3.0);
        self.add_sphere(base_pos + Vec3::new(0.0, 5.0, 0.0), 1.5);

        // Observation window (thin wall section)
        self.add_cube(
            base_pos + Vec3::new(-width / 2.0 + 0.3, 6.0, 0.0),
            0.1,
            3.0,
            5.0,
        );

        // Test platforms
        for i in 0..4 {
            let angle = (i as f32) * std::f32::consts::PI / 2.0;
            let radius = 5.0;
            let x = angle.cos() * radius;
            let z = angle.sin() * radius;
            
            self.add_cube(
                base_pos + Vec3::new(x, 0.5, z),
                2.0,
                1.0,
                2.0,
            );
        }
    }

    /// Reactor core - dramatic vertical shaft
    fn build_reactor_core(&mut self, base_pos: Vec3) {
        let radius = 8.0;
        let height = 20.0;

        // Central reactor column
        self.add_cube(
            base_pos + Vec3::new(0.0, height / 2.0, 0.0),
            2.0,
            height,
            2.0,
        );

        // Reactor sphere at top
        self.add_sphere(base_pos + Vec3::new(0.0, height - 2.0, 0.0), 2.5);

        // Circular platforms at different heights
        for level in 0..4 {
            let platform_y = level as f32 * 5.0;
            let num_segments = 8;
            
            for i in 0..num_segments {
                let angle = (i as f32) * std::f32::consts::PI * 2.0 / num_segments as f32;
                let x = angle.cos() * radius;
                let z = angle.sin() * radius;
                
                // Platform segment
                self.add_cube(
                    base_pos + Vec3::new(x, platform_y, z),
                    2.0,
                    0.3,
                    1.5,
                );
                
                // Railing
                self.add_cube(
                    base_pos + Vec3::new(x, platform_y + 0.6, z),
                    0.1,
                    1.0,
                    1.5,
                );
            }
        }

        // Support beams
        for i in 0..8 {
            let angle = (i as f32) * std::f32::consts::PI * 2.0 / 8.0;
            let x = angle.cos() * radius;
            let z = angle.sin() * radius;
            
            self.add_cube(
                base_pos + Vec3::new(x, height / 2.0, z),
                0.5,
                height,
                0.5,
            );
        }
    }

    /// Corridor - connecting hallway
    fn build_corridor(&mut self, base_pos: Vec3, length: f32, rotation_deg: f32) {
        let width = 3.0;
        let height = 4.0;
        
        let rot_rad = rotation_deg.to_radians();
        let forward = Vec3::new(rot_rad.sin(), 0.0, rot_rad.cos());
        let right = Vec3::new(forward.z, 0.0, -forward.x);

        // Floor
        self.add_plane(base_pos, width, length);

        // Walls
        self.add_wall(
            base_pos + right * (width / 2.0) + Vec3::new(0.0, height / 2.0, 0.0),
            0.3,
            height,
            length,
        );
        self.add_wall(
            base_pos - right * (width / 2.0) + Vec3::new(0.0, height / 2.0, 0.0),
            0.3,
            height,
            length,
        );

        // Ceiling
        self.add_plane_horizontal(
            base_pos + Vec3::new(0.0, height, 0.0),
            width,
            length,
        );

        // Wall lights every 4 meters
        let num_lights = (length / 4.0) as i32;
        for i in 0..num_lights {
            let z = -length / 2.0 + (i as f32 * 4.0);
            
            self.add_cube(
                base_pos + right * (width / 2.0 - 0.2) + Vec3::new(0.0, 3.0, z),
                0.3,
                0.2,
                0.5,
            );
        }

        // Pipes along ceiling
        self.add_cube(
            base_pos + right * 0.8 + Vec3::new(0.0, height - 0.2, 0.0),
            0.15,
            0.15,
            length,
        );
    }

    /// Observation deck - elevated viewing platform
    fn build_observation_deck(&mut self, base_pos: Vec3) {
        let width = 6.0;
        let depth = 4.0;
        let height = 3.0;

        // Platform floor
        self.add_cube(base_pos, width, 0.3, depth);

        // Glass railing
        self.add_cube(
            base_pos + Vec3::new(width / 2.0, 0.6, 0.0),
            0.05,
            1.0,
            depth,
        );

        // Control console
        self.add_cube(
            base_pos + Vec3::new(-width / 4.0, 0.8, 0.0),
            1.5,
            1.5,
            1.0,
        );

        // Screens (vertical thin cubes)
        for i in 0..3 {
            self.add_cube(
                base_pos + Vec3::new(-width / 4.0, 2.0, -1.0 + i as f32 * 0.8),
                0.05,
                0.8,
                0.6,
            );
        }

        // Support pillars from ground
        for i in 0..4 {
            let x = if i % 2 == 0 { -width / 3.0 } else { width / 3.0 };
            let z = if i < 2 { -depth / 3.0 } else { depth / 3.0 };
            
            self.add_cube(
                base_pos + Vec3::new(x, -4.0, z),
                0.4,
                8.0,
                0.4,
            );
        }
    }

    /// Loading bay - large cargo area
    fn build_loading_bay(&mut self, base_pos: Vec3) {
        let width = 15.0;
        let depth = 12.0;
        let height = 6.0;

        // Floor
        self.add_plane(base_pos, width, depth);

        // Cargo containers
        for x in 0..3 {
            for z in 0..2 {
                let pos = base_pos + Vec3::new(
                    -width / 3.0 + x as f32 * 4.0,
                    1.2,
                    -depth / 3.0 + z as f32 * 4.0,
                );
                self.add_cube(pos, 2.5, 2.4, 2.0);
            }
        }

        // Loading dock platform
        self.add_cube(
            base_pos + Vec3::new(0.0, 0.6, depth / 2.0),
            width,
            1.2,
            2.0,
        );

        // Overhead crane rail
        self.add_cube(
            base_pos + Vec3::new(0.0, height - 0.5, 0.0),
            0.4,
            0.4,
            depth,
        );

        // Crane hook
        self.add_cube(
            base_pos + Vec3::new(2.0, height - 2.0, 0.0),
            0.3,
            2.0,
            0.3,
        );
    }

    /// Stairwell - vertical circulation
    fn build_stairwell(&mut self, base_pos: Vec3) {
        let width = 3.0;
        let depth = 4.0;
        let total_height = 15.0;
        let stairs = 5;

        // Stairwell shaft walls
        self.add_wall(
            base_pos + Vec3::new(-width / 2.0, total_height / 2.0, 0.0),
            0.3,
            total_height,
            depth,
        );
        self.add_wall(
            base_pos + Vec3::new(width / 2.0, total_height / 2.0, 0.0),
            0.3,
            total_height,
            depth,
        );

        // Stairs (simplified as angled platforms)
        for i in 0..stairs {
            let y = i as f32 * 3.0;
            let z_offset = if i % 2 == 0 { 0.0 } else { -2.0 };
            
            // Landing
            self.add_cube(
                base_pos + Vec3::new(0.0, y, z_offset),
                width - 0.5,
                0.2,
                2.0,
            );
            
            // Stair steps (crude representation)
            for step in 0..4 {
                self.add_cube(
                    base_pos + Vec3::new(
                        0.0,
                        y + 0.3 + step as f32 * 0.3,
                        z_offset + 1.0 + step as f32 * 0.5,
                    ),
                    width - 0.5,
                    0.15,
                    0.4,
                );
            }
        }

        // Emergency lights
        for i in 0..stairs {
            let y = i as f32 * 3.0 + 1.5;
            self.add_cube(
                base_pos + Vec3::new(width / 2.0 - 0.2, y, 0.0),
                0.2,
                0.2,
                0.3,
            );
        }
    }

    /// Facility lighting - purposeful, atmospheric lighting
    fn add_facility_lighting(&mut self) {
        // ENTRANCE HALL - Warm welcoming light
        self.lights.push(LightConfig {
            light_type: LightType::Spot {
                inner_angle: 30.0_f32.to_radians(),
                outer_angle: 50.0_f32.to_radians(),
            },
            position: Vec3::new(0.0, 8.0, 0.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 2.0,
            color: Vec3::new(1.0, 0.95, 0.85), // Warm white
            attenuation_radius: 15.0,
            attenuation_falloff: 2.0,
        });

        // TESTING CHAMBER - Clinical bright white overhead
        self.lights.push(LightConfig {
            light_type: LightType::Spot {
                inner_angle: 40.0_f32.to_radians(),
                outer_angle: 60.0_f32.to_radians(),
            },
            position: Vec3::new(0.0, 10.0, -20.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 2.5,
            color: Vec3::new(0.95, 0.98, 1.0), // Cool clinical white
            attenuation_radius: 20.0,
            attenuation_falloff: 1.5,
        });

        // REACTOR CORE - Dramatic blue glow from below
        self.lights.push(LightConfig {
            light_type: LightType::Point,
            position: Vec3::new(20.0, 2.0, -20.0),
            direction: Vec3::new(0.0, 1.0, 0.0),
            intensity: 2.0,
            color: Vec3::new(0.2, 0.6, 1.0), // Bright blue
            attenuation_radius: 18.0,
            attenuation_falloff: 2.0,
        });

        // REACTOR TOP - Pulsing core
        self.lights.push(LightConfig {
            light_type: LightType::Point,
            position: Vec3::new(20.0, 18.0, -20.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 1.8,
            color: Vec3::new(0.4, 0.8, 1.0), // Electric blue
            attenuation_radius: 15.0,
            attenuation_falloff: 2.2,
        });

        // CORRIDOR LIGHTS - Evenly spaced cool lights
        for i in 0..5 {
            let z = -5.0 + i as f32 * 2.5;
            self.lights.push(LightConfig {
                light_type: LightType::Point,
                position: Vec3::new(0.0, 3.5, z),
                direction: Vec3::new(0.0, -1.0, 0.0),
                intensity: 0.8,
                color: Vec3::new(0.85, 0.9, 1.0), // Fluorescent
                attenuation_radius: 5.0,
                attenuation_falloff: 3.0,
            });
        }

        // OBSERVATION DECK - Control panel glow
        self.lights.push(LightConfig {
            light_type: LightType::Point,
            position: Vec3::new(-15.0, 9.0, -20.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 1.2,
            color: Vec3::new(0.3, 1.0, 0.5), // Green monitors
            attenuation_radius: 6.0,
            attenuation_falloff: 2.5,
        });

        // LOADING BAY - Industrial work lights
        self.lights.push(LightConfig {
            light_type: LightType::Spot {
                inner_angle: 35.0_f32.to_radians(),
                outer_angle: 55.0_f32.to_radians(),
            },
            position: Vec3::new(20.0, 5.0, 5.0),
            direction: Vec3::new(0.0, -1.0, 0.0),
            intensity: 2.2,
            color: Vec3::new(1.0, 0.9, 0.7), // Yellow work light
            attenuation_radius: 18.0,
            attenuation_falloff: 2.0,
        });

        // STAIRWELL - Emergency red lighting
        for i in 0..3 {
            self.lights.push(LightConfig {
                light_type: LightType::Point,
                position: Vec3::new(-10.0, i as f32 * 5.0 + 1.5, 0.0),
                direction: Vec3::new(0.0, -1.0, 0.0),
                intensity: 0.7,
                color: Vec3::new(1.0, 0.2, 0.1), // Emergency red
                attenuation_radius: 4.0,
                attenuation_falloff: 3.5,
            });
        }

        // ACCENT LIGHTS - Atmospheric fill
        // Entrance logo light
        self.lights.push(LightConfig {
            light_type: LightType::Point,
            position: Vec3::new(0.0, 3.0, -2.0),
            direction: Vec3::new(0.0, 0.0, -1.0),
            intensity: 1.0,
            color: Vec3::new(0.4, 0.7, 1.0), // Company blue
            attenuation_radius: 5.0,
            attenuation_falloff: 2.5,
        });
    }

    // === PRIMITIVE HELPERS ===

    fn add_cube(&mut self, pos: Vec3, width: f32, height: f32, depth: f32) {
        let transform = Mat4::from_translation(pos) 
            * Mat4::from_scale(Vec3::new(width, height, depth));
        self.meshes.push(MeshInstance {
            transform,
            mesh_type: MeshType::Cube,
        });
    }

    fn add_sphere(&mut self, pos: Vec3, radius: f32) {
        let transform = Mat4::from_translation(pos)
            * Mat4::from_scale(Vec3::splat(radius * 2.0));
        self.meshes.push(MeshInstance {
            transform,
            mesh_type: MeshType::Sphere,
        });
    }

    fn add_plane(&mut self, pos: Vec3, width: f32, depth: f32) {
        let transform = Mat4::from_translation(pos)
            * Mat4::from_scale(Vec3::new(width, 1.0, depth));
        self.meshes.push(MeshInstance {
            transform,
            mesh_type: MeshType::Plane,
        });
    }

    fn add_plane_horizontal(&mut self, pos: Vec3, width: f32, depth: f32) {
        self.add_plane(pos, width, depth);
    }

    fn add_wall(&mut self, pos: Vec3, width: f32, height: f32, thickness: f32) {
        self.add_cube(pos, width, height, thickness);
    }

    /// Convert meshes to TransformUniforms for rendering
    pub fn get_transforms(&self, mesh_type: MeshType) -> Vec<TransformUniforms> {
        self.meshes
            .iter()
            .filter(|m| m.mesh_type == mesh_type)
            .map(|m| TransformUniforms::from_matrix(m.transform))
            .collect()
    }
}
