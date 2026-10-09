mod component;
mod mapping;
mod runtime;
mod scene_props;
mod sub_props;
mod types;
mod water;

pub use component::PhysicsComponent;
pub use types::{
    CollisionChannel, CollisionPreset, CollisionResponse, InterpolationMethod, MotionType,
    RegisteredCollisionChannel, RegisteredCollisionPreset, RegisteredCollisionResponse,
    RegisteredInterpolationMethod, RegisteredMotionType, RegisteredSimulationInterface,
    SimulationInterface,
};
pub use water::{
    WATER_HITBOX_EDGE_SOFTNESS, WATER_HITBOX_SOURCES_BUFFER, WATER_HITBOX_STRENGTH,
    WaterHitboxSourceRow,
};
