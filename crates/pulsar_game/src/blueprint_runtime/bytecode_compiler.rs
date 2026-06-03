//! Bytecode compiler for blueprint classes.
//!
//! Compiles blueprint graphs from Plugin_Blueprints `.class` folders into
//! executable bytecode using PBGC (Pulsar Blueprint Graph Compiler).

use super::compiled_bytecode::{CompiledBytecode, VariableDescriptor};
use blueprint_compiler::{
    compile_graph_to_bytecode, BpProgram, GraphDescription as PbgcGraphDescription,
};
use pulsar_graph::{BlueprintAsset, ClassVariable, GraphDescription};
use std::collections::HashMap;
use std::path::Path;

/// Bytecode compiler for blueprint classes.
pub struct BytecodeCompiler {
    /// Compiler options and state
    _options: CompilerOptions,
}

/// Compiler options and settings.
#[derive(Debug, Clone)]
pub struct CompilerOptions {
    /// Enable optimization passes
    pub optimize: bool,

    /// Generate debug symbols
    pub debug_symbols: bool,
}

impl Default for CompilerOptions {
    fn default() -> Self {
        Self {
            optimize: true,
            debug_symbols: true,
        }
    }
}

/// Error type for bytecode compilation.
#[derive(Debug)]
pub enum CompilerError {
    /// IO error (file not found, permission denied, etc.)
    Io(std::io::Error),

    /// JSON parsing error
    Json(serde_json::Error),

    /// Blueprint compilation error from PBGC
    Compilation(String),

    /// Invalid blueprint structure
    Invalid(String),
}

impl std::fmt::Display for CompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompilerError::Io(e) => write!(f, "IO error: {}", e),
            CompilerError::Json(e) => write!(f, "JSON error: {}", e),
            CompilerError::Compilation(e) => write!(f, "Compilation error: {}", e),
            CompilerError::Invalid(e) => write!(f, "Invalid blueprint: {}", e),
        }
    }
}

impl std::error::Error for CompilerError {}

impl From<std::io::Error> for CompilerError {
    fn from(e: std::io::Error) -> Self {
        CompilerError::Io(e)
    }
}

impl From<serde_json::Error> for CompilerError {
    fn from(e: serde_json::Error) -> Self {
        CompilerError::Json(e)
    }
}

impl BytecodeCompiler {
    /// Create a new bytecode compiler with default options.
    pub fn new() -> Self {
        Self {
            _options: CompilerOptions::default(),
        }
    }

    /// Create a new bytecode compiler with custom options.
    pub fn with_options(options: CompilerOptions) -> Self {
        Self { _options: options }
    }

    /// Compile a blueprint class from a `.class` folder to bytecode.
    ///
    /// # Arguments
    /// * `class_path` - Path to the `.class` folder
    ///
    /// # Returns
    /// Compiled bytecode ready for execution.
    pub fn compile_class(&self, class_path: &Path) -> Result<CompiledBytecode, CompilerError> {
        // Load graph_save.json
        let graph_save_path = class_path.join("graph_save.json");

        if !graph_save_path.exists() {
            return Err(CompilerError::Invalid(format!(
                "graph_save.json not found in {:?}",
                class_path
            )));
        }

        let json = std::fs::read_to_string(&graph_save_path)?;
        let blueprint: BlueprintAsset = serde_json::from_str(&json)?;

        self.compile_blueprint(&blueprint)
    }

    /// Compile a BlueprintAsset to bytecode.
    ///
    /// This is the main compilation entry point.
    pub fn compile_blueprint(
        &self,
        blueprint: &BlueprintAsset,
    ) -> Result<CompiledBytecode, CompilerError> {
        let mut compiled =
            CompiledBytecode::new(blueprint.blueprint_metadata.blueprint_type.clone());

        // Extract and compile variables
        let variables = self.compile_variables(&blueprint.variables)?;
        for var in variables {
            compiled.add_variable(var);
        }

        // Calculate arena size
        compiled.calculate_arena_size();

        // Extract event graphs and compile each
        let event_graphs = self.extract_event_graphs(&blueprint.main_graph)?;

        for (event_name, event_graph) in event_graphs {
            let program = self.compile_event_graph(&event_graph)?;
            compiled.add_event_program(event_name, program);
        }

        Ok(compiled)
    }

    /// Compile blueprint variables to descriptors.
    fn compile_variables(
        &self,
        variables: &[ClassVariable],
    ) -> Result<Vec<VariableDescriptor>, CompilerError> {
        let mut descriptors = Vec::new();
        let mut current_offset = 0;

        for var in variables {
            let (size, align) = self.get_type_size_align(&var.data_type);

            // Align offset
            if current_offset % align != 0 {
                current_offset += align - (current_offset % align);
            }

            let default_value =
                self.parse_default_value(&var.data_type, var.default_value.as_deref())?;

            descriptors.push(VariableDescriptor::new(
                var.name.clone(),
                format!("{:?}", var.data_type),
                size,
                align,
                current_offset,
                default_value,
            ));

            current_offset += size;
        }

        Ok(descriptors)
    }

    /// Get size and alignment for a data type.
    fn get_type_size_align(&self, data_type: &pulsar_graph::DataType) -> (usize, usize) {
        use pulsar_graph::DataType;

        match data_type {
            DataType::Execution => (0, 1), // Execution has no data
            DataType::Data(type_info) => {
                let base_type = type_info.base_type.as_str();

                // Check for wrapper types that affect size
                if !type_info.wrappers.is_empty() {
                    use pulsar_graph::WrapperType;

                    // Most wrappers are pointer-sized structures
                    for wrapper in &type_info.wrappers {
                        match wrapper {
                            WrapperType::Vec => return (24, 8),     // Vec<T> is 24 bytes
                            WrapperType::HashMap => return (48, 8), // HashMap is larger
                            WrapperType::HashSet => return (48, 8), // HashSet is similar
                            WrapperType::Arc | WrapperType::Box => return (8, 8), // Pointer
                            WrapperType::Ref | WrapperType::RefMut => return (8, 8), // Reference
                            WrapperType::Option => {
                                // Option adds discriminant
                                let (inner_size, inner_align) =
                                    self.get_base_type_size_align(base_type);
                                return (inner_size + 1, inner_align);
                            }
                            WrapperType::Result => return (16, 8), // Result<T, E> approximate
                        }
                    }
                }

                // No wrappers, get base type size
                self.get_base_type_size_align(base_type)
            }
        }
    }

    /// Get size and alignment for a base type (without wrappers).
    fn get_base_type_size_align(&self, base_type: &str) -> (usize, usize) {
        match base_type {
            "i32" => (4, 4),
            "i64" => (8, 8),
            "f32" => (4, 4),
            "f64" => (8, 8),
            "bool" => (1, 1),
            "String" => (24, 8), // String struct (pointer + len + cap)
            "()" => (0, 1),      // Unit type
            _ => {
                // For custom types, assume pointer-sized
                tracing::warn!("Unknown size for type: {}", base_type);
                (8, 8)
            }
        }
    }

    /// Parse default value from string to bytes.
    fn parse_default_value(
        &self,
        data_type: &pulsar_graph::DataType,
        default: Option<&str>,
    ) -> Result<Vec<u8>, CompilerError> {
        use pulsar_graph::DataType;

        if default.is_none() {
            // Return zero bytes for the type
            let (size, _) = self.get_type_size_align(data_type);
            return Ok(vec![0; size]);
        }

        let default = default.unwrap();

        match data_type {
            DataType::Execution => Ok(vec![]),
            DataType::Data(type_info) => {
                let base_type = type_info.base_type.as_str();

                match base_type {
                    "i32" => {
                        let value: i32 = default.parse().map_err(|_| {
                            CompilerError::Invalid(format!("Invalid i32: {}", default))
                        })?;
                        Ok(value.to_le_bytes().to_vec())
                    }
                    "i64" => {
                        let value: i64 = default.parse().map_err(|_| {
                            CompilerError::Invalid(format!("Invalid i64: {}", default))
                        })?;
                        Ok(value.to_le_bytes().to_vec())
                    }
                    "f32" => {
                        let value: f32 = default.parse().map_err(|_| {
                            CompilerError::Invalid(format!("Invalid f32: {}", default))
                        })?;
                        Ok(value.to_le_bytes().to_vec())
                    }
                    "f64" => {
                        let value: f64 = default.parse().map_err(|_| {
                            CompilerError::Invalid(format!("Invalid f64: {}", default))
                        })?;
                        Ok(value.to_le_bytes().to_vec())
                    }
                    "bool" => {
                        let value = match default.to_lowercase().as_str() {
                            "true" | "1" => true,
                            "false" | "0" => false,
                            _ => {
                                return Err(CompilerError::Invalid(format!(
                                    "Invalid bool: {}",
                                    default
                                )))
                            }
                        };
                        Ok(vec![if value { 1 } else { 0 }])
                    }
                    _ => {
                        // For complex types, return zero bytes
                        let (size, _) = self.get_type_size_align(data_type);
                        Ok(vec![0; size])
                    }
                }
            }
        }
    }

    /// Extract event subgraphs from the main graph.
    ///
    /// Returns a map of event name -> event graph.
    fn extract_event_graphs(
        &self,
        main_graph: &GraphDescription,
    ) -> Result<HashMap<String, GraphDescription>, CompilerError> {
        let mut events = HashMap::new();

        // Find all nodes that start with "Event_"
        for (node_id, node) in &main_graph.nodes {
            if node.node_type.starts_with("Event_") {
                let event_name = node
                    .node_type
                    .strip_prefix("Event_")
                    .unwrap()
                    .to_lowercase();

                // Extract subgraph starting from this event node
                let subgraph = self.extract_subgraph_from_node(main_graph, node_id)?;

                events.insert(event_name, subgraph);
            }
        }

        if events.is_empty() {
            tracing::warn!("No event nodes found in blueprint");
        }

        Ok(events)
    }

    /// Extract a subgraph starting from a specific node.
    fn extract_subgraph_from_node(
        &self,
        graph: &GraphDescription,
        start_node: &str,
    ) -> Result<GraphDescription, CompilerError> {
        // For now, return the entire graph
        // In a more sophisticated implementation, we'd trace execution paths
        // and only include reachable nodes

        Ok(graph.clone())
    }

    /// Compile an event graph to bytecode.
    fn compile_event_graph(&self, graph: &GraphDescription) -> Result<BpProgram, CompilerError> {
        let bridge_json = serde_json::to_value(graph)?;
        let pbgc_graph: PbgcGraphDescription = serde_json::from_value(bridge_json)
            .map_err(|e| CompilerError::Compilation(format!("Graph conversion failed: {}", e)))?;

        // Use PBGC to compile the graph
        let programs = compile_graph_to_bytecode(&pbgc_graph)
            .map_err(|e| CompilerError::Compilation(format!("{:?}", e)))?;

        if programs.is_empty() {
            return Err(CompilerError::Compilation(
                "PBGC returned no programs".to_string(),
            ));
        }

        // Take the first program (PBGC may return multiple for complex graphs)
        Ok(programs.into_iter().next().unwrap())
    }
}

impl Default for BytecodeCompiler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compiler_creation() {
        let compiler = BytecodeCompiler::new();
        assert!(compiler._options.optimize);
        assert!(compiler._options.debug_symbols);
    }

    #[test]
    fn test_type_sizes() {
        let compiler = BytecodeCompiler::new();

        use pulsar_graph::{DataType, TypeInfo};

        let i32_type = DataType::Data(TypeInfo {
            base_type: "i32".to_string(),
            wrappers: vec![],
            is_wildcard: false,
        });

        let f32_type = DataType::Data(TypeInfo {
            base_type: "f32".to_string(),
            wrappers: vec![],
            is_wildcard: false,
        });

        let bool_type = DataType::Data(TypeInfo {
            base_type: "bool".to_string(),
            wrappers: vec![],
            is_wildcard: false,
        });

        assert_eq!(compiler.get_type_size_align(&i32_type), (4, 4));
        assert_eq!(compiler.get_type_size_align(&f32_type), (4, 4));
        assert_eq!(compiler.get_type_size_align(&bool_type), (1, 1));
    }

    #[test]
    fn test_parse_default_values() {
        let compiler = BytecodeCompiler::new();

        use pulsar_graph::{DataType, TypeInfo};

        let i32_type = DataType::Data(TypeInfo {
            base_type: "i32".to_string(),
            wrappers: vec![],
            is_wildcard: false,
        });

        let f32_type = DataType::Data(TypeInfo {
            base_type: "f32".to_string(),
            wrappers: vec![],
            is_wildcard: false,
        });

        let bool_type = DataType::Data(TypeInfo {
            base_type: "bool".to_string(),
            wrappers: vec![],
            is_wildcard: false,
        });

        // Test i32
        let bytes = compiler.parse_default_value(&i32_type, Some("42")).unwrap();
        assert_eq!(bytes, 42_i32.to_le_bytes().to_vec());

        // Test f32
        let bytes = compiler
            .parse_default_value(&f32_type, Some("3.14"))
            .unwrap();
        assert_eq!(bytes, 3.14_f32.to_le_bytes().to_vec());

        // Test bool
        let bytes = compiler
            .parse_default_value(&bool_type, Some("true"))
            .unwrap();
        assert_eq!(bytes, vec![1]);

        let bytes = compiler
            .parse_default_value(&bool_type, Some("false"))
            .unwrap();
        assert_eq!(bytes, vec![0]);

        // Test default (None)
        let bytes = compiler.parse_default_value(&i32_type, None).unwrap();
        assert_eq!(bytes, vec![0, 0, 0, 0]);
    }
}
