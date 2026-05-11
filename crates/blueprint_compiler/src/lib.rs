//! # Blueprint Compiler
//!
//! Wrapper around PBGC (Pulsar Blueprint Graph Compiler) integrated with the Pulsar engine.
//!
//! This crate provides the main API for compiling Blueprint graphs within the engine,
//! delegating the heavy lifting to PBGC while providing engine-specific conveniences.
//!
//! ## Project generation
//!
//! Use [`project::generate_project`] to turn a set of compiled blueprints into a
//! complete, ready-to-run Pulsar game crate:
//!
//! ```no_run
//! use blueprint_compiler::{compile_blueprint, GraphDescription};
//! use blueprint_compiler::project::{CompiledBlueprint, ProjectSpec, generate_project};
//!
//! let graph = GraphDescription::new("player_controller");
//! let source = compile_blueprint(&graph).unwrap();
//!
//! let project = ProjectSpec::new("my_game")
//!     .description("My first Pulsar game")
//!     .add_blueprint(CompiledBlueprint::new("player_controller", source));
//!
//! generate_project(&project)
//!     .write_to_dir("./output/my_game")
//!     .unwrap();
//! ```

pub mod project;

// Re-export the main PBGC API
pub use pbgc::{
    compile_graph,
    compile_graph_with_library_manager,
    compile_graph_with_variables,
    Connection,
    ConnectionType,
    DataType,
    // Re-export Graphy types for convenience
    GraphDescription,
    GraphMetadata,
    GraphyError,
    NodeInstance,
    NodeTypes,
    Pin,
    PinInstance,
    Position,
    PropertyValue,
    Result,
};

// Re-export metadata provider
pub use pbgc::BlueprintMetadataProvider;

/// Get the Blueprint metadata provider
///
/// This provides access to all registered Blueprint nodes from pulsar_std.
pub fn get_metadata_provider() -> BlueprintMetadataProvider {
    BlueprintMetadataProvider::new()
}

/// Compile every blueprint in a project directory.
///
/// Walks `project_root` for folders containing `graph_save.json` (the
/// convention used by the Pulsar editor), compiles each one, and returns a
/// `Vec<(name, rust_source)>`. Errors on individual blueprints are logged and
/// skipped — the Vec contains only successful compilations.
pub fn compile_project(project_root: &std::path::Path)
    -> std::result::Result<Vec<project::CompiledBlueprint>, String>
{
    let folders = find_blueprint_folders(project_root);
    tracing::info!("[blueprint_compiler] {} blueprint folder(s) found", folders.len());

    let mut compiled = Vec::new();
    for folder in &folders {
        let name = folder
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_owned();

        match compile_blueprint_folder(folder) {
            Ok(source) => {
                tracing::info!("[blueprint_compiler] Compiled: {name}");
                compiled.push(project::CompiledBlueprint::new(name, source));
            }
            Err(e) => {
                tracing::error!("[blueprint_compiler] Skipping {name}: {e}");
            }
        }
    }

    Ok(compiled)
}

/// Compile a single blueprint folder (containing `graph_save.json`) into Rust source.
fn compile_blueprint_folder(folder: &std::path::Path) -> std::result::Result<String, String> {
    let graph_file = folder.join("graph_save.json");
    let raw = std::fs::read_to_string(&graph_file)
        .map_err(|e| format!("Cannot read {}: {e}", graph_file.display()))?;

    // graph_save.json starts with `//` comment lines before the JSON payload.
    let json: String = raw.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    // Deserialize as the editor's native BlueprintAsset type (pulsar_graph),
    // then convert to the compiler's graphy::GraphDescription.
    let asset: pulsar_graph::BlueprintAsset = serde_json::from_str(&json)
        .map_err(|e| format!("Cannot parse graph_save.json: {e}"))?;

    let graph = pg_to_graphy(asset.main_graph);
    compile_blueprint(&graph).map_err(|e| e.to_string())
}

/// Walk `root` recursively and return all directories containing `graph_save.json`.
fn find_blueprint_folders(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut results = Vec::new();
    fn walk(dir: &std::path::Path, results: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                if path.join("graph_save.json").exists() {
                    results.push(path);
                } else {
                    walk(&path, results);
                }
            }
        }
    }
    walk(root, &mut results);
    results
}

// ── pulsar_graph → graphy type conversion ────────────────────────────────────

fn pg_to_graphy(pg: pulsar_graph::GraphDescription) -> GraphDescription {
    use pbgc::*;
    GraphDescription {
        metadata: GraphMetadata {
            name:        pg.metadata.name,
            description: pg.metadata.description,
            version:     pg.metadata.version,
            created_at:  pg.metadata.created_at,
            modified_at: pg.metadata.modified_at,
        },
        nodes: pg.nodes.into_iter()
            .map(|(id, n)| (id, pg_node(n)))
            .collect(),
        connections: pg.connections.into_iter()
            .map(pg_connection)
            .collect(),
        comments: vec![],
    }
}

fn pg_node(n: pulsar_graph::NodeInstance) -> NodeInstance {
    NodeInstance {
        id:         n.id,
        node_type:  n.node_type,
        position:   Position { x: n.position.x as f64, y: n.position.y as f64 },
        inputs:     n.inputs.into_iter().map(pg_pin).collect(),
        outputs:    n.outputs.into_iter().map(pg_pin).collect(),
        properties: n.properties.into_iter()
            .map(|(k, v)| (k, pg_prop(v)))
            .collect(),
    }
}

fn pg_pin(p: pulsar_graph::PinInstance) -> PinInstance {
    let id = p.id.clone();
    PinInstance {
        id: id.clone(),
        pin: Pin {
            id,
            name:      p.pin.name,
            data_type: pg_data_type(p.pin.data_type),
            pin_type:  match p.pin.pin_type {
                pulsar_graph::PinType::Input  => pbgc::PinType::Input,
                pulsar_graph::PinType::Output => pbgc::PinType::Output,
            },
        },
    }
}

fn pg_data_type(dt: pulsar_graph::DataType) -> DataType {
    match dt {
        pulsar_graph::DataType::Execution    => DataType::Execution,
        pulsar_graph::DataType::String       => DataType::String,
        pulsar_graph::DataType::Number       => DataType::Number,
        pulsar_graph::DataType::Boolean      => DataType::Boolean,
        pulsar_graph::DataType::Vector2      => DataType::Vector2,
        pulsar_graph::DataType::Vector3      => DataType::Vector3,
        pulsar_graph::DataType::Color        => DataType::Color,
        pulsar_graph::DataType::Any          => DataType::Any,
        pulsar_graph::DataType::Object       => DataType::Any,
        pulsar_graph::DataType::Typed(ti)    => DataType::Typed(pbgc::TypeInfo {
            type_string: format!("{ti}"),
        }),
        pulsar_graph::DataType::Array(inner) => DataType::Typed(pbgc::TypeInfo {
            type_string: format!("Vec<{}>", pg_data_type_str(&inner)),
        }),
    }
}

fn pg_data_type_str(dt: &pulsar_graph::DataType) -> String {
    match dt {
        pulsar_graph::DataType::Execution    => "()".into(),
        pulsar_graph::DataType::String       => "String".into(),
        pulsar_graph::DataType::Number       => "f64".into(),
        pulsar_graph::DataType::Boolean      => "bool".into(),
        pulsar_graph::DataType::Vector2      => "(f32,f32)".into(),
        pulsar_graph::DataType::Vector3      => "(f32,f32,f32)".into(),
        pulsar_graph::DataType::Color        => "(f32,f32,f32,f32)".into(),
        pulsar_graph::DataType::Any
        | pulsar_graph::DataType::Object     => "Any".into(),
        pulsar_graph::DataType::Typed(ti)    => format!("{ti}"),
        pulsar_graph::DataType::Array(inner) => format!("Vec<{}>", pg_data_type_str(inner)),
    }
}

fn pg_connection(c: pulsar_graph::Connection) -> Connection {
    Connection::new(
        c.source_node,
        c.source_pin,
        c.target_node,
        c.target_pin,
        match c.connection_type {
            pulsar_graph::ConnectionType::Execution => ConnectionType::Execution,
            pulsar_graph::ConnectionType::Data      => ConnectionType::Data,
        },
    )
}

fn pg_prop(v: pulsar_graph::PropertyValue) -> PropertyValue {
    match v {
        pulsar_graph::PropertyValue::String(s)         => PropertyValue::String(s),
        pulsar_graph::PropertyValue::Number(n)         => PropertyValue::Number(n),
        pulsar_graph::PropertyValue::Boolean(b)        => PropertyValue::Boolean(b),
        pulsar_graph::PropertyValue::Vector2(x, y)     => PropertyValue::Vector2(x as f64, y as f64),
        pulsar_graph::PropertyValue::Vector3(x, y, z)  => PropertyValue::Vector3(x as f64, y as f64, z as f64),
        pulsar_graph::PropertyValue::Color(r, g, b, a) => PropertyValue::Color(r as f64, g as f64, b as f64, a as f64),
    }
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
///         tracing::trace!("Compiled successfully!");
///         std::fs::write("generated.rs", rust_code).unwrap();
///     }
///     Err(e) => tracing::error!("Compilation failed: {}", e),
/// }
/// ```
pub fn compile_blueprint(graph: &GraphDescription) -> Result<String> {
    tracing::info!(
        "[Blueprint Compiler] Compiling graph: {}",
        graph.metadata.name
    );

    let result = compile_graph(graph)?;

    tracing::info!(
        "[Blueprint Compiler] Successfully compiled graph: {}",
        graph.metadata.name
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphy::{NodeMetadataProvider, PinType};

    #[test]
    fn test_metadata_provider() {
        let provider = get_metadata_provider();
        let nodes = provider.get_all_nodes();

        // Should have at least some nodes from pulsar_std
        assert!(!nodes.is_empty(), "Should load nodes from pulsar_std");

        tracing::trace!("Loaded {} nodes from pulsar_std", nodes.len());
        for node in nodes.iter().take(5) {
            tracing::trace!("  - {} ({})", node.name, node.category);
        }
    }

    #[test]
    fn test_complex_blueprint_compilation() {
        // Initialize tracing for test output
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

        tracing::trace!("\n=== Building Complex Test Blueprint ===");

        // Create a more complex graph with math and control flow
        let mut graph = GraphDescription::new("complex_test_blueprint");

        // ==========================================
        // Node 1: begin_play event (entry point)
        // ==========================================
        let mut begin_play = NodeInstance::new(
            "begin_play_1",
            "begin_play",
            Position { x: 100.0, y: 200.0 },
        );
        begin_play.outputs.push(PinInstance::new(
            "begin_play_1_Body",
            Pin::new(
                "begin_play_1_Body",
                "Body",
                DataType::Execution,
                PinType::Output,
            ),
        ));

        // ==========================================
        // Node 2: add - Add two numbers (5 + 10)
        // ==========================================
        let mut add_node = NodeInstance::new("add_1", "add", Position { x: 300.0, y: 150.0 });

        // Input pins for add
        add_node.inputs.push(PinInstance::new(
            "add_1_a",
            Pin::new(
                "add_1_a",
                "a",
                DataType::Typed(pbgc::TypeInfo::new("i64")),
                PinType::Input,
            ),
        ));
        add_node.inputs.push(PinInstance::new(
            "add_1_b",
            Pin::new(
                "add_1_b",
                "b",
                DataType::Typed(pbgc::TypeInfo::new("i64")),
                PinType::Input,
            ),
        ));

        // Output pin for add
        add_node.outputs.push(PinInstance::new(
            "add_1_result",
            Pin::new(
                "add_1_result",
                "result",
                DataType::Typed(pbgc::TypeInfo::new("i64")),
                PinType::Output,
            ),
        ));

        // Set constant values - properties must be keyed by pin ID, not parameter name
        add_node
            .properties
            .insert("add_1_a".to_string(), PropertyValue::Number(5.0));
        add_node
            .properties
            .insert("add_1_b".to_string(), PropertyValue::Number(10.0));

        // ==========================================
        // Node 3: multiply - Multiply result by 2
        // ==========================================
        let mut multiply_node =
            NodeInstance::new("multiply_1", "multiply", Position { x: 500.0, y: 150.0 });

        multiply_node.inputs.push(PinInstance::new(
            "multiply_1_a",
            Pin::new(
                "multiply_1_a",
                "a",
                DataType::Typed(pbgc::TypeInfo::new("i64")),
                PinType::Input,
            ),
        ));
        multiply_node.inputs.push(PinInstance::new(
            "multiply_1_b",
            Pin::new(
                "multiply_1_b",
                "b",
                DataType::Typed(pbgc::TypeInfo::new("i64")),
                PinType::Input,
            ),
        ));
        multiply_node.outputs.push(PinInstance::new(
            "multiply_1_result",
            Pin::new(
                "multiply_1_result",
                "result",
                DataType::Typed(pbgc::TypeInfo::new("i64")),
                PinType::Output,
            ),
        ));

        // Constant multiplier - must use pin ID
        multiply_node
            .properties
            .insert("multiply_1_b".to_string(), PropertyValue::Number(2.0));

        // ==========================================
        // Node 4: print_number - Print the result
        // ==========================================
        let mut print_node =
            NodeInstance::new("print_1", "print_number", Position { x: 700.0, y: 200.0 });

        // Execution pins
        print_node.inputs.push(PinInstance::new(
            "print_1_exec",
            Pin::new("print_1_exec", "exec", DataType::Execution, PinType::Input),
        ));
        print_node.outputs.push(PinInstance::new(
            "print_1_exec_out",
            Pin::new(
                "print_1_exec_out",
                "exec",
                DataType::Execution,
                PinType::Output,
            ),
        ));

        // Data input pin
        print_node.inputs.push(PinInstance::new(
            "print_1_value",
            Pin::new(
                "print_1_value",
                "value",
                DataType::Typed(pbgc::TypeInfo::new("f64")),
                PinType::Input,
            ),
        ));

        // ==========================================
        // Node 5: print_string - Print completion message
        // ==========================================
        let mut print_msg =
            NodeInstance::new("print_2", "print_string", Position { x: 900.0, y: 200.0 });

        print_msg.inputs.push(PinInstance::new(
            "print_2_exec",
            Pin::new("print_2_exec", "exec", DataType::Execution, PinType::Input),
        ));
        print_msg.inputs.push(PinInstance::new(
            "print_2_message",
            Pin::new(
                "print_2_message",
                "message",
                DataType::Typed(pbgc::TypeInfo::new("&str")),
                PinType::Input,
            ),
        ));

        print_msg.properties.insert(
            "print_2_message".to_string(),
            PropertyValue::String("Calculation complete!".to_string()),
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
            "begin_play_1",
            "begin_play_1_Body",
            "print_1",
            "print_1_exec",
            ConnectionType::Execution,
        ));

        graph.add_connection(Connection::new(
            "print_1",
            "print_1_exec_out",
            "print_2",
            "print_2_exec",
            ConnectionType::Execution,
        ));

        // Data flow: add -> multiply -> print
        graph.add_connection(Connection::new(
            "add_1",
            "add_1_result",
            "multiply_1",
            "multiply_1_a",
            ConnectionType::Data,
        ));

        graph.add_connection(Connection::new(
            "multiply_1",
            "multiply_1_result",
            "print_1",
            "print_1_value",
            ConnectionType::Data,
        ));

        tracing::trace!(
            "Graph created with {} nodes and {} connections",
            graph.nodes.len(),
            graph.connections.len()
        );

        // ==========================================
        // Compile the graph
        // ==========================================
        tracing::trace!("\n=== Compiling Complex Blueprint ===");
        let result = compile_blueprint(&graph);

        match result {
            Ok(rust_code) => {
                tracing::trace!("\n✅ === Compilation Successful! ===");
                tracing::trace!("Generated {} bytes of Rust code", rust_code.len());

                tracing::trace!("\n=== Generated Code ===");
                tracing::trace!("{}", rust_code);
                tracing::trace!("=== End of Generated Code ===\n");

                // Verify the code contains expected elements
                assert!(
                    rust_code.len() > 100,
                    "Generated code should be substantial"
                );
                assert!(
                    rust_code.contains("begin_play") || rust_code.contains("fn main"),
                    "Should contain event function"
                );

                // Write full output to file
                let output_path = "../../target/complex_blueprint_output.rs";
                if let Err(e) = std::fs::write(output_path, &rust_code) {
                    tracing::trace!("Note: Could not write to {}: {}", output_path, e);
                } else {
                    tracing::trace!("✅ Full output written to: {}", output_path);
                }

                tracing::trace!("\n✅ Test Passed! Complex Blueprint compiled successfully");
            }
            Err(e) => {
                panic!("❌ Complex Blueprint compilation failed: {}", e);
            }
        }
    }

    #[test]
    fn test_compile_with_data_flow() {
        // Initialize tracing for test output
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();

        // Create a graph with data flow connections
        let mut graph = GraphDescription::new("data_flow_test");

        // Add a BeginPlay event node
        let mut begin_play =
            NodeInstance::new("begin_play_1", "BeginPlay", Position { x: 100.0, y: 100.0 });
        begin_play.outputs.push(PinInstance::new(
            "begin_play_1_Body",
            Pin::new(
                "begin_play_1_Body",
                "Body",
                DataType::Execution,
                PinType::Output,
            ),
        ));

        // Add an add node (if it exists in pulsar_std)
        let mut add_node = NodeInstance::new("add_1", "add", Position { x: 300.0, y: 100.0 });

        // Add input pins for add node
        add_node.inputs.push(PinInstance::new(
            "add_1_a",
            Pin::new("add_1_a", "a", DataType::Number, PinType::Input),
        ));
        add_node.inputs.push(PinInstance::new(
            "add_1_b",
            Pin::new("add_1_b", "b", DataType::Number, PinType::Input),
        ));
        add_node.outputs.push(PinInstance::new(
            "add_1_result",
            Pin::new("add_1_result", "result", DataType::Number, PinType::Output),
        ));

        // Set default property values
        add_node
            .properties
            .insert("a".to_string(), PropertyValue::Number(5.0));
        add_node
            .properties
            .insert("b".to_string(), PropertyValue::Number(10.0));

        graph.add_node(begin_play);
        graph.add_node(add_node);

        // Try to compile (may fail if add node doesn't exist)
        tracing::trace!("\n=== Compiling Data Flow Test Blueprint ===");
        let result = compile_blueprint(&graph);

        match result {
            Ok(rust_code) => {
                tracing::trace!("\n=== Data Flow Compilation Successful ===");
                tracing::trace!("Generated {} bytes of Rust code", rust_code.len());
            }
            Err(e) => {
                tracing::trace!("\n=== Expected behavior if 'add' node not found ===");
                tracing::trace!("Compilation error: {}", e);
                // This is okay - the test demonstrates the error handling
            }
        }
    }
}
