//! # Blueprint Compiler
//!
//! Wrapper around PBGC (Pulsar Blueprint Graph Compiler) integrated with the Pulsar engine.
//!
//! This crate provides the main API for compiling Blueprint graphs within the engine,
//! delegating the heavy lifting to PBGC while providing engine-specific conveniences.

// Re-export the main PBGC API
pub use pbgc::{
    compile_graph,
    compile_graph_with_library_manager,
    compile_graph_with_variables,
    // Re-export Graphy types for convenience
    GraphDescription, NodeInstance, Connection, Pin, PinInstance,
    DataType, NodeTypes, Position, ConnectionType, PropertyValue,
    GraphMetadata, Result, GraphyError,
};

// Re-export metadata provider
pub use pbgc::BlueprintMetadataProvider;

/// Get the Blueprint metadata provider
///
/// This provides access to all registered Blueprint nodes from pulsar_std.
pub fn get_metadata_provider() -> BlueprintMetadataProvider {
    BlueprintMetadataProvider::new()
}

/// Compile a Blueprint graph to Rust code
///
/// This is a convenience function that wraps PBGC's compile_graph with
/// engine-specific logging and error handling.
///
/// # Arguments
///
/// * `graph` - The Blueprint graph to compile
///
/// # Returns
///
/// * `Ok(String)` - The generated Rust source code
/// * `Err(GraphyError)` - Compilation error with details
///
/// # Example
///
/// ```no_run
/// use blueprint_compiler::{compile_blueprint, GraphDescription};
///
/// let graph = GraphDescription::new("my_blueprint");
/// // ... add nodes and connections
///
/// match compile_blueprint(&graph) {
///     Ok(rust_code) => {
///         println!("Compiled successfully!");
///         std::fs::write("generated.rs", rust_code).unwrap();
///     }
///     Err(e) => eprintln!("Compilation failed: {}", e),
/// }
/// ```
pub fn compile_blueprint(graph: &GraphDescription) -> Result<String> {
    tracing::info!("[Blueprint Compiler] Compiling graph: {}", graph.metadata.name);
    
    let result = compile_graph(graph)?;
    
    tracing::info!("[Blueprint Compiler] Successfully compiled graph: {}", graph.metadata.name);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_provider() {
        let provider = get_metadata_provider();
        let nodes = provider.get_all_nodes();
        
        // Should have at least some nodes from pulsar_std
        assert!(!nodes.is_empty(), "Should load nodes from pulsar_std");
    }
}
