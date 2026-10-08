use engine_class_derive::{register_world_component};

use super::RigidbodyComponent;

// A World-resident typed component (Pulsar-Native#556). No physics engine
// reads it yet.
#[register_world_component]
impl RigidbodyComponent {}
