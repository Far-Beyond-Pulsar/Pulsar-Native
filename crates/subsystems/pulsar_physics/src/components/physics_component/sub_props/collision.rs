use engine_class_derive::engine_class;
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use super::super::{CollisionChannel, CollisionPreset, CollisionResponse};
use pulsar_reflection::{ReflectError, ReflectResult, pulsar_type};

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Collision", category_color = "#FF6B6B")]
pub struct CollisionPhysicsProps {
    #[property(category = "Collision")]
    pub collision_preset: CollisionPreset,
    #[property(category = "Collision")]
    pub override_collision_preset: bool,
    #[property(category = "Collision")]
    pub create_physics_state: bool,
    #[property(category = "Collision")]
    pub complex_as_simple: bool,
    #[property(min = 0.0, max = 65535.0, step = 1.0, category = "Collision")]
    pub collision_channel: u64,
    #[property(category = "Collision")]
    pub channel_responses: Vec<RegisteredChannelCollisionResponse>,
    #[property(category = "Collision")]
    pub all_channels_response: CollisionResponse,
}

impl Default for CollisionPhysicsProps {
    fn default() -> Self {
        Self {
            collision_preset: CollisionPreset::BlockAll,
            override_collision_preset: false,
            create_physics_state: true,
            complex_as_simple: false,
            collision_channel: CollisionPreset::BlockAll as u64,
            channel_responses: vec![],
            all_channels_response: CollisionResponse::Block,
        }
    }
}

impl CollisionPhysicsProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(ix) = obj.get("collision_preset").and_then(|v| v.as_u64()) {
            self.collision_preset = match ix {
                0 => CollisionPreset::BlockAll,
                1 => CollisionPreset::OverlapAll,
                2 => CollisionPreset::VisibilityAll,
                3 => CollisionPreset::PhysicsActor,
                4 => CollisionPreset::Game,
                5 => CollisionPreset::WorldDynamic,
                6 => CollisionPreset::WorldStatic,
                7 => CollisionPreset::DefaultTrace,
                _ => self.collision_preset,
            };
        }
        if let Some(v) = obj
            .get("override_collision_preset")
            .and_then(|v| v.as_bool())
        {
            self.override_collision_preset = v;
        }
        if let Some(v) = obj.get("create_physics_state").and_then(|v| v.as_bool()) {
            self.create_physics_state = v;
        }
        if let Some(v) = obj.get("complex_as_simple").and_then(|v| v.as_bool()) {
            self.complex_as_simple = v;
        }
        if let Some(v) = obj.get("collision_channel").and_then(|v| v.as_u64()) {
            self.collision_channel = v;
        }
        if let Some(arr) = obj.get("channel_responses").and_then(|v| v.as_array()) {
            let mut responses = vec![];
            for item in arr {
                if let Some(obj) = item.as_object() {
                    if let Some(channel) = obj.get("channel").and_then(|v| v.as_u64()) {
                        if let Some(response) = obj.get("response").and_then(|v| v.as_u64()) {
                            let channel = match channel {
                                0 => CollisionChannel::WorldDynamic,
                                1 => CollisionChannel::WorldStatic,
                                2 => CollisionChannel::Camera,
                                3 => CollisionChannel::Visibility,
                                4 => CollisionChannel::Game,
                                5 => CollisionChannel::PhysicsActor,
                                6 => CollisionChannel::Trigger,
                                7 => CollisionChannel::Character,
                                _ => CollisionChannel::Custom,
                            };
                            let response = match response {
                                0 => CollisionResponse::Ignore,
                                1 => CollisionResponse::Overlap,
                                2 => CollisionResponse::Block,
                                3 => CollisionResponse::BlockAndModify,
                                _ => CollisionResponse::Block,
                            };
                            responses.push(ChannelCollisionResponse { channel, response });
                        }
                    }
                }
            }
            self.channel_responses = responses;
        }
        if let Some(ix) = obj.get("all_channels_response").and_then(|v| v.as_u64()) {
            self.all_channels_response = match ix {
                0 => CollisionResponse::Ignore,
                1 => CollisionResponse::Overlap,
                2 => CollisionResponse::Block,
                3 => CollisionResponse::BlockAndModify,
                _ => CollisionResponse::Block,
            };
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "collision_preset".to_string(),
            Value::from(self.collision_preset as u64),
        );
        out.insert(
            "override_collision_preset".to_string(),
            Value::from(self.override_collision_preset),
        );
        out.insert(
            "create_physics_state".to_string(),
            Value::from(self.create_physics_state),
        );
        out.insert(
            "complex_as_simple".to_string(),
            Value::from(self.complex_as_simple),
        );
        out.insert(
            "collision_channel".to_string(),
            Value::from(self.collision_channel),
        );
        let responses: Vec<Value> = self
            .channel_responses
            .iter()
            .map(|r| {
                serde_json::json!({
                    "channel": r.channel as u64,
                    "response": r.response as u64
                })
            })
            .collect();
        out.insert("channel_responses".to_string(), Value::Array(responses));
        out.insert(
            "all_channels_response".to_string(),
            Value::from(self.all_channels_response as u64),
        );
    }
}

/// Describes the collision response for a specific channel
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelCollisionResponse {
    pub channel: CollisionChannel,
    pub response: CollisionResponse,
}

fn serialize_channel_collision_response_json(
    value: &ChannelCollisionResponse,
) -> ReflectResult<serde_json::Value> {
    Ok(serde_json::json!({
        "channel": value.channel as u64,
        "response": value.response as u64
    }))
}

fn deserialize_channel_collision_response_json(
    value: serde_json::Value,
) -> ReflectResult<ChannelCollisionResponse> {
    let obj = value.as_object().ok_or_else(|| {
        ReflectError::DeserializationFailed(
            "expected object for ChannelCollisionResponse".to_string(),
        )
    })?;
    let channel = obj
        .get("channel")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ReflectError::DeserializationFailed("missing channel field".to_string()))?;
    let response = obj
        .get("response")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ReflectError::DeserializationFailed("missing response field".to_string()))?;
    let channel = match channel {
        0 => CollisionChannel::WorldDynamic,
        1 => CollisionChannel::WorldStatic,
        2 => CollisionChannel::Camera,
        3 => CollisionChannel::Visibility,
        4 => CollisionChannel::Game,
        5 => CollisionChannel::PhysicsActor,
        6 => CollisionChannel::Trigger,
        7 => CollisionChannel::Character,
        _ => CollisionChannel::Custom,
    };
    let response = match response {
        0 => CollisionResponse::Ignore,
        1 => CollisionResponse::Overlap,
        2 => CollisionResponse::Block,
        3 => CollisionResponse::BlockAndModify,
        _ => CollisionResponse::Block,
    };
    Ok(ChannelCollisionResponse { channel, response })
}

#[pulsar_type(
    serialize_json_with = serialize_channel_collision_response_json,
    deserialize_json_with = deserialize_channel_collision_response_json
)]
type RegisteredChannelCollisionResponse = ChannelCollisionResponse;
