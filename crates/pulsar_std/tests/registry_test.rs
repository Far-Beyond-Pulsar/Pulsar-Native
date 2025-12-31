use pulsar_std::{get_all_nodes, get_node_by_name};

#[test]
fn test_metadata_collection() {
    let nodes = get_all_nodes();

    // Should have many nodes
    assert!(nodes.len() > 10, "Should have many registered nodes, found {}", nodes.len());

    // Test 'add' node
    let add_node = get_node_by_name("add").expect("Should find 'add' node");
    assert_eq!(add_node.name, "add");
    assert_eq!(add_node.category, "Math");
    assert_eq!(add_node.color, Some("#4A90E2"), "Color should be collected");
    assert_eq!(add_node.params.len(), 2, "Should have 2 parameters");
    assert_eq!(add_node.params[0].name, "a");
    assert_eq!(add_node.params[0].ty, "i64");
    assert_eq!(add_node.params[1].name, "b");
    assert_eq!(add_node.params[1].ty, "i64");
    assert_eq!(add_node.return_type, Some("i64"));
    assert!(!add_node.documentation.is_empty(), "Documentation should be collected");

        tracing::debug!("Add node docs: {:?}", add_node.documentation);

    // Test 'branch' node
    let branch_node = get_node_by_name("branch").expect("Should find 'branch' node");
    assert_eq!(branch_node.name, "branch");
    assert_eq!(branch_node.category, "Flow");
    assert_eq!(branch_node.color, Some("#BD10E0"), "Color should be collected");
    assert_eq!(branch_node.exec_outputs.len(), 2, "Should have 2 exec outputs");
    assert!(branch_node.exec_outputs.contains(&"True"));
    assert!(branch_node.exec_outputs.contains(&"False"));
    assert!(!branch_node.documentation.is_empty(), "Documentation should be collected");

        tracing::debug!("Branch node docs: {:?}", branch_node.documentation);

    // Test 'print_string' node
    let print_node = get_node_by_name("print_string").expect("Should find 'print_string' node");
    assert_eq!(print_node.name, "print_string");
    assert_eq!(print_node.category, "Debug");
    assert_eq!(print_node.color, Some("#7ED321"), "Color should be collected");

        tracing::debug!("Print node docs: {:?}", print_node.documentation);
}

#[test]
fn test_all_nodes_have_metadata() {
    let nodes = get_all_nodes();

    for node in nodes {
        // All nodes should have a name
        assert!(!node.name.is_empty(), "Node should have a name");

        // All nodes should have a category
        assert!(!node.category.is_empty(), "Node '{}' should have a category", node.name);

        // Print node info
            tracing::debug!("Node: {} | Category: {} | Color: {:?} | Docs: {} lines",
            node.name,
            node.category,
            node.color,
            node.documentation.len()
        );
    }
}
