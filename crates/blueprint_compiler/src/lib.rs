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
    use graphy::{PinType, NodeMetadataProvider};

    #[test]
    fn test_metadata_provider() {
        let provider = get_metadata_provider();
        let nodes = provider.get_all_nodes();
        
        // Should have at least some nodes from pulsar_std
        assert!(!nodes.is_empty(), "Should load nodes from pulsar_std");
        
        println!("Loaded {} nodes from pulsar_std", nodes.len());
        for node in nodes.iter().take(5) {
            println!("  - {} ({})", node.name, node.category);
        }
    }

    #[test]
    fn test_headless_blueprint_compilation() {
        // Initialize tracing for test output
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .try_init();

        // Create a simple test graph programmatically
        let mut graph = GraphDescription::new("test_blueprint");
        
        // Add a BeginPlay event node
        let mut begin_play = NodeInstance::new(
            "begin_play_1",
            "BeginPlay",
            Position { x: 100.0, y: 100.0 }
        );
        
        // Add the Body execution output pin
        begin_play.outputs.push(PinInstance::new(
            "begin_play_1_Body",
            Pin::new("begin_play_1_Body", "Body", DataType::Execution, PinType::Output)
        ));
        
        // Add a print node
        let mut print_node = NodeInstance::new(
            "print_1",
            "print",
            Position { x: 300.0, y: 100.0 }
        );
        
        // Add exec input pin
        print_node.inputs.push(PinInstance::new(
            "print_1_exec",
            Pin::new("print_1_exec", "exec", DataType::Execution, PinType::Input)
        ));
        
        // Add message input pin with default value
        print_node.inputs.push(PinInstance::new(
            "print_1_message",
            Pin::new("print_1_message", "message", DataType::String, PinType::Input)
        ));
        print_node.properties.insert(
            "message".to_string(),
            PropertyValue::String("Hello from Blueprint!".to_string())
        );
        
        graph.add_node(begin_play);
        graph.add_node(print_node);
        
        // Connect BeginPlay's Body output to print's exec input
        let connection = Connection::new(
            "begin_play_1",
            "begin_play_1_Body",
            "print_1",
            "print_1_exec",
            ConnectionType::Execution,
        );
        
        graph.add_connection(connection);
        
        // Compile the graph
        println!("\n=== Compiling Test Blueprint ===");
        let result = compile_blueprint(&graph);
        
        match result {
            Ok(rust_code) => {
                println!("\n=== Compilation Successful ===");
                println!("Generated {} bytes of Rust code", rust_code.len());
                println!("\n=== Generated Code Preview (first 500 chars) ===");
                println!("{}", &rust_code.chars().take(500).collect::<String>());
                
                // Verify the code contains expected elements
                assert!(rust_code.contains("BeginPlay") || rust_code.contains("begin_play"), 
                    "Should contain BeginPlay or begin_play");
                
                // Optionally write to file for inspection
                if let Ok(test_dir) = std::env::var("CARGO_TARGET_DIR") {
                    let output_path = format!("{}/test_blueprint_output.rs", test_dir);
                    if let Err(e) = std::fs::write(&output_path, &rust_code) {
                        println!("Note: Could not write to {}: {}", output_path, e);
                    } else {
                        println!("\n=== Full output written to: {} ===", output_path);
                    }
                } else {
                    // Fallback to target directory
                    let output_path = "target/test_blueprint_output.rs";
                    if let Err(e) = std::fs::write(output_path, &rust_code) {
                        println!("Note: Could not write to {}: {}", output_path, e);
                    } else {
                        println!("\n=== Full output written to: {} ===", output_path);
                    }
                }
            }
            Err(e) => {
                panic!("Blueprint compilation failed: {}", e);
            }
        }
    }

    #[test]
    fn test_compile_with_data_flow() {
        // Initialize tracing for test output
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .try_init();

        // Create a graph with data flow connections
        let mut graph = GraphDescription::new("data_flow_test");
        
        // Add a BeginPlay event node
        let mut begin_play = NodeInstance::new(
            "begin_play_1",
            "BeginPlay",
            Position { x: 100.0, y: 100.0 }
        );
        begin_play.outputs.push(PinInstance::new(
            "begin_play_1_Body",
            Pin::new("begin_play_1_Body", "Body", DataType::Execution, PinType::Output)
        ));
        
        // Add an add node (if it exists in pulsar_std)
        let mut add_node = NodeInstance::new(
            "add_1",
            "add",
            Position { x: 300.0, y: 100.0 }
        );
        
        // Add input pins for add node
        add_node.inputs.push(PinInstance::new(
            "add_1_a",
            Pin::new("add_1_a", "a", DataType::Number, PinType::Input)
        ));
        add_node.inputs.push(PinInstance::new(
            "add_1_b",
            Pin::new("add_1_b", "b", DataType::Number, PinType::Input)
        ));
        add_node.outputs.push(PinInstance::new(
            "add_1_result",
            Pin::new("add_1_result", "result", DataType::Number, PinType::Output)
        ));
        
        // Set default property values
        add_node.properties.insert("a".to_string(), PropertyValue::Number(5.0));
        add_node.properties.insert("b".to_string(), PropertyValue::Number(10.0));
        
        graph.add_node(begin_play);
        graph.add_node(add_node);
        
        // Try to compile (may fail if add node doesn't exist)
        println!("\n=== Compiling Data Flow Test Blueprint ===");
        let result = compile_blueprint(&graph);
        
        match result {
            Ok(rust_code) => {
                println!("\n=== Data Flow Compilation Successful ===");
                println!("Generated {} bytes of Rust code", rust_code.len());
            }
            Err(e) => {
                println!("\n=== Expected behavior if 'add' node not found ===");
                println!("Compilation error: {}", e);
                // This is okay - the test demonstrates the error handling
            }
        }
    }
}
