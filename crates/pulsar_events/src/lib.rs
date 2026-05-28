//! Central script-event registry for Pulsar Engine.
//!
//! [`SCRIPT_REGISTRY`] is a process-global, lock-protected map from actor keys
//! to [`ScriptRegistration`] entries.  It is written by [`ScriptComponent`]'s
//! `sync_component` every render/sync pass and read by the game runtime's
//! [`BlueprintDispatcher`] to know which scene objects have scripts attached and
//! where their bytecode lives.
//!
//! The registry is intentionally free of execution logic — it is a pure
//! registry.  Dispatching `BeginPlay`, `Tick`, and other events is the
//! responsibility of the consumer (`pulsar_game`).

use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

// ── Registration entry ────────────────────────────────────────────────────────

/// A single script attached to a scene object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptRegistration {
    /// Unique actor key — formatted as `"<scene_object_id>::script::<component_index>"`.
    pub actor_key: String,

    /// The scene object that owns this script.
    pub scene_object_id: String,

    /// Absolute or project-relative path to the blueprint directory
    /// (the directory containing `graph_save.json`).
    pub script_path: String,
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Thread-safe registry of all currently live [`ScriptRegistration`] entries.
pub struct ScriptRegistry {
    entries: HashMap<String, ScriptRegistration>,
}

impl ScriptRegistry {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Insert or replace the registration for `reg.actor_key`.
    pub fn register(&mut self, reg: ScriptRegistration) {
        self.entries.insert(reg.actor_key.clone(), reg);
    }

    /// Remove the registration for `actor_key`, if present.
    pub fn unregister(&mut self, actor_key: &str) {
        self.entries.remove(actor_key);
    }

    /// Retain only entries whose `actor_key` is in `live_keys`.
    ///
    /// Called at the end of each sync pass to cull stale script registrations
    /// (scene objects that were removed or had their ScriptComponent detached).
    pub fn retain_keys(&mut self, live_keys: &HashSet<String>) {
        self.entries.retain(|k, _| live_keys.contains(k));
    }

    /// Remove all entries.  Used when a scene is unloaded.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Iterate over all registered scripts.
    pub fn iter(&self) -> impl Iterator<Item = &ScriptRegistration> {
        self.entries.values()
    }

    /// Look up a registration by actor key.
    pub fn get(&self, actor_key: &str) -> Option<&ScriptRegistration> {
        self.entries.get(actor_key)
    }

    /// Return the number of registered scripts.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if no scripts are registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ── Global singleton ──────────────────────────────────────────────────────────

/// Process-global script registry.
///
/// Written each sync/render pass by [`ScriptComponent::sync_component`].
/// Read by the game runtime to build its [`BlueprintDispatcher`] instance map.
pub static SCRIPT_REGISTRY: Lazy<Mutex<ScriptRegistry>> =
    Lazy::new(|| Mutex::new(ScriptRegistry::new()));
