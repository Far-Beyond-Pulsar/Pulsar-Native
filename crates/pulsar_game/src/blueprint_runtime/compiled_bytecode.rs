//! Compiled bytecode data structures for blueprint execution.

use pbgc::BpProgram;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Compiled bytecode representation of a blueprint class.
///
/// This structure contains all the information needed to instantiate and execute
/// a blueprint at runtime, including bytecode programs for each event, variable
/// descriptors, and metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledBytecode {
    /// Format version for backward compatibility
    pub version: u32,

    /// Source blueprint class name
    pub source_class: String,

    /// Variable descriptors (layout information)
    pub variables: Vec<VariableDescriptor>,

    /// Compiled bytecode programs for each event
    /// Key: event name (e.g., "begin_play", "tick")
    /// Value: Compiled bytecode program
    pub event_programs: HashMap<String, BpProgram>,

    /// Total arena size needed for instance state (in bytes)
    pub arena_size: usize,
}

/// Descriptor for a blueprint variable.
///
/// Contains all information needed to allocate and access a variable
/// in the bytecode arena.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VariableDescriptor {
    /// Variable name
    pub name: String,

    /// Data type as string (e.g., "f32", "Vec3", "String")
    pub data_type: String,

    /// Size in bytes
    pub size: usize,

    /// Alignment requirement
    pub align: usize,

    /// Offset in the arena (in bytes)
    pub offset: usize,

    /// Default value as raw bytes
    pub default_value: Vec<u8>,
}

impl CompiledBytecode {
    /// Create a new empty compiled bytecode structure.
    pub fn new(source_class: impl Into<String>) -> Self {
        Self {
            version: 1,
            source_class: source_class.into(),
            variables: Vec::new(),
            event_programs: HashMap::new(),
            arena_size: 0,
        }
    }

    /// Add a variable descriptor.
    pub fn add_variable(&mut self, descriptor: VariableDescriptor) {
        self.variables.push(descriptor);
    }

    /// Add an event program.
    pub fn add_event_program(&mut self, event_name: impl Into<String>, program: BpProgram) {
        self.event_programs.insert(event_name.into(), program);
    }

    /// Calculate total arena size based on variables.
    pub fn calculate_arena_size(&mut self) {
        self.arena_size = self
            .variables
            .iter()
            .map(|v| v.offset + v.size)
            .max()
            .unwrap_or(0)
            .max(1024); // Minimum 1KB arena
    }

    /// Get a variable descriptor by name.
    pub fn get_variable(&self, name: &str) -> Option<&VariableDescriptor> {
        self.variables.iter().find(|v| v.name == name)
    }

    /// Get an event program by name.
    pub fn get_event_program(&self, event_name: &str) -> Option<&BpProgram> {
        self.event_programs.get(event_name)
    }

    /// List all available events.
    pub fn event_names(&self) -> Vec<&str> {
        self.event_programs.keys().map(|s| s.as_str()).collect()
    }
}

impl VariableDescriptor {
    /// Create a new variable descriptor.
    pub fn new(
        name: impl Into<String>,
        data_type: impl Into<String>,
        size: usize,
        align: usize,
        offset: usize,
        default_value: Vec<u8>,
    ) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            size,
            align,
            offset,
            default_value,
        }
    }

    /// Create a descriptor for an f32 variable.
    pub fn f32(name: impl Into<String>, offset: usize, default: f32) -> Self {
        Self::new(name, "f32", 4, 4, offset, default.to_le_bytes().to_vec())
    }

    /// Create a descriptor for an i32 variable.
    pub fn i32(name: impl Into<String>, offset: usize, default: i32) -> Self {
        Self::new(name, "i32", 4, 4, offset, default.to_le_bytes().to_vec())
    }

    /// Create a descriptor for a bool variable.
    pub fn bool(name: impl Into<String>, offset: usize, default: bool) -> Self {
        Self::new(
            name,
            "bool",
            1,
            1,
            offset,
            vec![if default { 1 } else { 0 }],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compiled_bytecode_creation() {
        let mut bytecode = CompiledBytecode::new("TestBlueprint");

        assert_eq!(bytecode.source_class, "TestBlueprint");
        assert_eq!(bytecode.version, 1);
        assert!(bytecode.variables.is_empty());
        assert!(bytecode.event_programs.is_empty());
    }

    #[test]
    fn test_variable_descriptor() {
        let var = VariableDescriptor::f32("health", 0, 100.0);

        assert_eq!(var.name, "health");
        assert_eq!(var.data_type, "f32");
        assert_eq!(var.size, 4);
        assert_eq!(var.align, 4);
        assert_eq!(var.offset, 0);
        assert_eq!(var.default_value, 100.0_f32.to_le_bytes().to_vec());
    }

    #[test]
    fn test_arena_size_calculation() {
        let mut bytecode = CompiledBytecode::new("Test");

        bytecode.add_variable(VariableDescriptor::f32("a", 0, 1.0));
        bytecode.add_variable(VariableDescriptor::i32("b", 4, 2));
        bytecode.add_variable(VariableDescriptor::bool("c", 8, true));

        bytecode.calculate_arena_size();

        assert_eq!(bytecode.arena_size, 1024); // Min 1KB

        // Add more variables to exceed 1KB
        for i in 0..300 {
            bytecode.add_variable(VariableDescriptor::f32(
                format!("var_{}", i),
                9 + (i * 4),
                0.0,
            ));
        }

        bytecode.calculate_arena_size();
        assert!(bytecode.arena_size > 1024);
    }
}
