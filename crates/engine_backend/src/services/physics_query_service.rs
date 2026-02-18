//! Physics query service for raycasting and spatial queries.
//! 
//! Provides viewport picking and gizmo interaction using Rapier3d QueryPipeline.

use std::sync::{Arc, Mutex};
use rapier3d::prelude::*;
use rapier3d::na::{Point3, Vector3, Isometry3};
use glam::{Vec3, Vec2};
use crate::scene::{SceneDb, ObjectId};

/// Result of a raycast query
#[derive(Debug, Clone)]
pub struct RaycastHit {
    pub object_id: ObjectId,
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

/// Identifies gizmo colliders vs scene colliders
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColliderTag {
    SceneObject,
    GizmoAxisX,
    GizmoAxisY,
    GizmoAxisZ,
}

/// Service for physics-based queries (raycasting, overlap tests)
pub struct PhysicsQueryService {
    collider_set: Arc<Mutex<ColliderSet>>,
    rigid_body_set: Arc<Mutex<RigidBodySet>>,
    island_manager: Arc<Mutex<IslandManager>>,
    
    /// Maps collider handles back to object IDs
    collider_to_object: Arc<Mutex<std::collections::HashMap<ColliderHandle, ObjectId>>>,
    
    /// Maps collider handles to gizmo axis tags
    collider_to_gizmo: Arc<Mutex<std::collections::HashMap<ColliderHandle, ColliderTag>>>,
}

impl PhysicsQueryService {
    pub fn new(
        collider_set: Arc<Mutex<ColliderSet>>,
        rigid_body_set: Arc<Mutex<RigidBodySet>>,
    ) -> Self {
        Self::new_with_island_manager(collider_set, rigid_body_set, Arc::new(Mutex::new(IslandManager::new())))
    }
    
    pub fn new_with_island_manager(
        collider_set: Arc<Mutex<ColliderSet>>,
        rigid_body_set: Arc<Mutex<RigidBodySet>>,
        island_manager: Arc<Mutex<IslandManager>>,
    ) -> Self {
        Self {
            collider_set,
            rigid_body_set,
            island_manager,
            collider_to_object: Arc::new(Mutex::new(std::collections::HashMap::new())),
            collider_to_gizmo: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// Perform a raycast from origin in direction
    /// 
    /// Note: This creates a temporary QueryPipeline each call. For better performance
    /// when doing multiple queries, consider batching or using PhysicsEngine's query pipeline.
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<RaycastHit> {
        tracing::info!("[PHYSICS] 🎯 Raycast from [{:.2}, {:.2}, {:.2}] dir [{:.2}, {:.2}, {:.2}]", 
            origin.x, origin.y, origin.z, direction.x, direction.y, direction.z);
        
        let ray = Ray::new(
            Point3::new(origin.x, origin.y, origin.z).into(),
            Vector3::new(direction.x, direction.y, direction.z).into(),
        );

        let rigid_body_set = self.rigid_body_set.lock().unwrap();
        let collider_set = self.collider_set.lock().unwrap();
        let collider_to_object = self.collider_to_object.lock().unwrap();
        
        tracing::info!("[PHYSICS] Checking {} colliders", collider_set.len());

        let filter = QueryFilter::default();

        // Perform raycast directly on collider set
        let mut closest_hit: Option<(ColliderHandle, f32)> = None;
        
        let mut tested_count = 0;
        for (handle, collider) in collider_set.iter() {
            // Only test colliders that are registered as scene objects
            if !collider_to_object.contains_key(&handle) {
                continue;
            }
            tested_count += 1;
            
            if let Some(toi) = collider.shape().cast_ray(
                collider.position(),
                &ray,
                max_distance,
                true,
            ) {
                tracing::debug!("[PHYSICS] Hit collider at distance {:.2}", toi);
                if closest_hit.is_none() || toi < closest_hit.unwrap().1 {
                    closest_hit = Some((handle, toi));
                }
            }
        }
        
        tracing::info!("[PHYSICS] Tested {} scene object colliders, closest_hit: {}", 
            tested_count, closest_hit.is_some());

        closest_hit.and_then(|(handle, toi)| {
            // Get object ID for this collider
            let object_id = collider_to_object.get(&handle)?;
            
            // Get hit point and normal
            let hit_point_rapier = ray.point_at(toi);
            let hit_point = Vec3::new(hit_point_rapier.x, hit_point_rapier.y, hit_point_rapier.z);

            // Get surface normal from collider
            let collider = collider_set.get(handle)?;
            let shape = collider.shape().as_typed_shape();
            let hit_normal = match shape {
                TypedShape::Ball(_ball) => {
                    let center = collider.position().translation;
                    let to_hit_x = hit_point_rapier.x - center.x;
                    let to_hit_y = hit_point_rapier.y - center.y;
                    let to_hit_z = hit_point_rapier.z - center.z;
                    Vec3::new(to_hit_x, to_hit_y, to_hit_z).normalize()
                }
                _ => {
                    // For other shapes, approximate normal from hit direction
                    -direction
                }
            };

            Some(RaycastHit {
                object_id: object_id.clone(),
                point: hit_point,
                normal: hit_normal,
                distance: toi,
            })
        })
    }

    /// Raycast specifically for gizmo interaction
    /// Returns the gizmo axis that was hit (if any)
    pub fn raycast_gizmo(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<ColliderTag> {
        let ray = Ray::new(
            Point3::new(origin.x, origin.y, origin.z).into(),
            Vector3::new(direction.x, direction.y, direction.z).into(),
        );

        let collider_set = self.collider_set.lock().unwrap();
        let collider_to_gizmo = self.collider_to_gizmo.lock().unwrap();

        // Iterate gizmo colliders and find closest hit
        let mut closest_hit: Option<(ColliderHandle, f32)> = None;
        
        for (handle, _tag) in collider_to_gizmo.iter() {
            if let Some(collider) = collider_set.get(*handle) {
                if let Some(toi) = collider.shape().cast_ray(
                    collider.position(),
                    &ray,
                    max_distance,
                    true,
                ) {
                    if closest_hit.is_none() || toi < closest_hit.unwrap().1 {
                        closest_hit = Some((*handle, toi));
                    }
                }
            }
        }

        closest_hit.and_then(|(handle, _)| {
            collider_to_gizmo.get(&handle).copied()
        })
    }

    /// Register a scene object's collider for raycasting
    pub fn register_scene_collider(&self, handle: ColliderHandle, object_id: ObjectId) {
        let mut mapping = self.collider_to_object.lock().unwrap();
        mapping.insert(handle, object_id);
    }

    /// Register a gizmo collider
    pub fn register_gizmo_collider(&self, handle: ColliderHandle, tag: ColliderTag) {
        let mut mapping = self.collider_to_gizmo.lock().unwrap();
        mapping.insert(handle, tag);
    }

    /// Remove a collider from tracking
    pub fn unregister_collider(&self, handle: ColliderHandle) {
        let mut obj_mapping = self.collider_to_object.lock().unwrap();
        let mut gizmo_mapping = self.collider_to_gizmo.lock().unwrap();
        obj_mapping.remove(&handle);
        gizmo_mapping.remove(&handle);
    }

    /// Sync colliders from SceneDB (recreate all scene colliders)
    pub fn sync_from_scene(&self, scene_db: &crate::scene::SceneDb) {
        let start_count = {
            let obj_mapping = self.collider_to_object.lock().unwrap();
            obj_mapping.len()
        };
        
        // Clear existing scene colliders (but not gizmo colliders)
        {
            let mut obj_mapping = self.collider_to_object.lock().unwrap();
            let mut collider_set = self.collider_set.lock().unwrap();
            let mut island_manager = self.island_manager.lock().unwrap();
            let mut rigid_body_set = self.rigid_body_set.lock().unwrap();
            
            // Remove all scene object colliders
            let handles_to_remove: Vec<ColliderHandle> = obj_mapping.keys().copied().collect();
            for handle in handles_to_remove {
                collider_set.remove(handle, &mut island_manager, &mut rigid_body_set, false);
                obj_mapping.remove(&handle);
            }
        }
        
        let mut created_count = 0;
        
        // Recreate colliders for all visible scene objects
        scene_db.for_each_entry(|entry| {
            use crate::scene::{ObjectType, MeshType};
            if !entry.is_visible() {
                return;
            }
            
            let pos = entry.get_position();
            let scale = entry.get_scale();
            let position = Isometry3::translation(pos[0], pos[1], pos[2]);
            
            // Create collider based on object type
            let shape: Option<SharedShape> = match entry.object_type {
                ObjectType::Mesh(MeshType::Cube) | ObjectType::Mesh(MeshType::Plane) => {
                    let half_extents = Vector3::new(scale[0] * 0.5, scale[1] * 0.5, scale[2] * 0.5);
                    Some(SharedShape::cuboid(half_extents.x, half_extents.y, half_extents.z))
                },
                ObjectType::Mesh(MeshType::Sphere) => {
                    let radius = (scale[0] + scale[1] + scale[2]) / 3.0 * 0.5;
                    Some(SharedShape::ball(radius))
                },
                ObjectType::Mesh(MeshType::Cylinder) => {
                    let radius = (scale[0] + scale[2]) / 2.0 * 0.5;
                    let half_height = scale[1] * 0.5;
                    Some(SharedShape::cylinder(half_height, radius))
                },
                _ => None,
            };
            
            if let Some(shape) = shape {
                let collider = ColliderBuilder::new(shape).position(position.into()).build();
                let mut collider_set = self.collider_set.lock().unwrap();
                let handle = collider_set.insert(collider);
                self.register_scene_collider(handle, entry.id.clone());
                created_count += 1;
                
                tracing::debug!("[PHYSICS] Created collider for object '{}' at pos [{:.2}, {:.2}, {:.2}]", 
                    entry.id, pos[0], pos[1], pos[2]);
            }
        });
    }

    /// Create gizmo colliders for the currently selected object
    /// 
    /// Gizmo colliders are simple shapes positioned at the object's location
    /// and are tagged with axis information for interaction
    pub fn create_gizmo_colliders(
        &self,
        position: Vec3,
        gizmo_type: super::GizmoType,
        scale: f32,
    ) {
        self.clear_gizmo_colliders();

        if gizmo_type == super::GizmoType::None {
            return;
        }

        let mut collider_set = self.collider_set.lock().unwrap();

        match gizmo_type {
            super::GizmoType::Translate => {
                // Create arrow shaft colliders for each axis
                let shaft_length = 1.0 * scale;
                let shaft_radius = 0.05 * scale;

                // X axis (red)
                let x_shape = SharedShape::cylinder(shaft_length / 2.0, shaft_radius);
                let x_pos = Isometry3::new(
                    Vector3::new(position.x + shaft_length / 2.0, position.y, position.z),
                    Vector3::new(0.0, 0.0, std::f32::consts::FRAC_PI_2),
                );
                let x_collider = ColliderBuilder::new(x_shape).position(x_pos.into()).build();
                let x_handle = collider_set.insert(x_collider);
                self.register_gizmo_collider(x_handle, ColliderTag::GizmoAxisX);

                // Y axis (green)
                let y_shape = SharedShape::cylinder(shaft_length / 2.0, shaft_radius);
                let y_pos = Isometry3::new(
                    Vector3::new(position.x, position.y + shaft_length / 2.0, position.z),
                    Vector3::new(0.0, 0.0, 0.0),
                );
                let y_collider = ColliderBuilder::new(y_shape).position(y_pos.into()).build();
                let y_handle = collider_set.insert(y_collider);
                self.register_gizmo_collider(y_handle, ColliderTag::GizmoAxisY);

                // Z axis (blue)
                let z_shape = SharedShape::cylinder(shaft_length / 2.0, shaft_radius);
                let z_pos = Isometry3::new(
                    Vector3::new(position.x, position.y, position.z + shaft_length / 2.0),
                    Vector3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
                );
                let z_collider = ColliderBuilder::new(z_shape).position(z_pos.into()).build();
                let z_handle = collider_set.insert(z_collider);
                self.register_gizmo_collider(z_handle, ColliderTag::GizmoAxisZ);
            }
            super::GizmoType::Rotate => {
                // Create torus colliders for rotation handles
                // For simplicity, use cylinder rings
                let ring_radius = 1.0 * scale;
                let ring_thickness = 0.05 * scale;

                // X axis ring (red) - around X
                let x_shape = SharedShape::cylinder(ring_thickness, ring_radius);
                let x_pos = Isometry3::new(
                    Vector3::new(position.x, position.y, position.z),
                    Vector3::new(0.0, 0.0, std::f32::consts::FRAC_PI_2),
                );
                let x_collider = ColliderBuilder::new(x_shape).position(x_pos.into()).build();
                let x_handle = collider_set.insert(x_collider);
                self.register_gizmo_collider(x_handle, ColliderTag::GizmoAxisX);

                // Y axis ring (green) - around Y
                let y_shape = SharedShape::cylinder(ring_thickness, ring_radius);
                let y_pos = Isometry3::new(
                    Vector3::new(position.x, position.y, position.z),
                    Vector3::new(0.0, 0.0, 0.0),
                );
                let y_collider = ColliderBuilder::new(y_shape).position(y_pos.into()).build();
                let y_handle = collider_set.insert(y_collider);
                self.register_gizmo_collider(y_handle, ColliderTag::GizmoAxisY);

                // Z axis ring (blue) - around Z
                let z_shape = SharedShape::cylinder(ring_thickness, ring_radius);
                let z_pos = Isometry3::new(
                    Vector3::new(position.x, position.y, position.z),
                    Vector3::new(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
                );
                let z_collider = ColliderBuilder::new(z_shape).position(z_pos.into()).build();
                let z_handle = collider_set.insert(z_collider);
                self.register_gizmo_collider(z_handle, ColliderTag::GizmoAxisZ);
            }
            super::GizmoType::Scale => {
                // Similar to translate but with cube endpoints
                let shaft_length = 1.0 * scale;
                let cube_size = 0.15 * scale;

                // X axis
                let x_shape = SharedShape::cuboid(shaft_length / 2.0, cube_size / 2.0, cube_size / 2.0);
                let x_pos = Isometry3::new(
                    Vector3::new(position.x + shaft_length / 2.0, position.y, position.z),
                    Vector3::new(0.0, 0.0, 0.0),
                );
                let x_collider = ColliderBuilder::new(x_shape).position(x_pos.into()).build();
                let x_handle = collider_set.insert(x_collider);
                self.register_gizmo_collider(x_handle, ColliderTag::GizmoAxisX);

                // Y axis
                let y_shape = SharedShape::cuboid(cube_size / 2.0, shaft_length / 2.0, cube_size / 2.0);
                let y_pos = Isometry3::new(
                    Vector3::new(position.x, position.y + shaft_length / 2.0, position.z),
                    Vector3::new(0.0, 0.0, 0.0),
                );
                let y_collider = ColliderBuilder::new(y_shape).position(y_pos.into()).build();
                let y_handle = collider_set.insert(y_collider);
                self.register_gizmo_collider(y_handle, ColliderTag::GizmoAxisY);

                // Z axis
                let z_shape = SharedShape::cuboid(cube_size / 2.0, cube_size / 2.0, shaft_length / 2.0);
                let z_pos = Isometry3::new(
                    Vector3::new(position.x, position.y, position.z + shaft_length / 2.0),
                    Vector3::new(0.0, 0.0, 0.0),
                );
                let z_collider = ColliderBuilder::new(z_shape).position(z_pos.into()).build();
                let z_handle = collider_set.insert(z_collider);
                self.register_gizmo_collider(z_handle, ColliderTag::GizmoAxisZ);
            }
            super::GizmoType::None => {}
        }

        // Update complete
        drop(collider_set);
    }

    /// Remove all gizmo colliders
    pub fn clear_gizmo_colliders(&self) {
        let mut collider_set = self.collider_set.lock().unwrap();
        let mut gizmo_mapping = self.collider_to_gizmo.lock().unwrap();

        // Find all gizmo collider handles
        let gizmo_handles: Vec<_> = gizmo_mapping.keys().copied().collect();

        // Remove them from collider set
        for handle in gizmo_handles {
            collider_set.remove(handle, &mut Default::default(), &mut Default::default(), false);
            gizmo_mapping.remove(&handle);
        }

        drop(collider_set);
        drop(gizmo_mapping);
    }
}

// Re-export GizmoType from gizmos module (will be defined shortly)
mod gizmo_types {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum GizmoType {
        None,
        Translate,
        Rotate,
        Scale,
    }
}

pub use gizmo_types::GizmoType;
