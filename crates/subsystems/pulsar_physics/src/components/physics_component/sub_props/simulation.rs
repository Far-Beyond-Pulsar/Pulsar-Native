use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::SimulationInterface;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Simulation", category_color = "#A78BFA", default_collapsed = true)]
pub struct SimulationPhysicsProps {
    #[property(category = "Simulation")]
    pub simulate_physics: bool,
    #[property(category = "Simulation")]
    pub generate_collision_events: bool,
    #[property(category = "Simulation")]
    pub wake_on_collision: bool,
    #[property(category = "Simulation")]
    pub enable_sleeping: bool,
    #[property(min = -100.0, max = 100.0, step = 0.01, category = "Simulation")]
    pub sleep_threshold: f32,
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Simulation")]
    pub max_delta_time: f32,
    #[property(category = "Simulation")]
    pub interface: SimulationInterface,
}

impl Default for SimulationPhysicsProps {
    fn default() -> Self {
        Self {
            simulate_physics: true,
            generate_collision_events: true,
            wake_on_collision: true,
            enable_sleeping: true,
            sleep_threshold: 0.0,
            max_delta_time: 0.0,
            interface: SimulationInterface::Game,
        }
    }
}

impl SimulationPhysicsProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("simulate_physics").and_then(|v| v.as_bool()) {
            self.simulate_physics = v;
        }
        if let Some(v) = obj
            .get("generate_collision_events")
            .and_then(|v| v.as_bool())
        {
            self.generate_collision_events = v;
        }
        if let Some(v) = obj.get("wake_on_collision").and_then(|v| v.as_bool()) {
            self.wake_on_collision = v;
        }
        if let Some(v) = obj.get("enable_sleeping").and_then(|v| v.as_bool()) {
            self.enable_sleeping = v;
        }
        if let Some(v) = obj.get("sleep_threshold").and_then(|v| v.as_f64()) {
            self.sleep_threshold = v as f32;
        }
        if let Some(v) = obj.get("max_delta_time").and_then(|v| v.as_f64()) {
            self.max_delta_time = v as f32;
        }
        if let Some(ix) = obj.get("interface").and_then(|v| v.as_u64()) {
            self.interface = match ix {
                0 => SimulationInterface::Game,
                1 => SimulationInterface::Physics,
                2 => SimulationInterface::Both,
                _ => self.interface,
            };
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "simulate_physics".to_string(),
            Value::from(self.simulate_physics),
        );
        out.insert(
            "generate_collision_events".to_string(),
            Value::from(self.generate_collision_events),
        );
        out.insert(
            "wake_on_collision".to_string(),
            Value::from(self.wake_on_collision),
        );
        out.insert(
            "enable_sleeping".to_string(),
            Value::from(self.enable_sleeping),
        );
        out.insert(
            "sleep_threshold".to_string(),
            Value::from(self.sleep_threshold),
        );
        out.insert(
            "max_delta_time".to_string(),
            Value::from(self.max_delta_time),
        );
        out.insert("interface".to_string(), Value::from(self.interface as u64));
    }
}
