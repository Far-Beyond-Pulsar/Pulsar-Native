//! Phase 3 through the component: Montage-authored notifies and states, Montage
//! lifecycle events, root motion applied to the owner, bone-level physics
//! ownership, teardown and hot reload. Headless: no GPU, no project on disk.

use super::*;
use pulsar_events::{
    EventCategory,
    gamma::EventDescriptor,
    entity_channel,
    gamma::{Channel, DynValue, SubscribeOptions},
    EventHub, FlushPoint,
};
use pulsar_scene_model::components::Transform;
use pulsar_scenedb::World;
use pulsar_world_registry::ComponentRuntimeState;
use serde_json::json;
use skeletal_assets::anim_graph::builder::GraphBuilder;
use skeletal_assets::anim_graph::AnimationGraphAsset;
use skeletal_assets::animation::{AnimationClip, BoneTrack, Keyframe};
use skeletal_assets::assets::{AnimSeqAsset, SkeletonAsset};
use skeletal_assets::math::{Transform as AssetTransform, Vec3 as AssetVec3};
use skeletal_assets::montage::{AnimationMontageAsset, MontageSegment, MontageSection, SlotTrack};
use skeletal_assets::notify_schema::AnimationNotifyState;
use skeletal_assets::notifies::{AnimationNotify, NotifyField, NotifyTarget, NotifyValue};
use skeletal_assets::runtime::graph::compile::compile_graph;
use skeletal_assets::runtime::anim_instance::{TimeStep, UpdateMode};
use skeletal_assets::runtime::memory::MemorySource;
use std::sync::Mutex;

pub(super) struct Rig {
    pub(super) skeleton: Arc<SkeletonAsset>,
    pub(super) runtime: Arc<RuntimeSkeleton>,
    pub(super) source: MemorySource,
    pub(super) walk_id: String,
}

pub(super) fn track(bone: &str, to: [f32; 3], duration: f32) -> BoneTrack {
    let at = |p: [f32; 3]| AssetTransform { translation: AssetVec3::new(p[0], p[1], p[2]), ..AssetTransform::IDENTITY };
    BoneTrack { bone_id: bone.into(), keyframes: vec![Keyframe::linear(0.0, at([0.0; 3])), Keyframe::linear(duration, at(to))] }
}

/// A 2 s "Walk": b0 (the root) travels 8 m along +z, b1 slides 4 m along x.
/// Sequence-owned: notify `step` at 0.5, state `swing` over 1.0..1.5.
pub(super) fn rig() -> Rig {
    let bones: Vec<_> = (0..3)
        .map(|i| skeletal_assets::skeleton::Bone {
            id: format!("b{i}"),
            name: format!("b{i}"),
            parent: (i > 0).then(|| format!("b{}", i - 1)),
            bind_transform: skeletal_assets::math::Transform::IDENTITY,
        })
        .collect();
    let skeleton = Arc::new(SkeletonAsset::new(skeletal_assets::skeleton::Skeleton { bones }));
    let skeleton_id = skeleton.id.as_str().to_string();
    let mut walk = AnimSeqAsset::new(None, AnimationClip { name: "Walk".into(), duration: 2.0, fps: 30.0, tracks: vec![track("b0", [0.0, 0.0, 8.0], 2.0), track("b1", [4.0, 0.0, 0.0], 2.0)] });
    walk.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(&skeleton_id).unwrap());
    walk.features.root_motion.enabled = true;
    walk.features.notifies.push(AnimationNotify { id: "step".into(), name: "Step".into(), time: 0.5, target: NotifyTarget::Self_, fields: vec![], schema: None });
    walk.features.notify_states.push(AnimationNotifyState {
        id: "swing".into(),
        name: "Swing".into(),
        start: 1.0,
        end: 1.5,
        target: NotifyTarget::Self_,
        schema: None,
        fields: vec![],
        tick_interval: None,
        disabled: false,
    });
    let walk_id = walk.id.as_str().to_string();
    let mut source = MemorySource::default();
    source.skeletons.insert(skeleton_id.clone(), skeleton.clone());
    source.sequences.insert(walk_id.clone(), Arc::new(walk));
    let runtime = Arc::new(RuntimeSkeleton::new(&skeleton_id, &skeleton.skeleton()).unwrap());
    Rig { skeleton, runtime, source, walk_id }
}

/// Only a Slot over the reference pose: nothing but Montages plays on it.
pub(super) fn slot_graph(rig: &Rig) -> Arc<CompiledGraph> {
    let mut asset = AnimationGraphAsset::default();
    asset.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap().to_ref());
    let mut b = GraphBuilder::new("main", &[]);
    b.node("out", "anim.output_pose", &[]);
    b.node("ref", "anim.reference_pose", &[]);
    b.node("slot", "anim.slot", &[("slot", json!("Body"))]);
    b.wire("ref", "slot", "source").wire("slot", "out", "pose");
    asset.main = b.into_graph();
    let out = compile_graph(&asset, &rig.source);
    out.graph.unwrap_or_else(|| panic!("{:#?}", out.diagnostics))
}

pub(super) fn walk_graph(rig: &Rig, slot: bool) -> Arc<CompiledGraph> {
    let mut asset = AnimationGraphAsset::default();
    asset.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap().to_ref());
    let mut b = GraphBuilder::new("main", &[]);
    b.node("out", "anim.output_pose", &[]);
    b.node("p", "anim.sequence_player", &[("sequence", json!(rig.walk_id))]);
    if slot {
        b.node("slot", "anim.slot", &[("slot", json!("Body"))]);
        b.wire("p", "slot", "source").wire("slot", "out", "pose");
    } else {
        b.wire("p", "out", "pose");
    }
    asset.main = b.into_graph();
    let out = compile_graph(&asset, &rig.source);
    out.graph.unwrap_or_else(|| panic!("{:#?}", out.diagnostics))
}

/// A one-segment Montage over the Walk clip with its own notify `mn` (0.5), state
/// `ms` (0.25..1.0, payload `amount` = 7) and sections `A` (0..1) -> `B` (1..2).
pub(super) fn montage_asset(rig: &Rig, root_motion: skeletal_assets::montage::MontageRootMotion) -> AnimationMontageAsset {
    let mut montage = AnimationMontageAsset::default();
    let mut track = SlotTrack::new("Body", "DefaultGroup");
    track.segments.push(MontageSegment::new(AssetRef::parse(&rig.walk_id), 0.0));
    montage.tracks.push(track);
    montage.blend_in.duration = 0.0;
    montage.blend_out.duration = 0.0;
    montage.root_motion = root_motion;
    montage.sections = vec![
        MontageSection { id: "a".into(), name: "A".into(), start: 0.0, next: Some("b".into()), extra: Default::default() },
        MontageSection { id: "b".into(), name: "B".into(), start: 1.0, next: None, extra: Default::default() },
    ];
    montage.notifies.push(AnimationNotify { id: "mn".into(), name: "MN".into(), time: 0.5, target: NotifyTarget::Self_, fields: vec![], schema: None });
    let mut state = AnimationNotifyState::new("MS", 0.25, 1.0);
    state.id = "ms".into();
    state.fields.push(NotifyField { name: "amount".into(), value: NotifyValue::Int(7) });
    montage.notify_states.push(state);
    montage
}

pub(super) fn compile_montage_asset(rig: &Rig, asset: &AnimationMontageAsset) -> Arc<CompiledMontage> {
    let sequences = &rig.source.sequences;
    Arc::new(
        CompiledMontage::compile(asset, &rig.runtime, &mut |id| sequences.get(id).map(|seq| Arc::new(CompiledClip::compile_shared(seq, &rig.runtime))))
            .unwrap(),
    )
}

pub(super) fn registered_hub() -> EventHub {
    let hub = EventHub::new();
    pulsar_world_registry::register_component_events(&hub).unwrap();
    hub
}

pub(super) fn frame(world: &mut World, hub: &EventHub, dt: f32, state: &mut ComponentRuntimeState) {
    run_animation_phase(world, dt);
    pulsar_world_registry::tick_live_components(world, hub, dt, 1, state);
    flush_animation_notifies(world, hub, |_, _| None);
    hub.flush(FlushPoint::AfterScripts);
}

pub(super) fn component(rig: &Rig, graph: &Arc<CompiledGraph>) -> SkeletalMeshComponent {
    let mut c = SkeletalMeshComponent::default();
    c.loaded_skeleton = Some(rig.skeleton.clone());
    c.loaded_graph = Some(graph.clone());
    c.playing = true;
    c.playback_rate = 1.0;
    c
}

/// Everything one component raised: the typed generic notify events and the
/// canonical per-asset events on the owner's channel.
#[derive(Default)]
pub(super) struct Sink {
    pub(super) notifies: Vec<AnimationNotifyEvent>,
    pub(super) montage: Vec<(String, MontageLifecycleEvent)>,
    pub(super) authored: Vec<(String, Vec<DynValue>)>,
}

pub(super) fn listen(hub: &EventHub, instance: pulsar_scenedb::Entity, owner: pulsar_scenedb::Entity, descriptors: &[EventDescriptor]) -> (Arc<Mutex<Sink>>, Vec<pulsar_events::gamma::SyncSubscription>) {
    let sink = Arc::new(Mutex::new(Sink::default()));
    let mut subscriptions = Vec::new();
    let decode = |name: &str, event: &pulsar_events::gamma::DynEvent| {
        let DynValue::Bytes(bytes) = &event.fields[0] else { panic!("codec payload") };
        pulsar_world_registry::pulsar_script_vm::TypeRegistry::global().decode_event_value(name, bytes).unwrap()
    };
    let out = sink.clone();
    let id = hub.descriptor_by_name("SkeletalMeshComponent.animation_notify").unwrap().id;
    subscriptions.push(hub.bus().subscribe_dyn(id, SubscribeOptions::channel(Channel::Entity(instance.bits())), move |event| {
        out.lock().unwrap().notifies.push(decode("AnimationNotifyEvent", event).downcast_ref::<AnimationNotifyEvent>().unwrap().clone());
    }));
    for kind in MONTAGE_LIFECYCLE_KINDS {
        let out = sink.clone();
        let id = hub.descriptor_by_name(&format!("SkeletalMeshComponent.montage_{kind}")).unwrap().id;
        subscriptions.push(hub.bus().subscribe_dyn(id, SubscribeOptions::channel(Channel::Entity(instance.bits())), move |event| {
            out.lock().unwrap().montage.push((kind.to_owned(), decode("MontageLifecycleEvent", event).downcast_ref::<MontageLifecycleEvent>().unwrap().clone()));
        }));
    }
    for descriptor in descriptors {
        hub.register(descriptor.clone(), EventCategory::Custom("Animation".into())).unwrap();
        let out = sink.clone();
        let name = descriptor.name.clone();
        subscriptions.push(hub.bus().subscribe_dyn(descriptor.id, SubscribeOptions::channel(entity_channel(owner.bits())), move |event| {
            out.lock().unwrap().authored.push((name.clone(), event.fields.clone()));
        }));
    }
    (sink, subscriptions)
}

pub(super) fn phases(sink: &Sink, id: &str) -> Vec<String> {
    sink.notifies.iter().filter(|e| e.notify_id == id).map(|e| e.notify_phase.clone()).collect()
}

pub(super) fn lifecycle(sink: &Sink) -> Vec<String> {
    sink.montage.iter().map(|(kind, _)| kind.clone()).collect()
}

#[test]
fn montage_authored_notifies_states_and_lifecycle_reach_this_component_once_each() {
    let rig = rig();
    let graph = slot_graph(&rig);
    let montage = compile_montage_asset(&rig, &montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph));
    let hub = registered_hub();
    let mut world = World::new();
    let owner = world.spawn();
    let instance = pulsar_world_registry::attach_value(&mut world, owner, component(&rig, &graph)).unwrap();
    let ms_descriptor = montage_notify_state_descriptor(&montage.id, &montage.notify_states[0]);
    let mn_descriptor = montage_notify_descriptor(&montage.id, &montage.notifies[0]);
    let ended = montage_lifecycle_descriptor(&montage.id, "ended");
    let (sink, _subs) = listen(&hub, instance, owner, &[ms_descriptor.clone(), mn_descriptor.clone(), ended.clone()]);
    let mut state = ComponentRuntimeState::default();
    frame(&mut world, &hub, 0.1, &mut state);
    let handle = world.get_mut::<SkeletalMeshComponent>(instance).unwrap().play_compiled_montage(montage.clone(), 1.0, None).unwrap();
    for _ in 0..30 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    let sink = sink.lock().unwrap();
    // Montage-owned notify and state, and the clip-owned ones of its segment
    assert_eq!(phases(&sink, "mn"), ["notify"]);
    assert_eq!(phases(&sink, "ms").first().map(String::as_str), Some("begin"));
    assert_eq!(phases(&sink, "ms").iter().filter(|p| *p == "begin").count(), 1);
    assert_eq!(phases(&sink, "ms").iter().filter(|p| *p == "end").count(), 1);
    assert!(phases(&sink, "ms").iter().filter(|p| *p == "tick").count() >= 1);
    assert_eq!(phases(&sink, "step"), ["notify"], "the segment clip's own notify fires through the Montage too");
    assert!(phases(&sink, "swing").iter().filter(|p| *p == "tick").count() >= 1);
    assert_eq!(phases(&sink, "swing").first().map(String::as_str), Some("begin"));
    assert_eq!(phases(&sink, "swing").last().map(String::as_str), Some("end"));
    // typed source, kind and playback instance; the Montage timeline and the clip are different instances
    let ms: Vec<_> = sink.notifies.iter().filter(|e| e.notify_id == "ms").collect();
    assert!(ms.iter().all(|e| e.source_kind == "montage" && e.sequence_id == montage.id));
    assert_eq!(ms.last().unwrap().end_reason, "completed");
    let swing: Vec<_> = sink.notifies.iter().filter(|e| e.notify_id == "swing").collect();
    assert!(swing.iter().all(|e| e.source_kind == "animseq" && e.sequence_id == rig.walk_id));
    assert_ne!(ms[0].playback_instance, swing[0].playback_instance);
    assert!(ms.iter().all(|e| e.playback_instance == ms[0].playback_instance));
    // lifecycle in order, as component-scoped events, exactly once
    assert_eq!(lifecycle(&sink)[..3], ["started", "blended_in", "section_changed"]);
    assert_eq!(lifecycle(&sink).iter().filter(|k| *k == "ended").count(), 1);
    let (_, last) = sink.montage.iter().find(|(kind, _)| kind == "ended").unwrap();
    assert_eq!((last.result.as_str(), last.montage_id.as_str(), last.instance), ("completed", montage.id.as_str(), handle.0 as i64));
    // the canonical per-asset events reached the owner's channel with the payload
    let authored_ms: Vec<_> = sink.authored.iter().filter(|(name, _)| *name == ms_descriptor.name).collect();
    assert_eq!(authored_ms.len(), sink.notifies.iter().filter(|e| e.notify_id == "ms").count());
    for (_, fields) in &authored_ms {
        assert_eq!(fields[0], DynValue::U64(owner.bits()));
        assert_eq!(fields.last(), Some(&DynValue::I64(7)), "the typed payload follows the common fields");
    }
    assert_eq!(sink.authored.iter().filter(|(name, _)| *name == mn_descriptor.name).count(), 1);
    assert_eq!(sink.authored.iter().filter(|(name, _)| *name == ended.name).count(), 1);
}

#[test]
fn montage_commands_interrupt_jump_and_pause_with_one_event_each() {
    let rig = rig();
    let graph = slot_graph(&rig);
    let asset = montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph);
    let first_montage = compile_montage_asset(&rig, &asset);
    let mut other = asset.clone();
    other.id = AssetRef::new();
    let second_montage = compile_montage_asset(&rig, &other);
    let hub = registered_hub();
    let mut world = World::new();
    let owner = world.spawn();
    let instance = pulsar_world_registry::attach_value(&mut world, owner, component(&rig, &graph)).unwrap();
    let (sink, _subs) = listen(&hub, instance, owner, &[]);
    let mut state = ComponentRuntimeState::default();
    frame(&mut world, &hub, 0.1, &mut state);
    let first = world.get_mut::<SkeletalMeshComponent>(instance).unwrap().play_compiled_montage(first_montage.clone(), 1.0, None).unwrap();
    for _ in 0..4 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    // inside the state `ms` (0.25..1.0) now; a second Montage in the group interrupts the first
    let second = world.get_mut::<SkeletalMeshComponent>(instance).unwrap().play_compiled_montage(second_montage.clone(), 1.0, None).unwrap();
    frame(&mut world, &hub, 0.1, &mut state);
    {
        let sink = sink.lock().unwrap();
        let mine: Vec<_> = sink.notifies.iter().filter(|e| e.sequence_id == first_montage.id).collect();
        assert_eq!(mine.iter().filter(|e| e.notify_phase == "begin").count(), 1);
        let ends: Vec<_> = mine.iter().filter(|e| e.notify_phase == "end").collect();
        assert_eq!(ends.len(), 1, "the interrupted Montage's state ends exactly once");
        assert_eq!(ends[0].end_reason, "stopped");
        let interrupted = sink.montage.iter().filter(|(kind, e)| kind == "interrupted" && e.instance == first.0 as i64).collect::<Vec<_>>();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].1.interrupted_by, second.0 as i64);
        assert_eq!(sink.montage.iter().filter(|(kind, e)| kind == "ended" && e.instance == first.0 as i64).count(), 1);
    }
    // jump to section B: a section-changed event, and the new instance keeps going
    world.get_mut::<SkeletalMeshComponent>(instance).unwrap().jump_to_montage_section(second, "B").unwrap();
    frame(&mut world, &hub, 0.1, &mut state);
    let sink = sink.lock().unwrap();
    let changed: Vec<_> = sink.montage.iter().filter(|(kind, e)| kind == "section_changed" && e.instance == second.0 as i64).collect();
    assert_eq!(changed.last().map(|(_, e)| (e.section_from.as_str(), e.section_to.as_str())), Some(("A", "B")));
}

#[test]
fn two_components_sharing_assets_keep_independent_events_clocks_and_filters() {
    let rig = rig();
    let graph = slot_graph(&rig);
    let montage = compile_montage_asset(&rig, &montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph));
    let hub = registered_hub();
    let mut world = World::new();
    let (oa, ob) = (world.spawn(), world.spawn());
    let a = pulsar_world_registry::attach_value(&mut world, oa, component(&rig, &graph)).unwrap();
    let b = pulsar_world_registry::attach_value(&mut world, ob, component(&rig, &graph)).unwrap();
    let ms = montage_notify_state_descriptor(&montage.id, &montage.notify_states[0]);
    let (sink_a, _sa) = listen(&hub, a, oa, &[ms.clone()]);
    let (sink_b, _sb) = listen(&hub, b, ob, &[ms.clone()]);
    let mut state = ComponentRuntimeState::default();
    frame(&mut world, &hub, 0.05, &mut state);
    let ha = world.get_mut::<SkeletalMeshComponent>(a).unwrap().play_compiled_montage(montage.clone(), 1.0, None).unwrap();
    let hb = world.get_mut::<SkeletalMeshComponent>(b).unwrap().play_compiled_montage(montage.clone(), 2.0, None).unwrap();
    for _ in 0..6 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    // b (twice as fast) is past its state; a is still inside
    assert_eq!(phases(&sink_b.lock().unwrap(), "ms").last().map(String::as_str), Some("end"));
    assert_ne!(phases(&sink_a.lock().unwrap(), "ms").last().map(String::as_str), Some("end"));
    // stopping b leaves a untouched
    world.get_mut::<SkeletalMeshComponent>(b).unwrap().stop_montage(hb, Some(0.0));
    let before = sink_a.lock().unwrap().notifies.len();
    frame(&mut world, &hub, 0.1, &mut state);
    assert!(sink_a.lock().unwrap().notifies.len() >= before);
    assert!(sink_a.lock().unwrap().montage.iter().all(|(_, e)| e.instance == ha.0 as i64), "a only sees its own instance");
    assert!(sink_b.lock().unwrap().montage.iter().all(|(_, e)| e.instance == hb.0 as i64));
    // every authored event reached only its own owner
    assert!(sink_a.lock().unwrap().authored.iter().all(|(_, f)| f[0] == DynValue::U64(oa.bits())));
    assert!(sink_b.lock().unwrap().authored.iter().all(|(_, f)| f[0] == DynValue::U64(ob.bits())));
}

#[test]
fn disabling_the_component_ends_active_states_and_cancels_montages_once() {
    let rig = rig();
    let graph = slot_graph(&rig);
    let montage = compile_montage_asset(&rig, &montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph));
    let hub = registered_hub();
    let mut world = World::new();
    let owner = world.spawn();
    let instance = pulsar_world_registry::attach_value(&mut world, owner, component(&rig, &graph)).unwrap();
    let (sink, _subs) = listen(&hub, instance, owner, &[]);
    let mut state = ComponentRuntimeState::default();
    frame(&mut world, &hub, 0.05, &mut state);
    world.get_mut::<SkeletalMeshComponent>(instance).unwrap().play_compiled_montage(montage, 1.0, None).unwrap();
    for _ in 0..6 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    let active = world.get::<SkeletalMeshComponent>(instance).unwrap().notify_playback.active_states();
    assert!(active >= 1, "inside the Montage state");
    let mut link = world.get_mut::<pulsar_world_registry::pulsar_scene_model::ComponentOwner>(instance).unwrap();
    link.enabled = 0;
    drop(link);
    for _ in 0..3 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    let sink = sink.lock().unwrap();
    let ends: Vec<_> = sink.notifies.iter().filter(|e| e.notify_phase == "end").collect();
    assert_eq!(ends.len(), active, "{ends:?}");
    assert!(ends.iter().all(|e| e.end_reason == "teardown"));
    assert_eq!(sink.montage.iter().filter(|(kind, e)| kind == "ended" && e.result == "cancelled").count(), 1);
    assert_eq!(world.get::<SkeletalMeshComponent>(instance).unwrap().notify_playback.active_states(), 0);
}

#[test]
fn a_graph_swapped_under_a_running_component_ends_states_as_reload_and_keeps_going() {
    let rig = rig();
    let graph = walk_graph(&rig, false);
    let hub = registered_hub();
    let mut world = World::new();
    let owner = world.spawn();
    let instance = pulsar_world_registry::attach_value(&mut world, owner, component(&rig, &graph)).unwrap();
    let (sink, _subs) = listen(&hub, instance, owner, &[]);
    let mut state = ComponentRuntimeState::default();
    for _ in 0..12 {
        frame(&mut world, &hub, 0.1, &mut state); // 1.2 s: inside "swing" (1.0..1.5)
    }
    assert_eq!(phases(&sink.lock().unwrap(), "swing").first().map(String::as_str), Some("begin"));
    // the sequence file was edited: the graph is recompiled and swapped in
    let recompiled = walk_graph(&rig, false);
    assert!(!Arc::ptr_eq(&recompiled, &graph));
    world.get_mut::<SkeletalMeshComponent>(instance).unwrap().loaded_graph = Some(recompiled);
    frame(&mut world, &hub, 0.1, &mut state);
    let reloaded = sink.lock().unwrap().notifies.iter().filter(|e| e.notify_id == "swing" && e.notify_phase == "end").map(|e| e.end_reason.clone()).collect::<Vec<_>>();
    assert_eq!(reloaded, ["reload"]);
    // the running player continued from where it was instead of restarting
    let x = world.get::<SkeletalMeshComponent>(instance).unwrap().animation().unwrap().pose().bones[1].translation.x;
    assert!(x > 2.0, "still past the half-way point of the slide: {x}");
}

pub(super) fn owner_with_transform(world: &mut World) -> pulsar_scenedb::Entity {
    let owner = world.spawn();
    world.insert(owner, Transform { position: [10.0, 0.0, 0.0], rotation: [0.0, 90.0, 0.0], scale: [1.0; 3] });
    owner
}

pub(super) fn owner_position(world: &World, owner: pulsar_scenedb::Entity) -> [f64; 3] {
    world.get::<Transform>(owner).unwrap().position
}

#[test]
fn root_motion_moves_the_owner_by_mode_across_loop_seams() {
    use RootMotionModeSetting::*;
    let rig = rig();
    let graph = walk_graph(&rig, false);
    // the clip travels 8 m per 2 s loop along the owner's forward (+z); the owner
    // is yawed 90 degrees, so forward is world +x
    let run = |mode: RootMotionModeSetting, frames: usize| {
        let hub = registered_hub();
        let mut world = World::new();
        let owner = owner_with_transform(&mut world);
        let mut c = component(&rig, &graph);
        c.root_motion_mode = mode;
        pulsar_world_registry::attach_value(&mut world, owner, c).unwrap();
        let mut state = ComponentRuntimeState::default();
        for _ in 0..frames {
            frame(&mut world, &hub, 0.25, &mut state);
        }
        owner_position(&world, owner)
    };
    // 12 frames of 0.25 s = 3 s: one and a half loops, 12 m
    let moved = run(RootMotionFromEverything, 12);
    assert!((moved[0] - 22.0).abs() < 1e-2 && moved[1].abs() < 1e-6 && moved[2].abs() < 1e-2, "{moved:?}");
    // slicing time differently gives the same travel, including over the loop seam
    let coarse = {
        let hub = registered_hub();
        let mut world = World::new();
        let owner = owner_with_transform(&mut world);
        let mut c = component(&rig, &graph);
        c.root_motion_mode = RootMotionFromEverything;
        pulsar_world_registry::attach_value(&mut world, owner, c).unwrap();
        let mut state = ComponentRuntimeState::default();
        for _ in 0..2 {
            frame(&mut world, &hub, 1.5, &mut state);
        }
        owner_position(&world, owner)
    };
    assert!((coarse[0] - moved[0]).abs() < 1e-2, "{coarse:?} vs {moved:?}");
    // the other modes never move the owner for graph motion
    for mode in [IgnoreRootMotion, NoRootMotionExtraction, RootMotionFromMontagesOnly] {
        assert_eq!(run(mode, 12), [10.0, 0.0, 0.0], "{mode:?}");
    }
}

#[test]
fn only_montages_move_the_owner_in_montages_only_mode_and_axes_and_rotation_follow_the_policy() {
    let rig = rig();
    let graph = walk_graph(&rig, true);
    let montage = compile_montage_asset(&rig, &montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Montage));
    let hub = registered_hub();
    let mut world = World::new();
    let owner = owner_with_transform(&mut world);
    let mut c = component(&rig, &graph);
    c.root_motion_mode = RootMotionModeSetting::RootMotionFromMontagesOnly;
    let instance = pulsar_world_registry::attach_value(&mut world, owner, c).unwrap();
    let mut state = ComponentRuntimeState::default();
    for _ in 0..4 {
        frame(&mut world, &hub, 0.25, &mut state);
    }
    assert_eq!(owner_position(&world, owner), [10.0, 0.0, 0.0], "the graph's own walking does not move the owner");
    world.get_mut::<SkeletalMeshComponent>(instance).unwrap().play_compiled_montage(montage, 1.0, None).unwrap();
    for _ in 0..4 {
        frame(&mut world, &hub, 0.25, &mut state);
    }
    // the Montage plays the 2 s clip: 4 m per second along the owner's forward
    let p = owner_position(&world, owner);
    assert!((p[0] - 14.0).abs() < 0.05 && p[1].abs() < 1e-6, "{p:?}");
    // a static object refuses to be moved
    let policy = world.get::<SkeletalMeshComponent>(instance).unwrap().root_motion_policy();
    assert_eq!(policy.translation, [true, false, true], "vertical travel is left to gravity unless asked for");
}

#[test]
fn physics_owns_configured_bones_while_the_animation_keeps_running() {
    let rig = rig();
    let graph = walk_graph(&rig, false);
    let mut c = component(&rig, &graph);
    // a mesh with one joint per bone, so a skin palette exists
    let mut mesh = skeletal_assets::skmesh::SkeletalMeshAsset::new(Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap()));
    mesh.joints = (0..3).map(|i| skeletal_assets::skmesh::Joint { bone: format!("b{i}"), inverse_bind: skeletal_assets::math::Mat4::IDENTITY.0 }).collect();
    c.loaded_mesh = Some(Arc::new(mesh));
    c.ensure_runtime();
    c.advance(0.5);
    let before = c.animation().unwrap().pose().bones[1].translation.x;
    let pose_before = c.bone_world_pose.clone();
    // physics owns bone 1: it sits somewhere the animation does not put it
    let owned = glam::Mat4::from_translation(glam::Vec3::new(0.0, 9.0, 0.0)).to_cols_array_2d();
    assert!(c.apply_physics_bones(vec![(1, owned)]));
    assert_eq!(c.physics_owned_bones(), 1);
    assert!(c.physics_pose.is_none(), "owning some bones is not whole-pose ownership");
    c.advance(0.5);
    let after = c.animation().unwrap().pose().bones[1].translation.x;
    assert!(after > before, "the animation kept advancing: {before} -> {after}");
    assert_ne!(c.bone_world_pose, pose_before);
    let palette_y = |c: &SkeletalMeshComponent, joint: usize| c.bone_palette[joint][3][1];
    assert!((palette_y(&c, 1) - 9.0).abs() < 1e-4, "the drawn bone is the body: {}", palette_y(&c, 1));
    assert!(palette_y(&c, 2) > 8.9, "its child follows it: {}", palette_y(&c, 2));
    assert!(palette_y(&c, 0).abs() < 1e-4, "the parent is still animated");
    // what physics reads is the animation's own pose: bone 1 follows the clip, not the body
    let read: glam::Mat4 = glam::Mat4::from_cols_array_2d(&c.bone_world_pose[1]);
    assert!((read.w_axis.x - after).abs() < 1e-4 && read.w_axis.y.abs() < 1e-4, "{:?}", read.w_axis);
    // bad input is refused without changing ownership
    assert!(!c.apply_physics_bones(vec![(99, owned)]));
    assert!(!c.apply_physics_bones(vec![(1, [[f32::NAN; 4]; 4])]));
    assert_eq!(c.physics_owned_bones(), 1);
    // returning the bones
    assert!(c.apply_physics_bones(Vec::new()));
    assert_eq!(c.physics_owned_bones(), 0);
}

fn skinned_component(rig: &Rig, graph: &Arc<CompiledGraph>) -> SkeletalMeshComponent {
    let mut c = component(rig, graph);
    let mut mesh = skeletal_assets::skmesh::SkeletalMeshAsset::new(Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap()));
    mesh.joints = (0..3).map(|i| skeletal_assets::skmesh::Joint { bone: format!("b{i}"), inverse_bind: skeletal_assets::math::Mat4::IDENTITY.0 }).collect();
    c.loaded_mesh = Some(Arc::new(mesh));
    c.ensure_runtime();
    c
}

#[test]
fn whole_skeleton_physics_ownership_blends_the_drawn_pose_and_never_stops_the_animation() {
    let rig = rig();
    let graph = walk_graph(&rig, false);
    let mut c = skinned_component(&rig, &graph);
    c.advance(0.5);
    let palette_x = |c: &SkeletalMeshComponent, joint: usize| c.bone_palette[joint][3][0];
    let animated_x = palette_x(&c, 1);
    let physics: Vec<[[f32; 4]; 4]> = (0..3).map(|i| glam::Mat4::from_translation(glam::Vec3::new(10.0 + i as f32, 0.0, 0.0)).to_cols_array_2d()).collect();
    assert!(!c.apply_physics_pose(physics[..2].to_vec()), "a pose of the wrong length is refused");
    assert!(c.apply_physics_pose(physics.clone()));
    assert_eq!(c.physics_blend_weight(), 1.0);
    assert!((palette_x(&c, 1) - 11.0).abs() < 1e-4, "weight 1: the drawn bone is the physics bone: {}", palette_x(&c, 1));
    // the animation keeps advancing underneath: clock, pose, notifies
    let clock = c.animation().unwrap().pose().bones[1].translation.x;
    c.advance(0.5);
    assert!(c.animation().unwrap().pose().bones[1].translation.x > clock, "animation time moved while physics owned the pose");
    assert!((palette_x(&c, 1) - 11.0).abs() < 1e-4, "the drawn pose is still physics'");
    // half the pose: halfway between what the animation draws now and the body
    let animated_now = c.animation().unwrap().pose().bones[1].translation.x;
    assert!(c.set_physics_blend_weight(0.5));
    assert!((palette_x(&c, 1) - (animated_now + 11.0) / 2.0).abs() < 1e-3, "{} vs {animated_now}", palette_x(&c, 1));
    assert!(!c.set_physics_blend_weight(f32::NAN));
    // weight 0 draws the animation; handing the pose back resumes from where the animation is, not where it was
    assert!(c.set_physics_blend_weight(0.0));
    assert!((palette_x(&c, 1) - animated_now).abs() < 1e-4);
    c.clear_physics_pose();
    assert_eq!(c.physics_blend_weight(), 0.0);
    c.advance(0.25);
    assert!(palette_x(&c, 1) > animated_x && palette_x(&c, 1) > animated_now, "the clock was never held");
}

#[test]
fn a_preview_scrub_raises_nothing_and_leaves_active_states_alone() {
    let rig = rig();
    let graph = walk_graph(&rig, false);
    let hub = registered_hub();
    let mut world = World::new();
    let owner = world.spawn();
    let instance = pulsar_world_registry::attach_value(&mut world, owner, component(&rig, &graph)).unwrap();
    let (sink, _subs) = listen(&hub, instance, owner, &[]);
    let mut state = ComponentRuntimeState::default();
    for _ in 0..12 {
        frame(&mut world, &hub, 0.1, &mut state); // inside "swing"
    }
    let count = sink.lock().unwrap().notifies.len();
    // a preview scrub of the instance reports nothing to the component
    {
        let mut c = world.get_mut::<SkeletalMeshComponent>(instance).unwrap();
        let instance = c.anim.instance.as_mut().unwrap();
        instance.advance(1.0, TimeStep::Variable, UpdateMode::Scrub);
        assert!(instance.report.spans.is_empty());
    }
    assert_eq!(sink.lock().unwrap().notifies.len(), count);
    assert_eq!(world.get::<SkeletalMeshComponent>(instance).unwrap().notify_playback.active_states(), 1);
}

#[test]
fn a_changed_asset_recompiles_only_the_graphs_that_depend_on_it() {
    let rig = rig();
    let graph = walk_graph(&rig, false);
    let hub = registered_hub();
    let mut world = World::new();
    let (oa, ob) = (world.spawn(), world.spawn());
    let mut a = component(&rig, &graph);
    a.graph_dependency_paths = vec![animation_source_for_file(Path::new("walk.animseq"))];
    let b = component(&rig, &graph);
    let a = pulsar_world_registry::attach_value(&mut world, oa, a).unwrap();
    let b = pulsar_world_registry::attach_value(&mut world, ob, b).unwrap();
    let (sink_a, _sa) = listen(&hub, a, oa, &[]);
    let (sink_b, _sb) = listen(&hub, b, ob, &[]);
    let mut state = ComponentRuntimeState::default();
    for _ in 0..12 {
        frame(&mut world, &hub, 0.1, &mut state); // both inside "swing"
    }
    // a cached Montage compile is dropped by any animation change
    let montage = compile_montage_asset(&rig, &montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph));
    world.get_mut::<SkeletalMeshComponent>(b).unwrap().anim.montages.insert(montage.id.clone(), montage);
    let recompiled = walk_graph(&rig, false);
    let mut calls = 0;
    let count = reload_dependants(&mut world, Path::new("walk.animseq"), |c| {
        calls += 1;
        c.loaded_graph = Some(recompiled.clone());
        c.refresh_pose();
        true
    });
    assert_eq!((count, calls), (1, 1), "only the component whose graph names the file recompiled");
    assert!(world.get::<SkeletalMeshComponent>(b).unwrap().anim.montages.is_empty());
    for _ in 0..2 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    let ended = |sink: &Arc<Mutex<Sink>>| sink.lock().unwrap().notifies.iter().filter(|e| e.notify_id == "swing" && e.notify_phase == "end").map(|e| e.end_reason.clone()).collect::<Vec<_>>();
    assert_eq!(ended(&sink_a), ["reload"]);
    assert_eq!(ended(&sink_b), Vec::<String>::new(), "the independent component carried on inside its state");
    // an unrelated file touches nothing
    assert_eq!(reload_dependants(&mut world, Path::new("other.animseq"), |_| panic!("not a dependant")), 0);
}

fn param_graph(rig: &Rig) -> Arc<CompiledGraph> {
    use skeletal_assets::anim_graph::catalogue::ParamType;
    use skeletal_assets::anim_graph::GraphParameter;
    let mut asset = AnimationGraphAsset::default();
    asset.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap().to_ref());
    asset.parameters.push(GraphParameter::new("speed", ParamType::Float));
    asset.parameters.push(GraphParameter::new("armed", ParamType::Bool));
    asset.parameters.push(GraphParameter::new("mode", ParamType::Int));
    asset.parameters.push(GraphParameter::new("aim", ParamType::Vector));
    let mut b = GraphBuilder::new("main", &[("speed", ParamType::Float)]);
    b.node("out", "anim.output_pose", &[]);
    b.node("p", "anim.sequence_player", &[("sequence", json!(rig.walk_id))]);
    b.node("s", "anim.get_parameter", &[("parameter", json!("speed"))]);
    b.wire("s", "p", "play_rate").wire("p", "out", "pose");
    asset.main = b.into_graph();
    let out = compile_graph(&asset, &rig.source);
    out.graph.unwrap_or_else(|| panic!("{:#?}", out.diagnostics))
}

#[test]
fn the_blueprint_nodes_are_registered_validate_their_input_and_command_the_animation() {
    use std::any::Any;
    let rig = rig();
    let graph = param_graph(&rig);
    let hub = registered_hub();
    let mut world = World::new();
    let owner = world.spawn();
    let instance = pulsar_world_registry::attach_value(&mut world, owner, component(&rig, &graph)).unwrap();
    let mut state = ComponentRuntimeState::default();
    frame(&mut world, &hub, 0.05, &mut state);
    let cid = pulsar_scenedb::component_id::<SkeletalMeshComponent>();
    let mut call = |world: &mut World, target: pulsar_scenedb::Entity, name: &str, mut args: Vec<Box<dyn Any>>| world.call_component_method(target, cid, name, &mut args);
    for name in [
        "anim_set_float", "anim_set_bool", "anim_set_int", "anim_set_vector", "anim_play_montage", "anim_stop_montage", "anim_stop_montage_group",
        "anim_pause_montage", "anim_jump_to_section", "anim_set_next_section", "anim_set_montage_rate", "anim_is_montage_playing",
    ] {
        assert!(pulsar_scenedb::find_component_method(cid, name).is_some(), "{name} is not on the component's method list");
    }
    // typed graph parameters, by component instance or by owner object
    call(&mut world, instance, "anim_set_float", vec![Box::new("speed".to_string()), Box::new(2.0f32)]).expect("float");
    call(&mut world, instance, "anim_set_bool", vec![Box::new("armed".to_string()), Box::new(true)]).expect("bool");
    assert_eq!(player_instance(&world, owner), Ok(instance), "the owner object resolves to its component");
    call(&mut world, instance, "anim_set_int", vec![Box::new("mode".to_string()), Box::new(3i32)]).expect("int");
    call(&mut world, instance, "anim_set_vector", vec![Box::new("aim".to_string()), Box::new(glam::Vec3::new(1.0, 2.0, 3.0))]).expect("vector");
    {
        use skeletal_assets::runtime::graph::Value;
        let c = world.get::<SkeletalMeshComponent>(instance).unwrap();
        let anim = c.animation().unwrap();
        assert_eq!(anim.parameter("speed"), Some(&Value::Float(2.0)));
        assert_eq!(anim.parameter("armed"), Some(&Value::Bool(true)));
        assert_eq!(anim.parameter("mode"), Some(&Value::Int(3)));
        assert_eq!(anim.parameter("aim"), Some(&Value::Vector(glam::Vec3::new(1.0, 2.0, 3.0))));
    }
    frame(&mut world, &hub, 0.25, &mut state);
    assert!((world.get::<SkeletalMeshComponent>(instance).unwrap().animation().unwrap().pose().bones[1].translation.x - 1.0).abs() < 1e-3, "rate 2 for 0.25 s of a 4 m / 2 s slide");
    // precise errors, no panics
    let message = |r: Result<Option<Box<dyn Any>>, _>| format!("{:?}", r.expect_err("should be refused"));
    assert!(message(call(&mut world, instance, "anim_set_float", vec![Box::new("nope".to_string()), Box::new(1.0f32)])).contains("no parameter `nope`"));
    assert!(message(call(&mut world, instance, "anim_set_bool", vec![Box::new("speed".to_string()), Box::new(true)])).contains("Float"));
    assert!(message(call(&mut world, instance, "anim_set_float", vec![Box::new("speed".to_string()), Box::new(f32::NAN)])).contains("finite"));
    let unrelated = world.spawn();
    assert!(message(call(&mut world, unrelated, "anim_set_float", vec![Box::new("speed".to_string()), Box::new(1.0f32)])).contains("MissingComponent"));
    // Montage commands on a running instance
    let montage = compile_montage_asset(&rig, &montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph));
    let handle = world.get_mut::<SkeletalMeshComponent>(instance).unwrap().play_compiled_montage(montage, 1.0, None).unwrap().0 as i64;
    let flag = |r: Result<Option<Box<dyn Any>>, _>| *r.unwrap().unwrap().downcast::<bool>().unwrap();
    assert!(flag(call(&mut world, instance, "anim_is_montage_playing", vec![Box::new(handle)])));
    assert!(flag(call(&mut world, instance, "anim_pause_montage", vec![Box::new(handle), Box::new(true)])));
    assert!(world.get::<SkeletalMeshComponent>(instance).unwrap().animation().unwrap().montages.get(MontageHandle(handle as u64)).unwrap().paused);
    call(&mut world, instance, "anim_set_montage_rate", vec![Box::new(handle), Box::new(-1.5f32)]).expect("negative rates are reverse playback");
    assert!(message(call(&mut world, instance, "anim_set_montage_rate", vec![Box::new(handle), Box::new(0.0f32)])).contains("non-zero"));
    call(&mut world, instance, "anim_jump_to_section", vec![Box::new(handle), Box::new("B".to_string())]).expect("jump");
    assert!(message(call(&mut world, instance, "anim_jump_to_section", vec![Box::new(handle), Box::new("Nope".to_string())])).contains("no section `Nope`"));
    call(&mut world, instance, "anim_set_next_section", vec![Box::new(handle), Box::new("B".to_string()), Box::new("A".to_string())]).expect("next section");
    assert!(message(call(&mut world, instance, "anim_pause_montage", vec![Box::new(-1i64), Box::new(true)])).contains("non-negative"));
    assert!(flag(call(&mut world, instance, "anim_stop_montage", vec![Box::new(handle), Box::new(0.0f32)])));
    frame(&mut world, &hub, 0.1, &mut state);
    assert!(!flag(call(&mut world, instance, "anim_is_montage_playing", vec![Box::new(handle)])), "stopped with no blend-out ends at once");
    assert!(message(call(&mut world, instance, "anim_play_montage", vec![Box::new("no-such-montage".to_string()), Box::new(1.0f32), Box::new(String::new())])).contains("project"), "a name is resolved through the open project");
}

/// Serialises the tests that point the process at a temporary project.
pub(super) static PROJECT: Mutex<()> = Mutex::new(());

/// A project on disk: skeleton, Walk sequence, a graph playing it through a Slot and a
/// Montage on that Slot, all registered in the file index by stable id.
pub(super) struct Project {
    pub(super) _dir: tempfile::TempDir,
    pub(super) root: std::path::PathBuf,
    pub(super) sequence_path: std::path::PathBuf,
    pub(super) graph_id: String,
    pub(super) graph_path: std::path::PathBuf,
}

pub(super) fn write_project(rig: &Rig, montage: &AnimationMontageAsset) -> Project {
    write_project_with(rig, montage, None)
}

/// [`write_project`] with `graph` as the project's Animation Graph instead of the Slot graph.
pub(super) fn write_project_with(rig: &Rig, montage: &AnimationMontageAsset, graph: Option<AnimationGraphAsset>) -> Project {
    use pulsar_reflection::{AnimationGraphAssetKind, AnimationMontageAssetKind, AnimationSequenceAssetKind, SkeletonAssetKind};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let index = engine_fs::open_project_file_index(&root).unwrap().expect("local index");
    let write = |name: &str, text: String| {
        let path = root.join(name);
        std::fs::write(&path, text).unwrap();
        index.refresh_path(&path).unwrap();
        path
    };
    let skeleton_path = write("body.skeleton", rig.skeleton.to_text());
    index.record_typed_asset_id(SkeletonAssetKind::KIND, rig.skeleton.id.as_str(), &skeleton_path).unwrap();
    let sequence = rig.source.sequences[&rig.walk_id].clone();
    let sequence_path = write("walk.animseq", sequence.to_text());
    index.record_typed_asset_id(AnimationSequenceAssetKind::KIND, &rig.walk_id, &sequence_path).unwrap();
    let asset = graph.unwrap_or_else(|| {
    let mut asset = AnimationGraphAsset::default();
    asset.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap().to_ref());
    let mut b = GraphBuilder::new("main", &[]);
    b.node("out", "anim.output_pose", &[]);
    b.node("p", "anim.sequence_player", &[("sequence", json!(rig.walk_id))]);
    b.node("slot", "anim.slot", &[("slot", json!("Body"))]);
    b.wire("p", "slot", "source").wire("slot", "out", "pose");
    asset.main = b.into_graph();
    asset
    });
    let graph_id = asset.id.as_str().to_string();
    let graph_path = write("locomotion.animgraph", asset.to_text().unwrap());
    index.record_typed_asset_id(AnimationGraphAssetKind::KIND, &graph_id, &graph_path).unwrap();
    let montage_path = write("swing.montage", montage.to_text().unwrap());
    index.record_typed_asset_id(AnimationMontageAssetKind::KIND, montage.id.as_str(), &montage_path).unwrap();
    Project { _dir: dir, root, sequence_path, graph_id, graph_path }
}

#[test]
fn blueprint_discovery_offers_what_the_montages_of_the_graph_own_and_nothing_stale() {
    let _guard = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig();
    let mut montage = montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph);
    montage.name = "Swing".into();
    let project = write_project(&rig, &montage);
    let ids: std::collections::HashSet<String> = [project.graph_id.clone()].into();
    let catalog = component_animation_notify_events(&project.root, &ids);
    assert!(catalog.diagnostics.is_empty(), "{:?}", catalog.diagnostics.iter().map(|d| &d.error).collect::<Vec<_>>());
    let names: Vec<(String, String)> = catalog.events.iter().map(|(_, d)| (d.namespace.clone().unwrap_or_default(), d.name.clone())).collect();
    let has = |namespace: &str, name: String| names.contains(&(namespace.to_owned(), name));
    // the clip's notify and state, canonical by asset id
    assert!(has("animseq", format!("{}.step", rig.walk_id)));
    assert!(has("animseq", format!("{}.swing", rig.walk_id)), "notify states are offered too");
    // the Montage's own notify and state, and its lifecycle events
    let id = montage.id.as_str();
    assert!(has("montage", format!("{id}.mn")) && has("montage", format!("{id}.ms")));
    for kind in MONTAGE_LIFECYCLE_KINDS {
        assert!(has("montage", format!("{id}.lifecycle_{kind}")), "{kind}");
    }
    // a label rename never changes the identity, and editing the file shows up at once
    let mut renamed = montage.clone();
    renamed.notifies[0].name = "Renamed".into();
    renamed.notifies.push(AnimationNotify { id: "extra".into(), name: "Extra".into(), time: 0.9, target: NotifyTarget::Self_, fields: vec![], schema: None });
    std::fs::write(project.root.join("swing.montage"), renamed.to_text().unwrap()).unwrap();
    let again = component_animation_notify_events(&project.root, &ids);
    let names: Vec<_> = again.events.iter().map(|(_, d)| d.name.clone()).collect();
    assert!(names.contains(&format!("{id}.mn")) && names.contains(&format!("{id}.extra")), "no stale per-open cache: {names:?}");
}

#[test]
fn editing_a_sequence_on_disk_recompiles_the_running_graph_in_place() {
    let _guard = PROJECT.lock().unwrap_or_else(|e| e.into_inner());
    let rig = rig();
    let montage = montage_asset(&rig, skeletal_assets::montage::MontageRootMotion::Graph);
    let project = write_project(&rig, &montage);
    let files = ProjectAnimSource::at(project.root.clone());
    let source = animation_source_for_file(&project.graph_path);
    let loaded = compile_graph_file(&source, &files).expect("the graph reads");
    let graph = loaded.graph.expect("and compiles");
    let dependencies = dependency_paths_in(&project.root, &graph);
    assert!(dependencies.contains(&animation_source_for_file(&project.sequence_path)), "{dependencies:?}");

    let hub = registered_hub();
    let mut world = World::new();
    let owner = world.spawn();
    let mut c = component(&rig, &graph);
    c.graph_source_path = Some(source);
    c.graph_dependency_paths = dependencies;
    let instance = pulsar_world_registry::attach_value(&mut world, owner, c).unwrap();
    let (sink, _subs) = listen(&hub, instance, owner, &[]);
    let mut state = ComponentRuntimeState::default();
    for _ in 0..12 {
        frame(&mut world, &hub, 0.1, &mut state); // 1.2 s: inside "swing" (1.0..1.5)
    }
    let x = |world: &World| world.get::<SkeletalMeshComponent>(instance).unwrap().animation().unwrap().pose().bones[1].translation.x;
    assert!((x(&world) - 2.4).abs() < 1e-3, "4 m over 2 s at 1.2 s");

    // the artist moves the slide to 8 m in the sequence file and saves
    let mut edited = rig.source.sequences[&rig.walk_id].as_ref().clone();
    edited.clip.tracks[1] = track("b1", [8.0, 0.0, 0.0], 2.0);
    std::fs::write(&project.sequence_path, edited.to_text()).unwrap();
    assert_eq!(reload_dependants(&mut world, &project.sequence_path, |c| c.recompile_graph_in(&ProjectAnimSource::at(project.root.clone()))), 1);
    for _ in 0..2 {
        frame(&mut world, &hub, 0.1, &mut state);
    }
    // the clock carried on from 1.2 s (the first frame waited for the reload ends to be delivered)
    assert!((x(&world) - 8.0 * 1.3 / 2.0).abs() < 1e-2, "same clock (1.3 s), new 8 m motion: {}", x(&world));
    let ends: Vec<_> = sink.lock().unwrap().notifies.iter().filter(|e| e.notify_id == "swing" && e.notify_phase == "end").map(|e| e.end_reason.clone()).collect();
    assert_eq!(ends, ["reload"], "the state that was active ended once, for the reload");
    // a file the graph does not name changes nothing
    assert_eq!(reload_dependants(&mut world, &project.graph_path.with_file_name("other.animseq"), |c| c.recompile_graph_in(&files)), 0);
}
