//! One deterministic end-to-end flow over a single project on disk.
//!
//! The project holds a skeleton with a named `UpperBody` mask, four sequences, a 2D
//! Blend Space, a locomotion Animation Graph (a state machine whose `Moving` state
//! plays the Blend Space) and an upper-body Montage that names the skeleton mask.
//! Everything the runtime plays is compiled from those files. Two components share
//! the assets. The flow edits the Walk sequence file (a notify and a notify state,
//! saved and reopened) and hot reloads the running graph, drives locomotion, plays
//! the Montage layered over it, jumps its section, moves the owner by root motion,
//! checks the events each component raised (state machine, Montage, authored
//! payload), and compares the pose physics reads with the palette the GPU skins with.

use super::phase3_tests::*;
use super::*;
use pulsar_events::gamma::DynValue;
use pulsar_scenedb::World;
use pulsar_world_registry::ComponentRuntimeState;
use serde_json::json;
use skeletal_assets::anim_graph::builder::GraphBuilder;
use skeletal_assets::anim_graph::catalogue::{GraphKind, ParamType};
use skeletal_assets::anim_graph::{AnimSubGraph, AnimationGraphAsset, GraphParameter};
use skeletal_assets::assets::AnimSeqAsset;
use skeletal_assets::blend_space::{BlendAxis, BlendSample, BlendSpaceAsset};
use skeletal_assets::bone_mask::{BoneMaskSpec, MaskBranch, NamedBoneMask};
use skeletal_assets::montage::MontageRootMotion;
use skeletal_assets::notifies::{AnimationNotify, NotifyField, NotifyTarget, NotifyValue};
use skeletal_assets::runtime::graph::StateEventKind;

struct Disk {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    walk_path: std::path::PathBuf,
    graph_path: std::path::PathBuf,
    graph_id: String,
    montage_id: String,
}

fn sequence(
    rig: &Rig,
    name: &str,
    tracks: Vec<skeletal_assets::animation::BoneTrack>,
    root_motion: bool,
) -> AnimSeqAsset {
    let mut seq = AnimSeqAsset::new(
        None,
        skeletal_assets::animation::AnimationClip {
            name: name.into(),
            duration: 2.0,
            fps: 30.0,
            tracks,
        },
    );
    seq.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap());
    seq.features.root_motion.enabled = root_motion;
    seq
}

fn sub(id: &str, kind: GraphKind, graph: GraphBuilder) -> AnimSubGraph {
    AnimSubGraph {
        id: id.into(),
        name: id.into(),
        kind,
        graph: graph.into_graph(),
    }
}

/// `Loco`: `Idle` plays the Idle sequence, `Moving` plays the 2D Blend Space on `Speed`/`Direction`;
/// `go` starts and (negated) stops it. The machine feeds the `Body` slot.
fn locomotion_graph(rig: &Rig, idle: &str, space: &str) -> AnimationGraphAsset {
    let params = [
        ("go", ParamType::Bool),
        ("Speed", ParamType::Float),
        ("Direction", ParamType::Float),
    ];
    let mut asset = AnimationGraphAsset::default();
    asset.skeleton = Some(
        skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str())
            .unwrap()
            .to_ref(),
    );
    for (name, ty) in params {
        asset.parameters.push(GraphParameter::new(name, ty));
    }
    let mut main = GraphBuilder::new("main", &params);
    main.node("out", "anim.output_pose", &[]);
    main.node("sm", "anim.state_machine", &[("machine", json!("Loco"))]);
    main.node("slot", "anim.slot", &[("slot", json!("Body"))]);
    main.wire("sm", "slot", "source")
        .wire("slot", "out", "pose");
    asset.main = main.into_graph();

    let mut m = GraphBuilder::new("Loco", &params);
    m.node("entry", "sm.entry", &[]);
    m.node(
        "s_Idle",
        "sm.state",
        &[
            ("name", json!("Idle")),
            ("pose_graph", json!("Loco_p_Idle")),
        ],
    );
    m.node(
        "s_Moving",
        "sm.state",
        &[
            ("name", json!("Moving")),
            ("pose_graph", json!("Loco_p_Moving")),
        ],
    );
    m.wire_pin("entry", "start", "s_Idle", "default");
    let mut idle_pose = GraphBuilder::new("Loco_p_Idle", &params);
    idle_pose.node("p", "anim.sequence_player", &[("sequence", json!(idle))]);
    idle_pose.node("r", "anim.state_result", &[]);
    idle_pose.wire("p", "r", "pose");
    asset
        .subgraphs
        .push(sub("Loco_p_Idle", GraphKind::StatePose, idle_pose));
    let mut moving_pose = GraphBuilder::new("Loco_p_Moving", &params);
    moving_pose.node(
        "space",
        "anim.blend_space_player",
        &[("blend_space", json!(space)), ("looping", json!("loop"))],
    );
    moving_pose.node(
        "speed",
        "anim.get_parameter",
        &[("parameter", json!("Speed"))],
    );
    moving_pose.node(
        "dir",
        "anim.get_parameter",
        &[("parameter", json!("Direction"))],
    );
    moving_pose.node("r", "anim.state_result", &[]);
    moving_pose
        .wire("speed", "space", "x")
        .wire("dir", "space", "y")
        .wire("space", "r", "pose");
    asset
        .subgraphs
        .push(sub("Loco_p_Moving", GraphKind::StatePose, moving_pose));
    for (id, from, to, duration, negate) in [
        ("start", "Idle", "Moving", 0.2, false),
        ("stop", "Moving", "Idle", 0.0, true),
    ] {
        let rule = format!("Loco_r_{id}");
        m.node(
            id,
            "sm.transition",
            &[("rule", json!(rule)), ("duration", json!(duration))],
        );
        m.wire_pin(&format!("s_{from}"), "leave", id, "from");
        m.wire_pin(&format!("s_{to}"), "arrive", id, "to");
        let mut r = GraphBuilder::new(&rule, &params);
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
            .push(sub(&rule, GraphKind::TransitionRule, r));
    }
    asset
        .subgraphs
        .push(sub("Loco", GraphKind::StateMachine, m));
    asset
}

fn write_disk(rig: &mut Rig) -> Disk {
    use pulsar_reflection::{
        AnimationGraphAssetKind, AnimationMontageAssetKind, AnimationSequenceAssetKind, AssetKind,
        BlendSpaceAssetKind, SkeletonAssetKind,
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let index = engine_fs::open_project_file_index(&root)
        .unwrap()
        .expect("local index");
    let write = |name: &str, text: String| {
        let path = root.join(name);
        std::fs::write(&path, text).unwrap();
        index.refresh_path(&path).unwrap();
        path
    };

    // the skeleton carries the named mask the Montage uses: b1 and everything below it
    let mut skeleton = (*rig.skeleton).clone();
    skeleton.masks.push(NamedBoneMask {
        name: "UpperBody".into(),
        spec: BoneMaskSpec {
            branches: vec![MaskBranch {
                bone: "b1".into(),
                depth: None,
                weight: 1.0,
            }],
            ..Default::default()
        },
    });
    let skeleton_path = write("body.skeleton", skeleton.to_text());
    index
        .record_typed_asset_id(
            SkeletonAssetKind::KIND,
            skeleton.id.as_str(),
            &skeleton_path,
        )
        .unwrap();
    rig.skeleton = Arc::new(skeleton);

    let idle = sequence(rig, "Idle", vec![], false);
    let strafe = sequence(
        rig,
        "Strafe",
        vec![track("b1", [0.0, 0.0, 4.0], 2.0)],
        false,
    );
    let swing = sequence(
        rig,
        "Swing",
        vec![
            track("b1", [0.0, 0.0, 6.0], 2.0),
            track("b2", [0.0, 5.0, 0.0], 2.0),
        ],
        false,
    );
    let walk = rig.source.sequences[&rig.walk_id].clone();
    let mut walk_path = std::path::PathBuf::new();
    for (name, seq) in [
        ("idle", &idle),
        ("strafe", &strafe),
        ("swing", &swing),
        ("walk", &*walk),
    ] {
        let path = write(&format!("{name}.animseq"), seq.to_text());
        index
            .record_typed_asset_id(AnimationSequenceAssetKind::KIND, seq.id.as_str(), &path)
            .unwrap();
        if name == "walk" {
            walk_path = path;
        }
    }

    let mut space = BlendSpaceAsset::default();
    space.skeleton = Some(
        skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str())
            .unwrap()
            .to_ref(),
    );
    space.x_axis = BlendAxis {
        name: "Speed".into(),
        min: 0.0,
        max: 1.0,
        ..BlendAxis::default()
    };
    space.y_axis = Some(BlendAxis {
        name: "Direction".into(),
        min: 0.0,
        max: 1.0,
        ..BlendAxis::default()
    });
    for (seq, x, y) in [(&idle, 0.0, 0.0), (&*walk, 1.0, 0.0), (&strafe, 0.0, 1.0)] {
        space.samples.push(BlendSample::new(
            AssetRef::parse(seq.id.as_str()),
            x,
            Some(y),
        ));
    }
    let space_path = write("move.blendspace", space.to_text().unwrap());
    index
        .record_typed_asset_id(BlendSpaceAssetKind::KIND, space.id.as_str(), &space_path)
        .unwrap();

    let graph = locomotion_graph(rig, idle.id.as_str(), space.id.as_str());
    let graph_id = graph.id.as_str().to_string();
    let graph_path = write("locomotion.animgraph", graph.to_text().unwrap());
    index
        .record_typed_asset_id(AnimationGraphAssetKind::KIND, &graph_id, &graph_path)
        .unwrap();

    // the upper-body Montage over the Swing clip, masked by the skeleton's UpperBody
    let mut montage = montage_asset(rig, MontageRootMotion::Graph);
    montage.tracks[0].segments[0].sequence = AssetRef::parse(swing.id.as_str());
    montage.tracks[0].mask = Some(BoneMaskSpec::named("UpperBody"));
    let montage_id = montage.id.as_str().to_string();
    let montage_path = write("swing.montage", montage.to_text().unwrap());
    index
        .record_typed_asset_id(AnimationMontageAssetKind::KIND, &montage_id, &montage_path)
        .unwrap();

    // runtime copies of the sequences, for compiling the Montage
    for seq in [&idle, &strafe, &swing] {
        rig.source
            .sequences
            .insert(seq.id.as_str().to_string(), Arc::new(seq.clone()));
    }
    Disk {
        _dir: dir,
        root,
        walk_path,
        graph_path,
        graph_id,
        montage_id,
    }
}

fn compile_from_disk(disk: &Disk) -> (Arc<CompiledGraph>, Vec<String>) {
    let source = animation_source_for_file(&disk.graph_path);
    let loaded = compile_graph_file(&source, &ProjectAnimSource::at(disk.root.clone()))
        .expect("the graph reads");
    let graph = loaded
        .graph
        .unwrap_or_else(|| panic!("{:#?}", loaded.diagnostics));
    let dependencies = dependency_paths_in(&disk.root, &graph);
    (graph, dependencies)
}

fn montage_from_disk(disk: &Disk, graph: &CompiledGraph) -> Arc<CompiledMontage> {
    use skeletal_assets::runtime::graph::compile::AssetSource;
    let files = ProjectAnimSource::at(disk.root.clone());
    let asset = files.montage(&disk.montage_id).expect("the Montage reads");
    Arc::new(
        CompiledMontage::compile(&asset, &graph.skeleton, &mut |id| {
            files
                .sequence(id)
                .map(|seq| Arc::new(CompiledClip::compile_shared(&seq, &graph.skeleton)))
        })
        .expect("the Montage compiles against the skeleton's masks"),
    )
}

fn mesh_for(rig: &Rig) -> Arc<skeletal_assets::skmesh::SkeletalMeshAsset> {
    let mut mesh = skeletal_assets::skmesh::SkeletalMeshAsset::new(Some(
        skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap(),
    ));
    mesh.joints = (0..3)
        .map(|i| skeletal_assets::skmesh::Joint {
            bone: format!("b{i}"),
            inverse_bind: skeletal_assets::math::Mat4::IDENTITY.0,
        })
        .collect();
    Arc::new(mesh)
}

fn bone(
    world: &World,
    e: pulsar_scenedb::Entity,
    i: usize,
) -> skeletal_assets::runtime::pose::BoneTransform {
    world
        .get::<SkeletalMeshComponent>(e)
        .unwrap()
        .animation()
        .unwrap()
        .pose()
        .bones[i]
        .clone()
}

#[test]
fn the_whole_character_flow_runs_over_one_project_on_disk() {
    use pulsar_reflection::AssetKind;
    let _guard = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let mut rig = rig();
    let disk = write_disk(&mut rig);
    let (graph, dependencies) = compile_from_disk(&disk);
    assert!(
        graph
            .skeleton
            .mask_library
            .iter()
            .any(|m| m.name == "UpperBody"),
        "the graph's skeleton came from the file with its named mask"
    );
    assert!(
        dependencies.contains(&animation_source_for_file(&disk.walk_path)),
        "{dependencies:?}"
    );
    let montage = montage_from_disk(&disk, &graph);
    assert!(
        montage.tracks[0].mask.is_some(),
        "the Montage resolved the named mask"
    );

    // two components share every asset; A's owner is placed and yawed, B's owner is placed beside it
    let hub = registered_hub();
    let mut world = World::new();
    let (owner_a, owner_b) = (
        owner_with_transform(&mut world),
        owner_with_transform(&mut world),
    );
    let make = |net: RootMotionAuthoritySetting| {
        let mut c = component(&rig, &graph);
        c.loaded_mesh = Some(mesh_for(&rig));
        c.root_motion_mode = RootMotionModeSetting::RootMotionFromEverything;
        c.root_motion_authority = net;
        c.graph_source_path = Some(animation_source_for_file(&disk.graph_path));
        c.graph_dependency_paths = dependencies.clone();
        c
    };
    let a = pulsar_world_registry::attach_value(
        &mut world,
        owner_a,
        make(RootMotionAuthoritySetting::AutonomousProxy),
    )
    .unwrap();
    let b = pulsar_world_registry::attach_value(
        &mut world,
        owner_b,
        make(RootMotionAuthoritySetting::SimulatedProxy),
    )
    .unwrap();
    let state_descriptor = montage_notify_state_descriptor(&montage.id, &montage.notify_states[0]);
    let (sink_a, _sa) = listen(&hub, a, owner_a, &[state_descriptor.clone()]);
    let (sink_b, _sb) = listen(&hub, b, owner_b, &[state_descriptor.clone()]);
    let (states_a, _canonical_a, _ta) =
        super::state_event_tests::listen_states(&hub, a, owner_a, &graph.id);
    let (states_b, _canonical_b, _tb) =
        super::state_event_tests::listen_states(&hub, b, owner_b, &graph.id);
    let mut state = ComponentRuntimeState::default();
    frame(&mut world, &hub, 0.0, &mut state);

    // 1. the machine starts in Idle on both; A starts moving (2D blend: forward at full speed)
    for entity in [a, b] {
        let mut c = world.get_mut::<SkeletalMeshComponent>(entity).unwrap();
        c.set_animation_float("Speed", 1.0).unwrap();
        c.set_animation_float("Direction", 0.0).unwrap();
    }
    world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .set_animation_bool("go", true)
        .unwrap();
    for _ in 0..8 {
        frame(&mut world, &hub, 0.25, &mut state); // 2 s
    }
    let kinds = |seen: &Arc<std::sync::Mutex<super::state_event_tests::Seen>>| {
        seen.lock()
            .unwrap()
            .iter()
            .map(|e| (e.0.clone(), e.2.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        kinds(&states_a),
        vec![
            ("entered".into(), "Idle".into()),
            ("transition_started".into(), "Idle".into()),
            ("entered".into(), "Moving".into()),
            ("exited".into(), "Idle".into()),
            ("transition_completed".into(), "Idle".into()),
        ],
        "A's state machine events, in order, on A only"
    );
    assert_eq!(
        kinds(&states_b),
        vec![("entered".into(), "Idle".into())],
        "B never left Idle"
    );
    assert!(
        bone(&world, a, 1).translation.x > 1.0,
        "the blend space walks A: {:?}",
        bone(&world, a, 1).translation
    );
    assert!(bone(&world, b, 1).translation.x.abs() < 1e-4, "B idles");
    // root motion: A is predicted and logged, the simulated proxy B is not moved by its animation
    assert!(
        owner_position(&world, owner_a)[0] > 10.5,
        "{:?}",
        owner_position(&world, owner_a)
    );
    assert_eq!(owner_position(&world, owner_b), [10.0, 0.0, 0.0]);
    let moves = world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .drain_root_motion_moves();
    assert!(!moves.is_empty() && moves.windows(2).all(|w| w[1].sequence == w[0].sequence + 1));

    // 2. edit the Walk sequence file: a notify and a notify state with a payload; save, reopen, hot reload
    let mut walk = skeletal_assets::assets::load_animseq_asset(&disk.walk_path).unwrap();
    walk.features.notifies.push(AnimationNotify {
        id: "edited_notify".into(),
        name: "Edited".into(),
        time: 0.1,
        target: NotifyTarget::Self_,
        fields: vec![],
        schema: None,
    });
    let mut edited_state =
        skeletal_assets::notify_schema::AnimationNotifyState::new("EditedState", 0.2, 1.8);
    edited_state.id = "edited_state".into();
    edited_state.fields.push(NotifyField {
        name: "amount".into(),
        value: NotifyValue::Int(9),
    });
    walk.features.notify_states.push(edited_state);
    skeletal_assets::assets::save_animseq_asset(&disk.walk_path, &walk).unwrap();
    // the editor re-indexes the file it saved
    let index = engine_fs::open_project_file_index(&disk.root)
        .unwrap()
        .expect("local index");
    index.refresh_path(&disk.walk_path).unwrap();
    index
        .record_typed_asset_id(
            pulsar_reflection::AnimationSequenceAssetKind::KIND,
            walk.id.as_str(),
            &disk.walk_path,
        )
        .unwrap();
    let reopened = skeletal_assets::assets::load_animseq_asset(&disk.walk_path).unwrap();
    assert_eq!(
        reopened
            .features
            .notifies
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>(),
        ["edited_notify", "step"],
        "ids survive save and reopen (kept in time order)"
    );
    assert_eq!(
        reopened.features.notify_states.last().unwrap().fields[0].value,
        NotifyValue::Int(9)
    );
    let files = ProjectAnimSource::at(disk.root.clone());
    assert_eq!(
        reload_dependants(&mut world, &disk.walk_path, |c| c
            .recompile_graph_in(&files)),
        2,
        "both components depend on the Walk sequence"
    );
    let edited_descriptor = animation_notify_state_descriptor(
        &reopened,
        reopened.features.notify_states.last().unwrap(),
    );
    let (sink_edit, _se) = listen(&hub, a, owner_a, &[edited_descriptor.clone()]);
    for _ in 0..8 {
        frame(&mut world, &hub, 0.25, &mut state);
    }
    {
        let seen = sink_edit.lock().unwrap();
        let phases: Vec<_> = seen
            .notifies
            .iter()
            .filter(|e| e.notify_id == "edited_state")
            .map(|e| e.notify_phase.as_str())
            .collect();
        assert!(
            phases.first() == Some(&"begin") && phases.contains(&"tick"),
            "the edited state runs in the live graph: {phases:?}"
        );
        assert!(
            seen.authored
                .iter()
                .any(|(name, fields)| *name == edited_descriptor.name
                    && fields.last() == Some(&DynValue::I64(9))),
            "with its authored payload"
        );
        assert!(
            seen.notifies.iter().any(|e| e.notify_id == "edited_notify"),
            "and the edited notify fires"
        );
    }

    // 3. the Montage layered over the upper body by the skeleton's named mask, on A only
    let handle = world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .play_compiled_montage(montage.clone(), 1.0, None)
        .unwrap();
    for _ in 0..2 {
        frame(&mut world, &hub, 0.25, &mut state);
    }
    let upper = bone(&world, a, 1);
    assert!(
        upper.translation.x.abs() < 1e-3 && (upper.translation.z - 6.0 * 0.5 / 2.0).abs() < 1e-2,
        "b1 (inside UpperBody) plays the Montage: {:?}",
        upper.translation
    );
    assert!(
        (bone(&world, a, 2).translation.y - 5.0 * 0.5 / 2.0).abs() < 1e-2,
        "the mask covers the subtree"
    );
    assert!(
        bone(&world, b, 1).translation.z.abs() < 1e-4,
        "B has no Montage"
    );

    // 4. section jump
    world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .jump_to_montage_section(handle, "B")
        .unwrap();
    frame(&mut world, &hub, 0.25, &mut state);
    for _ in 0..6 {
        frame(&mut world, &hub, 0.25, &mut state);
    }
    {
        let sink = sink_a.lock().unwrap();
        let section: Vec<_> = sink
            .montage
            .iter()
            .filter(|(kind, _)| kind == "section_changed")
            .map(|(_, e)| (e.section_from.as_str(), e.section_to.as_str()))
            .collect();
        assert_eq!(section.last(), Some(&("A", "B")), "{section:?}");
        assert_eq!(
            sink.montage
                .iter()
                .filter(|(kind, _)| kind == "ended")
                .count(),
            1
        );
        // 5. the Montage's authored state reached A's owner with its payload, and B's nothing
        let authored: Vec<_> = sink
            .authored
            .iter()
            .filter(|(name, _)| *name == state_descriptor.name)
            .collect();
        assert!(!authored.is_empty());
        assert!(authored
            .iter()
            .all(|(_, fields)| fields[0] == DynValue::U64(owner_a.bits())
                && fields.last() == Some(&DynValue::I64(7))));
    }
    {
        let sink = sink_b.lock().unwrap();
        assert!(
            sink.authored.is_empty() && sink.montage.is_empty(),
            "B raised none of A's Montage events"
        );
    }

    // 6. physics and GPU agree, then physics owns the whole pose without stopping the animation
    for entity in [a, b] {
        let c = world.get::<SkeletalMeshComponent>(entity).unwrap();
        for joint in 0..3 {
            let (read, drawn) = (
                glam::Mat4::from_cols_array_2d(&c.bone_world_pose[joint]),
                glam::Mat4::from_cols_array_2d(&c.bone_palette[joint]),
            );
            assert!(
                read.abs_diff_eq(drawn, 1e-4),
                "joint {joint}: physics read {read:?}, GPU palette {drawn:?}"
            );
        }
    }
    let rig_pose: Vec<[[f32; 4]; 4]> = (0..3)
        .map(|i| {
            glam::Mat4::from_translation(glam::Vec3::new(20.0 + i as f32, 0.0, 0.0))
                .to_cols_array_2d()
        })
        .collect();
    let clock_before = bone(&world, a, 1).translation;
    assert!(world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .apply_physics_pose(rig_pose.clone()));
    for _ in 0..2 {
        frame(&mut world, &hub, 0.25, &mut state);
    }
    {
        let c = world.get::<SkeletalMeshComponent>(a).unwrap();
        for joint in 0..3 {
            assert!(
                (c.bone_palette[joint][3][0] - (20.0 + joint as f32)).abs() < 1e-4,
                "the GPU draws the physics pose"
            );
        }
    }
    assert_ne!(
        bone(&world, a, 1).translation,
        clock_before,
        "while the animation underneath kept advancing"
    );
    world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .clear_physics_pose();
    frame(&mut world, &hub, 0.25, &mut state);
    assert!(
        world.get::<SkeletalMeshComponent>(a).unwrap().bone_palette[1][3][0] < 19.0,
        "handed back to the animation"
    );

    // 7. A stops: the machine returns to Idle with the exit events
    world
        .get_mut::<SkeletalMeshComponent>(a)
        .unwrap()
        .set_animation_bool("go", false)
        .unwrap();
    frame(&mut world, &hub, 0.25, &mut state);
    let tail = kinds(&states_a);
    assert_eq!(
        tail.last(),
        Some(&("transition_completed".into(), "Moving".into())),
        "{tail:?}"
    );
    let _ = StateEventKind::ALL;
}
