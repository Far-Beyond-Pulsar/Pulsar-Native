//! Validation test for the default blueprint.json

use std::fs;
use crate::graph::GraphDescription;
use crate::compiler::compile_graph;

/// Validation error type
#[derive(Debug)]
pub struct ValidationError {
    pub message: String,
}

impl ValidationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { message: msg.into() }
    }
}

/// Validate a blueprint graph
pub fn validate_blueprint(graph: &GraphDescription) -> Result<(), ValidationError> {
    // Basic validation
    if graph.nodes.is_empty() {
        return Err(ValidationError::new("Graph has no nodes"));
    }
    
    // Check for event nodes
    let has_event = graph.nodes.values().any(|n| 
        n.node_type == "main" || n.node_type == "begin_play"
    );
    
    if !has_event {
        return Err(ValidationError::new("Graph has no event nodes (main or begin_play)"));
    }
    
    Ok(())
}

/// Test loading and compiling the default blueprint.json
pub fn validate_default_blueprint() -> Result<String, String> {
    tracing::info!("\n=== Validating Default Blueprint ===");

    // Load blueprint.json
    let blueprint_path = "../../blueprint.json";
    let blueprint_content = fs::read_to_string(blueprint_path)
        .map_err(|e| format!("Failed to read {}: {}", blueprint_path, e))?;

    // Parse JSON to GraphDescription
    let graph: GraphDescription = serde_json::from_str(&blueprint_content)
        .map_err(|e| format!("Failed to parse blueprint JSON: {}", e))?;

    tracing::info!("✓ Loaded graph: {}", graph.metadata.name);
    tracing::info!("  - {} nodes", graph.nodes.len());
    tracing::info!("  - {} connections", graph.connections.len());

    // List nodes
    tracing::info!("\nNodes:");
    for (id, node) in &graph.nodes {
        tracing::info!("  - {} (type: {})", id, node.node_type);
    }

    // List connections
    tracing::info!("\nConnections:");
    for conn in &graph.connections {
        tracing::info!("  - {} -> {} ({} -> {})",
            conn.source_node, conn.target_node,
            conn.source_pin, conn.target_pin
        );
    }

    // Compile the graph
    tracing::info!("\nCompiling graph...");
    let compiled_code = compile_graph(&graph)?;

    tracing::info!("✓ Compilation successful!\n");
    tracing::info!("Generated code:");
    tracing::info!("{}", "=".repeat(80));
    tracing::info!("{}", compiled_code);
    tracing::info!("{}", "=".repeat(80));

    // Validate the generated code
    validate_generated_code(&compiled_code)?;

    tracing::info!("\n✓ All validation checks passed!");

    Ok(compiled_code)
}

/// Validate that the generated code has the expected structure
fn validate_generated_code(code: &str) -> Result<(), String> {
    tracing::info!("\nValidating generated code structure...");

    // Check for required elements
    let checks = vec![
        ("Header comment", code.contains("// Auto-generated code from Pulsar Blueprint")),
        ("use statement", code.contains("use pulsar_std::*;")),
        ("pub fn main()", code.contains("pub fn main()")),
        ("Pure node evaluations", code.contains("// Pure node evaluations")),
        ("add function call", code.contains("add(")),
        ("multiply function call", code.contains("multiply(")),
        ("equals function call", code.contains("equals(")),
        ("branch control flow", code.contains("if ")),
        ("print_string calls", code.contains("print_string")),
    ];

    for (name, passed) in checks {
        if passed {
            tracing::info!("  ✓ {}", name);
        } else {
            tracing::warn!("  ✗ {}", name);
            return Err(format!("Validation failed: missing {}", name));
        }
    }

    // Check that we have exactly one main function
    let main_count = code.matches("pub fn main()").count();
    if main_count != 1 {
        return Err(format!("Expected 1 main function, found {}", main_count));
    }
    tracing::info!("  ✓ Exactly 1 main function");

    // Check for control flow structure
    if !code.contains("if ") || !code.contains("else") {
        return Err("Missing if/else control flow structure".to_string());
    }
    tracing::info!("  ✓ Control flow structure (if/else)");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_default_blueprint() {
        let result = validate_default_blueprint();
        assert!(result.is_ok(), "Blueprint validation failed: {:?}", result.err());
    }
}
