//! Headless behaviour tests: the document, its conversion to and from the
//! `.rendergraph` asset, buffers, dynamic pins, diagnostics and layout.

use graphy::{Connection, ConnectionType, GraphCommand, NodeInstance, PinInstance, Position};
use graphy_gpui::apply_transaction;
use helio_default_passes::graphs::{base_registry, deferred_graph_asset};
use helio_graph_asset::{
    buffers, core_nodes, sections, Connection as AssetWire, GraphAsset, NodeDescriptor,
    NodeInstance as AssetNode, NodeRegistry, Params, PinDesc, PinRef, PinType as RenderPinType,
    Severity,
};
use serde_json::Value;

use crate::document::{
    decode, encode, section_node_pins, RenderDocument, ENTRY_ID, EXIT_ID, REROUTE,
};
use crate::model;

fn round_trip(asset: &GraphAsset, registry: &NodeRegistry) -> GraphAsset {
    RenderDocument::from_asset(asset, registry)
        .as_ref()
        .to_asset(registry)
}

fn node(id: &str, kind: &str) -> AssetNode {
    AssetNode {
        id: id.into(),
        kind: kind.into(),
        params: Params::new(),
        position: [10.0, 20.0],
    }
}

fn wire(a: &str, ap: &str, b: &str, bp: &str) -> AssetWire {
    AssetWire {
        from: PinRef::new(a, ap),
        to: PinRef::new(b, bp),
    }
}

#[test]
fn the_default_graph_survives_the_document_without_loss_or_id_churn() {
    let registry = base_registry();
    let asset = deferred_graph_asset(&registry);
    assert!(!asset.sections.is_empty(), "the default graph has sections");

    let doc = RenderDocument::from_asset(&asset, &registry);
    assert_eq!(doc.main.nodes.len(), asset.nodes.len());
    assert_eq!(doc.main.connections.len(), asset.connections.len());
    assert_eq!(doc.sections.len(), asset.sections.len());

    let back = doc.as_ref().to_asset(&registry);
    assert_eq!(back.nodes, asset.nodes, "nodes, ids and authoring order");
    assert_eq!(back.connections, asset.connections);
    assert_eq!(back.comments, asset.comments);
    assert_eq!(back.format_version, asset.format_version);
    assert_eq!(back.name, asset.name);
    assert_eq!(back.description, asset.description);
    for (section, restored) in asset.sections.iter().zip(&back.sections) {
        assert_eq!(section.id, restored.id);
        assert_eq!(section.name, restored.name);
        assert_eq!(section.inputs, restored.inputs);
        assert_eq!(section.outputs, restored.outputs);
        assert_eq!(section.nodes, restored.nodes);
        assert_eq!(section.connections, restored.connections);
    }

    // Saving what was saved changes nothing.
    let text = encode(&doc.as_ref(), &registry);
    let reloaded = decode(&text, &registry).unwrap();
    assert_eq!(encode(&reloaded.as_ref(), &registry), text);
    assert!(model::problems(&doc.as_ref().buildable_asset(&registry), &registry).is_empty());
}

#[test]
fn a_new_file_is_expanded_to_the_default_graph() {
    let registry = base_registry();
    let doc = decode(r#"{"format_version":1,"template":"deferred"}"#, &registry).unwrap();
    assert!(!doc.main.nodes.is_empty());
    assert!(decode("not json", &registry).is_err());
}

#[test]
fn a_section_node_shows_the_ports_of_its_graph() {
    let registry = base_registry();
    let asset = deferred_graph_asset(&registry);
    let doc = RenderDocument::from_asset(&asset, &registry);
    for section in &doc.sections {
        let placed = doc
            .main
            .nodes
            .values()
            .find(|n| sections::section_id(&n.node_type) == Some(section.id.as_str()))
            .expect("a node places each section");
        let pins = section_node_pins(&section.graph);
        let ids = |pins: &[PinInstance]| pins.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
        assert_eq!(
            ids(&placed.inputs),
            pins.inputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        );
        assert_eq!(
            ids(&placed.outputs),
            pins.outputs.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        );
        // The entry and exit nodes are the section's ports, not passes.
        assert!(section.graph.nodes.contains_key(ENTRY_ID));
        assert!(section.graph.nodes.contains_key(EXIT_ID));
    }
}

#[test]
fn edited_parameters_round_trip_as_their_declared_types() {
    let registry = base_registry();
    let mut asset = GraphAsset::default();
    asset.nodes.push(AssetNode {
        id: "buf".into(),
        kind: core_nodes::PERSISTENT_BUFFER.into(),
        params: [
            ("name".to_string(), Value::from("counters")),
            ("size_bytes".to_string(), Value::from(4096)),
        ]
        .into(),
        position: [10.0, 20.0],
    });
    let doc = RenderDocument::from_asset(&asset, &registry);
    let buffer = &doc.main.nodes["buf"];
    assert_eq!(
        model::type_name(buffer.outputs[0].data_type()),
        "pbuf:counters"
    );
    let back = doc.as_ref().to_asset(&registry);
    assert_eq!(back.nodes[0].params["size_bytes"], Value::from(4096));
    assert_eq!(back.nodes[0].params["name"], Value::from("counters"));
}

#[test]
fn nodes_this_build_does_not_know_keep_their_params_and_wires() {
    let registry = base_registry();
    let mut asset = GraphAsset::default();
    let mut stranger = node("x", "plugin.not_loaded");
    stranger.params = [("level".to_string(), Value::from(3))].into();
    asset.nodes = vec![node("start", core_nodes::ON_FRAME_START), stranger];
    asset.connections = vec![
        wire("start", "out", "x", "in"),
        wire("x", "done", "start", "back"),
    ];
    let doc = RenderDocument::from_asset(&asset, &registry);
    assert!(doc.main.nodes["x"].inputs.iter().any(|p| p.id == "in"));
    assert!(doc.main.nodes["x"].outputs.iter().any(|p| p.id == "done"));
    let back = round_trip(&asset, &registry);
    assert_eq!(back.nodes[1].params["level"], Value::from(3));
    assert!(back.connections.contains(&wire("start", "out", "x", "in")));
}

#[test]
fn a_reroute_keeps_its_pins_and_wires_and_is_bypassed_for_building() {
    let registry = base_registry();
    let mut asset = GraphAsset::default();
    let mut reroute = node("r", REROUTE);
    reroute.params = [("type".to_string(), Value::from("execution"))].into();
    asset.nodes = vec![
        node("early", core_nodes::ON_FRAME_START),
        reroute,
        node("out", core_nodes::OUTPUT),
    ];
    asset.connections = vec![wire("early", "out", "r", "in"), wire("r", "out", "out", "in")];

    let doc = RenderDocument::from_asset(&asset, &registry);
    assert_eq!(doc.main.connections.len(), 2, "wires through the reroute load");
    assert_eq!(doc.as_ref().to_asset(&registry), asset);

    let built = doc.as_ref().buildable_asset(&registry);
    assert_eq!(built.nodes.len(), 2);
    assert_eq!(built.connections, vec![wire("early", "out", "out", "in")]);
}

#[test]
fn nodes_added_in_the_editor_follow_the_authored_ones_by_id() {
    let registry = base_registry();
    let mut asset = GraphAsset::default();
    asset.nodes = vec![
        node("z", core_nodes::OUTPUT),
        node("a", core_nodes::ON_FRAME_START),
    ];
    let mut doc = RenderDocument::from_asset(&asset, &registry);
    doc.main.add_node(NodeInstance::new(
        "m",
        core_nodes::EXTERNAL,
        Position::new(0.0, 0.0),
    ));
    let ids: Vec<_> = doc
        .as_ref()
        .to_asset(&registry)
        .nodes
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(ids, ["z", "a", "m"]);
}

#[test]
fn buffers_flatten_the_same_from_the_document_as_from_the_asset() {
    let mut registry = NodeRegistry::with_core();
    registry.register(
        NodeDescriptor::new("t.pass", "Pass", "Test", "", |_| Ok(())).with_pins(
            vec![
                PinDesc::flow("in"),
                PinDesc::resource(RenderPinType::TEXTURE, "pre_aa"),
            ],
            vec![
                PinDesc::flow("out"),
                helio_graph_asset::output_pin(RenderPinType::TEXTURE, "pre_aa"),
            ],
        ),
    );
    let buffer = |id: &str, kind: &str| {
        let mut n = node(id, kind);
        n.params = [("buffer".to_string(), Value::from("tex:pre_aa"))].into();
        n
    };
    let mut asset = GraphAsset::default();
    asset.nodes = vec![
        node("start", core_nodes::ON_FRAME_START),
        node("a", "t.pass"),
        node("b", "t.pass"),
        node("out", core_nodes::OUTPUT),
        buffer("set_a", core_nodes::BUFFER_SET),
        buffer("get_b", core_nodes::BUFFER_GET),
    ];
    asset.connections = vec![
        wire("start", "out", "a", "in"),
        wire("a", "out", "b", "in"),
        wire("b", "out", "out", "in"),
        wire("a", "out_pre_aa", "set_a", "pre_aa"),
        wire("get_b", "pre_aa", "b", "pre_aa"),
    ];
    let doc = RenderDocument::from_asset(&asset, &registry);
    assert_eq!(doc.main.connections.len(), asset.connections.len());
    let direct = buffers::flatten(&asset, &registry);
    let through = buffers::flatten(&doc.as_ref().buildable_asset(&registry), &registry);
    assert_eq!(through.asset, direct.asset);
    assert!(through
        .asset
        .connections
        .contains(&wire("a", "out_pre_aa", "b", "pre_aa")));
    assert!(through
        .diagnostics
        .iter()
        .all(|d| d.severity != Severity::Error));
    // Get and Set titles come from their buffer.
    assert_eq!(
        model::buffer_title(
            core_nodes::BUFFER_GET,
            &model::properties_as_text(&doc.main.nodes["get_b"])
        ),
        Some("Get pre_aa".to_string())
    );
}

#[test]
fn renaming_a_resource_moves_its_wires_to_the_new_pin() {
    let registry = base_registry();
    let mut asset = GraphAsset::default();
    asset.nodes = vec![AssetNode {
        id: "buf".into(),
        kind: core_nodes::PERSISTENT_BUFFER.into(),
        params: [("name".to_string(), Value::from("counters"))].into(),
        position: [0.0, 0.0],
    }];
    let mut doc = RenderDocument::from_asset(&asset, &registry);

    let mut taker = NodeInstance::new("take", "taker", Position::new(0.0, 0.0));
    taker.add_input_pin("counters", model::data_type("pbuf:counters"));
    doc.main.add_node(taker);
    doc.main.add_connection(Connection::new(
        "buf",
        "counters",
        "take",
        "counters",
        ConnectionType::Data,
    ));

    let rename = GraphCommand::SetProperty {
        node: "buf".into(),
        key: "name".into(),
        value: Some(Value::from("totals")),
    };
    let mut after = doc.main.clone();
    apply_transaction(&mut after, &[rename.clone()]).unwrap();
    let mut commands = vec![rename];
    commands.extend(model::refresh_pins(&after, "buf", &registry));
    apply_transaction(&mut doc.main, &commands).unwrap();

    let pin = &doc.main.nodes["buf"].outputs[0];
    assert_eq!(pin.id, "totals");
    assert_eq!(model::type_name(pin.data_type()), "pbuf:totals");
    assert_eq!(doc.main.connections.len(), 1);
    assert_eq!(doc.main.connections[0].source_pin, "totals");
}

#[test]
fn diagnostics_name_what_stops_a_graph_from_building() {
    let registry = base_registry();
    let found = model::problems(&GraphAsset::default(), &registry);
    assert!(found.iter().any(|p| p.contains("Output")), "{found:?}");

    // Through the document: delete the Output node of the default graph.
    let asset = deferred_graph_asset(&registry);
    let mut doc = RenderDocument::from_asset(&asset, &registry);
    let output = doc
        .main
        .nodes
        .values()
        .find(|n| n.node_type == core_nodes::OUTPUT)
        .map(|n| n.id.clone())
        .expect("the default graph ends in an Output");
    apply_transaction(&mut doc.main, &[GraphCommand::RemoveNode { id: output }]).unwrap();
    let found = model::problems(&doc.as_ref().buildable_asset(&registry), &registry);
    assert!(found.iter().any(|p| p.contains("Output")), "{found:?}");
}

#[test]
fn auto_layout_moves_the_documents_nodes_and_adds_lanes() {
    let registry = base_registry();
    // Unboxed, so the passes form lanes in the main graph.
    let mut asset = sections::expand(&deferred_graph_asset(&registry)).asset;
    for node in &mut asset.nodes {
        node.position = [0.0, 0.0];
    }
    let doc = RenderDocument::from_asset(&asset, &registry);
    let commands = model::auto_layout_graph(&doc.main);
    assert!(commands
        .iter()
        .any(|c| matches!(c, GraphCommand::MoveNode { .. })));
    let mut graph = doc.main.clone();
    apply_transaction(&mut graph, &commands).unwrap();
    assert!(graph
        .comments
        .iter()
        .any(|c| c.id.starts_with(helio_graph_asset::LANE_PREFIX)));
}

#[test]
fn pin_types_carry_kind_persistence_and_resource() {
    let tex = PinDesc::resource(RenderPinType::TEXTURE, "pre_aa");
    let history = PinDesc::resource(RenderPinType::PERSISTENT_TEXTURE, "pre_aa");
    assert_eq!(model::pin_type_string(&tex), "tex:pre_aa");
    assert_eq!(model::pin_type_string(&history), "ptex:pre_aa");
    assert_eq!(model::pin_type_string(&PinDesc::flow("out")), "execution");

    use graphy_render::presenter::PinShape;
    let shapes: Vec<_> = ["execution", "tex:a", "depth:a", "buf:a", "scene:a"]
        .map(|t| model::pin_style(t).shape)
        .into();
    assert_eq!(
        shapes,
        [
            PinShape::Arrow,
            PinShape::Circle,
            PinShape::Diamond,
            PinShape::Square,
            PinShape::Hexagon
        ]
    );
    assert!(model::pin_style("ptex:a").ringed && model::pin_style("pbuf:a").ringed);
    assert!(!model::pin_style("tex:a").ringed && !model::pin_style("buf:a").ringed);
}

#[test]
fn the_catalogue_lists_every_node_and_the_factory_builds_its_pins() {
    let registry = base_registry();
    let catalog = model::catalog(&registry);
    assert_eq!(catalog.len(), registry.list().len());
    let buffer = model::create_node(
        &registry,
        core_nodes::PERSISTENT_BUFFER,
        "n",
        Position::new(0.0, 0.0),
    )
    .expect("persistent buffer node");
    assert_eq!(buffer.outputs.len(), 1);
    assert_eq!(model::type_name(buffer.outputs[0].data_type()), "pbuf:my_buffer");
    assert!(model::create_node(&registry, "nope", "n", Position::new(0.0, 0.0)).is_none());
}

#[test]
fn only_equal_pin_types_connect() {
    use graphy::{Compatibility, TypeResolver};
    let resolver = crate::profile::RenderResolver;
    let tex = model::data_type("tex:a");
    assert!(resolver
        .compatibility(&tex, &model::data_type("tex:a"))
        .is_connectable());
    assert!(matches!(
        resolver.compatibility(&tex, &model::data_type("ptex:a")),
        Compatibility::Incompatible(_)
    ));
    assert!(matches!(
        resolver.compatibility(&tex, &model::data_type("execution")),
        Compatibility::Incompatible(_)
    ));
}


#[test]
fn only_a_sound_graph_reaches_the_preview() {
    let registry = base_registry();
    let sound = deferred_graph_asset(&registry);
    let doc = RenderDocument::from_asset(&sound, &registry);
    assert_eq!(
        crate::preview::blocked(&doc.as_ref().buildable_asset(&registry), &registry),
        None
    );
    let blocked = crate::preview::blocked(&GraphAsset::default(), &registry);
    assert!(blocked.is_some_and(|why| why.contains("Output")));
}

fn sorted_wires(asset: &GraphAsset) -> Vec<(String, String, String, String)> {
    let mut wires: Vec<_> = asset
        .connections
        .iter()
        .map(|c| (c.from.node.clone(), c.from.pin.clone(), c.to.node.clone(), c.to.pin.clone()))
        .collect();
    wires.sort();
    wires
}

#[test]
fn collapsing_and_expanding_keep_the_pipeline_the_nodes_make() {
    use crate::collapse;
    let mut registry = NodeRegistry::with_core();
    registry.register(
        NodeDescriptor::new("t.pass", "Pass", "Test", "", |_| Ok(())).with_pins(
            vec![
                PinDesc::flow("in"),
                PinDesc::resource(RenderPinType::TEXTURE, "pre_aa"),
            ],
            vec![
                PinDesc::flow("out"),
                helio_graph_asset::output_pin(RenderPinType::TEXTURE, "pre_aa"),
            ],
        ),
    );
    let mut asset = GraphAsset::default();
    asset.nodes = vec![
        node("start", core_nodes::ON_FRAME_START),
        node("a", "t.pass"),
        node("b", "t.pass"),
        node("c", "t.pass"),
        node("out", core_nodes::OUTPUT),
    ];
    asset.connections = vec![
        wire("start", "out", "a", "in"),
        wire("a", "out", "b", "in"),
        wire("b", "out", "c", "in"),
        wire("c", "out", "out", "in"),
        wire("a", "out_pre_aa", "b", "pre_aa"),
        wire("b", "out_pre_aa", "c", "pre_aa"),
    ];
    let mut doc = RenderDocument::from_asset(&asset, &registry);
    assert_eq!(doc.main.connections.len(), asset.connections.len());

    // Collapse b and c.
    let collapsed = collapse::collapse(&doc.main, &["b".into(), "c".into()], &Default::default())
        .expect("collapses");
    apply_transaction(&mut doc.main, &collapsed.commands).unwrap();
    assert!(!doc.main.nodes.contains_key("b") && !doc.main.nodes.contains_key("c"));
    let placed = collapsed.node.id.clone();
    assert_eq!(doc.main.nodes[&placed].node_type, sections::section_kind(&collapsed.id));
    assert!(doc.main.nodes[&placed].inputs.len() >= 2, "flow and texture come in");
    let (section_id, section_name) = (collapsed.id.clone(), collapsed.name.clone());
    doc.sections.push(crate::document::SectionDoc {
        id: collapsed.id,
        name: collapsed.name,
        graph: collapsed.graph,
        order: collapsed.order,
    });

    // The section is organisation only: unboxed it is the graph we started from.
    let saved = doc.as_ref().to_asset(&registry);
    assert_eq!(saved.sections.len(), 1);
    assert_eq!(saved.sections[0].id, section_id);
    assert_eq!(saved.sections[0].name, section_name);
    let unboxed = sections::expand(&saved);
    assert!(unboxed.diagnostics.iter().all(|d| d.severity != Severity::Error));
    let mut ids: Vec<_> = unboxed.asset.nodes.iter().map(|n| n.id.clone()).collect();
    ids.sort();
    assert_eq!(ids, ["a", "b", "c", "out", "start"]);
    assert_eq!(sorted_wires(&unboxed.asset), sorted_wires(&asset));

    // And expanding it puts the nodes and wires back.
    let section_graph = doc.sections[0].graph.clone();
    let commands = collapse::expand(&doc.main, &placed, &section_graph).expect("expands");
    apply_transaction(&mut doc.main, &commands).unwrap();
    assert!(!doc.main.nodes.contains_key(&placed));
    let back = RenderDocument { sections: Vec::new(), ..doc }.as_ref().to_asset(&registry);
    assert_eq!(sorted_wires(&back), sorted_wires(&asset));
}

#[test]
fn collapsing_nothing_is_refused() {
    let registry = base_registry();
    let doc = RenderDocument::from_asset(&GraphAsset::default(), &registry);
    assert!(crate::collapse::collapse(&doc.main, &[], &Default::default()).is_err());
    assert!(crate::collapse::expand(&doc.main, "nope", &doc.main).is_err());
}
