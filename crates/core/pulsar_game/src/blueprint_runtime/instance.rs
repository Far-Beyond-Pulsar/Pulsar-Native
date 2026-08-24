//! Blueprint instance management with state tracking.
//!
//! Each instance represents a single runtime occurrence of a blueprint class,
//! with its own variable state stored in a ByteArena.

use super::byte_arena::ByteArena;
use super::compiled_bytecode::{CompiledBytecode, VariableDescriptor};
use super::executor::LoadedBlueprint;
use pulsar_scenedb::Entity;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use uuid::Uuid;

/// Execution mode for a blueprint instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlueprintExecutionMode {
    /// Execute using bytecode VM (development mode)
    Bytecode,
    /// Execute using compiled native Rust code (production mode)
    Native,
}

/// A runtime instance of a blueprint class.
///
/// Each instance has its own variable state and can execute events.
pub struct BlueprintInstance {
    /// Unique instance ID (for tracking)
    pub instance_id: Uuid,

    /// Scene object ID this instance is attached to
    pub object_id: String, // ObjectId from engine_backend

    /// Blueprint class name (e.g., "Enemy_Goblin")
    pub class_name: String,

    /// Scene entity this instance is bound to (#648).
    ///
    /// `None` until bound — either atomically at spawn time
    /// (`BlueprintDispatcher::spawn_instance_for_entity`) or later via
    /// `BlueprintDispatcher::bind_instance` once level entities exist.
    /// Unbound instances run their graphs normally; only `comp_*` nodes
    /// refuse (logged, null output), so pure-logic graphs are unaffected.
    entity: Option<Entity>,

    /// Execution mode
    pub execution_mode: BlueprintExecutionMode,

    /// State arena for bytecode execution
    pub bytecode_state: Option<ByteArena>,

    /// Variable descriptors (for reading/writing by name)
    variables: Vec<VariableDescriptor>,
}

impl BlueprintInstance {
    /// Create a new blueprint instance in bytecode execution mode.
    ///
    /// # Arguments
    /// * `object_id` - Scene object ID this instance is attached to
    /// * `loaded_blueprint` - Reference to loaded blueprint with compiled bytecode
    /// * `variable_overrides` - Optional variable values from prefab (JSON format)
    ///
    /// # Returns
    /// A new instance with initialized state arena.
    pub fn new_bytecode(
        object_id: String,
        loaded_blueprint: &LoadedBlueprint,
        variable_overrides: Option<HashMap<String, JsonValue>>,
    ) -> Self {
        let class_name = loaded_blueprint.class_name.clone();
        let variables = loaded_blueprint.bytecode.variables.clone();

        // Allocate arena. The size must cover the variable layout AND every
        // event program's scratch requirements — the VM executes programs
        // against THIS buffer and refuses to run when it is smaller than a
        // program's `arena_size` (typed error, event skipped). Older
        // bytecode files record only the variable extent here, so take the
        // max instead of trusting one source.
        let arena_size = Self::required_arena_size(&loaded_blueprint.bytecode);

        // Allocate arena
        let mut state = ByteArena::new(arena_size);

        // Initialize variables with defaults
        for var in &variables {
            let value = if let Some(ref overrides) = variable_overrides {
                overrides
                    .get(&var.name)
                    .and_then(|json| Self::serialize_json_to_bytes(json, &var.data_type).ok())
                    .unwrap_or_else(|| var.default_value.clone())
            } else {
                var.default_value.clone()
            };

            // Write value to arena at variable's offset
            unsafe {
                state.write_bytes_at(var.offset, &value);
            }
        }

        Self {
            instance_id: Uuid::new_v4(),
            object_id,
            class_name,
            entity: None,
            execution_mode: BlueprintExecutionMode::Bytecode,
            bytecode_state: Some(state),
            variables,
        }
    }

    /// Smallest arena that can run every event of `bytecode` plus hold its
    /// variable layout.
    ///
    /// Invariant: an instance arena is always ≥ its bytecode's declared
    /// extent AND ≥ each event program's scratch requirement; the VM's
    /// size check never fires for arenas built through this type.
    fn required_arena_size(bytecode: &CompiledBytecode) -> usize {
        bytecode
            .event_programs
            .values()
            .map(|program| program.arena_size)
            .max()
            .unwrap_or(0)
            .max(bytecode.arena_size)
    }

    /// Bind this instance to a scene entity, returning the previous binding
    /// if there was one.
    ///
    /// Rebinding to a different live entity is allowed (respawn flows);
    /// component ops address the NEW entity from the next dispatched event
    /// on. Callers are responsible for liveness — binding does not validate
    /// against a world because instances outlive no particular borrow.
    pub fn bind_entity(&mut self, entity: Entity) -> Option<Entity> {
        self.entity.replace(entity)
    }

    /// Detach this instance from its scene entity, returning the removed
    /// binding. Component ops refuse until it is bound again.
    pub fn unbind_entity(&mut self) -> Option<Entity> {
        self.entity.take()
    }

    /// The scene entity this instance's component ops address, if bound.
    pub fn bound_entity(&self) -> Option<Entity> {
        self.entity
    }

    /// Rebuild this instance's state arena against freshly-reloaded bytecode
    /// (hot-reload, #648).
    ///
    /// Variables whose `(name, data_type, offset, size)` all match the new
    /// layout carry their old bytes over; everything else resets to the new
    /// defaults. Exact-match-only preservation means layout drift can never
    /// alias old data into a differently-typed slot — safety beats state.
    pub fn rehydrate_after_reload(&mut self, loaded: &LoadedBlueprint) {
        let mut new_state = ByteArena::new(Self::required_arena_size(&loaded.bytecode));
        for var in &loaded.bytecode.variables {
            let carried = self
                .variables
                .iter()
                .find(|old| {
                    old.name == var.name
                        && old.data_type == var.data_type
                        && old.offset == var.offset
                        && old.size == var.size
                })
                .and_then(|old| {
                    let state = self.bytecode_state.as_ref()?;
                    // SAFETY: `old` came from this instance's descriptor list,
                    // so (offset, size) is inside the live arena.
                    Some(unsafe { state.read_bytes(old.offset, old.size) })
                });
            let value = carried.unwrap_or_else(|| var.default_value.clone());
            // SAFETY: `var` describes `loaded`'s own layout, which
            // `required_arena_size` sized this arena for.
            unsafe { new_state.write_bytes_at(var.offset, &value) };
        }
        self.variables = loaded.bytecode.variables.clone();
        self.bytecode_state = Some(new_state);
    }

    /// Get a variable's value as bytes.
    ///
    /// Returns None if the variable is not found or instance is in native mode.
    pub fn get_variable_bytes(&self, var_name: &str) -> Option<Vec<u8>> {
        if self.bytecode_state.is_none() {
            return None;
        }

        let var = self.variables.iter().find(|v| v.name == var_name)?;
        let state = self.bytecode_state.as_ref().unwrap();

        unsafe { Some(state.read_bytes(var.offset, var.size)) }
    }

    /// Set a variable's value from bytes.
    ///
    /// Returns false if the variable is not found or instance is in native mode.
    pub fn set_variable_bytes(&mut self, var_name: &str, value: &[u8]) -> bool {
        if self.bytecode_state.is_none() {
            return false;
        }

        let var = match self.variables.iter().find(|v| v.name == var_name) {
            Some(v) => v.clone(),
            None => return false,
        };

        if value.len() != var.size {
            tracing::warn!(
                "Variable '{}' size mismatch: expected {}, got {}",
                var_name,
                var.size,
                value.len()
            );
            return false;
        }

        let state = self.bytecode_state.as_mut().unwrap();
        unsafe {
            state.write_bytes_at(var.offset, value);
        }

        true
    }

    /// Get a variable's value as a typed Rust value.
    ///
    /// # Type Parameters
    /// * `T` - The type to deserialize to (must match variable's data_type)
    ///
    /// # Safety
    /// This performs unsafe memory operations. The caller must ensure:
    /// - The type T matches the variable's declared data_type
    /// - The variable is properly aligned
    pub unsafe fn get_variable<T: Copy>(&self, var_name: &str) -> Option<T> {
        if self.bytecode_state.is_none() {
            return None;
        }

        let var = self.variables.iter().find(|v| v.name == var_name)?;
        let state = self.bytecode_state.as_ref().unwrap();

        if std::mem::size_of::<T>() != var.size {
            tracing::warn!(
                "Variable '{}' type size mismatch: expected {}, got {}",
                var_name,
                var.size,
                std::mem::size_of::<T>()
            );
            return None;
        }

        Some(state.read::<T>(var.offset))
    }

    /// Set a variable's value from a typed Rust value.
    ///
    /// # Safety
    /// This performs unsafe memory operations. The caller must ensure:
    /// - The type T matches the variable's declared data_type
    /// - The variable is properly aligned
    pub unsafe fn set_variable<T: Copy>(&mut self, var_name: &str, value: T) -> bool {
        if self.bytecode_state.is_none() {
            return false;
        }

        let var = match self.variables.iter().find(|v| v.name == var_name) {
            Some(v) => v.clone(),
            None => return false,
        };

        if std::mem::size_of::<T>() != var.size {
            tracing::warn!(
                "Variable '{}' type size mismatch: expected {}, got {}",
                var_name,
                var.size,
                std::mem::size_of::<T>()
            );
            return false;
        }

        let state = self.bytecode_state.as_mut().unwrap();
        state.write_at(var.offset, &value);

        true
    }

    /// Get mutable access to the state arena.
    ///
    /// This is used by the executor to pass the arena to the VM.
    pub fn state_arena_mut(&mut self) -> Option<&mut ByteArena> {
        self.bytecode_state.as_mut()
    }

    /// Get the list of variable descriptors for this instance.
    pub fn variables(&self) -> &[VariableDescriptor] {
        &self.variables
    }

    /// Serialize a JSON value to bytes based on data type.
    ///
    /// This is used for variable overrides from prefabs.
    ///
    /// Primitive types keep their native little-endian fast paths. Every
    /// other type resolves through the reflection registry
    /// (`RUNTIME_TYPE_REGISTRY.get_by_name`) and marshals to the VM blob
    /// encoding (`pulsar_world_registry::marshal`) — `[u64 len][payload]`
    /// for strings/vectors, JSON for compound registered types — matching
    /// what component ops and `bytes_to_any` speak. Types unknown to both
    /// paths are typed errors, never silently zeroed.
    fn serialize_json_to_bytes(json: &JsonValue, data_type: &str) -> Result<Vec<u8>, String> {
        match data_type {
            "i32" | "I32" => {
                let value = json
                    .as_i64()
                    .ok_or_else(|| format!("Expected integer for i32, got {:?}", json))?
                    as i32;
                Ok(value.to_le_bytes().to_vec())
            }
            "i64" | "I64" => {
                let value = json
                    .as_i64()
                    .ok_or_else(|| format!("Expected integer for i64, got {:?}", json))?;
                Ok(value.to_le_bytes().to_vec())
            }
            "f32" | "F32" => {
                let value = json
                    .as_f64()
                    .ok_or_else(|| format!("Expected number for f32, got {:?}", json))?
                    as f32;
                Ok(value.to_le_bytes().to_vec())
            }
            "f64" | "F64" => {
                let value = json
                    .as_f64()
                    .ok_or_else(|| format!("Expected number for f64, got {:?}", json))?;
                Ok(value.to_le_bytes().to_vec())
            }
            "bool" | "Bool" => {
                let value = json
                    .as_bool()
                    .ok_or_else(|| format!("Expected boolean for bool, got {:?}", json))?;
                Ok(vec![if value { 1 } else { 0 }])
            }
            data_type => pulsar_world_registry::marshal::serialize_named_json(data_type, json)
                .map_err(|e| e.to_string()),
        }
    }
}

impl Drop for BlueprintInstance {
    fn drop(&mut self) {
        tracing::trace!(
            "Dropping BlueprintInstance {} for class '{}' on object '{}'",
            self.instance_id,
            self.class_name,
            self.object_id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint_runtime::compiled_bytecode::CompiledBytecode;
    use std::collections::HashMap;

    fn create_test_loaded_blueprint() -> LoadedBlueprint {
        let mut bytecode = CompiledBytecode::new("TestBlueprint".to_string());
        bytecode.add_variable(VariableDescriptor::f32("speed", 0, 5.0));
        bytecode.add_variable(VariableDescriptor::i32("health", 4, 100));
        bytecode.add_variable(VariableDescriptor::bool("is_active", 8, true));
        bytecode.calculate_arena_size();

        LoadedBlueprint {
            class_name: "TestBlueprint".to_string(),
            bytecode,
            programs: HashMap::new(),
        }
    }

    #[test]
    fn test_instance_creation() {
        let loaded = create_test_loaded_blueprint();
        let instance = BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, None);

        assert_eq!(instance.class_name, "TestBlueprint");
        assert_eq!(instance.object_id, "obj_001");
        assert_eq!(instance.execution_mode, BlueprintExecutionMode::Bytecode);
        assert!(instance.bytecode_state.is_some());
        assert_eq!(instance.variables.len(), 3);
    }

    #[test]
    fn test_instance_default_values() {
        let loaded = create_test_loaded_blueprint();
        let instance = BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, None);

        // Check default values
        unsafe {
            assert_eq!(instance.get_variable::<f32>("speed"), Some(5.0));
            assert_eq!(instance.get_variable::<i32>("health"), Some(100));
            assert_eq!(instance.get_variable::<u8>("is_active"), Some(1));
        }
    }

    #[test]
    fn test_instance_with_overrides() {
        let loaded = create_test_loaded_blueprint();

        let mut overrides = HashMap::new();
        overrides.insert("speed".to_string(), JsonValue::from(10.5));
        overrides.insert("health".to_string(), JsonValue::from(50));

        let instance =
            BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, Some(overrides));

        // Check overridden values
        unsafe {
            assert_eq!(instance.get_variable::<f32>("speed"), Some(10.5));
            assert_eq!(instance.get_variable::<i32>("health"), Some(50));
            assert_eq!(instance.get_variable::<u8>("is_active"), Some(1)); // Default
        }
    }

    #[test]
    fn test_get_set_variable_bytes() {
        let loaded = create_test_loaded_blueprint();
        let mut instance = BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, None);

        // Get variable bytes
        let speed_bytes = instance.get_variable_bytes("speed").unwrap();
        assert_eq!(speed_bytes.len(), 4);

        // Set variable bytes
        let new_speed = 15.0_f32;
        assert!(instance.set_variable_bytes("speed", &new_speed.to_le_bytes()));

        // Verify change
        unsafe {
            assert_eq!(instance.get_variable::<f32>("speed"), Some(15.0));
        }
    }

    #[test]
    fn test_get_set_typed_variable() {
        let loaded = create_test_loaded_blueprint();
        let mut instance = BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, None);

        // Set typed variable
        unsafe {
            assert!(instance.set_variable("health", 200_i32));
            assert_eq!(instance.get_variable::<i32>("health"), Some(200));
        }
    }

    #[test]
    fn test_nonexistent_variable() {
        let loaded = create_test_loaded_blueprint();
        let instance = BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, None);

        assert_eq!(instance.get_variable_bytes("nonexistent"), None);
    }

    #[test]
    fn test_state_arena_access() {
        let loaded = create_test_loaded_blueprint();
        let mut instance = BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, None);

        let arena = instance.state_arena_mut().unwrap();
        assert!(arena.size() >= 9); // At least 9 bytes (f32 + i32 + bool)
    }

    #[test]
    fn entity_binding_round_trips() {
        let loaded = create_test_loaded_blueprint();
        let mut instance = BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, None);
        assert_eq!(instance.bound_entity(), None, "instances start unbound");

        let mut world = pulsar_scenedb::World::new();
        let entity = world.spawn();
        assert_eq!(instance.bind_entity(entity), None);
        assert_eq!(instance.bound_entity(), Some(entity));

        // Respawn flows rebind; the old binding is returned.
        let next = world.spawn();
        assert_eq!(instance.bind_entity(next), Some(entity));
        assert_eq!(instance.unbind_entity(), Some(next));
        assert_eq!(instance.bound_entity(), None);
    }

    /// #648 hot-reload: exact-match variables carry across a reload;
    /// changed layouts reset instead of aliasing old bytes.
    #[test]
    fn rehydrate_after_reload_preserves_matching_layout_only() {
        let loaded = create_test_loaded_blueprint();
        let mut overrides = HashMap::new();
        overrides.insert("speed".to_string(), JsonValue::from(7.5));
        let mut instance =
            BlueprintInstance::new_bytecode("obj_001".to_string(), &loaded, Some(overrides));

        // Same layout: speed survives, untouched health resets to default.
        let same_layout = create_test_loaded_blueprint();
        instance.rehydrate_after_reload(&same_layout);
        unsafe {
            assert_eq!(instance.get_variable::<f32>("speed"), Some(7.5));
            assert_eq!(instance.get_variable::<i32>("health"), Some(100));
            assert_eq!(instance.get_variable::<u8>("is_active"), Some(1));
        }

        // Drifted layout (health moved): old bytes must NOT appear at the
        // new slot. Plant a sentinel in the OLD health slot first — if
        // preservation were byte-blind, the sentinel would leak into the
        // reload.
        unsafe { assert!(instance.set_variable("health", 999_i32)) };
        let mut drifted = CompiledBytecode::new("TestBlueprint".to_string());
        drifted.add_variable(VariableDescriptor::f32("speed", 0, 5.0));
        drifted.add_variable(VariableDescriptor::i32("health", 16, 100));
        drifted.calculate_arena_size();
        let drifted_loaded = LoadedBlueprint {
            class_name: "TestBlueprint".to_string(),
            bytecode: drifted,
            programs: HashMap::new(),
        };
        instance.rehydrate_after_reload(&drifted_loaded);
        unsafe {
            assert_eq!(instance.get_variable::<f32>("speed"), Some(7.5));
            assert_eq!(
                instance.get_variable::<i32>("health"),
                Some(100),
                "moved variable resets to default instead of aliasing old bytes"
            );
        }
    }

    #[test]
    fn test_json_to_bytes_serialization() {
        // Test i32
        let bytes =
            BlueprintInstance::serialize_json_to_bytes(&JsonValue::from(42), "i32").unwrap();
        assert_eq!(bytes, 42_i32.to_le_bytes().to_vec());

        // Test f32
        let bytes = BlueprintInstance::serialize_json_to_bytes(
            &JsonValue::from(std::f32::consts::PI),
            "f32",
        )
        .unwrap();
        assert_eq!(bytes, std::f32::consts::PI.to_le_bytes().to_vec());

        // Test bool
        let bytes =
            BlueprintInstance::serialize_json_to_bytes(&JsonValue::from(true), "bool").unwrap();
        assert_eq!(bytes, vec![1]);

        let bytes =
            BlueprintInstance::serialize_json_to_bytes(&JsonValue::from(false), "bool").unwrap();
        assert_eq!(bytes, vec![0]);
    }
}

#[cfg(test)]
mod override_marshalling_tests {
    //! #649: compound variable overrides marshal through the reflection
    //! registry into VM blob encodings instead of failing with
    //! "Unsupported type".

    use super::*;

    #[test]
    fn string_overrides_marshal_to_blob_encoding() {
        let bytes =
            BlueprintInstance::serialize_json_to_bytes(&serde_json::json!("hello"), "String")
                .expect("String overrides must marshal");
        // [u64 len][utf8] — the encoding any host reader (bytes_to_any,
        // deserialize_named_bytes) speaks.
        assert_eq!(&bytes[..8], &5_u64.to_le_bytes());
        assert_eq!(&bytes[8..], b"hello");
    }

    #[test]
    fn vector_overrides_marshal_to_blob_encoding() {
        let json = serde_json::json!([1.0f32, 2.0, 3.0]);
        let bytes = BlueprintInstance::serialize_json_to_bytes(&json, "Vec<f32>")
            .expect("Vec overrides must marshal");
        let count = usize::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
            .expect("count fits");
        assert_eq!(count, 3, "[u64 elem_count][elems] for Vector kind");
        assert_eq!(bytes.len(), 8 + count * 4);
    }

    #[test]
    fn unknown_types_are_typed_errors_not_zeroed() {
        let err = BlueprintInstance::serialize_json_to_bytes(
            &serde_json::json!({}),
            "DefinitelyNotARealType",
        )
        .unwrap_err();
        assert!(
            err.contains("not registered"),
            "error should name the registry miss: {err}"
        );
    }
}
