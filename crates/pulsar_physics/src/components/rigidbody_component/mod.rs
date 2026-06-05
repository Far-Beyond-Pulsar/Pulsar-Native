mod component;
mod mapping;
mod runtime;
mod scene_props;
mod sub_props;
mod types;

pub use component::RigidbodyComponent;
pub use types::{InterpolationMethod, MotionType};

// Re-export shared physics types from physics_component
pub use super::physics_component::{
    CollisionChannel, CollisionPreset, CollisionResponse, RegisteredCollisionChannel,
    RegisteredCollisionPreset, RegisteredCollisionResponse, RegisteredInterpolationMethod,
    RegisteredMotionType, RegisteredSimulationInterface, SimulationInterface,
};
