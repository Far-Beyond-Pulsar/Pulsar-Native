use engine_class_derive::{register_world_component};

use super::PhysicsComponent;

// A World-resident typed component (Pulsar-Native#556). No physics engine
// reads it yet; when one does, it reads the typed value from the World.
#[register_world_component]
impl PhysicsComponent {}
