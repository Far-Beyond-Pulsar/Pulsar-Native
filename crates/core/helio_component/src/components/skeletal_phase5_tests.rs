//! Phase 5: the plan's end-to-end character flow through the real component
//! runtime, headless and deterministic. A locomotion graph with a 2D Blend Space
//! (Speed x Direction) runs on a placed component; an upper-body Montage with a
//! bone mask layers over it; the section jumps; root motion moves the owner; the
//! Montage's authored notify state delivers its payload to its own component and
//! to no other; and the pose physics reads and the palette the GPU skins with
//! agree. (The editor half of the flow, edit/save/reopen of a Sequence notify and
//! state, is `skm_editor`'s `phase5_end_to_end` test.)

use super::phase3_tests::*;
use super::*;
use pulsar_scenedb::World;
use pulsar_world_registry::ComponentRuntimeState;
use serde_json::json;
use skeletal_assets::anim_graph::builder::GraphBuilder;
use skeletal_assets::anim_graph::catalogue::ParamType;
use skeletal_assets::anim_graph::{AnimationGraphAsset, GraphParameter};
use skeletal_assets::assets::AnimSeqAsset;
use skeletal_assets::blend_space::{BlendAxis, BlendSample, BlendSpaceAsset};
use skeletal_assets::bone_mask::{BoneMaskSpec, MaskBranch};
use skeletal_assets::montage::MontageRootMotion;
use skeletal_assets::runtime::graph::compile::compile_graph;

fn sequence(rig: &Rig, name: &str, tracks: Vec<skeletal_assets::animation::BoneTrack>, root_motion: bool) -> Arc<AnimSeqAsset> {
    let mut seq = AnimSeqAsset::new(
        None,
        skeletal_assets::animation::AnimationClip { name: name.into(), duration: 2.0, fps: 30.0, tracks },
    );
    seq.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap());
    seq.features.root_motion.enabled = root_motion;
    Arc::new(seq)
}

/// Locomotion over a 2D space: Idle at (0,0), Walk at (1,0), Strafe at (0,1).
fn locomotion(rig: &mut Rig) -> (Arc<CompiledGraph>, BlendSpaceAsset) {
    let idle = sequence(rig, "Idle", vec![], false);
    let strafe = sequence(rig, "Strafe", vec![track("b1", [0.0, 0.0, 4.0], 2.0)], false);
    let walk = rig.source.sequences[&rig.walk_id].clone();
    let mut space = BlendSpaceAsset::default();
    space.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap().to_ref());
    space.x_axis = BlendAxis { name: "Speed".into(), min: 0.0, max: 1.0, ..BlendAxis::default() };
    space.y_axis = Some(BlendAxis { name: "Direction".into(), min: 0.0, max: 1.0, ..BlendAxis::default() });
    for (seq, x, y) in [(&idle, 0.0, 0.0), (&walk, 1.0, 0.0), (&strafe, 0.0, 1.0)] {
        space.samples.push(BlendSample::new(AssetRef::parse(seq.id.as_str()), x, Some(y)));
    }
    rig.source.sequences.insert(idle.id.as_str().to_string(), idle);
    rig.source.sequences.insert(strafe.id.as_str().to_string(), strafe);
    rig.source.blend_spaces.insert(space.id.as_str().to_string(), Arc::new(space.clone()));

    let mut asset = AnimationGraphAsset::default();
    asset.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap().to_ref());
    asset.parameters.push(GraphParameter::new("Speed", ParamType::Float));
    asset.parameters.push(GraphParameter::new("Direction", ParamType::Float));
    let mut b = GraphBuilder::new("main", &[("Speed", ParamType::Float), ("Direction", ParamType::Float)]);
    b.node("out", "anim.output_pose", &[]);
    b.node("space", "anim.blend_space_player", &[("blend_space", json!(space.id.as_str())), ("looping", json!("loop"))]);
    b.node("speed", "anim.get_parameter", &[("parameter", json!("Speed"))]);
    b.node("dir", "anim.get_parameter", &[("parameter", json!("Direction"))]);
    b.node("slot", "anim.slot", &[("slot", json!("Body"))]);
    b.wire("speed", "space", "x").wire("dir", "space", "y").wire("space", "slot", "source").wire("slot", "out", "pose");
    asset.main = b.into_graph();
    let out = compile_graph(&asset, &rig.source);
    let graph = out.graph.unwrap_or_else(|| panic!("{:#?}", out.diagnostics));
    (graph, space)
}

fn mesh_for(rig: &Rig) -> Arc<skeletal_assets::skmesh::SkeletalMeshAsset> {
    let mut mesh = skeletal_assets::skmesh::SkeletalMeshAsset::new(Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap()));
    mesh.joints = (0..3).map(|i| skeletal_assets::skmesh::Joint { bone: format!("b{i}"), inverse_bind: skeletal_assets::math::Mat4::IDENTITY.0 }).collect();
    Arc::new(mesh)
}

fn bone(world: &World, e: pulsar_scenedb::Entity, i: usize) -> skeletal_assets::runtime::pose::BoneTransform {
    world.get::<SkeletalMeshComponent>(e).unwrap().animation().unwrap().pose().bones[i].clone()
}

#[test]
fn the_character_flow_runs_end_to_end_on_the_real_component_runtime() {
    let mut rig = rig();
    let (graph, space) = locomotion(&mut rig);
    assert!(space.is_2d());

    // an upper-body Montage over the Walk-shaped "Swing" clip: b1 and the bones below take it
    let swing = sequence(&rig, "Swing", vec![track("b1", [0.0, 0.0, 6.0], 2.0), track("b2", [0.0, 5.0, 0.0], 2.0)], false);
    rig.source.sequences.insert(swing.id.as_str().to_string(), swing.clone());
    let mut asset = montage_asset(&rig, MontageRootMotion::Graph);
    asset.tracks[0].segments[0].sequence = AssetRef::parse(swing.id.as_str());
    asset.tracks[0].mask = Some(BoneMaskSpec { branches: vec![MaskBranch { bone: "b1".into(), depth: None, weight: 1.0 }], default_weight: 0.0, ..Default::default() });
    let montage = compile_montage_asset(&rig, &asset);

    let hub = registered_hub();
    let mut world = World::new();
    let (owner_a, owner_b) = (owner_with_transform(&mut world), owner_with_transform(&mut world));
    let make = |mode| {
        let mut c = component(&rig, &graph);
        c.loaded_mesh = Some(mesh_for(&rig));
        c.root_motion_mode = mode;
        c
    };
    let a = pulsar_world_registry::attach_value(&mut world, owner_a, make(RootMotionModeSetting::RootMotionFromEverything)).unwrap();
    let b = pulsar_world_registry::attach_value(&mut world, owner_b, make(RootMotionModeSetting::RootMotionFromEverything)).unwrap();
    let state_descriptor = montage_notify_state_descriptor(&montage.id, &montage.notify_states[0]);
    let (sink_a, _sa) = listen(&hub, a, owner_a, &[state_descriptor.clone()]);
    let (sink_b, _sb) = listen(&hub, b, owner_b, &[state_descriptor.clone()]);
    let mut state = ComponentRuntimeState::default();

    // 1. 2D blend: both components walk, B is held at idle
    for (entity, speed) in [(a, 1.0), (b, 0.0)] {
        let mut c = world.get_mut::<SkeletalMeshComponent>(entity).unwrap();
        c.set_animation_float("Speed", speed).unwrap();
        c.set_animation_float("Direction", 0.0).unwrap();
    }
    for _ in 0..4 {
        frame(&mut world, &hub, 0.25, &mut state); // 1.0 s
    }
    assert!((bone(&world, a, 1).translation.x - 2.0).abs() < 1e-3, "walk at full speed: 4 m over 2 s at 1 s");
    assert!(bone(&world, b, 1).translation.x.abs() < 1e-4, "idle does not slide");
    // root motion moved only the walker: 4 m along the yawed owner's forward
    assert!((owner_position(&world, owner_a)[0] - 14.0).abs() < 1e-2, "{:?}", owner_position(&world, owner_a));
    assert_eq!(owner_position(&world, owner_b), [10.0, 0.0, 0.0]);

    // half speed is half the walk (the space blends along the Idle-Walk edge)
    world.get_mut::<SkeletalMeshComponent>(a).unwrap().set_animation_float("Speed", 0.5).unwrap();
    frame(&mut world, &hub, 0.25, &mut state); // clock 1.25 s
    assert!((bone(&world, a, 1).translation.x - 0.5 * 4.0 * 1.25 / 2.0).abs() < 1e-2, "{}", bone(&world, a, 1).translation.x);
    world.get_mut::<SkeletalMeshComponent>(a).unwrap().set_animation_float("Speed", 1.0).unwrap();

    // 2. Montage on component A only, layered over locomotion by the mask
    let handle = world.get_mut::<SkeletalMeshComponent>(a).unwrap().play_compiled_montage(montage.clone(), 1.0, None).unwrap();
    for _ in 0..2 {
        frame(&mut world, &hub, 0.25, &mut state); // 0.5 s into the Montage
    }
    let upper = bone(&world, a, 1);
    assert!(upper.translation.x.abs() < 1e-3 && (upper.translation.z - 6.0 * 0.5 / 2.0).abs() < 1e-2, "the masked bone plays the Montage clip: {:?}", upper.translation);
    // the unmasked root keeps playing locomotion: its travel is extracted to the owner, which keeps moving
    assert!(owner_position(&world, owner_a)[0] > 14.0 + 0.2, "{:?}", owner_position(&world, owner_a));
    assert!(bone(&world, a, 0).translation.z.abs() < 1e-3, "extracted root travel is not left in the pose");
    assert!((bone(&world, a, 2).translation.y - 5.0 * 0.5 / 2.0).abs() < 1e-2, "the mask covers the subtree under b1");
    assert!((bone(&world, b, 1).translation.z).abs() < 1e-4, "B has no Montage and is untouched");

    // 3. section jump
    world.get_mut::<SkeletalMeshComponent>(a).unwrap().jump_to_montage_section(handle, "B").unwrap();
    frame(&mut world, &hub, 0.25, &mut state);
    {
        let sink = sink_a.lock().unwrap();
        let changed: Vec<_> = sink.montage.iter().filter(|(kind, e)| kind == "section_changed" && e.instance == handle.0 as i64).collect();
        assert_eq!(changed.last().map(|(_, e)| (e.section_from.as_str(), e.section_to.as_str())), Some(("A", "B")));
    }
    for _ in 0..6 {
        frame(&mut world, &hub, 0.25, &mut state);
    }

    // 4. per-component event payload: A's state reached A's owner with the payload, B saw nothing
    let a_events = sink_a.lock().unwrap();
    let authored: Vec<_> = a_events.authored.iter().filter(|(name, _)| *name == state_descriptor.name).collect();
    assert!(!authored.is_empty(), "the Montage's notify state reached its component");
    for (_, fields) in &authored {
        assert_eq!(fields[0], pulsar_events::gamma::DynValue::U64(owner_a.bits()));
        assert_eq!(fields.last(), Some(&pulsar_events::gamma::DynValue::I64(7)), "the payload follows the common fields");
    }
    assert_eq!(a_events.montage.iter().filter(|(kind, _)| kind == "ended").count(), 1);
    let b_events = sink_b.lock().unwrap();
    assert!(b_events.authored.is_empty() && b_events.montage.is_empty() && b_events.notifies.iter().all(|e| e.notify_id != "ms"), "B raised nothing of A's");
    drop((a_events, b_events));

    // 5. physics and GPU agree: the pose physics reads is the palette the GPU skins with
    for entity in [a, b] {
        let c = world.get::<SkeletalMeshComponent>(entity).unwrap();
        assert_eq!(c.bone_world_pose.len(), 3);
        for joint in 0..3 {
            let (read, drawn) = (glam::Mat4::from_cols_array_2d(&c.bone_world_pose[joint]), glam::Mat4::from_cols_array_2d(&c.bone_palette[joint]));
            assert!(read.abs_diff_eq(drawn, 1e-4), "joint {joint}: physics read {read:?}, GPU palette {drawn:?}");
        }
    }
    // and once physics owns a bone, the palette shows the body while physics keeps reading the animation
    let body = glam::Mat4::from_translation(glam::Vec3::new(0.0, 9.0, 0.0)).to_cols_array_2d();
    let mut c = world.get_mut::<SkeletalMeshComponent>(a).unwrap();
    assert!(c.apply_physics_bones(vec![(1, body)]));
    let read_before = c.bone_world_pose[1];
    assert!((c.bone_palette[1][3][1] - 9.0).abs() < 1e-4, "the GPU draws the body");
    assert_eq!(c.bone_world_pose[1], read_before, "physics still reads the animated bone");
}
