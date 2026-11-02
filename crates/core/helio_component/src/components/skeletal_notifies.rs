//! runtime crossings belong to the live player, not its sampled pose.

use std::{collections::VecDeque, sync::Arc};

use engine_class_derive::{component_events, engine_class};
use skeletal_assets::{
    assets::AnimSeqAsset,
    notify_schema::AnimationNotifyState,
    runtime::{
        anim_instance::{TimeStep, UpdateMode},
        graph::{StateEvent, StateEventKind},
        montage::{CompiledMontage, MontageEvent, MontageEventKind, MontageResult},
        playback::{EndReason, NotifyItem, NotifyOccurrence, NotifyOwner, NotifyTracker},
    },
};

use super::{RootMotionMove, SkeletalMeshComponent};
use pulsar_events::{
    EventCategory, EventHub, entity_channel,
    gamma::{DynEvent, DynValue, EventDescriptor, FieldType},
};
use pulsar_scenedb::{Entity, World};
use skeletal_assets::notifies::{AnimationNotify, NotifyField, NotifyTarget, NotifyValue};

pub(super) fn load_sequence_for_events(path: &std::path::Path) -> Result<AnimSeqAsset, String> {
    let text = skeletal_assets::assets::read_text(path).map_err(|e| e.to_string())?;
    let sequence = AnimSeqAsset::parse(&text)?;
    if !sequence.features.notifies.is_empty() && AnimSeqAsset::peek_id(&text).is_none() {
        return Err(format!(
            "Save {} once to give its notifies a stable sequence identity",
            path.display()
        ));
    }
    Ok(sequence)
}

/// asset sources use the same canonical path as the skeletal asset ledger.
pub fn animation_source_for_file(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// register newly bound assets before returning from the session's asset pump.
pub fn register_loaded_animation_notifies(world: &mut World, hub: &EventHub) {
    for (_, player) in world.query::<&mut SkeletalMeshComponent>() {
        if let Err(error) = player.register_notify_events(hub) {
            tracing::warn!(%error, "animation notify schema conflicts with the running session");
        }
    }
}

/// Fields every notify/state/Montage-notify event carries before its authored
/// payload. `sequence_id` names the source asset whatever its kind (kept under
/// that name for compatibility); `source_kind` says which kind it is.
fn item_descriptor(namespace: &str, source_id: &str, item_id: &str, payload: &[NotifyField]) -> EventDescriptor {
    let mut fields = vec![
        ("entity".to_owned(), FieldType::U64),
        ("source".into(), FieldType::U64),
        ("sequence_id".into(), FieldType::Str),
        ("notify_id".into(), FieldType::Str),
        ("notify_name".into(), FieldType::Str),
        ("clip_time".into(), FieldType::F64),
        ("playback_time".into(), FieldType::F64),
        ("cycle".into(), FieldType::I64),
        ("generation".into(), FieldType::I64),
        ("playback_instance".into(), FieldType::I64),
        ("notify_phase".into(), FieldType::Str),
        ("tick_delta".into(), FieldType::F64),
        ("end_reason".into(), FieldType::Str),
        ("source_kind".into(), FieldType::Str),
    ];
    fields.extend(payload.iter().map(|f| {
        (
            f.name.clone(),
            match f.value {
                NotifyValue::Bool(_) => FieldType::Bool,
                NotifyValue::Int(_) => FieldType::I64,
                NotifyValue::Float(_) => FieldType::F64,
                NotifyValue::String(_) => FieldType::Str,
            },
        )
    }));
    EventDescriptor::dynamic_in(namespace, format!("{source_id}.{item_id}"), fields)
        .expect("animation namespaces are valid")
}

/// canonical binding keys use asset identities; labels are free to change.
pub fn animation_notify_descriptor(
    sequence: &AnimSeqAsset,
    notify: &AnimationNotify,
) -> EventDescriptor {
    item_descriptor("animseq", &sequence.id.to_string(), &notify.id, &notify.fields)
}

/// A Sequence-owned notify state raises Begin, Tick and End under one canonical
/// descriptor, told apart by its `notify_phase` field.
pub fn animation_notify_state_descriptor(
    sequence: &AnimSeqAsset,
    state: &AnimationNotifyState,
) -> EventDescriptor {
    item_descriptor("animseq", &sequence.id.to_string(), &state.id, &state.fields)
}

/// Notifies a Montage itself owns live in the `montage` namespace.
pub fn montage_notify_descriptor(montage_id: &str, notify: &AnimationNotify) -> EventDescriptor {
    item_descriptor("montage", montage_id, &notify.id, &notify.fields)
}

pub fn montage_notify_state_descriptor(montage_id: &str, state: &AnimationNotifyState) -> EventDescriptor {
    item_descriptor("montage", montage_id, &state.id, &state.fields)
}

/// The Montage lifecycle kinds, in the order they can happen to one instance.
pub const MONTAGE_LIFECYCLE_KINDS: [&str; 6] =
    ["started", "blended_in", "section_changed", "blending_out", "ended", "interrupted"];

/// Canonical descriptor of one lifecycle event of one Montage asset
/// (`montage::<montage id>.lifecycle_<kind>`). Blueprint nodes filter by the
/// Montage by naming it here; the same occurrence is also raised as the
/// generic component event `SkeletalMeshComponent.montage_<kind>`.
pub fn montage_lifecycle_descriptor(montage_id: &str, kind: &str) -> EventDescriptor {
    let fields = vec![
        ("entity".to_owned(), FieldType::U64),
        ("source".into(), FieldType::U64),
        ("montage_id".into(), FieldType::Str),
        ("instance".into(), FieldType::I64),
        ("section_from".into(), FieldType::Str),
        ("section_to".into(), FieldType::Str),
        ("result".into(), FieldType::Str),
        ("interrupted_by".into(), FieldType::I64),
        ("generation".into(), FieldType::I64),
    ];
    EventDescriptor::dynamic_in("montage", format!("{montage_id}.lifecycle_{kind}"), fields)
        .expect("montage is a valid namespace")
}

/// Canonical descriptor of one state machine event of one Animation Graph asset
/// (`animgraph::<graph id>.state_<kind>`). Events of machines in linked graphs carry the id of
/// the graph the component plays; `machine` names the machine. Also raised as the generic
/// component events `SkeletalMeshComponent.anim_state_entered` and friends. Never global.
pub fn anim_state_descriptor(graph_id: &str, kind: StateEventKind) -> EventDescriptor {
    let fields = vec![
        ("entity".to_owned(), FieldType::U64),
        ("source".into(), FieldType::U64),
        ("graph_id".into(), FieldType::Str),
        ("machine".into(), FieldType::Str),
        ("state".into(), FieldType::Str),
        ("from_state".into(), FieldType::Str),
        ("to_state".into(), FieldType::Str),
        ("transition".into(), FieldType::Str),
        ("generation".into(), FieldType::I64),
    ];
    EventDescriptor::dynamic_in("animgraph", format!("{graph_id}.state_{}", kind.name()), fields).expect("animgraph is a valid namespace")
}

pub fn register_animation_notifies(sequence: &AnimSeqAsset, hub: &EventHub) -> Result<(), String> {
    sequence.features.validate(sequence.clip.duration)?;
    let category = || EventCategory::Custom("Animation".into());
    for notify in &sequence.features.notifies {
        hub.register(animation_notify_descriptor(sequence, notify), category())
            .map_err(|e| e.to_string())?;
    }
    for state in &sequence.features.notify_states {
        hub.register(animation_notify_state_descriptor(sequence, state), category())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Register everything a Montage can raise: its own notifies and states and its
/// lifecycle events.
pub fn register_montage_events(montage: &CompiledMontage, hub: &EventHub) -> Result<(), String> {
    let category = || EventCategory::Custom("Animation".into());
    for notify in &montage.notifies {
        hub.register(montage_notify_descriptor(&montage.id, notify), category())
            .map_err(|e| e.to_string())?;
    }
    for state in &montage.notify_states {
        hub.register(montage_notify_state_descriptor(&montage.id, state), category())
            .map_err(|e| e.to_string())?;
    }
    for kind in MONTAGE_LIFECYCLE_KINDS {
        hub.register(montage_lifecycle_descriptor(&montage.id, kind), category())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}


/// Saved events that can be offered independently of broken project assets.
#[derive(Debug, Default)]
pub struct AnimationNotifyCatalog {
    pub events: Vec<(String, EventDescriptor)>,
    pub diagnostics: Vec<AnimationNotifyDiagnostic>,
}

#[derive(Debug)]
pub struct AnimationNotifyDiagnostic {
    pub path: std::path::PathBuf,
    pub error: String,
}

/// Refresh from saved assets when the palette or compiler requests its catalog.
/// Failures are local to a file/directory or a conflicting canonical identity.
/// No session handle or gameplay publication is kept on the process host bus.
pub fn project_animation_notify_events(root: &std::path::Path) -> AnimationNotifyCatalog {
    use std::collections::{BTreeMap, BTreeSet};
    let mut catalog = AnimationNotifyCatalog::default();
    let mut paths = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = match engine_fs::virtual_fs::list_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                catalog.diagnostics.push(AnimationNotifyDiagnostic {
                    path: dir,
                    error: error.to_string(),
                });
                continue;
            }
        };
        for entry in entries {
            let path = dir.join(&entry.name);
            if entry.is_dir && depth < 16 {
                if !entry.name.starts_with('.')
                    && !matches!(
                        entry.name.as_str(),
                        "target" | "node_modules" | "build" | "dist"
                    )
                {
                    stack.push((path, depth + 1));
                }
            } else if !entry.is_dir
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("animseq"))
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let items = paths
        .into_iter()
        .map(|path| {
            let loaded = load_sequence_for_events(&path);
            (path, loaded)
        })
        .collect();
    catalog_from(items, catalog)
}

/// Canonical event descriptors of the given parsed sequences. Conflicting
/// identities are quarantined as a set, independent of input order.
fn catalog_from(
    items: Vec<(std::path::PathBuf, Result<AnimSeqAsset, String>)>,
    mut catalog: AnimationNotifyCatalog,
) -> AnimationNotifyCatalog {
    use std::collections::{BTreeMap, BTreeSet};
    let hub = EventHub::new();
    let mut events = BTreeMap::new();
    let mut origins: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut conflicts: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    let key = |d: &EventDescriptor| (d.namespace.clone(), d.name.clone());
    for (path, loaded) in items {
        let sequence = match loaded {
            Ok(sequence) => sequence,
            Err(error) => {
                catalog
                    .diagnostics
                    .push(AnimationNotifyDiagnostic { path, error });
                continue;
            }
        };
        let items = sequence
            .features
            .notifies
            .iter()
            .map(|n| (n.name.as_str(), n.id.as_str(), animation_notify_descriptor(&sequence, n)))
            .chain(sequence.features.notify_states.iter().map(|s| (s.name.as_str(), s.id.as_str(), animation_notify_state_descriptor(&sequence, s))));
        for (item_name, item_id, descriptor) in items {
            let identity = key(&descriptor);
            origins
                .entry(identity.clone())
                .or_default()
                .push(path.clone());
            // Gamma detects canonical-name/schema and numeric-ID conflicts.
            // Record both claimants before filtering: a later matching copy
            // must never restore an identity already found to be ambiguous.
            if let Err(error) = hub.register(
                descriptor.clone(),
                EventCategory::Custom("Animation".into()),
            ) {
                let error = error.to_string();
                conflicts
                    .entry(identity.clone())
                    .or_default()
                    .insert(error.clone());
                if let Some(existing) = hub.descriptor(descriptor.id) {
                    conflicts.entry(key(&existing)).or_default().insert(error);
                }
            }
            events.entry(identity).or_insert((
                format!(
                    "{}.{} [{}]",
                    sequence.clip.name,
                    item_name,
                    item_id.chars().take(8).collect::<String>()
                ),
                descriptor,
            ));
        }
    }
    // Filter only after the complete scan, so filesystem ordering cannot pick
    // a winning schema. Keep other notifies even in the same sequence.
    for (identity, errors) in conflicts {
        events.remove(&identity);
        let files = &origins[&identity];
        let error = format!(
            "Quarantined {}::{}: {}; claimed by {}",
            identity.0.as_deref().unwrap_or("main"),
            identity.1,
            errors.into_iter().collect::<Vec<_>>().join("; "),
            files
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        );
        for path in files {
            catalog.diagnostics.push(AnimationNotifyDiagnostic {
                path: path.clone(),
                error: error.clone(),
            });
        }
    }
    catalog.events = events.into_values().collect();
    catalog.events.sort_by_key(|(_, descriptor)| descriptor.id);
    catalog
        .diagnostics
        .sort_by(|a, b| a.path.cmp(&b.path).then(a.error.cmp(&b.error)));
    catalog
}


/// Notify descriptors for the animation assets a skeletal mesh component names,
/// found through the project's asset index and the assets' own references: no
/// directory walk and nothing published on a bus. `assets` holds the ids of the
/// component's Animation Graph and/or its single sequence. For a graph the
/// closure is every sequence the graph can play: its Sequence Players, the samples
/// of its Blend Spaces, the sources of its Pose Databases, linked graphs, and the
/// sequences of Montages that have a track on one of the graph's Slots (their
/// sequence notifies fire while the Montage plays).
pub fn component_animation_notify_events(
    root: &std::path::Path,
    assets: &std::collections::HashSet<String>,
) -> AnimationNotifyCatalog {
    use crate::anim_source::ProjectAnimSource;
    use pulsar_reflection::{AnimationGraphAssetKind, AnimationMontageAssetKind, AssetKind};
    use skeletal_assets::runtime::graph::compile::AssetSource;
    use std::collections::BTreeSet;

    let source = ProjectAnimSource::at(root.to_path_buf());
    let mut catalog = AnimationNotifyCatalog::default();
    let mut sequences: BTreeSet<String> = BTreeSet::new();
    let mut slots: BTreeSet<(String, String)> = BTreeSet::new();
    let mut montages: Vec<Arc<skeletal_assets::montage::AnimationMontageAsset>> = Vec::new();
    let mut graphs_seen: BTreeSet<String> = BTreeSet::new();
    let mut graph_stack: Vec<String> = Vec::new();
    let index = engine_fs::open_project_file_index(root).ok().flatten();
    let kind_of = |id: &str| -> Option<&'static str> {
        let index = index.as_ref()?;
        [AnimationGraphAssetKind::KIND, pulsar_reflection::AnimationSequenceAssetKind::KIND]
            .into_iter()
            .find(|kind| index.path_for_asset_id_kind(kind, id).ok().flatten().is_some())
    };
    for id in assets {
        match kind_of(id) {
            Some(k) if k == AnimationGraphAssetKind::KIND => graph_stack.push(id.clone()),
            Some(_) => {
                sequences.insert(id.clone());
            }
            None => catalog.diagnostics.push(AnimationNotifyDiagnostic { path: std::path::PathBuf::from(id), error: "the animation asset is not in this project's asset index".into() }),
        }
    }
    while let Some(id) = graph_stack.pop() {
        if !graphs_seen.insert(id.clone()) {
            continue;
        }
        let Some(graph) = source.graph(&id) else {
            catalog.diagnostics.push(AnimationNotifyDiagnostic { path: std::path::PathBuf::from(&id), error: "the animation graph could not be loaded".into() });
            continue;
        };
        for (_, _, g) in graph.graphs() {
            for node in g.nodes.values().filter(|n| n.node_type == "anim.slot") {
                let text = |k: &str| node.properties.get(k).and_then(|v| v.as_str()).map(str::to_string);
                if let Some(slot) = text("slot") {
                    slots.insert((slot, text("group").unwrap_or_else(|| "DefaultGroup".into())));
                }
            }
        }
        for r in skeletal_assets::anim_graph::validate::asset_references(&graph) {
            match r.kind {
                k if k == pulsar_reflection::AnimationSequenceAssetKind::KIND => {
                    sequences.insert(r.id);
                }
                k if k == pulsar_reflection::BlendSpaceAssetKind::KIND => {
                    if let Some(space) = source.blend_space(&r.id) {
                        sequences.extend(space.samples.iter().filter_map(|s| s.sequence.as_ref().map(|s| s.as_str().to_string())));
                    }
                }
                k if k == pulsar_reflection::PoseDatabaseAssetKind::KIND => {
                    if let Some(db) = source.pose_db(&r.id) {
                        for s in db.sources.iter().filter(|s| s.enabled) {
                            match &s.source {
                                skeletal_assets::pose_db::PoseSource::Sequence { sequence } => {
                                    sequences.insert(sequence.as_str().to_string());
                                }
                                skeletal_assets::pose_db::PoseSource::BlendSpace { blend_space } => {
                                    if let Some(space) = source.blend_space(blend_space.as_str()) {
                                        sequences.extend(space.samples.iter().filter_map(|s| s.sequence.as_ref().map(|s| s.as_str().to_string())));
                                    }
                                }
                            }
                        }
                    }
                }
                k if k == AnimationGraphAssetKind::KIND => graph_stack.push(r.id),
                _ => {}
            }
        }
    }
    if !slots.is_empty() {
        if let Some(index) = &index {
            for (id, _) in index.assets_of_kind(AnimationMontageAssetKind::KIND).unwrap_or_default() {
                let Some(montage) = source.montage(&id) else { continue };
                if montage.tracks.iter().any(|t| slots.contains(&(t.slot.clone(), t.group.clone()))) {
                    sequences.extend(montage.segments().filter_map(|s| s.sequence.as_ref().map(|s| s.as_str().to_string())));
                    montages.push(montage);
                }
            }
        }
    }
    let items = sequences
        .into_iter()
        .map(|id| {
            let origin = index
                .as_ref()
                .and_then(|i| i.path_for_asset_id_kind(pulsar_reflection::AnimationSequenceAssetKind::KIND, &id).ok().flatten())
                .map(|p| root.join(p))
                .unwrap_or_else(|| std::path::PathBuf::from(&id));
            let loaded = match source.sequence(&id) {
                Some(seq) => Ok((*seq).clone()),
                None => Err("the sequence could not be loaded".to_string()),
            };
            (origin, loaded)
        })
        .collect();
    let mut catalog = catalog_from(items, catalog);
    // what the Montages own: their notifies and states, and their lifecycle events
    for montage in &montages {
        let id = montage.id.as_str();
        let name = if montage.name.is_empty() { id.chars().take(8).collect::<String>() } else { montage.name.clone() };
        let short = |item: &str| item.chars().take(8).collect::<String>();
        for notify in &montage.notifies {
            catalog.events.push((format!("{name}.{} [{}]", notify.name, short(&notify.id)), montage_notify_descriptor(id, notify)));
        }
        for state in &montage.notify_states {
            catalog.events.push((format!("{name}.{} [{}]", state.name, short(&state.id)), montage_notify_state_descriptor(id, state)));
        }
        for kind in MONTAGE_LIFECYCLE_KINDS {
            catalog.events.push((format!("{name} {}", kind.replace('_', " ")), montage_lifecycle_descriptor(id, kind)));
        }
    }
    // the state machine events of every graph the component plays
    for id in &graphs_seen {
        if !assets.contains(id) {
            continue;
        }
        let Some(graph) = source.graph(id) else { continue };
        if !graph.graphs().any(|(_, kind, _)| kind == skeletal_assets::anim_graph::catalogue::GraphKind::StateMachine) {
            continue;
        }
        let name = if graph.name.is_empty() { id.chars().take(8).collect::<String>() } else { graph.name.clone() };
        for kind in StateEventKind::ALL {
            catalog.events.push((format!("{name} state {}", kind.name().replace('_', " ")), anim_state_descriptor(id, kind)));
        }
    }
    catalog.events.sort_by_key(|(_, descriptor)| descriptor.id);
    catalog
}

/// How a pending occurrence is also shown to the generic component event bridge.
#[derive(Debug)]
enum Bridge {
    /// `SkeletalMeshComponent.animation_notify`: notifies and notify-state phases.
    Notify,
    /// `SkeletalMeshComponent.montage_<kind>`.
    Montage { kind: &'static str, event: MontageLifecycleEvent },
    /// `SkeletalMeshComponent.anim_state_entered` and the other three state machine events.
    State { kind: StateEventKind, event: AnimStateEvent },
}

/// One occurrence waiting for delivery through the component's event channel.
#[derive(Debug)]
struct PendingNotify {
    event: AnimationNotifyEvent,
    bridge: Bridge,
    /// The canonical per-asset event, published on the target entity's channel.
    authored: DynEvent,
    /// Registered on whichever bus finally delivers the occurrence.
    descriptor: EventDescriptor,
    target: NotifyTarget,
    generic_accepted: bool,
}

fn payload_values(fields: &[NotifyField]) -> impl Iterator<Item = DynValue> + '_ {
    fields.iter().map(|field| match &field.value {
        NotifyValue::Bool(v) => DynValue::Bool(*v),
        NotifyValue::Int(v) => DynValue::I64(*v),
        NotifyValue::Float(v) => DynValue::F64(*v),
        NotifyValue::String(v) => DynValue::Str(v.clone()),
    })
}

impl PendingNotify {
    /// A notify, or a phase of a notify state, from a playback instance.
    fn from_occurrence(occurrence: &NotifyOccurrence, generation: i64) -> Self {
        let descriptor = match (&occurrence.owner, occurrence.item) {
            (NotifyOwner::Sequence(s), NotifyItem::Notify(i)) => animation_notify_descriptor(s, &s.features.notifies[i]),
            (NotifyOwner::Sequence(s), NotifyItem::State(i)) => animation_notify_state_descriptor(s, &s.features.notify_states[i]),
            (NotifyOwner::Montage(m), NotifyItem::Notify(i)) => montage_notify_descriptor(&m.id, &m.notifies[i]),
            (NotifyOwner::Montage(m), NotifyItem::State(i)) => montage_notify_state_descriptor(&m.id, &m.notify_states[i]),
        };
        let event = AnimationNotifyEvent {
            sequence_id: occurrence.owner.id(),
            notify_id: occurrence.id().to_owned(),
            notify_name: occurrence.name().to_owned(),
            clip_time: occurrence.asset_time() as f64,
            playback_time: occurrence.position,
            cycle: occurrence.cycle,
            generation,
            playback_instance: occurrence.instance as i64,
            notify_phase: occurrence.phase.name().to_owned(),
            tick_delta: occurrence.delta as f64,
            end_reason: occurrence.end_reason.map_or("", EndReason::name).to_owned(),
            source_kind: occurrence.owner.kind().to_owned(),
        };
        let mut fields = vec![
            DynValue::U64(0),
            DynValue::U64(0),
            DynValue::Str(event.sequence_id.clone()),
            DynValue::Str(event.notify_id.clone()),
            DynValue::Str(event.notify_name.clone()),
            DynValue::F64(event.clip_time),
            DynValue::F64(event.playback_time),
            DynValue::I64(event.cycle),
            DynValue::I64(generation),
            DynValue::I64(event.playback_instance),
            DynValue::Str(event.notify_phase.clone()),
            DynValue::F64(event.tick_delta),
            DynValue::Str(event.end_reason.clone()),
            DynValue::Str(event.source_kind.clone()),
        ];
        fields.extend(payload_values(occurrence.fields()));
        Self {
            authored: DynEvent::new(descriptor.id, fields),
            descriptor,
            target: occurrence.target().clone(),
            generic_accepted: false,
            bridge: Bridge::Notify,
            event,
        }
    }

    /// A Montage lifecycle event of one instance. `interrupted` is the extra
    /// event raised after `ended` when the result is an interruption.
    fn lifecycle(event: &MontageEvent, kind: &'static str, generation: i64) -> Self {
        let mut data = MontageLifecycleEvent {
            montage_id: event.montage.clone(),
            instance: event.handle.0 as i64,
            generation,
            ..Default::default()
        };
        match &event.kind {
            MontageEventKind::SectionChanged { from, to } => {
                data.section_from = from.clone().unwrap_or_default();
                data.section_to = to.clone();
            }
            MontageEventKind::Ended(result) => {
                data.result = montage_result_name(*result).to_owned();
                if let MontageResult::Interrupted { by } = result {
                    data.interrupted_by = by.0 as i64;
                }
            }
            _ => {}
        }
        if kind == "interrupted" {
            data.result = "interrupted".to_owned();
        }
        let descriptor = montage_lifecycle_descriptor(&event.montage, kind);
        let authored = DynEvent::new(
            descriptor.id,
            vec![
                DynValue::U64(0),
                DynValue::U64(0),
                DynValue::Str(data.montage_id.clone()),
                DynValue::I64(data.instance),
                DynValue::Str(data.section_from.clone()),
                DynValue::Str(data.section_to.clone()),
                DynValue::Str(data.result.clone()),
                DynValue::I64(data.interrupted_by),
                DynValue::I64(generation),
            ],
        );
        Self {
            event: AnimationNotifyEvent::default(),
            bridge: Bridge::Montage { kind, event: data },
            authored,
            descriptor,
            target: NotifyTarget::Self_,
            generic_accepted: false,
        }
    }
}

impl PendingNotify {
    /// A state machine event of the component's graph.
    fn state(event: &StateEvent, graph_id: &str, generation: i64) -> Self {
        let data = AnimStateEvent {
            graph_id: graph_id.to_owned(),
            machine: event.machine.clone(),
            state: event.state.clone(),
            from_state: event.from_state.clone(),
            to_state: event.to_state.clone(),
            transition: event.transition.clone(),
            generation,
        };
        let descriptor = anim_state_descriptor(graph_id, event.kind);
        let authored = DynEvent::new(
            descriptor.id,
            vec![
                DynValue::U64(0),
                DynValue::U64(0),
                DynValue::Str(data.graph_id.clone()),
                DynValue::Str(data.machine.clone()),
                DynValue::Str(data.state.clone()),
                DynValue::Str(data.from_state.clone()),
                DynValue::Str(data.to_state.clone()),
                DynValue::Str(data.transition.clone()),
                DynValue::I64(generation),
            ],
        );
        Self { event: AnimationNotifyEvent::default(), bridge: Bridge::State { kind: event.kind, event: data }, authored, descriptor, target: NotifyTarget::Self_, generic_accepted: false }
    }
}

fn montage_result_name(result: MontageResult) -> &'static str {
    match result {
        MontageResult::Completed => "completed",
        MontageResult::Interrupted { .. } => "interrupted",
        MontageResult::Stopped => "stopped",
        MontageResult::Cancelled => "cancelled",
    }
}

/// Animation phase: advance every live skeletal mesh component's animation by
/// `delta_seconds`, publish its pose and move its owner by the root motion the
/// component's mode selects. The engine tick runs this **before** the ECS
/// schedule, so physics reads this frame's evaluated pose and owner transform;
/// gameplay and script writes to graph parameters made later in the frame apply
/// next frame.
pub fn run_animation_phase(world: &mut World, delta_seconds: f32) {
    let players: Vec<Entity> = world.query::<&SkeletalMeshComponent>().map(|(entity, _)| entity).collect();
    for instance in players {
        let link = world
            .get::<pulsar_world_registry::pulsar_scene_model::ComponentOwner>(instance)
            .map(|link| (link.is_enabled(), link.entity()));
        let live = link.is_some_and(|(enabled, owner)| enabled && world.is_alive(owner));
        let Some(mut player) = world.get_mut::<SkeletalMeshComponent>(instance) else { continue };
        let mut moved = None;
        if live {
            // a failed suffix is retried in the next AfterScripts publication phase.
            player.animation_phase_step(delta_seconds);
            moved = player.take_root_motion();
        } else if player.anim.started {
            player.animation_phase_stopped();
        }
        drop(player);
        if let (Some(delta), Some((_, owner))) = (moved, link) {
            apply_root_motion(world, owner, &delta);
        }
    }
}

/// Move `owner` by a root-motion delta in its own local frame. Moving is
/// subject to the same gates as script moves (`Static` objects stay put); a
/// refused move drops the delta.
fn apply_root_motion(world: &mut World, owner: Entity, delta: &skeletal_assets::runtime::pose::RootMotion) {
    use pulsar_world_registry::pulsar_scene_model::components::Transform;
    if let Err(reason) = pulsar_world_registry::pulsar_scene_model::motion::ensure_can_move(world, owner) {
        tracing::debug!(%reason, "root motion not applied");
        return;
    }
    let Some(mut transform) = world.get_mut::<Transform>(owner) else { return };
    let (position, rotation) = skeletal_assets::runtime::rootmotion::advance_transform(transform.position, transform.rotation, transform.scale, delta);
    if position != transform.position || rotation != transform.rotation {
        transform.position = position;
        transform.rotation = rotation;
    }
}

/// called after scripts release World, immediately before the AfterScripts flush.
/// object references are resolved from the source owner's current script instance.
/// A component that was disabled keeps the events its teardown produced (state
/// ends, cancelled Montages) as long as its owner lives.
pub fn flush_animation_notifies(
    world: &mut World,
    hub: &EventHub,
    resolve: impl Fn(Entity, &str) -> Option<Entity>,
) {
    let players: Vec<_> = world
        .query::<&SkeletalMeshComponent>()
        .map(|(entity, _)| entity)
        .collect();
    for instance in players {
        let Some(link) =
            world.get::<pulsar_world_registry::pulsar_scene_model::ComponentOwner>(instance)
        else {
            continue;
        };
        let owner = link.entity();
        let enabled = link.is_enabled();
        if !world.is_alive(owner) {
            world
                .get_mut::<SkeletalMeshComponent>(instance)
                .unwrap()
                .notify_playback
                .pending
                .clear();
            continue;
        }
        if enabled {
            world
                .get_mut::<SkeletalMeshComponent>(instance)
                .unwrap()
                .reconcile_notify_playback();
        }
        let target_names: Vec<_> = world
            .get::<SkeletalMeshComponent>(instance)
            .unwrap()
            .notify_playback
            .pending
            .iter()
            .map(|pending| pending.target.clone())
            .collect();
        let targets: Vec<_> = target_names
            .iter()
            .map(|target| {
                let target = match target {
                    NotifyTarget::Self_ => Some(owner),
                    NotifyTarget::ClassVariable(name) => resolve(owner, name),
                };
                target.filter(|&target| world.is_alive(target))
            })
            .collect();
        let context = pulsar_world_registry::ComponentContext::for_instance(instance, owner, hub);
        let mut player = world.get_mut::<SkeletalMeshComponent>(instance).unwrap();
        if let Err(error) = player.register_pending_events(hub) {
            tracing::warn!(%error, "animation notify schema conflicts with the running session");
        }
        if let Err(error) = player.publish_directed_notifies(&context.events, hub, owner, &targets)
        {
            tracing::warn!(%error, "could not queue animation notify; retaining it");
        }
    }
}

/// an owned occurrence, transported with the existing versioned event codec.
///
/// One type for instant notifies and for the phases of a notify state; `sequence_id`
/// names the owning asset whatever its kind (`source_kind` is `animseq` or
/// `montage`). `notify_phase` is `notify`, `begin`, `tick` or `end`; `end_reason` is
/// set on ends; `playback_instance` identifies the playback (a Sequence Player,
/// a Blend Space sample, a Montage segment, a Montage timeline) so a state's
/// begin/tick/end can be matched up.
#[engine_class(no_register)]
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AnimationNotifyEvent {
    #[property]
    pub sequence_id: String,
    #[property]
    pub notify_id: String,
    #[property]
    pub notify_name: String,
    #[property]
    pub clip_time: f64,
    #[property]
    pub playback_time: f64,
    #[property]
    pub cycle: i64,
    #[property]
    pub generation: i64,
    #[property]
    pub playback_instance: i64,
    #[property]
    pub notify_phase: String,
    #[property]
    pub tick_delta: f64,
    #[property]
    pub end_reason: String,
    #[property]
    pub source_kind: String,
}

/// A lifecycle event of one Montage instance on this component.
#[engine_class(no_register)]
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MontageLifecycleEvent {
    #[property]
    pub montage_id: String,
    /// The Montage instance (a handle, unique per component).
    #[property]
    pub instance: i64,
    #[property]
    pub section_from: String,
    #[property]
    pub section_to: String,
    /// On `ended`/`interrupted`: `completed`, `interrupted`, `stopped` or `cancelled`.
    #[property]
    pub result: String,
    /// The instance that interrupted this one, or -1.
    #[property]
    pub interrupted_by: i64,
    #[property]
    pub generation: i64,
}

/// A state machine event of the Animation Graph this component plays.
#[engine_class(no_register)]
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AnimStateEvent {
    #[property]
    pub graph_id: String,
    #[property]
    pub machine: String,
    /// The state entered or exited; for transitions, the state transitioned from.
    #[property]
    pub state: String,
    #[property]
    pub from_state: String,
    #[property]
    pub to_state: String,
    /// Id of the transition, blank when the machine starts.
    #[property]
    pub transition: String,
    #[property]
    pub generation: i64,
}

#[component_events(class = "SkeletalMeshComponent")]
pub trait SkeletalAnimationEvents {
    #[bp_event]
    fn animation_notify() -> AnimationNotifyEvent {}
    #[bp_event]
    fn montage_started() -> MontageLifecycleEvent {}
    #[bp_event]
    fn montage_blended_in() -> MontageLifecycleEvent {}
    #[bp_event]
    fn montage_section_changed() -> MontageLifecycleEvent {}
    #[bp_event]
    fn montage_blending_out() -> MontageLifecycleEvent {}
    #[bp_event]
    fn montage_ended() -> MontageLifecycleEvent {}
    #[bp_event]
    fn montage_interrupted() -> MontageLifecycleEvent {}
    #[bp_event]
    fn anim_state_entered() -> AnimStateEvent {}
    #[bp_event]
    fn anim_state_exited() -> AnimStateEvent {}
    #[bp_event]
    fn anim_transition_started() -> AnimStateEvent {}
    #[bp_event]
    fn anim_transition_completed() -> AnimStateEvent {}
}

/// Notify bookkeeping of one component. It owns no clock: the component's
/// animation instance is the only thing that advances time, and every span of
/// playback it reports goes through the tracker, which knows each playback
/// (Sequence Player, Blend Space sample, Montage segment, Montage timeline) and
/// the notify states active in it.
#[derive(Debug, Default)]
pub(super) struct NotifyPlayback {
    generation: i64,
    pending: VecDeque<PendingNotify>,
    tracker: NotifyTracker,
    registered_descriptor: Option<Arc<EventDescriptor>>,
    registered_sequence: Option<Arc<AnimSeqAsset>>,
}

impl NotifyPlayback {
    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub(super) fn next_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Everything one advance reported, in delivery order: the lifecycle events
    /// that precede the playback (started, blended in, section changed), the
    /// notifies and notify-state phases in playback order per playback, then the
    /// ones that follow it (blending out, ended).
    fn collect_report(&mut self, report: &skeletal_assets::runtime::anim_instance::UpdateReport, montage_events: &[MontageEvent], graph_id: &str) -> Result<(), String> {
        let generation = self.generation;
        self.pending.extend(report.state_events.iter().map(|e| PendingNotify::state(e, graph_id, generation)));
        let mut occurrences = Vec::new();
        let result = self.tracker.observe_report(report, &mut occurrences);
        self.push_lifecycle(montage_events.iter().filter(|e| is_early(&e.kind)), generation);
        self.pending.extend(occurrences.iter().map(|o| PendingNotify::from_occurrence(o, generation)));
        self.push_lifecycle(montage_events.iter().filter(|e| !is_early(&e.kind)), generation);
        result
    }

    pub(super) fn push_lifecycle_events(&mut self, events: &[MontageEvent]) {
        let generation = self.generation;
        self.push_lifecycle(events.iter(), generation);
    }

    fn push_lifecycle<'a>(&mut self, events: impl Iterator<Item = &'a MontageEvent>, generation: i64) {
        for event in events {
            let kind = match &event.kind {
                MontageEventKind::Started => "started",
                MontageEventKind::BlendedIn => "blended_in",
                MontageEventKind::SectionChanged { .. } => "section_changed",
                MontageEventKind::BlendingOut => "blending_out",
                MontageEventKind::Ended(_) => "ended",
            };
            self.pending.push_back(PendingNotify::lifecycle(event, kind, generation));
            if matches!(event.kind, MontageEventKind::Ended(MontageResult::Interrupted { .. })) {
                self.pending.push_back(PendingNotify::lifecycle(event, "interrupted", generation));
            }
        }
    }

    /// End every active notify state now (the component seeked, reloaded or is
    /// going away) and queue the ends.
    pub(super) fn end_all(&mut self, reason: EndReason) {
        let mut occurrences = Vec::new();
        self.tracker.end_all(reason, &mut occurrences);
        let generation = self.generation;
        self.pending.extend(occurrences.iter().map(|o| PendingNotify::from_occurrence(o, generation)));
    }

    /// Notify states currently active across all playbacks.
    #[cfg(test)]
    pub(super) fn active_states(&self) -> usize {
        self.tracker.active_states()
    }
}

fn is_early(kind: &MontageEventKind) -> bool {
    matches!(kind, MontageEventKind::Started | MontageEventKind::BlendedIn | MontageEventKind::SectionChanged { .. })
}

impl Clone for NotifyPlayback {
    fn clone(&self) -> Self {
        // a duplicated component must not inherit another instance's outbox.
        Self::default()
    }
}

impl SkeletalMeshComponent {
    pub(super) fn begin_notify_playback(&mut self) {
        self.reconcile_notify_playback();
        self.notify_playback.pending.clear();
        self.notify_playback.generation = self.notify_playback.generation.wrapping_add(1);
    }

    /// Bring the animation runtime in line with the loaded assets. Replacing the
    /// sequence or graph starts a fresh playback and discards unaccepted events;
    /// notify states that were active end with the reason `reload`.
    pub(super) fn reconcile_notify_playback(&mut self) {
        if self.ensure_runtime() {
            // `ensure_runtime` already discarded the old playback's unaccepted events
            self.notify_playback.end_all(EndReason::Reload);
        }
    }

    /// Seek the single-sequence player without crossings, discarding occurrences
    /// not yet accepted by Gamma. Active notify states end with the reason `seek`
    /// (a seek is gameplay-visible only through those ends). Already queued
    /// engine events keep their original sequence and generation.
    pub fn seek_animation(&mut self, seconds: f64) -> Result<(), String> {
        if !seconds.is_finite() || seconds < 0.0 || seconds > f32::MAX as f64 {
            return Err("Animation seek time must be finite and nonnegative".into());
        }
        self.reconcile_notify_playback();
        let looping = self.internal_looping();
        let duration = self.loaded_animation.as_ref().map(|a| a.clip.duration as f64);
        let phase = match duration {
            Some(duration) if !duration.is_finite() || duration < 0.0 => {
                return Err("Clip duration must be finite and nonnegative".into());
            }
            Some(duration) if looping && duration > 0.0 => seconds % duration,
            Some(duration) => seconds.min(duration),
            None => seconds,
        } as f32;
        if let Some(sequence) = &self.loaded_animation {
            sequence.features.validate(sequence.clip.duration)?;
        }
        let clock = if looping { seconds } else { phase as f64 };
        if let Some(instance) = self.anim.instance.as_mut() {
            if !instance.seek_single_sequence(clock) && self.loaded_graph.is_none() && self.loaded_animation.is_some() {
                return Err("The animation runtime has no sequence player to seek".into());
            }
        }
        self.notify_playback.generation = self.notify_playback.generation.wrapping_add(1);
        self.notify_playback.pending.clear();
        self.notify_playback.end_all(EndReason::Seek);
        self.elapsed_seconds = phase;
        if let Some(instance) = self.anim.instance.as_mut() {
            instance.refresh();
        }
        self.refresh_pose();
        Ok(())
    }

    pub fn restart_animation(&mut self) -> Result<(), String> {
        self.seek_animation(0.0)?;
        self.playing = true;
        Ok(())
    }

    pub fn stop_animation(&mut self) {
        self.playing = false;
        let _ = self.seek_animation(0.0);
    }

    /// Advance the instance and queue what it raised. Time scales by the size of
    /// `playback_rate`; its sign only selects the direction of the internal
    /// single-sequence player (graph nodes and Montages carry their own rates).
    pub(super) fn advance_notifies(&mut self, delta_seconds: f32) -> Result<(), String> {
        self.reconcile_notify_playback();
        if !self.playing || delta_seconds <= 0.0 {
            return Ok(());
        }
        if !delta_seconds.is_finite() || !self.playback_rate.is_finite() {
            return Err("Animation delta and playback rate must be finite".into());
        }
        let scaled = delta_seconds * self.playback_rate.abs();
        if scaled <= 0.0 {
            return Ok(());
        }
        let looping = self.internal_looping();
        let reverse = self.playback_rate < 0.0;
        let policy = self.root_motion_policy();
        let Some(instance) = self.anim.instance.as_mut() else { return Ok(()) };
        instance.set_root_motion_extraction(policy.mode.extracts());
        instance.advance(scaled, TimeStep::Variable, UpdateMode::Play);
        let position = instance.single_sequence_position();
        if position.is_some_and(|p| !p.is_finite() || p.abs() > f32::MAX as f64) {
            return Err("Animation playback time overflowed".into());
        }
        let events = std::mem::take(&mut instance.report.montage_events);
        self.anim.montage_events.extend(events.iter().cloned());
        let collected = self.notify_playback.collect_report(&instance.report, &events, self.loaded_graph.as_ref().map_or("", |g| g.id.as_str()));
        let delta = policy.delta(&instance.report.root_motion, &instance.report.montage_root_motion);
        if self.root_motion_authority.logs_moves() {
            let montage_delta = policy.delta(&instance.report.montage_root_motion, &instance.report.montage_root_motion);
            let montage = instance
                .montages
                .instances()
                .iter()
                .find(|m| !matches!(m.montage.root_motion, skeletal_assets::montage::MontageRootMotion::Ignore))
                .map(|m| (m.montage.id.clone(), m.position as f32));
            self.root_motion_sequence += 1;
            self.root_motion_moves.push(RootMotionMove { sequence: self.root_motion_sequence, delta_seconds: instance.report.advanced, delta: delta.clone(), montage_delta, montage });
        }
        // a simulated proxy's owner is moved by replication, never by its animation
        self.pending_root_motion = self.root_motion_authority.moves_owner().then_some(delta);
        if let (Some(position), Some(sequence)) = (position, self.loaded_animation.as_ref()) {
            let duration = sequence.clip.duration as f64;
            self.elapsed_seconds = if looping && duration > 0.0 { position.rem_euclid(duration) as f32 } else { position.clamp(0.0, duration) as f32 };
            // a one-shot (or zero-length) sequence stops at its end (its start, reversed)
            let finished = if reverse { position <= 0.0 } else { position >= duration };
            if (!looping || duration == 0.0) && finished {
                self.playing = false;
            }
        }
        self.refresh_pose();
        collected
    }

    pub(super) fn register_notify_events(&mut self, hub: &EventHub) -> Result<(), String> {
        // Montages that are playing can raise their lifecycle and own notifies
        let montages: Vec<Arc<CompiledMontage>> = self
            .anim
            .instance
            .as_ref()
            .map(|i| i.montages.instances().iter().map(|m| m.montage.clone()).collect())
            .unwrap_or_default();
        for montage in montages {
            register_montage_events(&montage, hub)?;
        }
        let Some(sequence) = &self.loaded_animation else {
            return Ok(());
        };
        if self
            .notify_playback
            .registered_descriptor
            .as_ref()
            .is_some_and(|old| {
                hub.descriptor(old.id)
                    .is_some_and(|current| Arc::ptr_eq(old, &current))
            })
            && self
                .notify_playback
                .registered_sequence
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, sequence))
        {
            return Ok(());
        }
        register_animation_notifies(sequence, hub)?;
        // a descriptor identifies this registration without retaining the session bus.
        self.notify_playback.registered_descriptor = sequence
            .features
            .notifies
            .first()
            .map(animation_notify_descriptor_id(sequence))
            .or_else(|| sequence.features.notify_states.first().map(|s| animation_notify_state_descriptor(sequence, s).id))
            .and_then(|id| hub.descriptor(id));
        self.notify_playback.registered_sequence = Some(sequence.clone());
        Ok(())
    }

    /// Register the descriptor of every queued occurrence (graph players, Blend
    /// Space samples and Montages, not just the single-sequence player).
    fn register_pending_events(&mut self, hub: &EventHub) -> Result<(), String> {
        let mut seen: Vec<u64> = Vec::new();
        for pending in &self.notify_playback.pending {
            if seen.contains(&pending.descriptor.id) {
                continue;
            }
            seen.push(pending.descriptor.id);
            hub.register(pending.descriptor.clone(), EventCategory::Custom("Animation".into()))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn publish_directed_notifies(
        &mut self,
        events: &pulsar_world_registry::ComponentEventWriter<'_>,
        hub: &EventHub,
        owner: Entity,
        targets: &[Option<Entity>],
    ) -> Result<(), String> {
        for target in targets {
            let Some(pending) = self.notify_playback.pending.front_mut() else {
                break;
            };
            // the generic bridge keeps its established owner/component compatibility.
            if !pending.generic_accepted {
                match &pending.bridge {
                    Bridge::Notify => events.animation_notify(pending.event.clone())?,
                    Bridge::State { kind, event } => {
                        let event = event.clone();
                        match kind {
                            StateEventKind::Entered => events.anim_state_entered(event)?,
                            StateEventKind::Exited => events.anim_state_exited(event)?,
                            StateEventKind::TransitionStarted => events.anim_transition_started(event)?,
                            StateEventKind::TransitionCompleted => events.anim_transition_completed(event)?,
                        }
                    }
                    Bridge::Montage { kind, event } => {
                        let event = event.clone();
                        match *kind {
                            "started" => events.montage_started(event)?,
                            "blended_in" => events.montage_blended_in(event)?,
                            "section_changed" => events.montage_section_changed(event)?,
                            "blending_out" => events.montage_blending_out(event)?,
                            "ended" => events.montage_ended(event)?,
                            _ => events.montage_interrupted(event)?,
                        }
                    }
                }
                pending.generic_accepted = true;
            }
            if let Some(target) = target {
                pending.authored.fields[0] = DynValue::U64(target.bits());
                pending.authored.fields[1] = DynValue::U64(owner.bits());
                hub.publish_dyn(entity_channel(target.bits()), pending.authored.clone())
                    .map_err(|e| e.to_string())?;
            }
            self.notify_playback.pending.pop_front();
        }
        Ok(())
    }
}

fn animation_notify_descriptor_id(sequence: &AnimSeqAsset) -> impl Fn(&AnimationNotify) -> u64 + '_ {
    move |notify| animation_notify_descriptor(sequence, notify).id
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_events::{
        EventHub, FlushPoint,
        gamma::{Channel, DynValue, SubscribeOptions},
    };
    use pulsar_scenedb::{Entity, World};
    use pulsar_world_registry::{ComponentContext, ComponentRuntimeState};
    use std::sync::Mutex;

    const EVENT: &str = "SkeletalMeshComponent.animation_notify";

    fn player() -> SkeletalMeshComponent {
        let sequence = serde_json::from_value(serde_json::json!({
            "version": 2, "name": "Walk", "duration": 1.0, "fps": 30.0, "tracks": [],
            "notifies": [
                {"id": "zero", "name": "Zero", "time": 0.0},
                {"id": "a", "name": "Step", "time": 0.5},
                {"id": "b", "name": "Step", "time": 0.5},
                {"id": "end", "name": "End", "time": 1.0}
            ]
        }))
        .unwrap();
        let mut player = SkeletalMeshComponent::default();
        player.loaded_animation = Some(Arc::new(sequence));
        player.legacy_looping = true;
        player.playing = true;
        player.playback_rate = 1.0;
        player
    }

    fn listen(
        hub: &EventHub,
        entity: Entity,
    ) -> (
        Arc<Mutex<Vec<AnimationNotifyEvent>>>,
        pulsar_events::gamma::SyncSubscription,
    ) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let output = got.clone();
        let id = hub.descriptor_by_name(EVENT).unwrap().id;
        let subscription = hub.bus().subscribe_dyn(
            id,
            SubscribeOptions::channel(Channel::Entity(entity.bits())),
            move |event| {
                let DynValue::Bytes(bytes) = &event.fields[0] else {
                    panic!("payload must use the event codec")
                };
                let value = pulsar_world_registry::pulsar_script_vm::TypeRegistry::global()
                    .decode_event_value("AnimationNotifyEvent", bytes)
                    .unwrap();
                output.lock().unwrap().push(
                    value
                        .downcast_ref::<AnimationNotifyEvent>()
                        .unwrap()
                        .clone(),
                );
            },
        );
        (got, subscription)
    }

    fn step(
        p: &mut SkeletalMeshComponent,
        context: &mut ComponentContext,
        dt: f32,
        hub: &EventHub,
        owner: Entity,
    ) {
        p.animation_phase_step(dt);
        p.tick(context, dt);
        let targets = vec![Some(owner); p.notify_playback.pending.len()];
        let _ = p.publish_directed_notifies(&context.events, hub, owner, &targets);
    }

    /// One engine frame's animation and component phases.
    fn frame(world: &mut World, hub: &EventHub, dt: f32, state: &mut ComponentRuntimeState) {
        run_animation_phase(world, dt);
        pulsar_world_registry::tick_live_components(world, hub, dt, 1, state);
    }

    fn registered_hub() -> EventHub {
        let hub = EventHub::new();
        pulsar_world_registry::register_component_events(&hub).unwrap();
        hub
    }

    #[test]
    fn live_tick_publishes_distinct_crossings_before_wrapping_and_sampling_is_silent() {
        let hub = registered_hub();
        let mut world = World::new();
        let owner = world.spawn();
        let instance = pulsar_world_registry::attach_value(&mut world, owner, player()).unwrap();
        let (got, _subscription) = listen(&hub, instance);
        let (owner_got, _owner_subscription) = listen(&hub, owner);
        let mut state = ComponentRuntimeState::default();
        frame(&mut world, &hub, 2.25, &mut state);
        assert!(got.lock().unwrap().is_empty());
        assert_eq!(
            world
                .get::<SkeletalMeshComponent>(instance)
                .unwrap()
                .elapsed_seconds,
            0.25
        );
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        let occurrences = got.lock().unwrap().clone();
        let ids: Vec<_> = occurrences
            .iter()
            .map(|e| (e.notify_id.as_str(), e.cycle, e.playback_time))
            .collect();
        assert_eq!(
            ids,
            [
                ("a", 0, 0.5),
                ("b", 0, 0.5),
                ("end", 0, 1.0),
                ("zero", 1, 1.0),
                ("a", 1, 1.5),
                ("b", 1, 1.5),
                ("end", 1, 2.0),
                ("zero", 2, 2.0)
            ]
        );
        assert_eq!(*owner_got.lock().unwrap(), occurrences);
        for _ in 0..3 {
            world
                .get_mut::<SkeletalMeshComponent>(instance)
                .unwrap()
                .refresh_pose();
            frame(&mut world, &hub, 0.0, &mut state);
            flush_animation_notifies(&mut world, &hub, |_, _| None);
            hub.flush(FlushPoint::AfterScripts);
        }
        assert_eq!(got.lock().unwrap().len(), 8);
        frame(&mut world, &hub, 0.25, &mut state);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(got.lock().unwrap()[8].cycle, 2);
        assert_eq!(got.lock().unwrap()[8].playback_time, 2.5);
        drop(_subscription);
        assert_eq!(
            hub.subscriber_count(
                hub.descriptor_by_name(EVENT).unwrap().id,
                Channel::Entity(instance.bits())
            ),
            0
        );
    }

    #[test]
    fn pause_seek_speed_restart_stop_and_replacement_keep_one_cursor() {
        let hub = registered_hub();
        let mut world = World::new();
        let entity = world.spawn();
        let (got, _subscription) = listen(&hub, entity);
        let mut context = ComponentContext::new(entity, &hub);
        let mut p = player();
        step(&mut p, &mut context, 0.5, &hub, entity);
        p.playing = false;
        step(&mut p, &mut context, 4.0, &hub, entity);
        p.seek_animation(20.25).unwrap();
        step(&mut p, &mut context, 4.0, &hub, entity);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(got.lock().unwrap().len(), 2);
        p.playing = true;
        p.playback_rate = 2.0;
        step(&mut p, &mut context, 0.125, &hub, entity);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(got.lock().unwrap()[2].playback_time, 20.5);
        assert_eq!(got.lock().unwrap()[2].cycle, 20);
        // a negative rate plays the single sequence backwards: 10 s of a 1 s loop with
        // four notifies per cycle
        p.playback_rate = -1.0;
        step(&mut p, &mut context, 10.0, &hub, entity);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(got.lock().unwrap().len(), 44);
        p.restart_animation().unwrap();
        p.playback_rate = 1.0;
        step(&mut p, &mut context, 0.5, &hub, entity);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(got.lock().unwrap()[44].playback_time, 0.5);
        {
            let events = got.lock().unwrap();
            assert_ne!(events[44].generation, events[2].generation);
        }
        p.stop_animation();
        step(&mut p, &mut context, 1.0, &hub, entity);
        assert_eq!(p.elapsed_seconds, 0.0);
        let old_id = got.lock().unwrap()[0].sequence_id.clone();
        p.loaded_animation = player().loaded_animation;
        p.restart_animation().unwrap();
        step(&mut p, &mut context, 0.5, &hub, entity);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_ne!(got.lock().unwrap()[46].sequence_id, old_id);
        assert_eq!(got.lock().unwrap().len(), 48);
    }

    #[test]
    fn publication_failure_retries_without_repeating_accepted_crossings() {
        let hub = EventHub::new();
        let mut world = World::new();
        let entity = world.spawn();
        let mut context = ComponentContext::new(entity, &hub);
        let mut p = player();
        step(&mut p, &mut context, 0.5, &hub, entity);
        assert_eq!(p.elapsed_seconds, 0.5);
        assert_eq!(p.notify_playback.pending.len(), 2);
        step(&mut p, &mut context, 0.5, &hub, entity);
        assert_eq!(
            p.elapsed_seconds, 0.5,
            "retry failure cannot advance beyond the retained events"
        );
        assert_eq!(hub.queued_len(), 0);
        pulsar_world_registry::register_component_events(&hub).unwrap();
        let (got, _subscription) = listen(&hub, entity);
        step(&mut p, &mut context, 0.5, &hub, entity);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        step(&mut p, &mut context, 0.5, &hub, entity);
        hub.flush(FlushPoint::AfterScripts);
        let ids: Vec<_> = got
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.notify_id.clone())
            .collect();
        assert_eq!(ids, ["a", "b", "end", "zero"]);
        assert!(p.notify_playback.pending.is_empty());
        step(&mut p, &mut context, 0.0, &hub, entity);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(got.lock().unwrap().len(), 4);
    }

    #[test]
    fn shared_sequence_instances_are_independent_and_queued_payloads_outlive_the_emitter() {
        let hub = registered_hub();
        let mut world = World::new();
        let owner = world.spawn();
        let a = player();
        let mut b = a.clone();
        b.playback_rate = 2.0;
        assert!(Arc::ptr_eq(
            a.loaded_animation.as_ref().unwrap(),
            b.loaded_animation.as_ref().unwrap()
        ));
        let a = pulsar_world_registry::attach_value(&mut world, owner, a).unwrap();
        let b = pulsar_world_registry::attach_value(&mut world, owner, b).unwrap();
        let (got_a, sub_a) = listen(&hub, a);
        let (got_b, _sub_b) = listen(&hub, b);
        let mut state = ComponentRuntimeState::default();
        frame(&mut world, &hub, 0.25, &mut state);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        world.despawn(b);
        pulsar_world_registry::process_component_removals(&world, &hub, &mut state);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert!(got_a.lock().unwrap().is_empty());
        assert_eq!(
            got_b.lock().unwrap().len(),
            2,
            "Gamma owns accepted payloads until delivery"
        );
        frame(&mut world, &hub, 0.25, &mut state);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(got_a.lock().unwrap().len(), 2);
        assert_eq!(got_b.lock().unwrap().len(), 2);
        drop(sub_a);
        world.despawn(a);
        state.end_all(&hub);
    }

    #[test]
    fn unaccepted_events_are_discarded_on_seek_replacement_and_clone() {
        let mut p = player();
        p.advance(0.5);
        assert_eq!(p.notify_playback.pending.len(), 2);
        assert!(p.clone().notify_playback.pending.is_empty());
        p.seek_animation(0.25).unwrap();
        assert!(p.notify_playback.pending.is_empty());
        p.advance(0.25);
        p.loaded_animation = player().loaded_animation;
        p.reconcile_notify_playback();
        assert!(p.notify_playback.pending.is_empty());
        p.legacy_looping = false;
        p.restart_animation().unwrap();
        p.advance(3.0);
        assert!(!p.playing);
        assert_eq!(p.elapsed_seconds, 1.0);
        let ids: Vec<_> = p
            .notify_playback
            .pending
            .iter()
            .map(|e| e.event.notify_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b", "end"]);
    }
    #[test]
    fn dispatch_uses_gamma_priority_order_and_queues_reentrant_publications() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let hub = registered_hub();
        let mut world = World::new();
        let entity = world.spawn();
        let id = hub.descriptor_by_name(EVENT).unwrap().id;
        let log = Arc::new(Mutex::new(Vec::new()));
        let once = Arc::new(AtomicBool::new(false));
        let (received, _received) = listen(&hub, entity);
        let high_log = log.clone();
        let _high = hub.bus().subscribe_dyn(
            id,
            SubscribeOptions::channel(Channel::Entity(entity.bits())).priority(10),
            move |_| high_log.lock().unwrap().push("high"),
        );
        let low_log = log.clone();
        let reentrant_hub = hub.clone();
        let _low = hub.bus().subscribe_dyn(
            id,
            SubscribeOptions::channel(Channel::Entity(entity.bits())),
            move |event| {
                low_log.lock().unwrap().push("low");
                if !once.swap(true, Ordering::Relaxed) {
                    reentrant_hub
                        .publish_dyn(Channel::Entity(entity.bits()), event.clone())
                        .unwrap();
                }
            },
        );
        let mut p = player();
        step(
            &mut p,
            &mut ComponentContext::new(entity, &hub),
            0.5,
            &hub,
            entity,
        );
        assert!(log.lock().unwrap().is_empty());
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        let report = hub.flush(FlushPoint::AfterScripts);
        assert!(!report.hit_round_limit);
        assert_eq!(
            *log.lock().unwrap(),
            ["high", "low", "high", "low", "high", "low"]
        );
        let events = received.lock().unwrap();
        let ids: Vec<_> = events.iter().map(|e| e.notify_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "a"]);
    }

    #[test]
    fn runtime_clock_keeps_large_cycle_offsets_and_zero_duration_is_silent() {
        let mut p = player();
        p.seek_animation(1_099_511_627_776.25).unwrap();
        let hub = registered_hub();
        let mut world = World::new();
        p.begin_play(&mut ComponentContext::new(world.spawn(), &hub));
        p.advance(0.25);
        assert_eq!(p.notify_playback.pending.len(), 2);
        assert_eq!(p.notify_playback.pending[0].event.cycle, 1_099_511_627_776);
        assert_eq!(
            p.notify_playback.pending[0].event.playback_time,
            1_099_511_627_776.5
        );
        assert_eq!(p.elapsed_seconds, 0.5);
        assert!(p.seek_animation(f64::NAN).is_err());
        assert_eq!(p.notify_playback.pending.len(), 2);
        p.stop_animation();
        assert!(p.notify_playback.pending.is_empty());
        let sequence = Arc::make_mut(p.loaded_animation.as_mut().unwrap());
        sequence.clip.duration = 0.0;
        sequence.features.notifies.retain(|n| n.time == 0.0);
        p.restart_animation().unwrap();
        p.advance(10.0);
        assert!(!p.playing);
        assert!(p.notify_playback.pending.is_empty());
        let saved = serde_json::to_value(&p).unwrap();
        assert!(saved.get("notify_playback").is_none());
        assert!(saved.get("elapsed_seconds").is_none());
    }
    #[test]
    fn seeking_and_playback_keep_working_while_physics_owns_the_pose() {
        let mut p = player();
        p.physics_pose = Some(vec![glam::Mat4::IDENTITY]);
        p.seek_animation(0.25).unwrap();
        assert_eq!(p.elapsed_seconds, 0.25);
        p.advance(0.5);
        assert!(p.elapsed_seconds > 0.25, "the clock moved: {}", p.elapsed_seconds);
        assert!(p.physics_pose.is_some(), "ownership is the rig's to give back");
    }
    #[test]
    fn descriptors_keep_identity_across_labels_and_refuse_live_schema_conflicts() {
        let hub = registered_hub();
        let p = player();
        let sequence = p.loaded_animation.as_ref().unwrap();
        register_animation_notifies(sequence, &hub).unwrap();
        register_animation_notifies(sequence, &hub).unwrap();
        let a = animation_notify_descriptor(sequence, &sequence.features.notifies[1]);
        let b = animation_notify_descriptor(sequence, &sequence.features.notifies[2]);
        assert_ne!(a.id, b.id, "equal notify labels must not share a binding");
        let mut renamed = (**sequence).clone();
        renamed.clip.name = "Renamed clip".into();
        renamed.features.notifies[1].name = "Renamed notify".into();
        let renamed_descriptor =
            animation_notify_descriptor(&renamed, &renamed.features.notifies[1]);
        assert_eq!(a, renamed_descriptor);
        register_animation_notifies(&renamed, &hub).unwrap();
        renamed.features.notifies[1]
            .fields
            .push(skeletal_assets::notifies::NotifyField {
                name: "strength".into(),
                value: NotifyValue::Float(1.0),
            });
        assert!(register_animation_notifies(&renamed, &hub).is_err());
        assert_eq!(*hub.descriptor(a.id).unwrap(), a);
        let new_session = registered_hub();
        register_animation_notifies(&renamed, &new_session).unwrap();
    }

    #[test]
    fn authored_notifies_are_directed_once_and_foreign_subscriptions_drop_cleanly() {
        use pulsar_events::gamma::{SubscribeOptions, ffi::ForeignBus};
        let hub = registered_hub();
        let mut world = World::new();
        let owner = world.spawn();
        let target = world.spawn();
        let unrelated = world.spawn();
        let mut p = player();
        let sequence = Arc::make_mut(p.loaded_animation.as_mut().unwrap());
        sequence.features.notifies.truncate(2);
        sequence.features.notifies[1].target = NotifyTarget::ClassVariable("target".into());
        sequence.features.notifies[1]
            .fields
            .push(skeletal_assets::notifies::NotifyField {
                name: "strength".into(),
                value: NotifyValue::Float(2.5),
            });
        let descriptor = animation_notify_descriptor(sequence, &sequence.features.notifies[1]);
        let instance = pulsar_world_registry::attach_value(&mut world, owner, p).unwrap();
        let mut state = ComponentRuntimeState::default();
        frame(&mut world, &hub, 0.5, &mut state);
        let foreign = unsafe { ForeignBus::from_raw(hub.export_raw()) }.unwrap();
        foreign.register_descriptor(&descriptor).unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let got = received.clone();
        let sub = foreign.subscribe_dyn(
            descriptor.id,
            SubscribeOptions::channel(entity_channel(target.bits())),
            move |event| got.lock().unwrap().push(event.clone()),
        );
        let wrong = Arc::new(Mutex::new(0));
        let mut unrelated_subs = Vec::new();
        for channel in [
            entity_channel(owner.bits()),
            entity_channel(instance.bits()),
            entity_channel(unrelated.bits()),
            Channel::Global,
        ] {
            let wrong = wrong.clone();
            unrelated_subs.push(hub.bus().subscribe_dyn(
                descriptor.id,
                SubscribeOptions::channel(channel),
                move |_| *wrong.lock().unwrap() += 1,
            ));
        }
        hub.flush(FlushPoint::AfterPhysics);
        assert!(received.lock().unwrap().is_empty());
        flush_animation_notifies(&mut world, &hub, |source, variable| {
            assert_eq!(source, owner);
            assert_eq!(variable, "target");
            Some(target)
        });
        assert!(received.lock().unwrap().is_empty());
        hub.flush(FlushPoint::AfterScripts);
        let events = received.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].fields[0], DynValue::U64(target.bits()));
        assert_eq!(events[0].fields[1], DynValue::U64(owner.bits()));
        assert_eq!(events[0].fields.last(), Some(&DynValue::F64(2.5)));
        drop(events);
        assert_eq!(*wrong.lock().unwrap(), 0);

        // an old reference cannot address a recycled slot's listener.
        world.despawn(target);
        let recycled = world.spawn();
        assert_eq!(target.index(), recycled.index());
        assert_ne!(target.generation(), recycled.generation());
        let stale = wrong.clone();
        let _recycled = hub.bus().subscribe_dyn(
            descriptor.id,
            SubscribeOptions::channel(entity_channel(recycled.bits())),
            move |_| *stale.lock().unwrap() += 1,
        );
        frame(&mut world, &hub, 1.0, &mut state);
        flush_animation_notifies(&mut world, &hub, |_, _| Some(target));
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(received.lock().unwrap().len(), 1);
        assert_eq!(*wrong.lock().unwrap(), 0);
        // a live target with no listener, or an unresolved variable, is harmless.
        drop(sub);
        let no_listener = world.spawn();
        for resolved in [Some(no_listener), None] {
            frame(&mut world, &hub, 1.0, &mut state);
            flush_animation_notifies(&mut world, &hub, |_, _| resolved);
            hub.flush(FlushPoint::AfterScripts);
        }
        assert_eq!(
            hub.subscriber_count(descriptor.id, entity_channel(target.bits())),
            0
        );
        drop(unrelated_subs);
        drop(_recycled);
        drop(foreign);
        state.end_all(&hub);
        hub.drain_queued();
    }

    #[test]
    fn project_catalog_retains_valid_events_beside_invalid_assets() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let p = player();
        let sequence = p.loaded_animation.as_ref().unwrap();
        std::fs::write(dir.join("valid.animseq"), sequence.to_text()).unwrap();
        std::fs::write(dir.join("broken.animseq"), "{broken").unwrap();
        std::fs::write(dir.join("unreadable.animseq"), [0xff, 0xfe]).unwrap();
        let mut record = serde_json::to_value(&**sequence).unwrap();
        record["version"] = serde_json::json!(999);
        std::fs::write(dir.join("future.animseq"), record.to_string()).unwrap();
        record = serde_json::to_value(&**sequence).unwrap();
        record.as_object_mut().unwrap().remove("id");
        std::fs::write(dir.join("idless.animseq"), record.to_string()).unwrap();
        let catalog = project_animation_notify_events(&dir);
        let events = catalog.events;
        assert_eq!(events.len(), 4);
        assert_eq!(catalog.diagnostics.len(), 4);
        for file in [
            "broken.animseq",
            "unreadable.animseq",
            "future.animseq",
            "idless.animseq",
        ] {
            let diagnostic = catalog
                .diagnostics
                .iter()
                .find(|d| d.path == dir.join(file))
                .unwrap();
            assert!(!diagnostic.error.is_empty());
        }
        let hub = EventHub::new();
        for (_, descriptor) in events {
            hub.register(descriptor, EventCategory::Custom("Animation".into()))
                .unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_catalog_quarantines_conflicts_but_keeps_unrelated_identities() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let p = player();
        let mut sequence = (**p.loaded_animation.as_ref().unwrap()).clone();
        std::fs::write(dir.join("a.animseq"), sequence.to_text()).unwrap();
        sequence.features.notifies[0]
            .fields
            .push(skeletal_assets::notifies::NotifyField {
                name: "strength".into(),
                value: NotifyValue::Float(1.0),
            });
        let conflicting = animation_notify_descriptor(&sequence, &sequence.features.notifies[0]);
        std::fs::write(dir.join("b.animseq"), sequence.to_text()).unwrap();
        // A third claimant matches the first: it must not restore the quarantined identity.
        sequence.features.notifies[0].fields.clear();
        std::fs::write(dir.join("c.animseq"), sequence.to_text()).unwrap();
        let original = animation_notify_descriptor(&sequence, &sequence.features.notifies[0]);
        let catalog = project_animation_notify_events(&dir);
        let events = catalog.events;
        assert_eq!(events.len(), 3);
        assert_eq!(catalog.diagnostics.len(), 3);
        for file in ["a.animseq", "b.animseq", "c.animseq"] {
            let diagnostic = catalog
                .diagnostics
                .iter()
                .find(|d| d.path == dir.join(file))
                .unwrap();
            assert!(diagnostic.error.contains("Quarantined animseq::"));
            assert!(
                diagnostic.error.contains("a.animseq") && diagnostic.error.contains("b.animseq")
            );
        }
        assert!(
            events
                .iter()
                .all(|(_, d)| d.id != original.id && d.id != conflicting.id)
        );
        let hub = EventHub::new();
        for (_, descriptor) in events {
            hub.register(descriptor, EventCategory::Custom("Animation".into()))
                .unwrap();
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_catalog_reports_an_unreadable_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing");
        let catalog = project_animation_notify_events(&path);
        assert!(catalog.events.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(catalog.diagnostics[0].path, path);
        assert!(!catalog.diagnostics[0].error.is_empty());
    }

    #[test]
    fn project_catalog_refreshes_saved_labels_schema_and_removals() {
        let dir = std::env::temp_dir().join(format!(
            "animation-catalog-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("walk.animseq");
        let p = player();
        let mut sequence = (**p.loaded_animation.as_ref().unwrap()).clone();
        skeletal_assets::assets::save_animseq_asset(&file, &sequence).unwrap();
        let original = project_animation_notify_events(&dir).events;
        assert_eq!(original.len(), 4);
        sequence.clip.name = "Renamed".into();
        sequence.features.notifies[0].name = "New zero".into();
        skeletal_assets::assets::save_animseq_asset(&file, &sequence).unwrap();
        let renamed = project_animation_notify_events(&dir).events;
        assert_eq!(
            original.iter().map(|(_, d)| d.id).collect::<Vec<_>>(),
            renamed.iter().map(|(_, d)| d.id).collect::<Vec<_>>()
        );
        assert_ne!(
            original.iter().map(|(label, _)| label).collect::<Vec<_>>(),
            renamed.iter().map(|(label, _)| label).collect::<Vec<_>>()
        );
        std::fs::remove_file(file).unwrap();
        assert!(project_animation_notify_events(&dir).events.is_empty());
        // legacy sequences remain readable, but event bindings need a saved asset id.
        let idless = dir.join("idless.animseq");
        let mut record = serde_json::to_value(&sequence).unwrap();
        record.as_object_mut().unwrap().remove("id");
        std::fs::write(&idless, record.to_string()).unwrap();
        let catalog = project_animation_notify_events(&dir);
        assert!(catalog.events.is_empty());
        assert_eq!(catalog.diagnostics.len(), 1);
        assert_eq!(catalog.diagnostics[0].path, idless);
        assert!(
            catalog.diagnostics[0]
                .error
                .contains("stable sequence identity")
        );
        std::fs::remove_file(idless).unwrap();

        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn authored_self_events_keep_shared_clip_instances_independent() {
        let hub = registered_hub();
        let mut world = World::new();
        let owner_a = world.spawn();
        let owner_b = world.spawn();
        let a = player();
        let mut b = a.clone();
        b.playback_rate = 2.0;
        let sequence = a.loaded_animation.as_ref().unwrap();
        let descriptor = animation_notify_descriptor(sequence, &sequence.features.notifies[1]);
        let a_count = Arc::new(Mutex::new(0));
        let b_count = Arc::new(Mutex::new(0));
        let mut subscriptions = Vec::new();
        for (owner, count) in [(owner_a, a_count.clone()), (owner_b, b_count.clone())] {
            subscriptions.push(hub.bus().subscribe_dyn(
                descriptor.id,
                SubscribeOptions::channel(entity_channel(owner.bits())),
                move |_| *count.lock().unwrap() += 1,
            ));
        }
        pulsar_world_registry::attach_value(&mut world, owner_a, a).unwrap();
        pulsar_world_registry::attach_value(&mut world, owner_b, b).unwrap();
        let mut state = ComponentRuntimeState::default();
        frame(&mut world, &hub, 0.25, &mut state);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!((*a_count.lock().unwrap(), *b_count.lock().unwrap()), (0, 1));
        frame(&mut world, &hub, 0.25, &mut state);
        flush_animation_notifies(&mut world, &hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!((*a_count.lock().unwrap(), *b_count.lock().unwrap()), (1, 1));
        state.end_all(&hub);
    }

    #[test]
    fn an_authored_publication_failure_keeps_only_the_unaccepted_suffix() {
        let hub = registered_hub();
        let mut world = World::new();
        let owner = world.spawn();
        let mut p = player();
        let sequence = p.loaded_animation.as_ref().unwrap();
        let a = animation_notify_descriptor(sequence, &sequence.features.notifies[1]);
        let b = animation_notify_descriptor(sequence, &sequence.features.notifies[2]);
        hub.register(a.clone(), EventCategory::Gameplay).unwrap();
        let (generic, _generic) = listen(&hub, owner);
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let mut subscriptions = Vec::new();
        for descriptor in [&a, &b] {
            let got = delivered.clone();
            let id = descriptor.id;
            subscriptions.push(hub.bus().subscribe_dyn(
                id,
                SubscribeOptions::channel(entity_channel(owner.bits())),
                move |_| got.lock().unwrap().push(id),
            ));
        }
        p.advance(0.5);
        let writer = pulsar_world_registry::ComponentEventWriter::new(owner, &hub);
        assert!(
            p.publish_directed_notifies(&writer, &hub, owner, &[Some(owner), Some(owner)])
                .is_err()
        );
        assert_eq!(p.notify_playback.pending.len(), 1);
        assert!(p.notify_playback.pending[0].generic_accepted);
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(*delivered.lock().unwrap(), [a.id]);
        assert_eq!(generic.lock().unwrap().len(), 2);
        register_animation_notifies(p.loaded_animation.as_ref().unwrap(), &hub).unwrap();
        p.publish_directed_notifies(&writer, &hub, owner, &[Some(owner)])
            .unwrap();
        hub.flush(FlushPoint::AfterScripts);
        assert_eq!(*delivered.lock().unwrap(), [a.id, b.id]);
        assert_eq!(
            generic.lock().unwrap().len(),
            2,
            "retry must not duplicate the accepted generic bridge"
        );
        assert!(p.notify_playback.pending.is_empty());
    }
}
