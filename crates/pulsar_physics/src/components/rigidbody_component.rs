//! Rigidbody component for dynamic physics simulation

use engine_class_derive::EngineClass;
use serde::{Deserialize, Serialize};

/// Rigidbody component for objects affected by forces and gravity
///
/// This component demonstrates additional physics properties beyond
/// the basic PhysicsComponent.
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Physics")]
pub struct RigidbodyComponent {
    /// Linear velocity in world space (m/s)
    #[property]
    pub velocity: [f32; 3],

    /// Angular velocity (radians/s)
    #[property]
    pub angular_velocity: [f32; 3],

    /// Linear drag coefficient
    #[property(min = 0.0, max = 10.0, step = 0.1)]
    pub linear_damping: f32,

    /// Angular drag coefficient
    #[property(min = 0.0, max = 10.0, step = 0.1)]
    pub angular_damping: f32,

    /// Gravity scale multiplier (1.0 = normal gravity, 0.0 = no gravity)
    #[property(min = 0.0, max = 10.0, step = 0.1)]
    pub gravity_scale: f32,

    /// Whether the rigidbody is affected by gravity
    #[property]
    pub use_gravity: bool,

    /// Lock position on specific axes
    #[property]
    pub freeze_position_x: bool,

    #[property]
    pub freeze_position_y: bool,

    #[property]
    pub freeze_position_z: bool,

    /// Lock rotation on specific axes
    #[property]
    pub freeze_rotation_x: bool,

    #[property]
    pub freeze_rotation_y: bool,

    #[property]
    pub freeze_rotation_z: bool,
}
