use pulsar_reflection::{ReflectError, ReflectResult, pulsar_type};
use serde::{Deserialize, Serialize};

/// Unreal-style collision channels
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionChannel {
    /// Dynamic world objects
    WorldDynamic,
    /// Static world geometry
    WorldStatic,
    /// Camera
    Camera,
    /// Visibility (blocks ray traces but not physics)
    Visibility,
    /// Game objects
    Game,
    /// Physics actors
    PhysicsActor,
    /// Trigger volumes
    Trigger,
    /// Character controller
    Character,
    /// Custom user-defined channel
    Custom,
}

impl Default for CollisionChannel {
    fn default() -> Self {
        Self::WorldDynamic
    }
}

impl From<CollisionChannel> for u64 {
    fn from(channel: CollisionChannel) -> u64 {
        match channel {
            CollisionChannel::WorldDynamic => 1 << 0,
            CollisionChannel::WorldStatic => 1 << 1,
            CollisionChannel::Camera => 1 << 2,
            CollisionChannel::Visibility => 1 << 3,
            CollisionChannel::Game => 1 << 4,
            CollisionChannel::PhysicsActor => 1 << 5,
            CollisionChannel::Trigger => 1 << 6,
            CollisionChannel::Character => 1 << 7,
            CollisionChannel::Custom => 1 << 31,
        }
    }
}

fn serialize_collision_channel_json(value: &CollisionChannel) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(&(*value as u64))
        .map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_collision_channel_json(value: serde_json::Value) -> ReflectResult<CollisionChannel> {
    let ix = value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "u64",
        found: format!("{:?}", value),
    })?;
    Ok(match ix {
        0 => CollisionChannel::WorldDynamic,
        1 => CollisionChannel::WorldStatic,
        2 => CollisionChannel::Camera,
        3 => CollisionChannel::Visibility,
        4 => CollisionChannel::Game,
        5 => CollisionChannel::PhysicsActor,
        6 => CollisionChannel::Trigger,
        7 => CollisionChannel::Character,
        31 => CollisionChannel::Custom,
        _ => CollisionChannel::Custom,
    })
}

#[pulsar_type(
    serialize_json_with = serialize_collision_channel_json,
    deserialize_json_with = deserialize_collision_channel_json
)]
pub type RegisteredCollisionChannel = CollisionChannel;

/// Unreal-style collision response type
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionResponse {
    /// Ignore the channel completely
    Ignore,
    /// Generate overlap events but block nothing
    Overlap,
    /// Block the channel
    Block,
    /// Block and allow modification (e.g. destructible meshes)
    BlockAndModify,
}

impl Default for CollisionResponse {
    fn default() -> Self {
        Self::Block
    }
}

fn serialize_collision_response_json(
    value: &CollisionResponse,
) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(&(*value as u64))
        .map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_collision_response_json(
    value: serde_json::Value,
) -> ReflectResult<CollisionResponse> {
    let ix = value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "u64",
        found: format!("{:?}", value),
    })?;
    Ok(match ix {
        0 => CollisionResponse::Ignore,
        1 => CollisionResponse::Overlap,
        2 => CollisionResponse::Block,
        3 => CollisionResponse::BlockAndModify,
        _ => CollisionResponse::Block,
    })
}

#[pulsar_type(
    serialize_json_with = serialize_collision_response_json,
    deserialize_json_with = deserialize_collision_response_json
)]
pub type RegisteredCollisionResponse = CollisionResponse;

/// Unreal-style collision presets
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionPreset {
    /// Block all channels
    BlockAll,
    /// Overlap all channels
    OverlapAll,
    /// Overlap visibility only, block nothing
    VisibilityAll,
    /// Overlap physics actors
    PhysicsActor,
    /// Overlap game objects
    Game,
    /// Dynamic world object preset
    WorldDynamic,
    /// Static world object preset
    WorldStatic,
    /// Default trace channel
    DefaultTrace,
}

impl Default for CollisionPreset {
    fn default() -> Self {
        Self::BlockAll
    }
}

fn serialize_collision_preset_json(value: &CollisionPreset) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(&(*value as u64))
        .map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_collision_preset_json(value: serde_json::Value) -> ReflectResult<CollisionPreset> {
    let ix = value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "u64",
        found: format!("{:?}", value),
    })?;
    Ok(match ix {
        0 => CollisionPreset::BlockAll,
        1 => CollisionPreset::OverlapAll,
        2 => CollisionPreset::VisibilityAll,
        3 => CollisionPreset::PhysicsActor,
        4 => CollisionPreset::Game,
        5 => CollisionPreset::WorldDynamic,
        6 => CollisionPreset::WorldStatic,
        7 => CollisionPreset::DefaultTrace,
        _ => CollisionPreset::BlockAll,
    })
}

#[pulsar_type(
    serialize_json_with = serialize_collision_preset_json,
    deserialize_json_with = deserialize_collision_preset_json
)]
pub type RegisteredCollisionPreset = CollisionPreset;

/// Which interface generates collision events
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimulationInterface {
    /// Game interface only
    Game,
    /// Physics interface only
    Physics,
    /// Both interfaces
    Both,
}

impl Default for SimulationInterface {
    fn default() -> Self {
        Self::Game
    }
}

fn serialize_simulation_interface_json(
    value: &SimulationInterface,
) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(&(*value as u64))
        .map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_simulation_interface_json(
    value: serde_json::Value,
) -> ReflectResult<SimulationInterface> {
    let ix = value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "u64",
        found: format!("{:?}", value),
    })?;
    Ok(match ix {
        0 => SimulationInterface::Game,
        1 => SimulationInterface::Physics,
        2 => SimulationInterface::Both,
        _ => SimulationInterface::Game,
    })
}

#[pulsar_type(
    serialize_json_with = serialize_simulation_interface_json,
    deserialize_json_with = deserialize_simulation_interface_json
)]
pub type RegisteredSimulationInterface = SimulationInterface;

/// Transform interpolation method for physics sync
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterpolationMethod {
    /// No interpolation
    None,
    /// Linear interpolation
    Lerp,
    /// Spherical linear interpolation
    Slerp,
}

impl Default for InterpolationMethod {
    fn default() -> Self {
        Self::None
    }
}

fn serialize_interpolation_method_json(
    value: &InterpolationMethod,
) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(&(*value as u64))
        .map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_interpolation_method_json(
    value: serde_json::Value,
) -> ReflectResult<InterpolationMethod> {
    let ix = value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "u64",
        found: format!("{:?}", value),
    })?;
    Ok(match ix {
        0 => InterpolationMethod::None,
        1 => InterpolationMethod::Lerp,
        2 => InterpolationMethod::Slerp,
        _ => InterpolationMethod::None,
    })
}

#[pulsar_type(
    serialize_json_with = serialize_interpolation_method_json,
    deserialize_json_with = deserialize_interpolation_method_json
)]
pub type RegisteredInterpolationMethod = InterpolationMethod;

/// Motion type for the physics object
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MotionType {
    /// Dynamic rigidbody
    Dynamic,
    /// Kinematic static (moved by animation/script)
    KinematicStatic,
    /// Static (immovable)
    Static,
}

impl Default for MotionType {
    fn default() -> Self {
        Self::Static
    }
}

fn serialize_motion_type_json(value: &MotionType) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(&(*value as u64))
        .map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_motion_type_json(value: serde_json::Value) -> ReflectResult<MotionType> {
    let ix = value.as_u64().ok_or_else(|| ReflectError::TypeMismatch {
        expected: "u64",
        found: format!("{:?}", value),
    })?;
    Ok(match ix {
        0 => MotionType::Dynamic,
        1 => MotionType::KinematicStatic,
        2 => MotionType::Static,
        _ => MotionType::Static,
    })
}

#[pulsar_type(
    serialize_json_with = serialize_motion_type_json,
    deserialize_json_with = deserialize_motion_type_json
)]
pub type RegisteredMotionType = MotionType;
