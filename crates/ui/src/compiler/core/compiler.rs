//! # Main Compiler Entry Points
//!
//! Provides the primary public API for compiling Blueprint graphs to Rust code.
//!
//! This module contains the main `compile_graph` function that orchestrates the entire
//! compilation pipeline, as well as helper functions for specific compilation scenarios.

use std::collections::HashMap;
use std::sync::OnceLock;
use crate::graph::GraphDescription;
use super::metadata::{NodeMetadata, extract_node_metadata};
use crate::compiler::analysis::{DataResolver, ExecutionRouting};
use crate::compiler::generation;
use crate::compiler::utils::SubGraphExpander;

/// Cached node metadata from pulsar_std
///
/// Node metadata is extracted once on first use and cached for subsequent compilations.
/// This avoids repeatedly parsing the pulsar_std crate.
static NODE_METADATA: OnceLock<HashMap<String, NodeMetadata>> = OnceLock::new();

/// Get cached node metadata, initializing if necessary
///
/// This function provides thread-safe, lazy initialization of node metadata.
/// The metadata is extracted from pulsar_std on first call and cached for
/// all subsequent calls.
///
/// # Returns
///
/// A reference to the cached metadata map, indexed by node name.
pub fn get_node_metadata() -> &'static HashMap<String, NodeMetadata> {
    NODE_METADATA.get_or_init(|| {
        match extract_node_metadata() {
            Ok(metadata) => {
                tracing::info!("[COMPILER] Loaded {} node definitions from pulsar_std", metadata.len());
                metadata
            }
            Err(e) => {
                tracing::error!("[COMPILER] Failed to extract node metadata: {}", e);
                HashMap::new()
            }
        }
    })
}

/// Compile a Blueprint graph to Rust source code
///
/// This is the main entry point for the Blueprint compiler. It takes a visual node graph
/// and generates equivalent Rust source code that can be compiled and executed.
///
/// # Arguments
///
/// * `graph` - The Blueprint graph to compile
///
/// # Returns
///
/// * `Ok(String)` - The generated Rust source code
/// * `Err(String)` - A descriptive error message if compilation fails
///
/// # Examples
///
/// ```rust,no_run
/// use pulsar_engine::compiler::core::compile_graph;
/// use pulsar_engine::graph::GraphDescription;
///
/// let graph = GraphDescription::new("test");
/// match compile_graph(&graph) {
///     Ok(code) => println!("Generated:\n{}", code),
///     Err(e) => eprintln!("Error: {}", e),
/// }
/// ```
///
/// # Compilation Phases
///
/// 1. **Metadata Loading** - Extract node definitions from pulsar_std
/// 2. **Data Flow Analysis** - Build dependency graph and evaluation order  
/// 3. **Execution Flow Analysis** - Map execution connections between nodes
/// 4. **Code Generation** - Generate Rust code using node-specific strategies
///
/// # Error Conditions
///
/// * No event nodes found (e.g., no `main` or `begin_play` entry point)
/// * Unknown node types referenced
/// * Type mismatches in connections
/// * Circular dependencies in pure nodes
/// * Missing required connections
pub fn compile_graph(graph: &GraphDescription) -> Result<String, String> {
    compile_graph_with_library_manager(graph, None)
}

/// Compile a Blueprint graph with sub-graph expansion support
///
/// This extended version of `compile_graph` supports expanding sub-graph instances
/// before compilation. Sub-graphs are Blueprint macros that can be instantiated
/// multiple times within a graph.
///
/// # Arguments
///
/// * `graph` - The Blueprint graph to compile
/// * `library_manager` - Optional library manager providing sub-graph definitions
///
/// # Returns
///
/// * `Ok(String)` - The generated Rust source code
/// * `Err(String)` - A descriptive error message if compilation fails
///
/// # Sub-Graph Expansion
///
/// When a library manager is provided:
/// 1. All sub-graph instances are identified (nodes with `subgraph:` or `macro:` prefix)
/// 2. Each instance is expanded inline, replacing it with its constituent nodes
/// 3. Connections are rewired through the sub-graph's input/output nodes
/// 4. Expansion happens recursively for nested sub-graphs
/// 5. Circular reference detection prevents infinite expansion
///
/// # Examples
///
/// ```rust,no_run
/// use pulsar_engine::compiler::core::compile_graph_with_library_manager;
/// use pulsar_engine::graph::{GraphDescription, LibraryManager};
///
/// let graph = GraphDescription::new("test");
/// let lib_manager = LibraryManager::new();
/// // ... register sub-graph libraries ...
///
/// match compile_graph_with_library_manager(&graph, Some(lib_manager)) {
///     Ok(code) => println!("Generated:\n{}", code),
///     Err(e) => eprintln!("Error: {}", e),
/// }
/// ```
pub fn compile_graph_with_library_manager(
    graph: &GraphDescription,
    library_manager: Option<crate::graph::LibraryManager>,
) -> Result<String, String> {
    tracing::info!("[COMPILER] Starting Blueprint compilation");
    tracing::info!("[COMPILER] Graph: {} ({} nodes, {} connections)",
             graph.metadata.name,
             graph.nodes.len(),
             graph.connections.len());

    // Create a mutable copy for expansion
    let mut expanded_graph = graph.clone();

    // Phase 0: Expand sub-graphs if library manager is provided
    if let Some(lib_manager) = library_manager {
        tracing::info!("[COMPILER] Phase 0: Expanding sub-graphs...");
        let expander = SubGraphExpander::new(lib_manager);
        expander.expand_all(&mut expanded_graph)?;
        tracing::info!("[COMPILER] Sub-graph expansion complete ({} nodes after expansion)",
                 expanded_graph.nodes.len());
    }

    // Phase 1: Get node metadata
    tracing::info!("[COMPILER] Phase 1: Loading node metadata...");
    let metadata = get_node_metadata();
    if metadata.is_empty() {
        return Err("No node metadata available - ensure pulsar_std is accessible".to_string());
    }
    tracing::info!("[COMPILER] Loaded {} node types", metadata.len());

    // Phase 2: Build data flow resolver (no variables for generic compile)
    tracing::info!("[COMPILER] Phase 2: Analyzing data flow...");
    let variables = HashMap::new();
    let data_resolver = DataResolver::build_with_variables(&expanded_graph, metadata, variables.clone())?;
    tracing::info!("[COMPILER] Data flow analysis complete");
    tracing::info!("[COMPILER]   - {} pure nodes in evaluation order", 
             data_resolver.get_pure_evaluation_order().len());

    // Phase 3: Build execution routing
    tracing::info!("[COMPILER] Phase 3: Analyzing execution flow...");
    let exec_routing = ExecutionRouting::build_from_graph(&expanded_graph);
    tracing::info!("[COMPILER] Execution flow analysis complete");

    // Phase 4: Generate code
    tracing::info!("[COMPILER] Phase 4: Generating Rust code...");
    let code = generation::code_generator::generate_program(
        &expanded_graph,
        metadata,
        &data_resolver,
        &exec_routing,
        variables,
    )?;

    tracing::info!("[COMPILER] Code generation complete ({} bytes)", code.len());
    tracing::info!("[COMPILER] Compilation successful!");

    Ok(code)
}

/// Compile a graph with class variables
///
/// This variant supports Blueprint classes with member variables. The variables
/// are generated with appropriate thread-safe wrappers (Cell/RefCell + Arc).
///
/// # Arguments
///
/// * `graph` - The Blueprint graph to compile
/// * `variables` - Map of variable names to their Rust types
///
/// # Returns
///
/// * `Ok(String)` - The generated Rust source code including variable declarations
/// * `Err(String)` - A descriptive error message if compilation fails
///
/// # Variable Generation
///
/// Variables are generated with thread-safe wrappers:
/// - Copy types: `Arc<Cell<T>>`
/// - Non-Copy types: `Arc<RefCell<T>>`
///
/// This allows variables to be safely accessed and modified across threads
/// (e.g., in spawned threads from `thread_spawn` node).
pub fn compile_graph_with_variables(
    graph: &GraphDescription,
    variables: HashMap<String, String>,
) -> Result<String, String> {
    tracing::info!("[COMPILER] Compiling with {} class variables", variables.len());

    let mut expanded_graph = graph.clone();
    let metadata = get_node_metadata();

    if metadata.is_empty() {
        return Err("No node metadata available".to_string());
    }

    // Build data flow resolver with variables
    let data_resolver = DataResolver::build_with_variables(&expanded_graph, metadata, variables.clone())?;
    let exec_routing = ExecutionRouting::build_from_graph(&expanded_graph);

    // Generate code
    let code = generation::code_generator::generate_program(
        &expanded_graph,
        metadata,
        &data_resolver,
        &exec_routing,
        variables,
    )?;

    Ok(code)
}

/// Legacy compilation entry point (deprecated)
///
/// This function is kept for backward compatibility but returns an error.
/// The old .tron template system has been replaced by the macro-based system.
///
/// # Deprecated
///
/// Use `compile_graph` instead. The new compiler uses the `#[blueprint]` macro
/// system from pulsar_std, which is more maintainable and type-safe than the
/// old template-based approach.
#[deprecated(note = "Use compile_graph instead - this uses the obsolete .tron template system")]
pub fn compile_graph_legacy(graph: &GraphDescription) -> Result<String, String> {
    tracing::warn!("[COMPILER] Warning: Legacy .tron template compiler is no longer supported");
    Err("Legacy compiler has been removed - use new macro-based compiler via compile_graph()".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;

    #[test]
    fn test_metadata_caching() {
        // First call initializes
        let meta1 = get_node_metadata();
        
        // Second call returns cached
        let meta2 = get_node_metadata();
        
        // Should be same pointer (cached)
        assert_eq!(meta1 as *const _, meta2 as *const _);
    }

    #[test]
    fn test_compile_empty_graph() {
        let graph = GraphDescription::new("empty");
        
        // Should fail - no event nodes
        let result = compile_graph(&graph);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No event nodes"));
    }

    #[test]
    fn test_compile_simple_main() {
        let mut graph = GraphDescription::new("simple");
        
        // Add main event
        let mut main_node = NodeInstance::new("main", "main", Position { x: 0.0, y: 0.0 });
        main_node.add_output_pin("Body", DataType::Execution);
        graph.add_node(main_node);
        
        // Compile
        let result = compile_graph(&graph);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());
        
        let code = result.unwrap();
        assert!(code.contains("pub fn main"), "Should generate main function");
    }
}
