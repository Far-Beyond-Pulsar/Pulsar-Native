//! State machine events through the component: entered, exited, transition
//! started and completed arrive as component-scoped events (generic and
//! canonical), never globally, with the machine, state and transition named.

use super::phase3_tests::*;
use super::*;
use pulsar_events::{
    entity_channel,
    gamma::{Channel, DynValue, SubscribeOptions},
    EventCategory, EventHub,
};
use pulsar_scenedb::World;
use pulsar_world_registry::ComponentRuntimeState;
use serde_json::json;
use skeletal_assets::anim_graph::builder::GraphBuilder;
use skeletal_assets::anim_graph::catalogue::{GraphKind, ParamType};
use skeletal_assets::anim_graph::{AnimSubGraph, AnimationGraphAsset, GraphParameter};
use skeletal_assets::runtime::graph::compile::compile_graph;
use skeletal_assets::runtime::graph::StateEventKind;
use std::sync::Mutex;

fn sub(id: &str, kind: GraphKind, graph: GraphBuilder) -> AnimSubGraph {
    AnimSubGraph {
        id: id.into(),
        name: id.into(),
        kind,
        graph: graph.into_graph(),
    }
}

/// Machine `Loco`: `Idle` and `Moving` (both play the Walk clip), `Idle -> Moving` when the
/// Bool parameter `go` is set, over 0.2 s, and `Moving -> Idle` when it is cleared, instantly.
pub(super) fn machine_graph(rig: &Rig) -> Arc<CompiledGraph> {
    let asset = machine_asset_for(rig);
    let out = compile_graph(&asset, &rig.source);
    out.graph
        .unwrap_or_else(|| panic!("{:#?}", out.diagnostics))
}

pub(super) fn machine_asset_for(rig: &Rig) -> AnimationGraphAsset {
    let params = [("go", ParamType::Bool)];
    let mut asset = AnimationGraphAsset::default();
    asset.skeleton = Some(
        skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str())
            .unwrap()
            .to_ref(),
    );
    asset
        .parameters
        .push(GraphParameter::new("go", ParamType::Bool));
    let mut main = GraphBuilder::new("main", &params);
    main.node("out", "anim.output_pose", &[]);
    main.node("sm", "anim.state_machine", &[("machine", json!("Loco"))]);
    main.wire("sm", "out", "pose");
    asset.main = main.into_graph();

    let mut m = GraphBuilder::new("Loco", &params);
    m.node("entry", "sm.entry", &[]);
    for (i, state) in ["Idle", "Moving"].iter().enumerate() {
        let pose_id = format!("Loco_p_{state}");
        m.node(
            &format!("s_{state}"),
            "sm.state",
            &[("name", json!(state)), ("pose_graph", json!(pose_id))],
        );
        if i == 0 {
            m.wire_pin("entry", "start", "s_Idle", "default");
        }
        let mut p = GraphBuilder::new(&pose_id, &params);
        p.node(
            "p",
            "anim.sequence_player",
            &[("sequence", json!(rig.walk_id))],
        );
        p.node("r", "anim.state_result", &[]);
        p.wire("p", "r", "pose");
        asset.subgraphs.push(sub(&pose_id, GraphKind::StatePose, p));
    }
    for (id, from, to, duration, negate) in [
        ("start", "Idle", "Moving", 0.2, false),
        ("stop", "Moving", "Idle", 0.0, true),
    ] {
        let rule_id = format!("Loco_r_{id}");
        m.node(
            id,
            "sm.transition",
            &[("rule", json!(rule_id)), ("duration", json!(duration))],
        );
        m.wire_pin(&format!("s_{from}"), "leave", id, "from");
        m.wire_pin(&format!("s_{to}"), "arrive", id, "to");
        let mut r = GraphBuilder::new(&rule_id, &params);
        r.node("res", "rule.result", &[]);
        r.node("g", "anim.get_parameter", &[("parameter", json!("go"))]);
        if negate {
            r.node("n", "logic.not", &[]);
            r.wire("g", "n", "value").wire("n", "res", "can_enter");
        } else {
            r.wire("g", "res", "can_enter");
        }
        asset
            .subgraphs
            .push(sub(&rule_id, GraphKind::TransitionRule, r));
    }
    asset
        .subgraphs
        .push(sub("Loco", GraphKind::StateMachine, m));
    asset
}

/// `(event kind, machine, state, from, to, transition)` of what the component raised, in order.
pub(super) type Seen = Vec<(String, String, String, String, String, String)>;

pub(super) fn listen_states(
    hub: &EventHub,
    instance: pulsar_scenedb::Entity,
    owner: pulsar_scenedb::Entity,
    graph_id: &str,
) -> (
    Arc<Mutex<Seen>>,
    Arc<Mutex<Vec<String>>>,
    Vec<pulsar_events::gamma::SyncSubscription>,
) {
    let generic = Arc::new(Mutex::new(Seen::new()));
    let canonical = Arc::new(Mutex::new(Vec::new()));
    let mut subscriptions = Vec::new();
    for (generic_name, kind) in [
        ("anim_state_entered", StateEventKind::Entered),
        ("anim_state_exited", StateEventKind::Exited),
        ("anim_transition_started", StateEventKind::TransitionStarted),
        (
            "anim_transition_completed",
            StateEventKind::TransitionCompleted,
        ),
    ] {
        let out = generic.clone();
        let id = hub
            .descriptor_by_name(&format!("SkeletalMeshComponent.{generic_name}"))
            .unwrap()
            .id;
        subscriptions.push(hub.bus().subscribe_dyn(
            id,
            SubscribeOptions::channel(Channel::Entity(instance.bits())),
            move |event| {
                let DynValue::Bytes(bytes) = &event.fields[0] else {
                    panic!("codec payload")
                };
                let value = pulsar_world_registry::pulsar_script_vm::TypeRegistry::global()
                    .decode_event_value("AnimStateEvent", bytes)
                    .unwrap();
                let e = value.downcast_ref::<AnimStateEvent>().unwrap().clone();
                out.lock().unwrap().push((
                    kind.name().into(),
                    e.machine,
                    e.state,
                    e.from_state,
                    e.to_state,
                    e.transition,
                ));
            },
        ));
        let descriptor = anim_state_descriptor(graph_id, kind);
        hub.register(
            descriptor.clone(),
            EventCategory::Custom("Animation".into()),
        )
        .unwrap();
        let out = canonical.clone();
        let name = descriptor.name.clone();
        subscriptions.push(hub.bus().subscribe_dyn(
            descriptor.id,
            SubscribeOptions::channel(entity_channel(owner.bits())),
            move |_| out.lock().unwrap().push(name.clone()),
        ));
    }
    (generic, canonical, subscriptions)
}

fn row(
    kind: &str,
    state: &str,
    from: &str,
    to: &str,
    transition: &str,
) -> (String, String, String, String, String, String) {
    (
        kind.to_string(),
        "Loco".to_string(),
        state.to_string(),
        from.to_string(),
        to.to_string(),
        transition.to_string(),
    )
}

#[test]
fn state_machine_events_reach_their_own_component_in_order_and_no_other() {
    let rig = rig();
    let graph = machine_graph(&rig);
    let hub = registered_hub();
    let mut world = World::new();
    let (owner_a, owner_b) = (world.spawn(), world.spawn());
    let a =
        pulsar_world_registry::attach_value(&mut world, owner_a, component(&rig, &graph)).unwrap();
    let b =
        pulsar_world_registry::attach_value(&mut world, owner_b, component(&rig, &graph)).unwrap();
    let (seen_a, canonical_a, _sa) = listen_states(&hub, a, owner_a, &graph.id);
    let (seen_b, _canonical_b, _sb) = listen_states(&hub, b, owner_b, &graph.id);
    let mut state = ComponentRuntimeState::default();

    frame(&mut world, &hub, 0.1, &mut state);
    world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .set_animation_bool("go", true)
        .unwrap();
    for _ in 0..4 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .set_animation_bool("go", false)
        .unwrap();
    for _ in 0..2 {
        frame(&mut world, &hub, 0.1, &mut state);
    }

    let seen_a = seen_a.lock().unwrap().clone();
    assert_eq!(seen_a[0], row("entered", "Idle", "Idle", "Idle", ""));
    assert_eq!(
        seen_a[1..].to_vec(),
        vec![
            row("transition_started", "Idle", "Idle", "Moving", "start"),
            row("entered", "Moving", "Idle", "Moving", "start"),
            row("exited", "Idle", "Idle", "Moving", "start"),
            row("transition_completed", "Idle", "Idle", "Moving", "start"),
            row("transition_started", "Moving", "Moving", "Idle", "stop"),
            row("entered", "Idle", "Moving", "Idle", "stop"),
            row("exited", "Moving", "Moving", "Idle", "stop"),
            row("transition_completed", "Moving", "Moving", "Idle", "stop"),
        ]
    );
    let canonical = canonical_a.lock().unwrap().clone();
    assert_eq!(
        canonical.len(),
        seen_a.len(),
        "one canonical occurrence per generic event"
    );
    assert!(canonical
        .iter()
        .all(|name| name.starts_with(&format!("{}.state_", graph.id))));
    assert_eq!(
        seen_b.lock().unwrap().clone(),
        vec![row("entered", "Idle", "Idle", "Idle", "")],
        "B saw only its own start"
    );
}

#[test]
fn blueprint_events_of_a_graph_with_a_machine_have_four_distinct_canonical_names() {
    let rig = rig();
    let graph = machine_graph(&rig);
    let names: Vec<_> = StateEventKind::ALL
        .iter()
        .map(|k| anim_state_descriptor(&graph.id, *k).name)
        .collect();
    assert!(names.iter().all(|n| n.contains(".state_")));
    assert_eq!(
        names.iter().collect::<std::collections::HashSet<_>>().len(),
        4
    );
}

#[test]
fn blueprint_discovery_offers_the_state_events_of_a_graph_that_has_a_machine_and_only_that() {
    let _guard = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig();
    let montage = montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph);
    let with = write_project_with(&rig, &montage, Some(machine_asset_for(&rig)));
    let ids: std::collections::HashSet<String> = [with.graph_id.clone()].into();
    let catalog = component_animation_notify_events(&with.root, &ids);
    let names: Vec<String> = catalog.events.iter().map(|(_, d)| d.name.clone()).collect();
    for kind in StateEventKind::ALL {
        assert!(
            names.contains(&format!("{}.state_{}", with.graph_id, kind.name())),
            "{kind:?}: {names:?}"
        );
    }
    let without = write_project(&rig, &montage);
    let ids: std::collections::HashSet<String> = [without.graph_id.clone()].into();
    let names: Vec<String> = component_animation_notify_events(&without.root, &ids)
        .events
        .iter()
        .map(|(_, d)| d.name.clone())
        .collect();
    assert!(
        !names.iter().any(|n| n.contains(".state_")),
        "a graph with no state machine offers none: {names:?}"
    );
}
