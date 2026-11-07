//! CPU-side keyframe animation evaluation
//!
//! This module provides animation playback for skeletal meshes.
//! Animation data from SolidRS is evaluated on the CPU to produce bone matrices.

use glam::Mat4;
use solid_rs::scene::{Animation, Skin};

/// Animation state for a skinned mesh
#[derive(Debug, Clone)]
pub struct AnimationState {
    /// The skeleton (joint hierarchy + inverse bind matrices)
    pub skin: Skin,
    /// Available animations
    pub animations: Vec<Animation>,
    /// Currently playing animation index
    pub current_animation: Option<usize>,
    /// Current playback time (seconds)
    pub current_time: f32,
    /// Whether to loop the animation
    pub looping: bool,
    /// Final bone matrices (world-space × inverse-bind)
    /// Updated by evaluate()
    pub bone_matrices: Vec<Mat4>,
    /// Dirty flag (true when matrices changed)
    pub dirty: bool,
}

impl AnimationState {
    /// Create a new animation state from a skin and animations
    pub fn new(skin: Skin, animations: Vec<Animation>) -> Self {
        let bone_count = skin.joints.len();
        Self {
            skin,
            animations,
            current_animation: None,
            current_time: 0.0,
            looping: false,
            bone_matrices: vec![Mat4::IDENTITY; bone_count],
            dirty: false,
        }
    }

    /// Start playing an animation
    pub fn play(&mut self, animation_index: usize, looping: bool) {
        if animation_index < self.animations.len() {
            self.current_animation = Some(animation_index);
            self.current_time = 0.0;
            self.looping = looping;
            self.dirty = true;
        }
    }

    /// Stop animation playback
    pub fn stop(&mut self) {
        self.current_animation = None;
    }

    /// Update animation (call each frame)
    pub fn update(&mut self, delta_time: f32) {
        if let Some(_anim_idx) = self.current_animation {
            self.current_time += delta_time;

            // TODO: Implement full keyframe evaluation
            // For now, just mark as dirty
            self.dirty = true;
        }
    }

    /// Get the current bone matrices
    pub fn get_matrices(&self) -> &[Mat4] {
        &self.bone_matrices
    }
}
