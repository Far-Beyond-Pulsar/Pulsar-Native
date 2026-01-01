//! Test that compiles the exact default graph from panel.rs

use crate::graph::*;
use crate::compiler::compile_graph;

pub fn test_default_ui_graph_compilation() -> Result<String, String> {
    tracing::info!("\n=== Testing Default UI Graph Compilation ===\n");

    // Create the exact same graph as in panel.rs:68-324
    let mut graph = GraphDescription::new("Default UI Graph");

    // main_event node
    let mut main_node = NodeInstance::new("main_event", "main", Position { x: 100.0, y: 200.0 });
    main_node.add_output_pin("Body", DataType::Execution);
    graph.add_node(main_node);

    // add_node: add(2, 3)
    let mut add_node = NodeInstance::new("add_node", "add", Position { x: 400.0, y: 80.0 });
    add_node.add_input_pin("a", DataType::Typed(TypeInfo::parse("i64")));
    add_node.add_input_pin("b", DataType::Typed(TypeInfo::parse("i64")));
    add_node.set_property("a", PropertyValue::Number(2.0));
    add_node.set_property("b", PropertyValue::Number(3.0));
    add_node.add_output_pin("result", DataType::Typed(TypeInfo::parse("i64")));
    graph.add_node(add_node);

    // greater_node: greater_than(result, 3)
    let mut greater_node = NodeInstance::new("greater_node", "greater_than", Position { x: 620.0, y: 80.0 });
    greater_node.add_input_pin("a", DataType::Typed(TypeInfo::parse("i64")));
    greater_node.add_input_pin("b", DataType::Typed(TypeInfo::parse("i64")));
    greater_node.set_property("b", PropertyValue::Number(3.0));
    greater_node.add_output_pin("result", DataType::Typed(TypeInfo::parse("bool")));
    graph.add_node(greater_node);

    // branch_node
    let mut branch_node = NodeInstance::new("branch_node", "branch", Position { x: 400.0, y: 280.0 });
    branch_node.add_input_pin("exec", DataType::Execution);
    branch_node.add_input_pin("condition", DataType::Typed(TypeInfo::parse("bool")));
    branch_node.add_output_pin("True", DataType::Execution);
    branch_node.add_output_pin("False", DataType::Execution);
    graph.add_node(branch_node);

    // print_true
    let mut print_true = NodeInstance::new("print_true", "print_string", Position { x: 680.0, y: 220.0 });
    print_true.add_input_pin("exec", DataType::Execution);
    print_true.add_input_pin("message", DataType::Typed(TypeInfo::parse("String")));
    print_true.set_property("message", PropertyValue::String("Result is greater than 3! ✓".to_string()));
    print_true.add_output_pin("exec_out", DataType::Execution);
    graph.add_node(print_true);

    // print_false
    let mut print_false = NodeInstance::new("print_false", "print_string", Position { x: 680.0, y: 360.0 });
    print_false.add_input_pin("exec", DataType::Execution);
    print_false.add_input_pin("message", DataType::Typed(TypeInfo::parse("String")));
    print_false.set_property("message", PropertyValue::String("Result is 3 or less. ✗".to_string()));
    print_false.add_output_pin("exec_out", DataType::Execution);
    graph.add_node(print_false);

    // Connections
    graph.add_connection(Connection::new(
        "conn_main_branch",
        "main_event",
        "Body",
        "branch_node",
        "exec",
        ConnectionType::Execution,
    ));

    graph.add_connection(Connection::new(
        "conn_add_gt",
        "add_node",
        "result",
        "greater_node",
        "a",
        ConnectionType::Data,
    ));

    graph.add_connection(Connection::new(
        "conn_gt_branch",
        "greater_node",
        "result",
        "branch_node",
        "condition",
        ConnectionType::Data,
    ));

    graph.add_connection(Connection::new(
        "conn_branch_true",
        "branch_node",
        "True",
        "print_true",
        "exec",
        ConnectionType::Execution,
    ));

    graph.add_connection(Connection::new(
        "conn_branch_false",
        "branch_node",
        "False",
        "print_false",
        "exec",
        ConnectionType::Execution,
    ));

    tracing::info!("Graph structure:");
    tracing::info!("  - {} nodes", graph.nodes.len());
    tracing::info!("  - {} connections", graph.connections.len());

    // Compile it
    tracing::info!("\nCompiling...\n");
    let code = compile_graph(&graph)?;

    tracing::info!("Generated code:");
    tracing::info!("{}", "=".repeat(80));
    tracing::info!("{}", code);
    tracing::info!("{}", "=".repeat(80));

    // Validate structure
    validate_code(&code)?;

    Ok(code)
}

fn validate_code(code: &str) -> Result<(), String> {
    tracing::info!("\nValidation:");

    let checks = vec![
        ("pub fn main()", code.contains("pub fn main()")),
        ("add(2, 3)", code.contains("add(2, 3)") || code.contains("add (2 , 3)")),
        ("greater_than", code.contains("greater_than")),
        ("if statement", code.contains("if ")),
        ("Result is greater than 3", code.contains("Result is greater than 3")),
        ("Result is 3 or less", code.contains("Result is 3 or less")),
    ];

    for (name, passed) in checks {
        if passed {
            tracing::info!("  ✓ {}", name);
        } else {
            tracing::warn!("  ✗ {}", name);
            return Err(format!("Missing: {}", name));
        }
    }

    tracing::info!("\n✓ All checks passed!");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_ui_graph() {
        let result = test_default_ui_graph_compilation();
        assert!(result.is_ok(), "Failed: {:?}", result.err());
    }
}
