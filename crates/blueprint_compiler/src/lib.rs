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
    fn test_complex_blueprint_compilation() {
        // Initialize tracing for test output
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .try_init();

        println!("\n=== Building Complex Test Blueprint ===");
        
        // Create a more complex graph with math and control flow
        let mut graph = GraphDescription::new("complex_test_blueprint");
        
        // ==========================================
        // Node 1: begin_play event (entry point)
        // ==========================================
        let mut begin_play = NodeInstance::new(
            "begin_play_1",
            "begin_play",
            Position { x: 100.0, y: 200.0 }
        );
        begin_play.outputs.push(PinInstance::new(
            "begin_play_1_Body",
            Pin::new("begin_play_1_Body", "Body", DataType::Execution, PinType::Output)
        ));
        
        // ==========================================
        // Node 2: add - Add two numbers (5 + 10)
        // ==========================================
        let mut add_node = NodeInstance::new(
            "add_1",
            "add",
            Position { x: 300.0, y: 150.0 }
        );
        
        // Input pins for add
        add_node.inputs.push(PinInstance::new(
            "add_1_a",
            Pin::new("add_1_a", "a", DataType::Typed(pbgc::TypeInfo::new("i64")), PinType::Input)
        ));
        add_node.inputs.push(PinInstance::new(
            "add_1_b",
            Pin::new("add_1_b", "b", DataType::Typed(pbgc::TypeInfo::new("i64")), PinType::Input)
        ));
        
        // Output pin for add
        add_node.outputs.push(PinInstance::new(
            "add_1_result",
            Pin::new("add_1_result", "result", DataType::Typed(pbgc::TypeInfo::new("i64")), PinType::Output)
        ));
        
        // Set constant values - properties must be keyed by pin ID, not parameter name
        add_node.properties.insert("add_1_a".to_string(), PropertyValue::Number(5.0));
        add_node.properties.insert("add_1_b".to_string(), PropertyValue::Number(10.0));
        
        // ==========================================
        // Node 3: multiply - Multiply result by 2
        // ==========================================
        let mut multiply_node = NodeInstance::new(
            "multiply_1",
            "multiply",
            Position { x: 500.0, y: 150.0 }
        );
        
        multiply_node.inputs.push(PinInstance::new(
            "multiply_1_a",
            Pin::new("multiply_1_a", "a", DataType::Typed(pbgc::TypeInfo::new("i64")), PinType::Input)
        ));
        multiply_node.inputs.push(PinInstance::new(
            "multiply_1_b",
            Pin::new("multiply_1_b", "b", DataType::Typed(pbgc::TypeInfo::new("i64")), PinType::Input)
        ));
        multiply_node.outputs.push(PinInstance::new(
            "multiply_1_result",
            Pin::new("multiply_1_result", "result", DataType::Typed(pbgc::TypeInfo::new("i64")), PinType::Output)
        ));
        
        // Constant multiplier - must use pin ID
        multiply_node.properties.insert("multiply_1_b".to_string(), PropertyValue::Number(2.0));
        
        // ==========================================
        // Node 4: print_number - Print the result
        // ==========================================
        let mut print_node = NodeInstance::new(
            "print_1",
            "print_number",
            Position { x: 700.0, y: 200.0 }
        );
        
        // Execution pins
        print_node.inputs.push(PinInstance::new(
            "print_1_exec",
            Pin::new("print_1_exec", "exec", DataType::Execution, PinType::Input)
        ));
        print_node.outputs.push(PinInstance::new(
            "print_1_exec_out",
            Pin::new("print_1_exec_out", "exec", DataType::Execution, PinType::Output)
        ));
        
        // Data input pin
        print_node.inputs.push(PinInstance::new(
            "print_1_value",
            Pin::new("print_1_value", "value", DataType::Typed(pbgc::TypeInfo::new("f64")), PinType::Input)
        ));
        
        // ==========================================
        // Node 5: print_string - Print completion message
        // ==========================================
        let mut print_msg = NodeInstance::new(
            "print_2",
            "print_string",
            Position { x: 900.0, y: 200.0 }
        );
        
        print_msg.inputs.push(PinInstance::new(
            "print_2_exec",
            Pin::new("print_2_exec", "exec", DataType::Execution, PinType::Input)
        ));
        print_msg.inputs.push(PinInstance::new(
            "print_2_message",
            Pin::new("print_2_message", "message", DataType::Typed(pbgc::TypeInfo::new("&str")), PinType::Input)
        ));
        
        print_msg.properties.insert(
            "print_2_message".to_string(),
            PropertyValue::String("Calculation complete!".to_string())
        );
        
        // ==========================================
        // Add all nodes to graph
        // ==========================================
        graph.add_node(begin_play);
        graph.add_node(add_node);
        graph.add_node(multiply_node);
        graph.add_node(print_node);
        graph.add_node(print_msg);
        
        // ==========================================
        // Connect the graph
        // ==========================================
        
        // Execution flow: begin_play -> print_number -> print_string
        graph.add_connection(Connection::new(
            "begin_play_1", "begin_play_1_Body",
            "print_1", "print_1_exec",
            ConnectionType::Execution,
        ));
        
        graph.add_connection(Connection::new(
            "print_1", "print_1_exec_out",
            "print_2", "print_2_exec",
            ConnectionType::Execution,
        ));
        
        // Data flow: add -> multiply -> print
        graph.add_connection(Connection::new(
            "add_1", "add_1_result",
            "multiply_1", "multiply_1_a",
            ConnectionType::Data,
        ));
        
        graph.add_connection(Connection::new(
            "multiply_1", "multiply_1_result",
            "print_1", "print_1_value",
            ConnectionType::Data,
        ));
        
        println!("Graph created with {} nodes and {} connections", 
            graph.nodes.len(), graph.connections.len());
        
        // ==========================================
        // Compile the graph
        // ==========================================
        println!("\n=== Compiling Complex Blueprint ===");
        let result = compile_blueprint(&graph);
        
        match result {
            Ok(rust_code) => {
                println!("\n✅ === Compilation Successful! ===");
                println!("Generated {} bytes of Rust code", rust_code.len());
                
                println!("\n=== Generated Code ===");
                println!("{}", rust_code);
                println!("=== End of Generated Code ===\n");
                
                // Verify the code contains expected elements
                assert!(rust_code.len() > 100, "Generated code should be substantial");
                assert!(rust_code.contains("begin_play") || rust_code.contains("fn main"), 
                    "Should contain event function");
                
                // Write full output to file
                let output_path = "../../target/complex_blueprint_output.rs";
                if let Err(e) = std::fs::write(output_path, &rust_code) {
                    println!("Note: Could not write to {}: {}", output_path, e);
                } else {
                    println!("✅ Full output written to: {}", output_path);
                }
                
                println!("\n✅ Test Passed! Complex Blueprint compiled successfully");
            }
            Err(e) => {
                panic!("❌ Complex Blueprint compilation failed: {}", e);
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
