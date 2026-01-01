//! Comprehensive tests for the macro-based compiler system

#[cfg(test)]
mod tests {
    use crate::compiler::*;
    use crate::graph::*;
    use std::collections::HashMap;

    /// Helper to create a simple test graph
    fn create_test_graph() -> GraphDescription {
        GraphDescription {
            nodes: HashMap::new(),
            connections: Vec::new(),
            comments: Vec::new(),
            metadata: GraphMetadata {
                name: "test_graph".to_string(),
                description: "Test graph".to_string(),
                version: "1.0.0".to_string(),
                created_at: String::new(),
                modified_at: String::new(),
            },
        }
    }

    #[test]
    fn test_node_metadata_extraction() {
        // Test that we can extract node metadata from pulsar_std
        let metadata = node_metadata::extract_node_metadata();
        assert!(metadata.is_ok(), "Failed to extract node metadata: {:?}", metadata.err());

        let metadata = metadata.unwrap();
        assert!(!metadata.is_empty(), "No nodes found in pulsar_std");

        // Check for some expected nodes
        assert!(metadata.contains_key("add"), "Missing 'add' node");
        assert!(metadata.contains_key("branch"), "Missing 'branch' node");
        assert!(metadata.contains_key("print_string"), "Missing 'print_string' node");

        // Test add node (pure)
        let add_node = metadata.get("add").unwrap();
        assert_eq!(add_node.node_type, node_metadata::NodeTypes::pure);
        assert_eq!(add_node.params.len(), 2);
        assert!(add_node.return_type.is_some());
        assert!(add_node.exec_inputs.is_empty());
        assert!(add_node.exec_outputs.is_empty());

        // Test branch node (control flow)
        let branch_node = metadata.get("branch").unwrap();
        assert_eq!(branch_node.node_type, node_metadata::NodeTypes::control_flow);
        assert_eq!(branch_node.params.len(), 1);
        assert_eq!(branch_node.exec_outputs.len(), 2);
        assert!(branch_node.exec_outputs.contains(&"True"));
        assert!(branch_node.exec_outputs.contains(&"False"));

        // Test print_string node (function)
        let print_node = metadata.get("print_string").unwrap();
        assert_eq!(print_node.node_type, node_metadata::NodeTypes::fn_);
        assert_eq!(print_node.params.len(), 1);
        assert_eq!(print_node.exec_inputs.len(), 1);
        assert_eq!(print_node.exec_outputs.len(), 1);
    }

    #[test]
    fn test_simple_add_graph() {
        // Create a graph with: add(5, 3)
        let mut graph = create_test_graph();

        let mut add_node = NodeInstance::new("add_1", "add", Position { x: 0.0, y: 0.0 });
        add_node.add_input_pin("a", DataType::Typed(TypeInfo::parse("i64")));
        add_node.add_input_pin("b", DataType::Typed(TypeInfo::parse("i64")));
        add_node.set_property("a", PropertyValue::Number(5.0));
        add_node.set_property("b", PropertyValue::Number(3.0));
        add_node.add_output_pin("result", DataType::Typed(TypeInfo::parse("i64")));

        graph.add_node(add_node);

        // Test data resolver
        let metadata = node_metadata::extract_node_metadata().unwrap();
        let data_resolver = data_resolver::DataResolver::build(&graph, &metadata);
        assert!(data_resolver.is_ok(), "Failed to build data resolver: {:?}", data_resolver.err());

        let resolver = data_resolver.unwrap();
        let a_expr = resolver.generate_input_expression("add_1", "a", &graph);
        let b_expr = resolver.generate_input_expression("add_1", "b", &graph);

        assert!(a_expr.is_ok(), "Failed to generate a expression: {:?}", a_expr.err());
        assert!(b_expr.is_ok(), "Failed to generate b expression: {:?}", b_expr.err());
    }

    #[test]
    fn test_simple_execution_chain() {
        // Create a graph with: begin_play -> print_string("Hello")
        let mut graph = create_test_graph();

        // Begin play node
        let mut begin_node = NodeInstance::new("begin_1", "begin_play", Position { x: 0.0, y: 0.0 });
        begin_node.add_output_pin("exec", DataType::Execution);

        // Print node
        let mut print_node = NodeInstance::new("print_1", "print_string", Position { x: 100.0, y: 0.0 });
        print_node.add_input_pin("exec", DataType::Execution);
        print_node.add_input_pin("message", DataType::Typed(TypeInfo::parse("String")));
        print_node.set_property("message", PropertyValue::String("Hello".to_string()));

        graph.add_node(begin_node);
        graph.add_node(print_node);

        // Connect them
        let conn = Connection::new(
            "conn_1",
            "begin_1",
            "exec",
            "print_1",
            "exec",
            ConnectionType::Execution,
        );
        graph.add_connection(conn);

        // Test execution routing
        let routing = execution_routing::ExecutionRouting::build_from_graph(&graph);
        let connected = routing.get_connected_nodes("begin_1", "exec");
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0], "print_1");
    }

    #[test]
    fn test_branch_control_flow() {
        // Create a graph with: branch(true) -> [print("yes"), print("no")]
        let mut graph = create_test_graph();

        // Branch node
        let mut branch_node = NodeInstance::new("branch_1", "branch", Position { x: 0.0, y: 0.0 });
        branch_node.add_input_pin("exec", DataType::Execution);
        branch_node.add_input_pin("condition", DataType::Typed(TypeInfo::parse("bool")));
        branch_node.add_output_pin("True", DataType::Execution);
        branch_node.add_output_pin("False", DataType::Execution);
        branch_node.set_property("condition", PropertyValue::Boolean(true));

        // Print nodes
        let mut print_true = NodeInstance::new("print_true", "print_string", Position { x: 100.0, y: -50.0 });
        print_true.add_input_pin("exec", DataType::Execution);
        print_true.add_input_pin("message", DataType::Typed(TypeInfo::parse("String")));
        print_true.set_property("message", PropertyValue::String("Yes!".to_string()));

        let mut print_false = NodeInstance::new("print_false", "print_string", Position { x: 100.0, y: 50.0 });
        print_false.add_input_pin("exec", DataType::Execution);
        print_false.add_input_pin("message", DataType::Typed(TypeInfo::parse("String")));
        print_false.set_property("message", PropertyValue::String("No!".to_string()));

        graph.add_node(branch_node);
        graph.add_node(print_true);
        graph.add_node(print_false);

        // Connect
        graph.add_connection(Connection::new("c1", "branch_1", "True", "print_true", "exec", ConnectionType::Execution));
        graph.add_connection(Connection::new("c2", "branch_1", "False", "print_false", "exec", ConnectionType::Execution));

        // Test routing
        let routing = execution_routing::ExecutionRouting::build_from_graph(&graph);
        let true_nodes = routing.get_connected_nodes("branch_1", "True");
        let false_nodes = routing.get_connected_nodes("branch_1", "False");

        assert_eq!(true_nodes.len(), 1);
        assert_eq!(true_nodes[0], "print_true");
        assert_eq!(false_nodes.len(), 1);
        assert_eq!(false_nodes[0], "print_false");
    }

    #[test]
    fn test_full_compilation_simple() {
        // Create complete graph: main event -> print_string("Hello World")
        let mut graph = create_test_graph();

        // Create main event node (defines fn main())
        let mut main_node = NodeInstance::new("main_1", "main", Position { x: 0.0, y: 0.0 });
        main_node.add_output_pin("Body", DataType::Execution);

        let mut print_node = NodeInstance::new("print_1", "print_string", Position { x: 100.0, y: 0.0 });
        print_node.add_input_pin("exec", DataType::Execution);
        print_node.add_input_pin("message", DataType::Typed(TypeInfo::parse("String")));
        print_node.set_property("message", PropertyValue::String("Hello World".to_string()));
        print_node.add_output_pin("exec_out", DataType::Execution);

        graph.add_node(main_node);
        graph.add_node(print_node);

        // Connect main's Body output to print_string's exec input
        graph.add_connection(Connection::new(
            "c1",
            "main_1",
            "Body",
            "print_1",
            "exec",
            ConnectionType::Execution,
        ));

        // Compile
        let result = compile_graph(&graph);

        match result {
            Ok(code) => {
                tracing::info!("Generated code:\n{}", code);
                assert!(code.contains("pub fn main"), "Should generate main function");
                assert!(code.contains("print_string"), "Should call print_string");
                assert!(code.contains("Hello World"), "Should include the message");
            }
            Err(e) => panic!("Compilation failed: {}", e),
        }
    }

    #[test]
    fn test_ast_utils_exec_output_replacement() {
        use crate::compiler::ast_utils::*;

        let func_source = r#"
            pub fn branch(condition: bool) {
                if condition {
                    exec_output!("True");
                } else {
                    exec_output!("False");
                }
            }
        "#;

        let mut exec_replacements = HashMap::new();
        exec_replacements.insert("True".to_string(), "println!(\"true path\");".to_string());
        exec_replacements.insert("False".to_string(), "println!(\"false path\");".to_string());

        let param_substitutions = HashMap::new();

        let result = inline_control_flow_function(
            func_source,
            exec_replacements,
            param_substitutions,
        );

        assert!(result.is_ok(), "Failed to inline function: {:?}", result.err());
        let inlined = result.unwrap();

        tracing::info!("Inlined code:\n{}", inlined);
        assert!(inlined.contains("true path"));
        assert!(inlined.contains("false path"));
        assert!(!inlined.contains("exec_output!"));
    }

    #[test]
    fn test_data_dependency_resolution() {
        // Create graph with data dependency: multiply(add(2, 3), 4)
        let mut graph = create_test_graph();

        // Add node
        let mut add_node = NodeInstance::new("add_1", "add", Position { x: 0.0, y: 0.0 });
        add_node.add_input_pin("a", DataType::Typed(TypeInfo::parse("i64")));
        add_node.add_input_pin("b", DataType::Typed(TypeInfo::parse("i64")));
        add_node.set_property("a", PropertyValue::Number(2.0));
        add_node.set_property("b", PropertyValue::Number(3.0));
        add_node.add_output_pin("result", DataType::Typed(TypeInfo::parse("i64")));

        // Multiply node
        let mut multiply_node = NodeInstance::new("mul_1", "multiply", Position { x: 100.0, y: 0.0 });
        multiply_node.add_input_pin("a", DataType::Typed(TypeInfo::parse("i64")));
        multiply_node.add_input_pin("b", DataType::Typed(TypeInfo::parse("i64")));
        multiply_node.set_property("b", PropertyValue::Number(4.0));
        multiply_node.add_output_pin("result", DataType::Typed(TypeInfo::parse("i64")));

        graph.add_node(add_node);
        graph.add_node(multiply_node);

        // Connect add output to multiply input
        graph.add_connection(Connection::new(
            "c1",
            "add_1",
            "result",
            "mul_1",
            "a",
            ConnectionType::Data,
        ));

        // Build data resolver
        let metadata = node_metadata::extract_node_metadata().unwrap();
        let resolver = data_resolver::DataResolver::build(&graph, &metadata);
        assert!(resolver.is_ok(), "Failed to build resolver: {:?}", resolver.err());

        let resolver = resolver.unwrap();

        // Check evaluation order
        let eval_order = resolver.get_pure_evaluation_order();
        assert_eq!(eval_order.len(), 2);
        assert_eq!(eval_order[0], "add_1"); // add must be evaluated first
        assert_eq!(eval_order[1], "mul_1"); // then multiply

        // Check that multiply's input comes from add's output
        let source = resolver.get_input_source("mul_1", "a");
        assert!(source.is_some());
        if let Some(data_resolver::DataSource::Connection { source_node_id, source_pin }) = source {
            assert_eq!(source_node_id, "add_1");
            assert_eq!(source_pin, "result");
        } else {
            panic!("Expected connection data source");
        }
    }
}
