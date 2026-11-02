//! Runtime skeletal mesh instances and SceneDB asset bindings.
//!
//! The asset files are addressed by typed stable ids. SceneDB owns the loaded
//! mesh, skeleton and clip payloads; the component mirrors the vertex skin
//! stream and the current joint palette into the renderer's shared GPU scene.
//!
//! # Animation
//!
//! The component is the player. It owns one [`AnimInstance`] (graph state,
//! parameters, Montage instances, evaluated and previous pose); the compiled
//! graph and every compiled clip are shared with other components. A component
//! that only names a single sequence (scenes from before the graph runtime) plays
//! it through a compiled one-node graph in that same instance, never a second
//! player.
//!
//! Per frame the engine runs the **animation phase** ([`run_animation_phase`])
//! before the ECS schedule, so the articulated-physics rig reads this frame's
//! pose. Parameters and Montage commands written by gameplay or scripts later in
//! the frame take effect in the next frame's animation phase. While the physics
//! rig owns the pose ([`SkeletalMeshComponent::apply_physics_pose`]) the
//! animation keeps running (clock, notifies, Montages, root motion) and the drawn
//! pose blends toward the physics pose by the physics blend weight; clearing
//! physics ownership shows the animation again, which never stopped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use engine_class_derive::{
    engine_class, register_component_runtime, register_scene_props_applier,
    register_world_component,
};
use helio::PackedVertex;
use pulsar_reflection::{
    AnimationGraphAssetKind, AnimationMontageAssetKind, AnimationSequenceAssetKind, AssetKind,
    AssetRef, Reflectable, ScenePropsProjector, SkeletalMeshAssetKind, SkeletonAssetKind,
};
use pulsar_scenedb::{AssetId, AssetPayload, AssetView, Entity, World};
use serde_json::Value;
use skeletal_assets::diagnostics::Diagnostic;
use skeletal_assets::runtime::anim_instance::{single_sequence_graph, AnimInstance};
use skeletal_assets::runtime::clip::CompiledClip;
use skeletal_assets::runtime::graph::compile::{compile_graph, compile_montage};
use skeletal_assets::runtime::graph::{CompiledGraph, ParamError, Value as AnimValue};
use skeletal_assets::runtime::montage::{CompiledMontage, MontageEvent, MontageHandle, PlayRefused};
use skeletal_assets::runtime::pose::{RootMotion, RuntimeSkeleton};
use skeletal_assets::runtime::playback::EndReason;
use skeletal_assets::runtime::rootmotion::{RootMotionMode, RootMotionPolicy, RootRotation};
use std::collections::HashMap;

use crate::anim_source::ProjectAnimSource;
use crate::components::{MeshGeometryKey, MeshSectionDraw, StaticMeshDraw};
use crate::mesh_cache::MeshSection;

#[path = "skeletal_notifies.rs"]
mod notifies;
pub use notifies::{
    AnimationNotifyCatalog, AnimationNotifyDiagnostic, AnimationNotifyEvent, MONTAGE_LIFECYCLE_KINDS,
    AnimStateEvent, MontageLifecycleEvent, SkeletalAnimationEvents, anim_state_descriptor, SkeletalAnimationEventsEventWriterExt,
    animation_notify_descriptor, animation_notify_state_descriptor, montage_lifecycle_descriptor,
    montage_notify_descriptor, montage_notify_state_descriptor, register_montage_events,
    animation_source_for_file, flush_animation_notifies, project_animation_notify_events,
    component_animation_notify_events, register_animation_notifies, register_loaded_animation_notifies, run_animation_phase,
};

pub const SKELETAL_MESH_ASSET_KIND: &str = "skmesh";
pub const SKELETON_ASSET_KIND: &str = "skeleton";
pub const ANIMATION_SEQUENCE_ASSET_KIND: &str = "animseq";
pub const ANIMATION_GRAPH_ASSET_KIND: &str = "anim_graph";
pub const SKELETAL_SKIN_VERTICES_BUFFER: &str = "skeletal_skin_vertices";
pub const SKELETAL_BONE_PALETTE_BUFFER: &str = "skeletal_bone_palette";
pub const SKELETAL_PREVIOUS_BONE_PALETTE_BUFFER: &str = "skeletal_previous_bone_palette";
pub const SKELETAL_SKIN_HANDLES_BUFFER: &str = "skeletal_skin_vertices::handles";
pub const SKELETAL_PALETTE_HANDLES_BUFFER: &str = "skeletal_bone_palette::handles";

/// Per-vertex four-influence skin data. Kept in a storage buffer parallel to
/// `builtin_mesh_vertex`, so static geometry keeps its established vertex ABI.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SkinVertex {
    pub joints: [u32; 4],
    pub weights: [f32; 4],
}
unsafe impl pulsar_scenedb::page::Pod for SkinVertex {}

#[derive(Clone)]
pub struct LoadedSkeletalMesh(pub Arc<skeletal_assets::skmesh::SkeletalMeshAsset>);
#[derive(Clone)]
pub struct LoadedSkeleton(pub Arc<skeletal_assets::assets::SkeletonAsset>);
#[derive(Clone)]
pub struct LoadedAnimationSequence(pub Arc<skeletal_assets::assets::AnimSeqAsset>);
/// A compiled animation graph, or the reasons it could not be compiled.
#[derive(Clone)]
pub struct LoadedAnimationGraph {
    pub graph: Option<Arc<CompiledGraph>>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Which loaded assets the current animation instance was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RuntimeKey {
    skeleton: usize,
    graph: usize,
    animation: usize,
    looping: bool,
    /// The internal single-sequence player plays backwards (negative `playback_rate`).
    reverse: bool,
}

fn ptr_of<T>(value: &Option<Arc<T>>) -> usize {
    value.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize)
}

/// Mutable animation state of one component. Never serialized or cloned: a
/// duplicated component starts its own playback.
#[derive(Debug, Default)]
pub(crate) struct AnimRuntime {
    instance: Option<AnimInstance>,
    skeleton: Option<Arc<RuntimeSkeleton>>,
    skeleton_source: usize,
    key: Option<RuntimeKey>,
    /// The first animation phase of this live period has run.
    started: bool,
    montages: HashMap<String, Arc<CompiledMontage>>,
    montage_events: Vec<MontageEvent>,
    /// The compiled clip of the internal single-sequence player, kept across a
    /// direction change so its notify playbacks keep their asset.
    single_clip: Option<(usize, Arc<CompiledClip>)>,
}

impl Clone for AnimRuntime {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// Which root motion the component applies to its owner each frame.
/// See `skeletal_assets::runtime::rootmotion` for the exact behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum RootMotionModeSetting {
    /// Nothing is extracted: the root bone moves inside the pose.
    NoRootMotionExtraction,
    /// Extracted and discarded: the mesh stays on the owner (the default).
    #[default]
    IgnoreRootMotion,
    /// Graph players and Montages move the owner.
    RootMotionFromEverything,
    /// Only Montages in `Montage` root-motion mode move the owner.
    RootMotionFromMontagesOnly,
}

/// Who owns root motion when the component is networked.
///
/// Root motion is movement, and movement has one authority. The contract:
/// - `Standalone` (the default): this process runs the animation and moves the owner.
/// - `Authority` (the server): runs the animation, moves the owner, and logs every move so a
///   correction for an owning client can be derived from the same data.
/// - `AutonomousProxy` (the owning client): runs the animation and moves the owner at once
///   (prediction) and logs every move with its sequence number for the server to verify.
/// - `SimulatedProxy` (every other client): the animation advances so the pose looks right,
///   but the owner is never moved by it; its transform comes from replication.
///
/// Montage-owned root motion follows the same role; each logged move carries the Montage part
/// and the Montage playback it came from, so the server can start the same Montage at the
/// same position. Delivery of the moves between processes is a networking-layer concern (see
/// `.agents/ANIMATION.md`, Networked root motion).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum RootMotionAuthoritySetting {
    #[default]
    Standalone,
    Authority,
    AutonomousProxy,
    SimulatedProxy,
}

impl RootMotionAuthoritySetting {
    /// Whether the animation moves the owner in this process.
    pub fn moves_owner(self) -> bool {
        !matches!(self, Self::SimulatedProxy)
    }

    /// Whether moves are logged for exchange with the other side.
    pub fn logs_moves(self) -> bool {
        matches!(self, Self::Authority | Self::AutonomousProxy)
    }
}

/// One advance's root motion as the authority and the owning client exchange it.
#[derive(Clone, Debug, PartialEq)]
pub struct RootMotionMove {
    /// Counts up from 1 per component; a correction names the last move it accepts.
    pub sequence: u64,
    /// Animation time the advance ran, after the playback rate.
    pub delta_seconds: f32,
    /// The owner move after the component's root-motion policy (what was applied or would be).
    pub delta: RootMotion,
    /// The part of it that came from Montages.
    pub montage_delta: RootMotion,
    /// The Montage playing with root motion at the end of the advance, with its position.
    pub montage: Option<(String, f32)>,
}

/// How much of the extracted rotation turns the owner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum RootMotionRotationSetting {
    /// The owner never turns.
    NoRotation,
    /// Only the turn about the up axis (the default).
    #[default]
    YawOnly,
    /// The complete rotation.
    FullRotation,
}

impl From<RootMotionModeSetting> for RootMotionMode {
    fn from(mode: RootMotionModeSetting) -> Self {
        match mode {
            RootMotionModeSetting::NoRootMotionExtraction => Self::NoRootMotionExtraction,
            RootMotionModeSetting::IgnoreRootMotion => Self::IgnoreRootMotion,
            RootMotionModeSetting::RootMotionFromEverything => Self::RootMotionFromEverything,
            RootMotionModeSetting::RootMotionFromMontagesOnly => Self::RootMotionFromMontagesOnly,
        }
    }
}

impl From<RootMotionRotationSetting> for RootRotation {
    fn from(rotation: RootMotionRotationSetting) -> Self {
        match rotation {
            RootMotionRotationSetting::NoRotation => Self::None,
            RootMotionRotationSetting::YawOnly => Self::Yaw,
            RootMotionRotationSetting::FullRotation => Self::Full,
        }
    }
}

/// A skinned mesh placed in the scene. The authored links are typed UUIDs;
/// loaded data and GPU-ready geometry are derived and never serialized.
#[engine_class(
    category = "Rendering",
    default,
    clone,
    debug,
    serialize,
    deserialize,
    scene_store
)]
#[scene_store(collect_handles = crate::components::collect_skeletal_assets)]
pub struct SkeletalMeshComponent {
    #[property]
    pub mesh: Option<AssetRef<SkeletalMeshAssetKind>>,
    /// The Animation Graph this component runs. The one authored animation source.
    #[serde(default)]
    #[property]
    pub animation_graph: Option<AssetRef<AnimationGraphAssetKind>>,
    /// Compatibility, one release: scenes saved before the graph runtime named a
    /// single sequence here. It decodes into the component's internal
    /// single-sequence player (runtime state, not a second authored system) and is
    /// still written back under the same key so no scene loses its animation. It
    /// is removed when the one-release window closes.
    #[serde(default, rename = "animation", skip_serializing_if = "Option::is_none")]
    legacy_animation: Option<AssetRef<AnimationSequenceAssetKind>>,
    #[serde(default, rename = "looping", skip_serializing_if = "is_false")]
    legacy_looping: bool,
    /// Play or pause the whole animation instance (graph, Montages, internal player).
    #[serde(default)]
    #[property]
    pub playing: bool,
    #[serde(default = "default_playback_rate")]
    #[property]
    pub playback_rate: f32,
    /// Which root motion moves the owner: none (stay put), everything, or Montages only.
    #[serde(default)]
    #[property]
    pub root_motion_mode: RootMotionModeSetting,
    /// How much of the root motion's rotation turns the owner.
    #[serde(default)]
    #[property]
    pub root_motion_rotation: RootMotionRotationSetting,
    /// Also apply the vertical (up) part of root motion. Off by default: height
    /// belongs to gravity and ground contact, not to the clip.
    #[serde(default)]
    #[property]
    pub root_motion_vertical: bool,
    /// Who owns root motion when networked; see [`RootMotionAuthoritySetting`].
    #[serde(default)]
    #[property]
    pub root_motion_authority: RootMotionAuthoritySetting,

    #[serde(skip)]
    pub mesh_asset_id: AssetId,
    /// The skeleton the mesh is bound to, read from the mesh header. The mesh
    /// is the only authority; the component never stores a skeleton ref.
    #[serde(skip)]
    pub mesh_skeleton: Option<AssetRef<SkeletonAssetKind>>,
    #[serde(skip)]
    pub skeleton_asset_id: AssetId,
    /// Repairable problems with the component's asset links, recomputed whenever
    /// they change. Never serialized.
    #[serde(skip)]
    pub diagnostics: Vec<Diagnostic>,
    /// A skeleton ref found in an old saved component; kept only to report a
    /// disagreement with the mesh.
    #[serde(skip)]
    legacy_skeleton: Option<AssetRef<SkeletonAssetKind>>,
    #[serde(skip)]
    pub animation_asset_id: AssetId,
    #[serde(skip)]
    pub animation_graph_id: AssetId,
    #[serde(skip)]
    pub loaded_mesh: Option<Arc<skeletal_assets::skmesh::SkeletalMeshAsset>>,
    #[serde(skip)]
    pub loaded_skeleton: Option<Arc<skeletal_assets::assets::SkeletonAsset>>,
    /// The sequence of the internal single-sequence player.
    #[serde(skip)]
    pub loaded_animation: Option<Arc<skeletal_assets::assets::AnimSeqAsset>>,
    /// The compiled graph for `animation_graph`, shared with every component running it.
    #[serde(skip)]
    pub loaded_graph: Option<Arc<CompiledGraph>>,
    /// Compile problems of the graph, recomputed whenever it is (re)bound.
    #[serde(skip)]
    pub graph_diagnostics: Vec<Diagnostic>,
    /// Phase of the internal single-sequence player within its clip, for
    /// inspection and the physics handoff. Read-only: seek with `seek_animation`.
    #[serde(skip)]
    pub elapsed_seconds: f32,
    #[serde(skip)]
    notify_playback: notifies::NotifyPlayback,
    /// Root motion of the latest advance the owner has not been moved by yet.
    #[serde(skip)]
    pending_root_motion: Option<RootMotion>,
    /// Moves logged for exchange, oldest first (only for roles that log).
    #[serde(skip)]
    root_motion_moves: Vec<RootMotionMove>,
    #[serde(skip)]
    root_motion_sequence: u64,
    /// Bones the physics rig owns right now: mesh-space matrices that replace the
    /// animated bone in the published palette (the animation keeps running).
    #[serde(skip)]
    physics_bone_overrides: Vec<(usize, [[f32; 4]; 4])>,
    /// Canonical paths of the assets the running graph depends on, so an edit to
    /// one of them recompiles the graph.
    #[serde(skip)]
    graph_dependency_paths: Vec<String>,
    #[serde(skip)]
    graph_source_path: Option<String>,
    /// Per-component animation state: the instance, parameters and Montages.
    #[serde(skip)]
    anim: AnimRuntime,
    /// Evaluated parent-composed transforms in skeleton-local space, ordered
    /// exactly like `loaded_skeleton.bones`. These are pre-inverse-bind and
    /// intended for runtime consumers such as articulated physics bodies.
    #[serde(skip)]
    pub bone_world_pose: Vec<[[f32; 4]; 4]>,
    /// Whole-skeleton physics ownership: the rig's mesh-space pose and how much of the drawn
    /// pose it replaces (Unreal's physics blend weight). The animation keeps running either way.
    #[serde(skip)]
    physics_pose: Option<Vec<glam::Mat4>>,
    #[serde(skip)]
    physics_blend_weight: Option<f32>,
    #[serde(skip)]
    pub bounds_local: [f32; 4],
    #[serde(skip)]
    pub mesh_sections: Vec<MeshSection>,
    #[serde(skip)]
    pub geometry_key: MeshGeometryKey,

    /// The regular mesh vertex/index payloads are still consumed by Helio's
    /// GPU-driven scene join. Skin influences and the bone palette are read by
    /// the skinned G-buffer vertex path using these per-instance ranges.
    #[gpu(buffer = "builtin_mesh_vertex", mirror = Once, content_id = "geometry_key")]
    #[serde(skip)]
    pub vertices: Vec<PackedVertex>,
    #[gpu(buffer = "builtin_mesh_index", mirror = Once, content_id = "geometry_key")]
    #[serde(skip)]
    pub indices: Vec<u32>,
    #[gpu(buffer = "skeletal_skin_vertices", mirror = Once, content_id = "geometry_key")]
    #[serde(skip)]
    pub skin_vertices: Vec<SkinVertex>,
    #[serde(skip)]
    pub bone_palette: Vec<[[f32; 4]; 4]>,
    /// Current palette followed by the previous-frame palette in the same
    /// variable-length GPU range; SceneJoin exposes the half-size count.
    #[gpu(buffer = "skeletal_bone_palette")]
    #[serde(skip)]
    pub gpu_bone_palettes: Vec<[[f32; 4]; 4]>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn default_playback_rate() -> f32 {
    1.0
}

#[register_scene_props_applier]
impl ScenePropsProjector for SkeletalMeshComponent {
    const CLASS_NAME: &'static str = "SkeletalMeshComponent";
    fn apply_scene_props(props: &mut HashMap<String, Value>, _component_data: Option<&Value>) {
        props.remove("mesh");
        props.remove("skeleton");
        props.remove("animation");
        props.remove("looping");
    }
}

#[register_world_component(
    decode = decode_skeletal_mesh_component,
    property_written = skeletal_mesh_property_written
)]
impl SkeletalMeshComponent {}

impl SkeletalMeshComponent {
    /// Rebuild all resolved SceneDB references after a decode or authored edit.
    pub fn attach_assets(&mut self) {
        register_skeletal_mesh_assets();
        self.mesh_asset_id = asset_id_for_ref(self.mesh.as_ref());
        self.mesh_skeleton = mesh_skeleton_ref(self.mesh.as_ref());
        self.skeleton_asset_id = asset_id_for_ref(self.mesh_skeleton.as_ref());
        self.animation_asset_id = asset_id_for_ref(self.legacy_animation.as_ref());
        self.animation_graph_id = asset_id_for_ref(self.animation_graph.as_ref());
        if self.mesh_asset_id.is_zero() {
            self.clear_mesh();
        }
        if self.animation_graph_id.is_zero() {
            self.loaded_graph = None;
            self.graph_diagnostics.clear();
        }
        if self.animation_asset_id.is_zero() {
            self.loaded_animation = None;
        }
        self.refresh_pose();
        self.refresh_diagnostics();
    }

    /// Advance the animation by `delta_seconds` and refresh the pose and palette.
    /// The engine calls this from [`run_animation_phase`] once per frame.
    pub fn advance(&mut self, delta_seconds: f32) {
        if let Err(error) = self.advance_notifies(delta_seconds) {
            self.playing = false;
            tracing::warn!(%error, "could not advance skeletal animation");
        }
    }

    /// One animation-phase step: begin playback on the first live frame, hold while
    /// notify events wait for delivery, otherwise advance.
    pub(crate) fn animation_phase_step(&mut self, delta_seconds: f32) {
        if !self.anim.started {
            self.begin_notify_playback();
            self.anim.started = true;
        }
        if self.notify_playback.has_pending() {
            return;
        }
        self.advance(delta_seconds);
    }

    /// The component left the live set (disabled or its owner is gone).
    pub(crate) fn animation_phase_stopped(&mut self) {
        self.anim.started = false;
        self.notify_playback.clear_pending();
        let cancelled = self
            .anim
            .instance
            .as_mut()
            .map(|i| {
                i.shutdown();
                std::mem::take(&mut i.report.montage_events)
            })
            .unwrap_or_default();
        self.anim.montage_events.extend(cancelled.iter().cloned());
        // teardown ends every active notify state once, and each Montage reports Cancelled
        self.notify_playback.end_all(EndReason::Teardown);
        self.notify_playback.push_lifecycle_events(&cancelled);
        self.pending_root_motion = None;
        self.root_motion_moves.clear();
        self.physics_bone_overrides.clear();
        self.physics_pose = None;
        self.physics_blend_weight = None;
        // a re-enabled component rebuilds its instance
        self.anim.instance = None;
        self.anim.key = None;
    }

    fn internal_looping(&self) -> bool {
        self.legacy_looping
    }

    /// Start (or replace) the internal single-sequence player. The reference is
    /// kept under the compatibility key so it survives a save.
    pub fn play_sequence(&mut self, sequence: Option<AssetRef<AnimationSequenceAssetKind>>, looping: bool) {
        self.legacy_animation = sequence;
        self.legacy_looping = looping;
        self.attach_assets();
        self.playing = self.legacy_animation.is_some();
    }

    fn clear_mesh(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.skin_vertices.clear();
        self.bone_palette.clear();
        self.gpu_bone_palettes.clear();
        self.bone_world_pose.clear();
        self.mesh_sections.clear();
        self.bounds_local = [0.0, 0.0, 0.0, 0.5];
        self.geometry_key = MeshGeometryKey::NONE;
    }

    fn load_mesh(&mut self, asset: &skeletal_assets::skmesh::SkeletalMeshAsset) {
        self.clear_mesh();
        let Some(lod) = asset.lod0() else {
            return;
        };
        self.vertices = lod
            .vertices
            .iter()
            .map(|v| {
                let normal = glam::Vec3::from_array(v.normal).normalize_or_zero();
                PackedVertex::from_components(
                    v.position,
                    normal.to_array(),
                    v.uv,
                    [1.0, 0.0, 0.0],
                    1.0,
                )
            })
            .collect();
        self.indices = lod.indices.clone();
        self.skin_vertices = lod
            .vertices
            .iter()
            .map(|v| SkinVertex {
                joints: v.joints.map(u32::from),
                weights: v.weights,
            })
            .collect();
        self.mesh_sections = lod
            .sections
            .iter()
            .map(|s| MeshSection {
                first_index: s.first_index,
                index_count: s.index_count,
                material_slot: s.material_slot,
            })
            .collect();
        self.bounds_local = bounding_sphere(lod.vertices.iter().map(|v| v.position));
        // SceneDB's keyed mesh pools use this identity to make the geometry
        // resident exactly once per source mesh while poses change separately.
        let key = asset
            .id
            .to_string()
            .bytes()
            .fold(0xcbf29ce484222325u128, |h, b| {
                (h ^ u128::from(b)).wrapping_mul(0x100000001b3)
            })
            .max(1);
        self.geometry_key = MeshGeometryKey(key);
    }

    /// Bring the animation instance in line with the loaded assets. Returns
    /// whether a new playback started (the assets changed): unaccepted notify
    /// events of the old playback are discarded then.
    pub(super) fn ensure_runtime(&mut self) -> bool {
        let key = RuntimeKey {
            skeleton: ptr_of(&self.loaded_skeleton),
            graph: ptr_of(&self.loaded_graph),
            animation: ptr_of(&self.loaded_animation),
            looping: self.legacy_looping,
            reverse: self.playback_rate < 0.0,
        };
        if self.anim.key == Some(key) && self.anim.instance.is_some() {
            return false;
        }
        let previous = self.anim.key;
        // the runtime skeleton: the mesh's, or an empty one so an animation can
        // still run its clock (and notifies) before a mesh has loaded
        if self.anim.skeleton.is_none() || self.anim.skeleton_source != key.skeleton {
            let built = self.loaded_skeleton.as_ref().and_then(|asset| {
                RuntimeSkeleton::new(asset.id.as_str(), &asset.skeleton())
                    .map_err(|error| tracing::warn!(%error, "the skeleton cannot be used for animation"))
                    .ok()
            });
            let empty = || RuntimeSkeleton::new("", &skeletal_assets::skeleton::Skeleton::default()).expect("empty skeleton");
            self.anim.skeleton = Some(Arc::new(built.unwrap_or_else(empty)));
            self.anim.skeleton_source = key.skeleton;
            self.anim.montages.clear();
        }
        let skeleton = self.anim.skeleton.clone().expect("set above");
        let same_skeleton_now = previous.is_some_and(|p| p.skeleton == key.skeleton);

        let mut chosen: Option<Arc<CompiledGraph>> = None;
        let mut single = false;
        if let Some(graph) = &self.loaded_graph {
            if graph.skeleton.id == skeleton.id {
                chosen = Some(graph.clone());
            }
        }
        if chosen.is_none() {
            if let Some(sequence) = &self.loaded_animation {
                let fits = skeleton.is_empty() || sequence.skeleton.as_ref().is_none_or(|id| id.as_str() == skeleton.id);
                if fits {
                    let clip = match &self.anim.single_clip {
                        Some((ptr, clip)) if *ptr == key.animation && same_skeleton_now => clip.clone(),
                        _ => Arc::new(CompiledClip::compile_shared(sequence, &skeleton)),
                    };
                    self.anim.single_clip = Some((key.animation, clip.clone()));
                    chosen = Some(single_sequence_graph(skeleton.clone(), clip, self.legacy_looping, key.reverse));
                    single = true;
                }
            }
        }
        let same_skeleton = previous.is_some_and(|p| p.skeleton == key.skeleton);
        let sequence_replaced = single && previous.is_none_or(|p| p.animation != key.animation);
        let mut cancelled: Vec<MontageEvent> = Vec::new();
        let mut instance = match self.anim.instance.take() {
            Some(existing) if same_skeleton && !sequence_replaced => existing,
            old => {
                // the Montages of a discarded instance are cancelled, and say so
                if let Some(mut old) = old {
                    old.shutdown();
                    let events = std::mem::take(&mut old.report.montage_events);
                    self.anim.montage_events.extend(events.iter().cloned());
                    cancelled.extend(events);
                }
                AnimInstance::new(skeleton.clone())
            }
        };
        instance.set_root_motion_extraction(RootMotionMode::from(self.root_motion_mode).extracts());
        match chosen {
            Some(graph) => {
                if let Err(error) = instance.set_graph(graph) {
                    tracing::warn!(%error, "animation graph refused");
                    instance = AnimInstance::new(skeleton);
                }
            }
            None => instance.clear_graph(),
        }
        instance.refresh();
        if single {
            self.elapsed_seconds = 0.0;
        }
        self.anim.instance = Some(instance);
        self.anim.key = Some(key);
        self.notify_playback.clear_pending();
        self.notify_playback.next_generation();
        self.notify_playback.push_lifecycle_events(&cancelled);
        true
    }

    /// Publish the animation instance's current pose: the physics-facing
    /// mesh-space transforms and the current/previous GPU skin palettes.
    fn refresh_pose(&mut self) {
        self.reconcile_notify_playback();
        let (Some(mesh), Some(skeleton)) = (&self.loaded_mesh, &self.loaded_skeleton) else {
            self.bone_palette.clear();
            self.gpu_bone_palettes.clear();
            self.bone_world_pose.clear();
            return;
        };
        if mesh
            .skeleton
            .as_ref()
            .is_some_and(|id| id.as_str() != skeleton.id.as_str())
        {
            tracing::warn!(mesh = %mesh.id, skeleton = %skeleton.id, "skeletal mesh and skeleton references do not match");
            self.bone_palette.clear();
            self.gpu_bone_palettes.clear();
            self.bone_world_pose.clear();
            return;
        }
        let (Some(instance), Some(runtime)) = (&self.anim.instance, &self.anim.skeleton) else { return };
        let pose = instance.pose().clone();
        if pose.bones.len() != runtime.len() {
            self.bone_palette.clear();
            self.gpu_bone_palettes.clear();
            self.bone_world_pose.clear();
            return;
        }
        let mesh_space = runtime.to_mesh_space(&pose.bones);
        // what physics reads is the animation's own pose; bones physics owns replace
        // theirs only in what is drawn
        self.bone_world_pose = mesh_space.iter().map(|m| m.to_cols_array_2d()).collect();
        let drawn = if self.physics_bone_overrides.is_empty() {
            mesh_space
        } else {
            let owned: Vec<(usize, glam::Mat4)> = self.physics_bone_overrides.iter().map(|(bone, m)| (*bone, glam::Mat4::from_cols_array_2d(m))).collect();
            runtime.to_mesh_space_with_overrides(&pose.bones, &owned)
        };
        let drawn = match (&self.physics_pose, self.physics_blend_weight.unwrap_or(1.0)) {
            (Some(physics), weight) if physics.len() == drawn.len() && weight > 0.0 => drawn.iter().zip(physics).map(|(animated, owned)| blend_mesh_space(animated, owned, weight)).collect(),
            _ => drawn,
        };
        let world: HashMap<_, _> = runtime
            .ids
            .iter()
            .zip(&drawn)
            .map(|(id, m)| (id.clone(), skeletal_assets::math::Mat4(m.to_cols_array())))
            .collect();
        let palette = skeletal_assets::skinning::joint_matrices(mesh, &world);
        self.set_palette(
            palette
                .iter()
                .map(|m| glam::Mat4::from_cols_array(&m.0).to_cols_array_2d())
                .collect(),
        );
    }

    /// Recompute [`Self::diagnostics`] from the resolved links.
    pub fn refresh_diagnostics(&mut self) {
        self.diagnostics = link_diagnostics(
            self.mesh.as_ref(),
            self.mesh_asset_id.is_zero(),
            self.mesh_skeleton.as_ref(),
            self.legacy_skeleton.as_ref(),
            self.loaded_animation.as_deref().and_then(|a| a.skeleton.as_ref()).map(|id| id.as_str().to_string()),
            self.legacy_animation.is_some(),
        );
        self.diagnostics.extend(self.graph_diagnostics.iter().cloned());
        if self.animation_graph.is_some() && self.loaded_graph.is_some() {
            if self.legacy_animation.is_some() {
                self.diagnostics.push(
                    Diagnostic::warning(
                        "component.sequence_shadowed_by_graph",
                        "The component names an animation graph and an old single sequence; the graph plays and the sequence is ignored",
                    )
                    .at("animation")
                    .repair("Remove the single sequence or use it inside the graph"),
                );
            }
            if let (Some(graph), Some(skeleton)) = (&self.loaded_graph, &self.mesh_skeleton) {
                if graph.skeleton.id != skeleton.as_str() {
                    self.diagnostics.push(
                        Diagnostic::error("component.graph_skeleton_mismatch", "The animation graph is for a different skeleton than the mesh, so it is not run")
                            .at("animation_graph")
                            .repair("Pick a graph made for the mesh's skeleton"),
                    );
                }
            }
        }
        for d in &self.diagnostics {
            tracing::warn!(code = d.code, "{}", d.message);
        }
    }

    /// Replace the evaluated pose with articulated-body transforms in
    /// skeleton-local space, ordered like `loaded_skeleton.bones`. This also
    /// refreshes the GPU skin palette. Returns false if assets are not ready,
    /// the pose has the wrong number of bones, or any matrix is non-finite.
    /// While active, animation playback will not overwrite the physics pose.
    pub fn apply_physics_pose(&mut self, pose: Vec<[[f32; 4]; 4]>) -> bool {
        let (Some(_), Some(skeleton)) = (&self.loaded_mesh, &self.loaded_skeleton) else {
            return false;
        };
        if pose.len() != skeleton.bones.len() || pose.iter().flatten().flatten().any(|value| !value.is_finite()) {
            return false;
        }
        self.physics_pose = Some(pose.iter().map(glam::Mat4::from_cols_array_2d).collect());
        if self.anim.instance.is_some() {
            self.refresh_pose();
        } else {
            self.publish_physics_only();
        }
        true
    }

    /// The physics pose is the whole drawn pose when there is no animation instance to blend with.
    fn publish_physics_only(&mut self) {
        let (Some(mesh), Some(skeleton), Some(pose)) = (&self.loaded_mesh, &self.loaded_skeleton, &self.physics_pose) else { return };
        let world: HashMap<_, _> = skeleton
            .bones
            .iter()
            .zip(pose)
            .map(|(bone, matrix)| (bone.id.clone(), skeletal_assets::math::Mat4(matrix.to_cols_array())))
            .collect();
        let palette = skeletal_assets::skinning::joint_matrices(mesh, &world);
        self.bone_world_pose = pose.iter().map(|m| m.to_cols_array_2d()).collect();
        self.set_palette(palette.iter().map(|m| glam::Mat4::from_cols_array(&m.0).to_cols_array_2d()).collect());
    }

    /// How much of the drawn pose the whole-skeleton physics pose replaces, 0 (none) to 1
    /// (all, the default once a pose is applied). The animation advances at every weight, so
    /// handing the pose back is a blend, not a restart. False for a non-finite weight.
    pub fn set_physics_blend_weight(&mut self, weight: f32) -> bool {
        if !weight.is_finite() {
            return false;
        }
        self.physics_blend_weight = Some(weight.clamp(0.0, 1.0));
        if self.physics_pose.is_some() && self.anim.instance.is_some() {
            self.refresh_pose();
        }
        true
    }

    pub fn physics_blend_weight(&self) -> f32 {
        if self.physics_pose.is_some() { self.physics_blend_weight.unwrap_or(1.0) } else { 0.0 }
    }

    fn set_palette(&mut self, current: Vec<[[f32; 4]; 4]>) {
        let previous = if self.bone_palette.len() == current.len() {
            self.bone_palette.clone()
        } else {
            // New asset or first evaluation: no motion from an uninitialized pose.
            current.clone()
        };
        self.bone_palette = current;
        self.gpu_bone_palettes.clear();
        self.gpu_bone_palettes.extend_from_slice(&self.bone_palette);
        self.gpu_bone_palettes.extend_from_slice(&previous);
    }

    /// The policy the component applies to the root motion of each advance.
    pub fn root_motion_policy(&self) -> RootMotionPolicy {
        RootMotionPolicy {
            mode: self.root_motion_mode.into(),
            translation: [true, self.root_motion_vertical, true],
            rotation: self.root_motion_rotation.into(),
        }
    }

    /// The owner move the latest advance asks for, once: the engine applies it in
    /// the animation phase. `None` when there is nothing to apply.
    /// The moves logged since the last call, oldest first. Empty for roles that do not log.
    pub fn drain_root_motion_moves(&mut self) -> Vec<RootMotionMove> {
        std::mem::take(&mut self.root_motion_moves)
    }

    pub(crate) fn take_root_motion(&mut self) -> Option<RootMotion> {
        self.pending_root_motion.take().filter(|delta| !delta.is_identity())
    }

    // ---- physics ownership of bones ----------------------------------------------------

    /// Physics owns these bones: each entry is a skeleton bone index and its
    /// mesh-space matrix (column-major, like [`Self::bone_world_pose`]). They
    /// replace the animated bones in the published skin palette, their
    /// descendants follow them, and the animation keeps running for every other
    /// bone. `bone_world_pose` stays the animation's own pose (it is what physics
    /// reads next frame), so there is no feedback loop and no second writer: the
    /// rig calls this once per frame after the simulation, and this component is
    /// the only thing that writes the palette. An empty list returns every bone
    /// to the animation. False when the assets are not ready or an entry is bad.
    pub fn apply_physics_bones(&mut self, bones: Vec<(usize, [[f32; 4]; 4])>) -> bool {
        let Some(runtime) = self.anim.skeleton.as_ref() else { return false };
        if bones.iter().any(|(bone, m)| *bone >= runtime.len() || m.iter().flatten().any(|v| !v.is_finite())) {
            return false;
        }
        self.physics_bone_overrides = bones;
        self.refresh_pose();
        true
    }

    /// Number of bones physics owns through [`Self::apply_physics_bones`].
    pub fn physics_owned_bones(&self) -> usize {
        self.physics_bone_overrides.len()
    }

    /// Return pose ownership to the animation player and refresh its current
    /// frame immediately.
    pub fn clear_physics_pose(&mut self) {
        self.physics_pose = None;
        self.physics_blend_weight = None;
        self.physics_bone_overrides.clear();
        self.refresh_pose();
    }

    // ---- gameplay API: parameters and Montages ---------------------------------------

    /// The component's animation instance (graph state, parameters, Montages, pose).
    pub fn animation(&self) -> Option<&AnimInstance> {
        self.anim.instance.as_ref()
    }

    /// Write a graph parameter. Names and types are validated against the running
    /// graph; the value is read by the next animation phase.
    pub fn set_animation_parameter(&mut self, name: &str, value: AnimValue) -> Result<(), ParamError> {
        self.ensure_runtime();
        match self.anim.instance.as_mut() {
            Some(instance) => instance.set_parameter(name, value),
            None => Err(ParamError::Unknown(name.to_string())),
        }
    }

    pub fn set_animation_float(&mut self, name: &str, value: f32) -> Result<(), ParamError> {
        self.set_animation_parameter(name, AnimValue::Float(value))
    }
    pub fn set_animation_bool(&mut self, name: &str, value: bool) -> Result<(), ParamError> {
        self.set_animation_parameter(name, AnimValue::Bool(value))
    }
    pub fn set_animation_int(&mut self, name: &str, value: i32) -> Result<(), ParamError> {
        self.set_animation_parameter(name, AnimValue::Int(value))
    }
    pub fn set_animation_vector(&mut self, name: &str, value: [f32; 3]) -> Result<(), ParamError> {
        self.set_animation_parameter(name, AnimValue::Vector(glam::Vec3::from_array(value)))
    }

    /// Play a Montage asset on this component. The montage is compiled for the
    /// mesh's skeleton on first use and cached. Reads project files on a cache miss.
    pub fn play_montage(
        &mut self,
        montage: &AssetRef<AnimationMontageAssetKind>,
        rate: f32,
        section: Option<&str>,
    ) -> Result<MontageHandle, String> {
        self.ensure_runtime();
        let skeleton = self.anim.skeleton.clone().ok_or("the component has no skeleton to pose")?;
        let compiled = match self.anim.montages.get(montage.as_str()) {
            Some(m) => m.clone(),
            None => {
                let source = ProjectAnimSource::open().ok_or("no project is open")?;
                let m = compile_montage(montage.as_str(), &skeleton, &source)
                    .map_err(|d| d.into_iter().map(|d| d.message).collect::<Vec<_>>().join("; "))?;
                self.anim.montages.insert(montage.as_str().to_string(), m.clone());
                m
            }
        };
        self.play_compiled_montage(compiled, rate, section).map_err(|e| format!("{e:?}"))
    }

    /// Play an already compiled Montage (hosts that load assets themselves).
    pub fn play_compiled_montage(
        &mut self,
        montage: Arc<CompiledMontage>,
        rate: f32,
        section: Option<&str>,
    ) -> Result<MontageHandle, PlayRefused> {
        self.ensure_runtime();
        let instance = self.anim.instance.as_mut().ok_or(PlayRefused::InvalidRate)?;
        instance.montages.play(montage, rate, section)
    }

    pub fn stop_montage(&mut self, handle: MontageHandle, blend_out: Option<f32>) -> bool {
        self.anim.instance.as_mut().is_some_and(|i| i.montages.stop(handle, blend_out))
    }

    pub fn stop_montage_group(&mut self, group: &str, blend_out: Option<f32>) -> usize {
        self.anim.instance.as_mut().map_or(0, |i| i.montages.stop_group(group, blend_out))
    }

    pub fn pause_montage(&mut self, handle: MontageHandle, paused: bool) -> bool {
        self.anim.instance.as_mut().is_some_and(|i| i.montages.pause(handle, paused))
    }

    pub fn set_montage_rate(&mut self, handle: MontageHandle, rate: f32) -> Result<(), PlayRefused> {
        self.anim.instance.as_mut().ok_or(PlayRefused::InvalidRate)?.montages.set_rate(handle, rate)
    }

    pub fn jump_to_montage_section(&mut self, handle: MontageHandle, section: &str) -> Result<(), PlayRefused> {
        self.anim.instance.as_mut().ok_or_else(|| PlayRefused::UnknownSection(section.to_string()))?.montages.jump_to_section(handle, section)
    }

    pub fn set_montage_next_section(&mut self, handle: MontageHandle, section: &str, next: &str) -> Result<(), PlayRefused> {
        self.anim.instance.as_mut().ok_or_else(|| PlayRefused::UnknownSection(section.to_string()))?.montages.set_next_section(handle, section, next)
    }

    /// Montage lifecycle events (started, section changes, blend out, result) since
    /// the last call, in order. Delivery as component-scoped Blueprint events is Phase 3.
    pub fn take_montage_events(&mut self) -> Vec<MontageEvent> {
        std::mem::take(&mut self.anim.montage_events)
    }

    /// State machine and IK debug lines of the latest animation update.
    pub fn animation_trace(&self) -> &[String] {
        self.anim.instance.as_ref().map_or(&[], |i| i.report.trace.as_slice())
    }

    /// Root motion produced by the latest animation update, in the owner's space.
    /// The engine applies the part the component's root motion mode selects to the
    /// owner's transform in the animation phase (see `root_motion_policy`).
    pub fn animation_root_motion(&self) -> skeletal_assets::runtime::pose::RootMotion {
        self.anim.instance.as_ref().map_or(skeletal_assets::runtime::pose::RootMotion::IDENTITY, |i| i.report.root_motion)
    }
}

fn bounding_sphere(positions: impl Iterator<Item = [f32; 3]>) -> [f32; 4] {
    let points: Vec<_> = positions.map(glam::Vec3::from_array).collect();
    if points.is_empty() {
        return [0.0, 0.0, 0.0, 0.5];
    }
    let min = points
        .iter()
        .copied()
        .reduce(glam::Vec3::min)
        .unwrap_or(glam::Vec3::ZERO);
    let max = points
        .iter()
        .copied()
        .reduce(glam::Vec3::max)
        .unwrap_or(glam::Vec3::ZERO);
    let center = (min + max) * 0.5;
    let radius = points
        .iter()
        .map(|p| p.distance(center))
        .fold(0.0, f32::max);
    [center.x, center.y, center.z, radius]
}

/// Problems with a component's asset links, as repairable diagnostics.
///
/// `animation_skeleton` is the skeleton id of the loaded animation, if loaded.
pub(crate) fn link_diagnostics(
    mesh: Option<&AssetRef<SkeletalMeshAssetKind>>,
    mesh_unresolved: bool,
    mesh_skeleton: Option<&AssetRef<SkeletonAssetKind>>,
    legacy_skeleton: Option<&AssetRef<SkeletonAssetKind>>,
    animation_skeleton: Option<String>,
    has_animation: bool,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if mesh.is_some() && mesh_unresolved {
        out.push(
            Diagnostic::error("component.mesh_unresolved", "The skeletal mesh is not in this project's asset index")
                .at("mesh")
                .repair("Pick the mesh again or restore the missing file"),
        );
    } else if mesh.is_some() && mesh_skeleton.is_none() {
        out.push(
            Diagnostic::error("component.mesh_without_skeleton", "The skeletal mesh names no skeleton, so it cannot be posed")
                .at("mesh")
                .repair("Bind the mesh to a skeleton in the Skeletal Mesh editor"),
        );
    }
    if let Some(legacy) = legacy_skeleton {
        if mesh_skeleton.is_some_and(|m| m != legacy) {
            out.push(
                Diagnostic::warning(
                    "component.legacy_skeleton_disagrees",
                    "This component was saved with a skeleton that differs from its mesh's; the mesh's skeleton is used and the old value is dropped on the next save",
                )
                .at("skeleton")
                .repair("Check that the mesh is the one you meant, then save the scene"),
            );
        }
    }
    if has_animation {
        if let (Some(animation), Some(mesh)) = (animation_skeleton, mesh_skeleton) {
            if animation != mesh.as_str() {
                out.push(
                    Diagnostic::error(
                        "component.animation_skeleton_mismatch",
                        "The animation is for a different skeleton than the mesh, so it is not played",
                    )
                    .at("animation")
                    .repair("Pick an animation made for the mesh's skeleton"),
                );
            }
        }
    }
    out
}

/// The skeleton the mesh at `mesh` is bound to, read from its header only.
fn mesh_skeleton_ref(mesh: Option<&AssetRef<SkeletalMeshAssetKind>>) -> Option<AssetRef<SkeletonAssetKind>> {
    let path = path_for_ref(mesh?)?;
    match skeletal_assets::skmesh::SkeletalMeshAsset::peek_header_at(&path) {
        Ok((_, skeleton)) => skeleton.map(|s| s.to_ref()),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "could not read the skeletal mesh header");
            None
        }
    }
}

fn skeleton_id(reference: Option<&AssetRef<SkeletonAssetKind>>) -> Option<String> {
    reference.map(|reference| reference.as_str().to_owned())
}

/// The project file a typed reference resolves to, through the file index.
fn path_for_ref<K: AssetKind>(reference: &AssetRef<K>) -> Option<PathBuf> {
    let root = PathBuf::from(engine_state::get_project_path()?);
    let path = engine_fs::open_project_file_index(&root)
        .ok()
        .flatten()
        .and_then(|index| index.path_for_asset_id_kind(K::KIND, reference.as_str()).ok().flatten());
    let Some(path) = path else {
        tracing::warn!(kind = K::KIND, id = reference.as_str(), "skeletal asset id is not indexed in this project");
        return None;
    };
    let full_path = root.join(&path);
    Some(std::fs::canonicalize(&full_path).unwrap_or(full_path))
}

fn asset_id_for_ref<K: AssetKind>(reference: Option<&AssetRef<K>>) -> AssetId {
    let Some(path) = reference.and_then(path_for_ref) else {
        return AssetId::ZERO;
    };
    AssetId::declare(K::KIND, &path.to_string_lossy())
}

pub fn register_skeletal_mesh_assets() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        pulsar_scenedb::register_loader(SKELETAL_MESH_ASSET_KIND, load_skeletal_mesh);
        pulsar_scenedb::register_loader(SKELETON_ASSET_KIND, load_skeleton);
        pulsar_scenedb::register_loader(ANIMATION_SEQUENCE_ASSET_KIND, load_animation_sequence);
        pulsar_scenedb::register_loader(ANIMATION_GRAPH_ASSET_KIND, load_animation_graph);
        pulsar_scenedb::register_binding::<SkeletalMeshComponent>(
            SKELETAL_MESH_ASSET_KIND,
            bind_skeletal_mesh,
        );
        pulsar_scenedb::register_binding::<SkeletalMeshComponent>(
            SKELETON_ASSET_KIND,
            bind_skeleton,
        );
        pulsar_scenedb::register_binding::<SkeletalMeshComponent>(
            ANIMATION_SEQUENCE_ASSET_KIND,
            bind_animation_sequence,
        );
        pulsar_scenedb::register_binding::<SkeletalMeshComponent>(
            ANIMATION_GRAPH_ASSET_KIND,
            bind_animation_graph,
        );
    });
}

fn load_skeletal_mesh(source: &str) -> Result<AssetPayload, String> {
    let bytes = engine_fs::virtual_fs::read_file(Path::new(source))
        .map_err(|e| format!("could not read skeletal mesh '{source}': {e}"))?;
    let mesh = skeletal_assets::skmesh::SkeletalMeshAsset::decode(&bytes)
        .map_err(|e| format!("could not decode skeletal mesh '{source}': {e}"))?;
    Ok(Arc::new(LoadedSkeletalMesh(Arc::new(mesh))))
}

fn load_skeleton(source: &str) -> Result<AssetPayload, String> {
    let skeleton = skeletal_assets::assets::load_skeleton_asset(Path::new(source))
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(LoadedSkeleton(Arc::new(skeleton))))
}

fn load_animation_sequence(source: &str) -> Result<AssetPayload, String> {
    let animation = notifies::load_sequence_for_events(Path::new(source))?;
    Ok(Arc::new(LoadedAnimationSequence(Arc::new(animation))))
}

/// Compile the graph with everything it names, in the project the engine has open.
/// A graph that does not compile still loads: the payload carries the diagnostics.
fn compile_graph_file(source: &str, project: &ProjectAnimSource) -> Result<LoadedAnimationGraph, String> {
    let text = skeletal_assets::assets::read_text(Path::new(source)).map_err(|e| format!("could not read animation graph '{source}': {e}"))?;
    let asset = skeletal_assets::anim_graph::AnimationGraphAsset::parse(&text).map_err(|e| format!("could not decode animation graph '{source}': {e}"))?;
    let out = compile_graph(&asset, project);
    Ok(LoadedAnimationGraph { graph: out.graph, diagnostics: out.diagnostics })
}

fn load_animation_graph(source: &str) -> Result<AssetPayload, String> {
    let project = ProjectAnimSource::open().ok_or("no project is open to resolve the graph's assets")?;
    compile_graph_file(source, &project).map(|loaded| Arc::new(loaded) as AssetPayload)
}

/// Canonical source paths of the assets a compiled graph depends on.
fn dependency_paths(graph: &CompiledGraph) -> Vec<String> {
    let Some(root) = engine_state::get_project_path().map(PathBuf::from) else { return Vec::new() };
    dependency_paths_in(&root, graph)
}

fn dependency_paths_in(root: &Path, graph: &CompiledGraph) -> Vec<String> {
    let Some(index) = engine_fs::open_project_file_index(root).ok().flatten() else { return Vec::new() };
    let mut paths: Vec<String> = graph
        .dependencies
        .iter()
        .filter_map(|(kind, id)| index.path_for_asset_id_kind(kind, id).ok().flatten())
        .map(|path| animation_source_for_file(&root.join(path)))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// An animation asset file changed on disk (the engine's asset-update path calls
/// this with its path). Every component whose running graph depends on the file
/// recompiles that graph in place: node state (clocks, state machine states,
/// parameters) survives for nodes that still exist, notify states that were
/// active end with the reason `reload`, and Montages compile afresh the next
/// time they are played (an instance already playing finishes with the version it
/// started with). Returns how many components recompiled. The graph file itself
/// and sequences held directly by a component reload through SceneDB
/// (`World::asset_source_changed`).
pub fn reload_animation_dependencies(world: &mut World, changed: &Path) -> usize {
    reload_dependants(world, changed, SkeletalMeshComponent::recompile_graph)
}

fn reload_dependants(world: &mut World, changed: &Path, mut recompile: impl FnMut(&mut SkeletalMeshComponent) -> bool) -> usize {
    let changed = animation_source_for_file(changed);
    let mut recompiled = 0;
    for (_, player) in world.query::<&mut SkeletalMeshComponent>() {
        player.anim.montages.clear();
        if player.graph_dependency_paths.contains(&changed) && recompile(player) {
            recompiled += 1;
        }
    }
    recompiled
}

fn path_for_ref_opt<K: AssetKind>(reference: Option<&AssetRef<K>>) -> Option<String> {
    path_for_ref(reference?).map(|path| animation_source_for_file(&path))
}

impl SkeletalMeshComponent {
    /// Recompile the running graph from its file and swap it in place. False when
    /// there is no graph file to read or it does not compile (the running graph
    /// then stays as it was, and the diagnostics say why).
    pub(crate) fn recompile_graph(&mut self) -> bool {
        match ProjectAnimSource::open() {
            Some(project) => self.recompile_graph_in(&project),
            None => false,
        }
    }

    /// [`Self::recompile_graph`] against a given project.
    pub(crate) fn recompile_graph_in(&mut self, project: &ProjectAnimSource) -> bool {
        let Some(source) = self.graph_source_path.clone() else { return false };
        match compile_graph_file(&source, project) {
            Ok(loaded) => {
                let compiled = loaded.graph.is_some();
                self.graph_diagnostics = loaded.diagnostics;
                if compiled {
                    self.graph_dependency_paths = loaded.graph.as_deref().map(|g| dependency_paths_in(project.root(), g)).unwrap_or_default();
                    self.loaded_graph = loaded.graph;
                    // swaps the graph into the running instance and ends notify states as `reload`
                    self.refresh_pose();
                }
                self.refresh_diagnostics();
                compiled
            }
            Err(error) => {
                tracing::warn!(%error, "could not recompile the animation graph after an asset changed");
                false
            }
        }
    }

    /// Resolve a Montage asset the way Blueprint nodes name it: a stable asset id,
    /// or the file name of a Montage in the project when it is unique.
    fn montage_ref(name: &str) -> Result<AssetRef<AnimationMontageAssetKind>, String> {
        if let Some(reference) = AssetRef::parse(name) {
            return Ok(reference);
        }
        let root = engine_state::get_project_path().map(PathBuf::from).ok_or("no project is open")?;
        let index = engine_fs::open_project_file_index(&root).ok().flatten().ok_or("the project has no asset index")?;
        let mut found = index
            .assets_of_kind(AnimationMontageAssetKind::KIND)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, path)| Path::new(path).file_stem().is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(name)));
        match (found.next(), found.next()) {
            (Some((id, _)), None) => AssetRef::parse(&id).ok_or_else(|| format!("`{id}` is not a valid asset id")),
            (None, _) => Err(format!("no Montage named `{name}` in this project")),
            _ => Err(format!("more than one Montage is named `{name}`; use its asset id")),
        }
    }
}

/// The skeletal mesh component a script or Blueprint reference names: the
/// instance itself, or the first one on its owner object.
fn player_instance(world: &World, entity: Entity) -> Result<Entity, String> {
    if world.get::<SkeletalMeshComponent>(entity).is_some() {
        return Ok(entity);
    }
    let owner = pulsar_scene_model::attachments::owner_of(world, entity).unwrap_or(entity);
    pulsar_world_registry::instances::resolve_instance(world, owner, "SkeletalMeshComponent", 0).ok_or_else(|| "the object has no skeletal mesh component".into())
}

fn player_mut(world: &mut World, entity: Entity) -> Result<pulsar_scenedb::Mut<'_, SkeletalMeshComponent>, String> {
    let instance = player_instance(world, entity)?;
    world.get_mut::<SkeletalMeshComponent>(instance).ok_or_else(|| "the object has no skeletal mesh component".into())
}

fn montage_handle(instance: i64) -> Result<MontageHandle, String> {
    u64::try_from(instance).map(MontageHandle).map_err(|_| "a Montage instance is a non-negative handle".to_string())
}

fn param_error(error: ParamError) -> String {
    match error {
        ParamError::Unknown(name) => format!("the animation graph has no parameter `{name}`"),
        ParamError::WrongType { name, expected } => format!("graph parameter `{name}` is a {expected:?}, not the value type of this node"),
        ParamError::NotFinite(name) => format!("graph parameter `{name}` needs a finite value"),
    }
}

fn refused(error: PlayRefused) -> String {
    match error {
        PlayRefused::GroupBusy { by } => format!("the Montage group is busy (instance {})", by.0),
        PlayRefused::LowerPriority { by } => format!("Montage instance {} has equal or higher priority", by.0),
        PlayRefused::UnknownSection(name) => format!("the Montage has no section `{name}`"),
        PlayRefused::InvalidRate => "a Montage rate must be finite and non-zero".to_string(),
    }
}

/// Blueprint and script surface of the animation instance. Every node validates
/// its input and reports a precise error; commands apply in the next animation
/// phase, in call order.
#[pulsar_scenedb::component_methods]
impl SkeletalMeshComponent {
    /// Set a float parameter of the running animation graph.
    #[world_method(category = "Animation|Graph")]
    fn anim_set_float(world: &mut World, entity: Entity, name: String, value: f32) -> Result<(), String> {
        player_mut(world, entity)?.set_animation_float(&name, value).map_err(param_error)
    }

    /// Set a bool parameter of the running animation graph.
    #[world_method(category = "Animation|Graph")]
    fn anim_set_bool(world: &mut World, entity: Entity, name: String, value: bool) -> Result<(), String> {
        player_mut(world, entity)?.set_animation_bool(&name, value).map_err(param_error)
    }

    /// Set an int parameter of the running animation graph.
    #[world_method(category = "Animation|Graph")]
    fn anim_set_int(world: &mut World, entity: Entity, name: String, value: i32) -> Result<(), String> {
        player_mut(world, entity)?.set_animation_int(&name, value).map_err(param_error)
    }

    /// Set a vector parameter of the running animation graph.
    #[world_method(category = "Animation|Graph")]
    fn anim_set_vector(world: &mut World, entity: Entity, name: String, value: glam::Vec3) -> Result<(), String> {
        player_mut(world, entity)?.set_animation_vector(&name, value.to_array()).map_err(param_error)
    }

    /// Play a Montage (an asset id, or the file name of a unique Montage) from its
    /// first section or from `section`. Returns the instance handle the other
    /// Montage nodes take. Fails when the group's interruption policy refuses it.
    #[world_method(category = "Animation|Montage")]
    fn anim_play_montage(world: &mut World, entity: Entity, montage: String, rate: f32, section: String) -> Result<i64, String> {
        let reference = SkeletalMeshComponent::montage_ref(&montage)?;
        let mut player = player_mut(world, entity)?;
        let section = (!section.is_empty()).then_some(section.as_str());
        player.play_montage(&reference, rate, section).map(|handle| handle.0 as i64)
    }

    /// Stop one Montage instance, blending out over `blend_out` seconds (negative
    /// uses the Montage's own blend-out).
    #[world_method(category = "Animation|Montage")]
    fn anim_stop_montage(world: &mut World, entity: Entity, instance: i64, blend_out: f32) -> Result<bool, String> {
        let handle = montage_handle(instance)?;
        Ok(player_mut(world, entity)?.stop_montage(handle, (blend_out >= 0.0 && blend_out.is_finite()).then_some(blend_out)))
    }

    /// Stop every Montage of a group. Returns how many were stopped.
    #[world_method(category = "Animation|Montage")]
    fn anim_stop_montage_group(world: &mut World, entity: Entity, group: String, blend_out: f32) -> Result<i64, String> {
        let blend = (blend_out >= 0.0 && blend_out.is_finite()).then_some(blend_out);
        Ok(player_mut(world, entity)?.stop_montage_group(&group, blend) as i64)
    }

    /// Pause or resume one Montage instance (its clock and notifies hold).
    #[world_method(category = "Animation|Montage")]
    fn anim_pause_montage(world: &mut World, entity: Entity, instance: i64, paused: bool) -> Result<bool, String> {
        let handle = montage_handle(instance)?;
        Ok(player_mut(world, entity)?.pause_montage(handle, paused))
    }

    /// Jump a Montage instance to the start of a section.
    #[world_method(category = "Animation|Montage")]
    fn anim_jump_to_section(world: &mut World, entity: Entity, instance: i64, section: String) -> Result<(), String> {
        let handle = montage_handle(instance)?;
        player_mut(world, entity)?.jump_to_montage_section(handle, &section).map_err(refused)
    }

    /// Change which section follows `section` for this Montage instance only.
    #[world_method(category = "Animation|Montage")]
    fn anim_set_next_section(world: &mut World, entity: Entity, instance: i64, section: String, next: String) -> Result<(), String> {
        let handle = montage_handle(instance)?;
        player_mut(world, entity)?.set_montage_next_section(handle, &section, &next).map_err(refused)
    }

    /// Set the play rate of a Montage instance; negative plays it backwards.
    #[world_method(category = "Animation|Montage")]
    fn anim_set_montage_rate(world: &mut World, entity: Entity, instance: i64, rate: f32) -> Result<(), String> {
        let handle = montage_handle(instance)?;
        player_mut(world, entity)?.set_montage_rate(handle, rate).map_err(refused)
    }

    /// Whether an instance is still playing (blending in, playing or blending out).
    #[world_method(pure, category = "Animation|Montage")]
    fn anim_is_montage_playing(world: &World, entity: Entity, instance: i64) -> Result<bool, String> {
        let handle = montage_handle(instance)?;
        let instance = player_instance(world, entity)?;
        let player = world.get::<SkeletalMeshComponent>(instance).ok_or("the object has no skeletal mesh component")?;
        Ok(player.animation().is_some_and(|a| a.montages.get(handle).is_some()))
    }
}

fn bind_skeletal_mesh(component: &mut SkeletalMeshComponent, view: &AssetView) {
    if view.id != component.mesh_asset_id {
        return;
    }
    if let Some(loaded) = view.payload::<LoadedSkeletalMesh>() {
        component.load_mesh(&loaded.0);
        component.loaded_mesh = Some(loaded.0.clone());
    } else {
        component.loaded_mesh = None;
        component.clear_mesh();
    }
    component.refresh_pose();
}

fn bind_skeleton(component: &mut SkeletalMeshComponent, view: &AssetView) {
    if view.id != component.skeleton_asset_id {
        return;
    }
    component.loaded_skeleton = view
        .payload::<LoadedSkeleton>()
        .map(|loaded| loaded.0.clone());
    component.refresh_pose();
}

fn bind_animation_sequence(component: &mut SkeletalMeshComponent, view: &AssetView) {
    if view.id != component.animation_asset_id {
        return;
    }
    component.loaded_animation = view
        .payload::<LoadedAnimationSequence>()
        .map(|loaded| loaded.0.clone());
    let _ = component.seek_animation(0.0);
}

fn bind_animation_graph(component: &mut SkeletalMeshComponent, view: &AssetView) {
    if view.id != component.animation_graph_id {
        return;
    }
    match view.payload::<LoadedAnimationGraph>() {
        Some(loaded) => {
            component.loaded_graph = loaded.graph.clone();
            component.graph_diagnostics = loaded.diagnostics.clone();
            component.graph_dependency_paths = loaded.graph.as_deref().map(dependency_paths).unwrap_or_default();
            component.graph_source_path = path_for_ref_opt(component.animation_graph.as_ref());
        }
        None => {
            component.loaded_graph = None;
            component.graph_diagnostics.clear();
            component.graph_dependency_paths.clear();
        }
    }
    component.refresh_pose();
    component.refresh_diagnostics();
}

/// Handle-ledger collection for the stable references.
pub(crate) fn collect_skeletal_assets(
    component: &SkeletalMeshComponent,
    out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>,
) {
    register_skeletal_mesh_assets();
    for id in [
        asset_id_for_ref(component.mesh.as_ref()),
        asset_id_for_ref(mesh_skeleton_ref(component.mesh.as_ref()).as_ref()),
        asset_id_for_ref(component.legacy_animation.as_ref()),
        asset_id_for_ref(component.animation_graph.as_ref()),
    ] {
        out.push(id.handle());
    }
}

fn decode_skeletal_mesh_component(data: &Value) -> Result<SkeletalMeshComponent, String> {
    let mut component: SkeletalMeshComponent = pulsar_world_registry::decode_json(data)?;
    // Scenes saved before the mesh became the only skeleton authority carry a
    // `skeleton` ref. It is not part of the component any more; if it disagrees
    // with the mesh the user is told, and the next save drops it.
    component.legacy_skeleton = data
        .get("skeleton")
        .and_then(Value::as_str)
        .and_then(AssetRef::parse);
    component.attach_assets();
    Ok(component)
}

fn skeletal_mesh_property_written(component: &mut SkeletalMeshComponent, property: Option<&str>) {
    if matches!(
        property,
        None | Some("mesh") | Some("animation") | Some("animation_graph")
    ) {
        component.attach_assets();
    }
    component.refresh_diagnostics();
}

#[register_component_runtime]
impl SkeletalMeshComponent {
    fn begin_play(&mut self, _context: &mut pulsar_world_registry::ComponentContext) {
        // playback itself begins in the animation phase, which has already run this frame
        self.reconcile_notify_playback();
    }

    /// Lifecycle only: the animation advances in the engine's animation phase
    /// ([`run_animation_phase`]), before physics, not here.
    fn tick(&mut self, context: &mut pulsar_world_registry::ComponentContext, _delta_seconds: f32) {
        self.reconcile_notify_playback();
        if let Err(error) = self.register_notify_events(context.events.hub()) {
            tracing::warn!(%error, "could not register animation notifies");
        }
    }
}

fn skeletal_mesh_draw_dispatch(
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
    row: u32,
    data: *const (),
    is_new_insert: bool,
) {
    // SAFETY: SceneDB invokes the registration only for this component id.
    let component = unsafe { &*(data as *const SkeletalMeshComponent) };
    let sections = component
        .mesh_sections
        .iter()
        .map(|section| {
            let surface = crate::mesh_cache::ImportedSurfaceMaterial::default();
            MeshSectionDraw {
                first_index: section.first_index,
                index_count: section.index_count,
                material_class: 0,
                graph_hash_lo: 0,
                graph_hash_hi: 0,
                _pad: [0; 3],
                material: crate::components::surface_material(&surface, [u32::MAX; 5]),
            }
        })
        .collect::<Vec<_>>();
    let draw = StaticMeshDraw {
        bounds_local: component.bounds_local,
        flags: helio::INSTANCE_FLAG_MOVABLE,
        sections,
    };
    pulsar_scenedb::gpu::write_derived_row(mirror, row, &draw, is_new_insert);
}

fn skeletal_mesh_draw_clear(mirror: &pulsar_scenedb::gpu::GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<StaticMeshDraw>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<SkeletalMeshComponent>,
        dispatch: skeletal_mesh_draw_dispatch,
    }
}
pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<SkeletalMeshComponent>,
        clear: skeletal_mesh_draw_clear,
    }
}

#[cfg(test)]
mod authority_tests {
    use super::*;

    const MESH: &str = "11111111-1111-4111-8111-111111111111";
    const SKELETON_A: &str = "aaaaaaaa-0000-4000-8000-00000000000a";
    const SKELETON_B: &str = "bbbbbbbb-0000-4000-8000-00000000000b";

    fn skeleton(id: &str) -> Option<AssetRef<SkeletonAssetKind>> {
        AssetRef::parse(id)
    }

    fn codes(d: &[Diagnostic]) -> Vec<&'static str> {
        d.iter().map(|d| d.code).collect()
    }

    #[test]
    fn the_component_has_no_authored_skeleton_and_old_values_do_not_come_back() {
        let old = serde_json::json!({
            "mesh": MESH,
            "skeleton": SKELETON_B,
            "animation": null,
            "playing": true,
            "looping": false,
            "playback_rate": 1.0
        });
        let component = decode_skeletal_mesh_component(&old).expect("old scenes still load");
        assert_eq!(component.mesh.as_ref().map(|m| m.as_str()), Some(MESH));
        let saved = serde_json::to_value(&component).unwrap();
        assert!(saved.get("skeleton").is_none(), "the skeleton is no longer written: {saved}");
        assert_eq!(saved["mesh"], MESH);
        // and a fresh component never writes one either
        assert!(serde_json::to_value(SkeletalMeshComponent::default()).unwrap().get("skeleton").is_none());
    }

    #[test]
    fn an_old_skeleton_that_disagrees_with_the_mesh_is_a_repairable_diagnostic() {
        let mesh: AssetRef<SkeletalMeshAssetKind> = AssetRef::parse(MESH).unwrap();
        let agree = link_diagnostics(Some(&mesh), false, skeleton(SKELETON_A).as_ref(), skeleton(SKELETON_A).as_ref(), None, false);
        assert!(agree.is_empty(), "{agree:?}");
        let disagree = link_diagnostics(Some(&mesh), false, skeleton(SKELETON_A).as_ref(), skeleton(SKELETON_B).as_ref(), None, false);
        assert_eq!(codes(&disagree), ["component.legacy_skeleton_disagrees"]);
        assert!(disagree[0].repair.is_some());
        assert_eq!(disagree[0].severity, skeletal_assets::issue::Severity::Warning);
        // no legacy value, no diagnostic
        assert!(link_diagnostics(Some(&mesh), false, skeleton(SKELETON_A).as_ref(), None, None, false).is_empty());
    }

    #[test]
    fn unresolved_meshes_meshes_without_skeletons_and_foreign_animations_are_reported() {
        let mesh: AssetRef<SkeletalMeshAssetKind> = AssetRef::parse(MESH).unwrap();
        assert_eq!(codes(&link_diagnostics(Some(&mesh), true, None, None, None, false)), ["component.mesh_unresolved"]);
        assert_eq!(codes(&link_diagnostics(Some(&mesh), false, None, None, None, false)), ["component.mesh_without_skeleton"]);
        let wrong = link_diagnostics(Some(&mesh), false, skeleton(SKELETON_A).as_ref(), None, Some(SKELETON_B.into()), true);
        assert_eq!(codes(&wrong), ["component.animation_skeleton_mismatch"]);
        let right = link_diagnostics(Some(&mesh), false, skeleton(SKELETON_A).as_ref(), None, Some(SKELETON_A.into()), true);
        assert!(right.is_empty());
        // no mesh at all is not an error: the component is simply empty
        assert!(link_diagnostics(None, true, None, None, None, false).is_empty());
    }
}

/// A mesh-space matrix between `animated` and `owned`: translation and scale lerp, rotation slerp.
fn blend_mesh_space(animated: &glam::Mat4, owned: &glam::Mat4, weight: f32) -> glam::Mat4 {
    if weight >= 1.0 - 1e-6 {
        return *owned;
    }
    let (sa, ra, ta) = animated.to_scale_rotation_translation();
    let (so, ro, to) = owned.to_scale_rotation_translation();
    glam::Mat4::from_scale_rotation_translation(sa.lerp(so, weight), ra.slerp(ro, weight), ta.lerp(to, weight))
}

#[cfg(test)]
mod animation_runtime_tests {
    use super::*;
    use pulsar_events::{gamma::{Channel, DynValue, SubscribeOptions}, EventHub, FlushPoint};
    use pulsar_scenedb::World;
    use pulsar_world_registry::ComponentRuntimeState;
    use serde_json::json;
    use skeletal_assets::anim_graph::builder::GraphBuilder;
    use skeletal_assets::anim_graph::catalogue::ParamType;
    use skeletal_assets::anim_graph::{AnimationGraphAsset, GraphParameter};
    use skeletal_assets::assets::{AnimSeqAsset, SkeletonAsset};
    use skeletal_assets::runtime::graph::compile::compile_graph;
    use skeletal_assets::runtime::memory::MemorySource;
    use std::sync::Mutex;

    struct Rig {
        skeleton: Arc<SkeletonAsset>,
        source: MemorySource,
    }

    /// Three-bone skeleton, one sequence "Walk" (1 s, notify "Step" at 0.5) that
    /// slides bone b1 along x, and a graph playing it at a `speed`-controlled rate.
    fn rig() -> (Rig, Arc<CompiledGraph>, String) {
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
        let walk: AnimSeqAsset = serde_json::from_value(json!({
            "version": 2, "skeleton": skeleton_id, "name": "Walk", "duration": 1.0, "fps": 30.0,
            "tracks": [{"bone_id": "b1", "keyframes": [
                {"time": 0.0, "transform": {"translation": {"x": 0.0, "y": 0.0, "z": 0.0}, "rotation": {"x": 0.0, "y": 0.0, "z": 0.0}, "scale": {"x": 1.0, "y": 1.0, "z": 1.0}}},
                {"time": 1.0, "transform": {"translation": {"x": 4.0, "y": 0.0, "z": 0.0}, "rotation": {"x": 0.0, "y": 0.0, "z": 0.0}, "scale": {"x": 1.0, "y": 1.0, "z": 1.0}}}
            ]}],
            "notifies": [{"id": "step", "name": "Step", "time": 0.5}]
        }))
        .unwrap();
        let walk_id = walk.id.as_str().to_string();
        let mut source = MemorySource::default();
        source.skeletons.insert(skeleton_id.clone(), skeleton.clone());
        source.sequences.insert(walk_id.clone(), Arc::new(walk));
        let mut asset = AnimationGraphAsset::default();
        asset.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(&skeleton_id).unwrap().to_ref());
        asset.parameters.push(GraphParameter::new("speed", ParamType::Float));
        let mut b = GraphBuilder::new("main", &[("speed", ParamType::Float)]);
        b.node("out", "anim.output_pose", &[]);
        b.node("p", "anim.sequence_player", &[("sequence", json!(walk_id))]);
        b.node("s", "anim.get_parameter", &[("parameter", json!("speed"))]);
        b.wire("s", "p", "play_rate").wire("p", "out", "pose");
        asset.main = b.into_graph();
        let out = compile_graph(&asset, &source);
        let graph = out.graph.unwrap_or_else(|| panic!("{:#?}", out.diagnostics));
        (Rig { skeleton, source }, graph, walk_id)
    }

    fn graph_component(rig: &Rig, graph: &Arc<CompiledGraph>) -> SkeletalMeshComponent {
        let mut c = SkeletalMeshComponent::default();
        c.loaded_skeleton = Some(rig.skeleton.clone());
        c.loaded_graph = Some(graph.clone());
        c.playing = true;
        c.playback_rate = 1.0;
        c
    }

    fn registered_hub() -> EventHub {
        let hub = EventHub::new();
        pulsar_world_registry::register_component_events(&hub).unwrap();
        hub
    }

    fn frame(world: &mut World, hub: &EventHub, dt: f32, state: &mut ComponentRuntimeState) {
        run_animation_phase(world, dt);
        pulsar_world_registry::tick_live_components(world, hub, dt, 1, state);
        flush_animation_notifies(world, hub, |_, _| None);
        hub.flush(FlushPoint::AfterScripts);
    }

    fn x1(world: &World, e: pulsar_scenedb::Entity) -> f32 {
        world.get::<SkeletalMeshComponent>(e).unwrap().animation().unwrap().pose().bones[1].translation.x
    }

    #[test]
    fn an_old_scene_decodes_into_the_internal_player_and_still_round_trips() {
        let id = "22222222-2222-4222-8222-222222222222";
        let old = json!({"mesh": null, "animation": id, "playing": true, "looping": true, "playback_rate": 2.0});
        let c = decode_skeletal_mesh_component(&old).unwrap();
        assert_eq!(c.legacy_animation.as_ref().map(|r| r.as_str()), Some(id));
        assert!(c.internal_looping() && c.animation_graph.is_none());
        let saved = serde_json::to_value(&c).unwrap();
        assert_eq!(saved["animation"], id, "no scene loses its animation on the way through");
        assert_eq!(saved["looping"], true);
        assert_eq!(saved["playback_rate"], 2.0);
        let fresh = serde_json::to_value(SkeletalMeshComponent::default()).unwrap();
        assert!(fresh.get("animation").is_none() && fresh.get("looping").is_none());
        // the reflected property panel never shows the compatibility keys
        let mut props: HashMap<String, Value> = saved.as_object().unwrap().clone().into_iter().collect();
        <SkeletalMeshComponent as ScenePropsProjector>::apply_scene_props(&mut props, None);
        assert!(!props.contains_key("animation") && !props.contains_key("looping") && props.contains_key("animation_graph"));
        // one runtime: a graph wins over the old sequence, and says so
        let (rig, graph, _) = rig();
        let mut both = graph_component(&rig, &graph);
        both.legacy_animation = AssetRef::parse(id);
        both.animation_graph = AssetRef::parse("33333333-3333-4333-8333-333333333333");
        both.refresh_diagnostics();
        assert!(both.diagnostics.iter().any(|d| d.code == "component.sequence_shadowed_by_graph"));
        both.ensure_runtime();
        assert_eq!(both.animation().unwrap().graph().unwrap().id, graph.id);
    }

    #[test]
    fn a_graph_component_runs_in_the_animation_phase_with_typed_parameters_and_raises_graph_player_notifies() {
        let (rig, graph, walk_id) = rig();
        let hub = registered_hub();
        let mut world = World::new();
        let owner = world.spawn();
        let instance = pulsar_world_registry::attach_value(&mut world, owner, graph_component(&rig, &graph)).unwrap();
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let id = hub.descriptor_by_name("SkeletalMeshComponent.animation_notify").unwrap().id;
        let _sub = hub.bus().subscribe_dyn(id, SubscribeOptions::channel(Channel::Entity(instance.bits())), move |event| {
            let DynValue::Bytes(bytes) = &event.fields[0] else { panic!("codec payload") };
            let value = pulsar_world_registry::pulsar_script_vm::TypeRegistry::global().decode_event_value("AnimationNotifyEvent", bytes).unwrap();
            sink.lock().unwrap().push(value.downcast_ref::<AnimationNotifyEvent>().unwrap().clone());
        });
        let mut state = ComponentRuntimeState::default();
        {
            let mut c = world.get_mut::<SkeletalMeshComponent>(instance).unwrap();
            assert!(c.set_animation_float("speed", 2.0).is_ok());
            assert!(matches!(c.set_animation_bool("speed", true), Err(ParamError::WrongType { .. })));
            assert!(matches!(c.set_animation_float("nope", 1.0), Err(ParamError::Unknown(_))));
        }
        frame(&mut world, &hub, 0.2, &mut state);
        assert!((x1(&world, instance) - 1.6).abs() < 1e-3, "0.2 s at rate 2 is 0.4 of the 4 m slide: {}", x1(&world, instance));
        assert!(got.lock().unwrap().is_empty(), "the 0.5 s notify is not crossed yet");
        frame(&mut world, &hub, 0.1, &mut state);
        let events = got.lock().unwrap().clone();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!((events[0].notify_id.as_str(), events[0].sequence_id.as_str()), ("step", walk_id.as_str()));
        // pause holds the pose; the physics rig owning the pose does not
        world.get_mut::<SkeletalMeshComponent>(instance).unwrap().playing = false;
        let held = x1(&world, instance);
        frame(&mut world, &hub, 1.0, &mut state);
        assert_eq!(x1(&world, instance), held);
        world.get_mut::<SkeletalMeshComponent>(instance).unwrap().playing = true;
        world.get_mut::<SkeletalMeshComponent>(instance).unwrap().physics_pose = Some(vec![glam::Mat4::IDENTITY]);
        frame(&mut world, &hub, 0.125, &mut state);
        assert!(x1(&world, instance) != held, "physics owns the whole pose and the clock still moves");
        world.get_mut::<SkeletalMeshComponent>(instance).unwrap().clear_physics_pose();
    }

    #[test]
    fn two_components_share_the_compiled_graph_and_keep_independent_state_through_teardown() {
        let (rig, graph, _) = rig();
        let hub = registered_hub();
        let mut world = World::new();
        let (oa, ob) = (world.spawn(), world.spawn());
        let a = pulsar_world_registry::attach_value(&mut world, oa, graph_component(&rig, &graph)).unwrap();
        let b = pulsar_world_registry::attach_value(&mut world, ob, graph_component(&rig, &graph)).unwrap();
        world.get_mut::<SkeletalMeshComponent>(a).unwrap().set_animation_float("speed", 1.0).unwrap();
        world.get_mut::<SkeletalMeshComponent>(b).unwrap().set_animation_float("speed", 3.0).unwrap();
        let mut state = ComponentRuntimeState::default();
        frame(&mut world, &hub, 0.25, &mut state);
        assert!((x1(&world, a) - 1.0).abs() < 1e-3 && (x1(&world, b) - 3.0).abs() < 1e-3);
        let ga = world.get::<SkeletalMeshComponent>(a).unwrap().animation().unwrap().graph().unwrap().clone();
        let gb = world.get::<SkeletalMeshComponent>(b).unwrap().animation().unwrap().graph().unwrap().clone();
        assert!(Arc::ptr_eq(&ga, &gb), "one compiled graph for both components");
        // removing one never disturbs the other
        world.despawn(b);
        pulsar_world_registry::process_component_removals(&world, &hub, &mut state);
        frame(&mut world, &hub, 0.25, &mut state);
        assert!((x1(&world, a) - 2.0).abs() < 1e-3);
        // disabling the owner stops the animation without panicking
        let mut link = world.get_mut::<pulsar_world_registry::pulsar_scene_model::ComponentOwner>(a).unwrap();
        let was = link.is_enabled();
        link.enabled = 0;
        drop(link);
        frame(&mut world, &hub, 0.25, &mut state);
        assert!(world.get::<SkeletalMeshComponent>(a).unwrap().animation().is_none(), "the instance was torn down");
        let mut link = world.get_mut::<pulsar_world_registry::pulsar_scene_model::ComponentOwner>(a).unwrap();
        link.enabled = u32::from(was);
        drop(link);
        frame(&mut world, &hub, 0.25, &mut state);
        assert!(world.get::<SkeletalMeshComponent>(a).unwrap().animation().is_some(), "re-enabling starts a fresh playback");
    }

    #[test]
    fn montages_overlay_the_graph_pose_and_report_through_the_component() {
        use skeletal_assets::montage::{AnimationMontageAsset, MontageSegment, SlotTrack};
        use skeletal_assets::runtime::montage::{MontageEventKind, MontageResult};
        let (rig, _graph0, walk_id) = rig();
        // graph: walk -> Slot("UpperBody")
        let mut asset = AnimationGraphAsset::default();
        asset.skeleton = Some(skeletal_assets::ids::SkeletonId::parse(rig.skeleton.id.as_str()).unwrap().to_ref());
        let mut b = GraphBuilder::new("main", &[]);
        b.node("out", "anim.output_pose", &[]);
        b.node("p", "anim.sequence_player", &[("sequence", json!(walk_id))]);
        b.node("slot", "anim.slot", &[("slot", json!("UpperBody"))]);
        b.wire("p", "slot", "source").wire("slot", "out", "pose");
        asset.main = b.into_graph();
        let graph = compile_graph(&asset, &rig.source).graph.expect("compiles");
        // montage: the same clip on UpperBody, only below b2, one second long
        let mut montage = AnimationMontageAsset::default();
        let mut track = SlotTrack::new("UpperBody", "DefaultGroup");
        track.segments.push(MontageSegment::new(AssetRef::parse(&walk_id), 0.0));
        track.mask = Some(skeletal_assets::bone_mask::BoneMaskSpec {
            branches: vec![skeletal_assets::bone_mask::MaskBranch { bone: "b2".into(), depth: None, weight: 1.0 }],
            default_weight: 0.0,
            ..Default::default()
        });
        montage.tracks.push(track);
        montage.blend_in.duration = 0.0;
        montage.blend_out.duration = 0.0;
        let runtime_skeleton = Arc::new(RuntimeSkeleton::new(rig.skeleton.id.as_str(), &rig.skeleton.skeleton()).unwrap());
        let sequences = &rig.source.sequences;
        let compiled = CompiledMontage::compile(&montage, &runtime_skeleton, &mut |id| {
            let asset = sequences.get(id)?;
            Some(Arc::new(CompiledClip::compile_shared(asset, &runtime_skeleton)))
        })
        .unwrap();
        let hub = registered_hub();
        let mut world = World::new();
        let owner = world.spawn();
        let c = pulsar_world_registry::attach_value(&mut world, owner, graph_component(&rig, &graph)).unwrap();
        let mut state = ComponentRuntimeState::default();
        frame(&mut world, &hub, 0.25, &mut state);
        let handle = world.get_mut::<SkeletalMeshComponent>(c).unwrap().play_compiled_montage(Arc::new(compiled), 1.0, None).unwrap();
        frame(&mut world, &hub, 0.5, &mut state);
        {
            let comp = world.get::<SkeletalMeshComponent>(c).unwrap();
            let pose = comp.animation().unwrap().pose();
            assert!((pose.bones[1].translation.x - 3.0).abs() < 1e-3, "locomotion is untouched outside the mask: {}", pose.bones[1].translation.x);
        }
        frame(&mut world, &hub, 2.0, &mut state);
        let events = world.get_mut::<SkeletalMeshComponent>(c).unwrap().take_montage_events();
        assert!(events.iter().any(|e| e.handle == handle && matches!(e.kind, MontageEventKind::Started)));
        assert!(events.iter().any(|e| e.handle == handle && matches!(e.kind, MontageEventKind::Ended(MontageResult::Completed))), "{events:?}");
        assert!(world.get_mut::<SkeletalMeshComponent>(c).unwrap().take_montage_events().is_empty(), "drained");
    }
}

#[cfg(test)]
#[path = "skeletal_phase3_tests.rs"]
mod phase3_tests;

#[cfg(test)]
#[path = "skeletal_phase5_tests.rs"]
mod phase5_tests;

#[cfg(test)]
#[path = "skeletal_state_event_tests.rs"]
mod state_event_tests;

#[cfg(test)]
#[path = "skeletal_net_tests.rs"]
mod net_tests;

#[cfg(test)]
#[path = "skeletal_unified_tests.rs"]
mod unified_tests;
