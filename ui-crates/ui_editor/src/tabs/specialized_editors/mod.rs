//! Specialized Editors Module
//!
//! This module contains specialized editor panels for specific asset types
//! that can be opened as needed. These are more specialized than the main
//! editor tabs and are typically used for editing specific resource types.
//!
//! ## Available Editors
//!
//! - `animation_editor` - Animation timeline and keyframe editing
//! - `behavior_editor` - AI behavior tree editor
//! - `diagram_editor` - General diagram and flowchart editor
//! - `foliage_editor` - Vegetation and foliage placement
//! - `material_editor` - Material and shader editing
//! - `navmesh_editor` - Navigation mesh editing
//! - `particle_editor` - Particle system editor
//! - `physics_editor` - Physics properties editor
//! - `prefab_editor` - Prefab asset editor
//! - `skeleton_editor` - Skeletal animation editor
//! - `sound_editor` - Sound asset editor
//! - `terrain_editor` - Terrain sculpting and painting
//! - `ui_editor` - UI layout editor

pub mod animation_editor;
pub mod behavior_editor;
pub mod diagram_editor;
pub mod foliage_editor;
pub mod material_editor;
pub mod navmesh_editor;
pub mod particle_editor;
pub mod physics_editor;
pub mod prefab_editor;
pub mod skeleton_editor;
pub mod sound_editor;
pub mod terrain_editor;
pub mod ui_editor;
