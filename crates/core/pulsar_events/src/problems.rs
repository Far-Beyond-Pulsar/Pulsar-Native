//! Script problems on the host bus (Pulsar-Native#854, #868).
//!
//! Script errors raised while a game runs (a VM runtime error, a class
//! whose module does not link, a waiting call a class reload had to drop)
//! are reported as [`ScriptProblem`]s: which class, instance and function,
//! and the source location the module's debug info gives (for a Blueprint,
//! the graph node). The Play-in-Editor host collects them from the game
//! (the game dylib has its own copy of this crate, so they cross the PIE
//! ABI as JSON) and publishes them here, on the editor's process-wide
//! [host bus](crate::host); the problems panel and the Blueprint editor
//! (a plugin attached to the same bus) subscribe.
//!
//! On the bus a report is the dynamic event `ScriptProblems` with one
//! string field, `json`: a [`ScriptProblemsEvent`] as JSON. Delivery is
//! synchronous on the publishing thread, like asset updates.

use std::path::PathBuf;

use gamma::{Channel, DynEvent, DynValue, EventDescriptor, FieldType, SubscribeOptions};
use serde::{Deserialize, Serialize};

use crate::host::{HostBus, HostSubscription, host_bus};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProblemSeverity {
    #[default]
    Error,
    Warning,
}

/// One script problem. Every field but `message` is optional: fill what
/// is known.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptProblem {
    #[serde(default)]
    pub severity: ProblemSeverity,
    /// The script class (module) name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The class GUID, when the class is a project class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_id: Option<String>,
    /// The instance (object) id, for a runtime error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// The script function (a Blueprint event: `tick`, `begin_play`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The graph node id (from the module's debug info).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The source file: the class directory joined with the debug info's
    /// file when known, else the class directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
    /// The game frame it happened in, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<u64>,
}

impl ScriptProblem {
    /// `Class::function (node N): message`, the one-line form.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        if let Some(class) = &self.class {
            out.push_str(class);
        }
        if let Some(function) = &self.function {
            if !out.is_empty() {
                out.push_str("::");
            }
            out.push_str(function);
        }
        if let Some(node) = &self.node {
            out.push_str(&format!(" (node {node})"));
        }
        if let Some(instance) = &self.instance {
            out.push_str(&format!(" [{instance}]"));
        }
        if out.is_empty() {
            self.message.clone()
        } else {
            format!("{out}: {}", self.message)
        }
    }
}

/// What travels on the bus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptProblemsEvent {
    /// A new problem.
    Reported(ScriptProblem),
    /// Forget every reported problem (a new Play session started).
    Cleared,
}

/// The `ScriptProblems` descriptor on the host bus.
pub fn descriptor() -> EventDescriptor {
    EventDescriptor::dynamic("ScriptProblems", [("json", FieldType::Str)])
}

fn registered(bus: &HostBus) -> Option<u64> {
    let descriptor = descriptor();
    let id = descriptor.id;
    match bus.register_descriptor(&descriptor) {
        Ok(()) => Some(id),
        Err(error) => {
            tracing::error!("problems bus: cannot register ScriptProblems: {error}");
            None
        }
    }
}

fn publish_on(bus: &HostBus, event: &ScriptProblemsEvent) {
    let Some(id) = registered(bus) else { return };
    let Ok(json) = serde_json::to_string(event) else { return };
    if let Err(error) = bus.publish_dyn(Channel::Global, &DynEvent::new(id, vec![DynValue::Str(json)])) {
        tracing::error!("problems bus: publish failed: {error}");
    }
}

/// Report `problem` to every subscriber on the host bus.
pub fn publish_script_problem(problem: ScriptProblem) {
    publish_on(host_bus(), &ScriptProblemsEvent::Reported(problem));
}

/// Tell subscribers to forget the problems reported so far.
pub fn publish_script_problems_cleared() {
    publish_on(host_bus(), &ScriptProblemsEvent::Cleared);
}

/// Call `callback` for every [`ScriptProblemsEvent`] on the host bus until
/// the returned subscription is dropped.
pub fn subscribe_script_problems(
    callback: impl Fn(&ScriptProblemsEvent) + Send + Sync + 'static,
) -> HostSubscription {
    subscribe_on(host_bus(), callback)
}

fn subscribe_on(bus: &HostBus, callback: impl Fn(&ScriptProblemsEvent) + Send + Sync + 'static) -> HostSubscription {
    let id = registered(bus).unwrap_or_else(|| descriptor().id);
    bus.subscribe_dyn(id, SubscribeOptions::default(), move |event| {
        let Some(DynValue::Str(json)) = event.fields.first() else { return };
        match serde_json::from_str::<ScriptProblemsEvent>(json) {
            Ok(event) => callback(&event),
            Err(error) => tracing::warn!("problems bus: malformed ScriptProblems ignored: {error}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn problems_round_trip_the_bus_and_through_a_plugin_view() {
        let host = HostBus::local();
        let plugin = unsafe { HostBus::foreign(host.export_raw().unwrap()) }.unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&seen);
        let _sub = subscribe_on(&plugin, move |e| s.lock().unwrap().push(e.clone()));
        let problem = ScriptProblem {
            class: Some("Door".into()),
            function: Some("tick".into()),
            node: Some("divide_7".into()),
            message: "integer division by zero".into(),
            ..Default::default()
        };
        publish_on(&host, &ScriptProblemsEvent::Reported(problem.clone()));
        publish_on(&host, &ScriptProblemsEvent::Cleared);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![ScriptProblemsEvent::Reported(problem.clone()), ScriptProblemsEvent::Cleared]
        );
        assert_eq!(problem.summary(), "Door::tick (node divide_7): integer division by zero");
    }
}
