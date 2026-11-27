//! Play-mode level bootstrap: hydrate a `.level` file into
//! the scene `World`.
//!
//! This is the runtime counterpart of the editor's own load path
//! (`scene_edit::level_io::load_from_file`): one authoritative copy of the scene,
//! owned by SceneDB, that renderers and gameplay share -- NOT a direct
//! imperative load into a Helio `Scene` (that was `pulsar_scene::
//! SceneLoader`, now legacy/import-only).
//!
//! Hydration mirrors the editor exactly:
//!
//! - Objects/transforms/hierarchy/visibility are spawned straight into the
//!   world with [`SceneWorldExt::spawn_object`].
//! - Every component record becomes a component-instance entity attached
//!   to its object through `pulsar_world_registry::
//!   attach_records`, enabled or not: a `#[register_world_component]` class
//!   holds its typed value (`StaticMeshComponent`'s decode loads its mesh
//!   asset here -- it resolves paths via `engine_state::get_project_path()`,
//!   so callers must have set that before calling [`RuntimeLevel::load`]).
//! - A class this build does not register is attached as an explicit
//!   `UnresolvedComponent` that keeps its JSON for lossless saving; a
//!   registered class whose data does not decode fails the load.
//!
//! Component data source precedence matches the editor's ("persisted
//! components are authoritative when present"): a top-level
//! `components` entry for an object wins over that object's own
//! `component_instances`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use parking_lot::RwLock;
use pulsar_scene::component_instances_from_props;
use pulsar_scene::format::{ObjectType as FileObjectType, SceneFile};
use serde_json::Value;

use pulsar_scenedb::{Entity, World};

use crate::scene::hierarchy_transform;
use crate::scene::{
    ObjectType, RenderProps, SceneWorldExt, SharedScene, SpawnObject, Transform, Visibility,
};

/// Errors from [`RuntimeLevel::load`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeLevelError {
    #[error("failed to read '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
    #[error("unsupported scene version '{0}' (expected 1.x or 2.x)")]
    UnsupportedVersion(String),
    #[error("failed to hydrate component {class_name} on object {object_id}: {message}")]
    ComponentHydration {
        object_id: String,
        class_name: String,
        message: String,
    },
}

/// Editor camera state persisted in the level file (`editor.camera`) --
/// position (f64: a planet's surface is millions of metres out) + world
/// yaw/pitch in radians, the convention `FlyCamera::new` takes. Lets a
/// play-mode camera start where the editor view was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorCamera {
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// What a level load reports beyond hydration itself: the editor camera
/// seed. Which scripts run is not an extra: placed classes are
/// `ClassInstance` components in the world, which the script driver
/// (`pulsar_game::scripting::ScriptDriver`) follows.
#[derive(Clone, Debug, Default)]
pub struct LevelExtras {
    /// The camera saved under `editor.camera`, if any.
    pub editor_camera: Option<EditorCamera>,
}

/// A level file read, parsed and migrated, ready to hydrate
/// ([`RuntimeLevel::prepare`]). It holds no world: it is plain data, so it can
/// be built on any thread and handed to the one that applies it.
#[derive(Clone)]
pub struct PreparedLevelFile {
    file: SceneFile,
    registry: pulsar_class::ClassRegistry,
}

impl PreparedLevelFile {
    /// The camera saved under `editor.camera`, if any.
    pub fn editor_camera(&self) -> Option<EditorCamera> {
        editor_camera(&self.file.editor)
    }

    /// Check, without a world, what hydrating this level would refuse: its
    /// version, its objects' ids and parent order, and that every registered
    /// component's data decodes. A level switch calls it on the worker, so a
    /// level that cannot load leaves the one being played as it is. (Ids that
    /// collide with objects already in a world are only checked by the
    /// hydration itself.) Slow for a large level: not for a frame thread.
    pub fn validate(&self) -> Result<(), RuntimeLevelError> {
        profiling::profile_scope!("runtime_level::validate");
        check_version(&version_string(&self.file.version))?;
        let nothing = World::new();
        let mut seen = HashSet::new();
        for obj in &self.file.objects {
            validate_object(&nothing, &mut seen, obj)?;
        }
        let persisted = persisted_components(&self.file.components);
        for obj in &self.file.objects {
            for record in component_records_of(&persisted, obj) {
                pulsar_world_registry::instances::check_record(
                    &pulsar_scene_model::ComponentInstance {
                        class_name: record.class_name.clone(),
                        enabled: record.enabled,
                        data: record.data,
                    },
                )
                .map_err(|message| RuntimeLevelError::ComponentHydration {
                    object_id: obj.id.clone(),
                    class_name: record.class_name,
                    message,
                })?;
            }
        }
        Ok(())
    }

    /// Start hydrating this level a little at a time ([`LevelHydration`]), for
    /// a frame thread that can spend only a few milliseconds per frame.
    pub fn begin_hydration(self) -> Result<LevelHydration, RuntimeLevelError> {
        LevelHydration::new(self.file, self.registry)
    }
}

/// Where a [`LevelHydration`] stands after a step.
pub enum HydrationProgress {
    /// Call `step` again (next frame).
    More,
    /// The level is in the world.
    Done(LevelExtras),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HydrationStage {
    Validate,
    Objects,
    Frames,
    Props,
    Components,
    Classes,
    Finish,
    Done,
}

/// How many class roots one expansion handles at once.
const CLASS_ROOTS_PER_UNIT: usize = 16;
/// One object whose components take this long to hydrate is reported.
const SLOW_OBJECT: std::time::Duration = std::time::Duration::from_millis(8);
/// How many objects one pass of the property stage covers.
const PROPS_PER_UNIT: usize = 128;

/// A level hydrated in bounded steps. Each [`step`](Self::step) does whole
/// units of work (one object spawned, one object's components attached, a
/// handful of class roots expanded) until its time budget is spent, then
/// returns, so a frame thread holds the world's write lock for a few
/// milliseconds at a time however large the level is. The world is partial
/// between steps: renderers show what has arrived, and gameplay must not tick
/// until [`HydrationProgress::Done`].
///
/// Hydrating all at once is this run with an unlimited budget
/// (`RuntimeLevel::hydrate_migrated`), so there is one implementation.
pub struct LevelHydration {
    file: SceneFile,
    registry: pulsar_class::ClassRegistry,
    version: String,
    stage: HydrationStage,
    cursor: usize,
    spawned: Vec<Entity>,
    /// Stable ids validated so far (the duplicate check spans the whole file).
    seen: HashSet<String>,
    persisted: HashMap<String, Vec<ComponentRecord>>,
    roots: Vec<Entity>,
    extras: LevelExtras,
}

impl LevelHydration {
    fn new(
        file: SceneFile,
        registry: pulsar_class::ClassRegistry,
    ) -> Result<Self, RuntimeLevelError> {
        let version = version_string(&file.version);
        check_version(&version)?;
        let extras = LevelExtras {
            editor_camera: editor_camera(&file.editor),
        };
        Ok(Self {
            persisted: persisted_components(&file.components),
            file,
            registry,
            version,
            stage: HydrationStage::Validate,
            cursor: 0,
            spawned: Vec::new(),
            seen: HashSet::new(),
            roots: Vec::new(),
            extras,
        })
    }

    /// How far along it is, 0 to 1 (by units of work, not time).
    pub fn fraction(&self) -> f32 {
        let objects = self.file.objects.len().max(1) as f32;
        match self.stage {
            HydrationStage::Validate => 0.0,
            HydrationStage::Objects => 0.3 * self.cursor as f32 / objects,
            HydrationStage::Frames | HydrationStage::Props => 0.3,
            HydrationStage::Components => 0.3 + 0.5 * self.cursor as f32 / objects,
            HydrationStage::Classes => 0.8 + 0.15 * self.cursor as f32 / objects,
            HydrationStage::Finish => 0.95,
            HydrationStage::Done => 1.0,
        }
    }

    /// Do units of work until `budget` is spent (always at least one, so a
    /// zero budget still makes progress). Validates before it changes the
    /// world, so a malformed level leaves nothing behind.
    pub fn step(
        &mut self,
        world: &mut World,
        budget: std::time::Duration,
    ) -> Result<HydrationProgress, RuntimeLevelError> {
        profiling::profile_scope!("level_hydration::step");
        let started = std::time::Instant::now();
        loop {
            match self.stage {
                HydrationStage::Validate => {
                    // Parent-before-child order is the format's own DFS
                    // guarantee (see `SceneFile::objects` doc), which is what
                    // spawning requires. One object a unit: a level of tens of
                    // thousands of objects is not one frame's work.
                    let Some(obj) = self.file.objects.get(self.cursor) else {
                        self.spawned.reserve(self.file.objects.len());
                        self.seen = HashSet::new();
                        self.advance(HydrationStage::Objects);
                        continue;
                    };
                    validate_object(world, &mut self.seen, obj)?;
                    self.cursor += 1;
                }
                HydrationStage::Objects => {
                    let Some(obj) = self.file.objects.get(self.cursor) else {
                        self.advance(HydrationStage::Frames);
                        continue;
                    };
                    profiling::profile_scope!("level_hydration::spawn_object");
                    self.spawned.push(spawn_file_object(world, obj)?);
                    self.cursor += 1;
                }
                HydrationStage::Frames => {
                    // A level saved before parent-relative transforms holds
                    // world-space values for parented objects: make them local,
                    // in place.
                    // (A format version that predates parent-relative
                    // transforms only; it converts parents and children
                    // together, so it is one unit.)
                    if hierarchy_transform::file_has_world_space_transforms(&self.version) {
                        profiling::profile_scope!("level_hydration::legacy_transforms");
                        hierarchy_transform::convert_legacy_world_transforms(world, &self.spawned);
                    }
                    self.advance(HydrationStage::Props);
                }
                HydrationStage::Props => {
                    // Rigid frames (an object prop) become frame components and
                    // the content-space tag follows its prop: per object, a
                    // chunk at a time. The frames' slots are allocated once, in
                    // the finish stage.
                    if self.cursor >= self.spawned.len() {
                        self.advance(HydrationStage::Components);
                        continue;
                    }
                    let end = (self.cursor + PROPS_PER_UNIT).min(self.spawned.len());
                    let chunk = &self.spawned[self.cursor..end];
                    crate::scene::frames::mark_frames_from_props(world, chunk);
                    for &entity in chunk {
                        crate::scene::partitions::apply_space_prop(world, entity);
                    }
                    self.cursor = end;
                }
                HydrationStage::Components => {
                    let Some(obj) = self.file.objects.get(self.cursor) else {
                        self.roots = self
                            .file
                            .objects
                            .iter()
                            .filter_map(|obj| world.entity_for(&obj.id))
                            .collect();
                        self.advance(HydrationStage::Classes);
                        continue;
                    };
                    self.cursor += 1;
                    let Some(entity) = world.entity_for(&obj.id) else {
                        continue;
                    };
                    let instances = component_records_of(&self.persisted, obj);
                    // Legacy shapes were migrated with the file
                    // (`pulsar_class::records`).
                    profiling::profile_scope!("level_hydration::components");
                    let unit_started = std::time::Instant::now();
                    hydrate_components(world, entity, &obj.id, &instances)?;
                    // One object's components are the smallest unit there is: one
                    // that outlasts the frame's budget is named, so it can be fixed
                    // where it is slow (its class's own hydration).
                    let took = unit_started.elapsed();
                    if took >= SLOW_OBJECT {
                        tracing::warn!(
                            object = %obj.id,
                            name = %obj.name,
                            classes = ?instances.iter().map(|record| record.class_name.as_str()).collect::<Vec<_>>(),
                            took_ms = took.as_secs_f64() * 1000.0,
                            "One object's components took longer than a frame's hydration budget"
                        );
                    }
                }
                HydrationStage::Classes => {
                    // Placed classes: rebuild each instance from the current
                    // class definition, then apply its overrides. Unresolved
                    // classes keep their ClassInstance and are only warned about.
                    if self.cursor >= self.roots.len() {
                        self.advance(HydrationStage::Finish);
                        continue;
                    }
                    profiling::profile_scope!("level_hydration::classes");
                    let end = (self.cursor + CLASS_ROOTS_PER_UNIT).min(self.roots.len());
                    let report = pulsar_class::world::expand_roots(
                        world,
                        &self.registry,
                        &self.roots[self.cursor..end],
                    );
                    for id in &report.unresolved {
                        tracing::warn!(object = %id, "Placed class instance has no class in this project");
                    }
                    self.cursor = end;
                }
                HydrationStage::Finish => {
                    // Plugin singleton rules are repaired on load; directional
                    // lights are unrestricted.
                    {
                        profiling::profile_scope!("level_hydration::enforce_rules");
                        crate::scene::level_rules::enforce_on_load(world);
                    }
                    // Components are hydrated now: everything that defines a
                    // space (a sublevel placement is a component) gets its slot.
                    {
                        profiling::profile_scope!("level_hydration::sync_spaces");
                        crate::scene::frames::sync_spaces(world);
                    }
                    // What the migration could not turn into a ClassInstance (a
                    // second class bound to one object) has no script instance
                    // any more.
                    for (stable_id, bindings) in &self.file.blueprint_bindings {
                        for binding in bindings {
                            tracing::warn!(
                                object = %stable_id,
                                class = %binding.class_name,
                                "Legacy script binding not migrated (one class per object); it does not run"
                            );
                        }
                    }
                    self.advance(HydrationStage::Done);
                }
                HydrationStage::Done => return Ok(HydrationProgress::Done(self.extras.clone())),
            }
            if started.elapsed() >= budget && self.stage != HydrationStage::Done {
                return Ok(HydrationProgress::More);
            }
        }
    }

    fn advance(&mut self, stage: HydrationStage) {
        self.stage = stage;
        self.cursor = 0;
    }
}

/// A scene loaded for runtime use: one shared SceneDB scene plus
/// the level-file extras gameplay cares about (editor camera seed).
pub struct RuntimeLevel {
    scene: SharedScene,
    extras: LevelExtras,
}

impl RuntimeLevel {
    /// Load and hydrate a level file. See the module doc for the hydration
    /// contract; call `engine_state::set_project_path` first so asset-
    /// resolving hydrates (`StaticMeshComponent`) can find project files.
    pub fn load(path: &Path) -> Result<Self, RuntimeLevelError> {
        Self::load_with_classes(path, &project_class_registry())
    }

    /// [`load`](Self::load) with an explicit class registry instead of the
    /// current project's (tools, tests).
    pub fn load_with_classes(
        path: &Path,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<Self, RuntimeLevelError> {
        let file = load_scene_file(path, registry)?;
        Self::from_scene_file_with_classes(file, registry)
    }
    /// Load a level file and hydrate it into an EXISTING world -- the
    /// one-world play-mode path: the tick loop's
    /// shared store is authoritative, so the level merges INTO it
    /// (additively; setup-time-registered actors survive) instead of the
    /// level constructing its own store. Duplicate stable ids between the
    /// file and live state are errors, never silent re-spawns.
    ///
    /// Returns the file's extras ([`LevelExtras`]: the editor camera). Call
    /// `engine_state::set_project_path` first so asset-resolving hydrates
    /// (`StaticMeshComponent`) can find project files and placed classes
    /// resolve against the project's classes.
    pub fn load_into(path: &Path, world: &mut World) -> Result<LevelExtras, RuntimeLevelError> {
        Self::load_into_with_classes(path, world, &project_class_registry())
    }

    /// [`load_into`](Self::load_into) with an explicit class registry
    /// (tools, tests).
    ///
    /// **Blocking**: it reads the file, waits for the level's assets (up to
    /// [`crate::scene::LEVEL_ASSET_WAIT`]) and loads sublevels inline. It is for
    /// headless runs, tools and tests. A host with a window or a UI uses
    /// [`prepare`](Self::prepare) on a worker and [`apply_prepared`](Self::apply_prepared)
    /// instead.
    pub fn load_into_with_classes(
        path: &Path,
        world: &mut World,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<LevelExtras, RuntimeLevelError> {
        let prepared = Self::prepare(path, registry)?;
        let editor_camera = prepared.editor_camera();
        Self::hydrate_migrated(prepared.file, world, registry)?;
        crate::scene::sublevel_display::sync_level_instances(world);
        crate::scene::resolve_level_assets(world);
        Ok(LevelExtras { editor_camera })
    }

    /// Read, parse and migrate a level file, and nothing else: no world, no
    /// lock, no asset work. This is the slow, I/O-bound half of a load, so
    /// it runs on a worker; [`apply_prepared`](Self::apply_prepared) is the
    /// half that touches the world.
    pub fn prepare(
        path: &Path,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<PreparedLevelFile, RuntimeLevelError> {
        profiling::profile_scope!("runtime_level::prepare");
        let mut file = load_scene_file(path, registry)?;
        migrate_scene_file(&mut file, registry);
        Ok(PreparedLevelFile {
            file,
            registry: registry.clone(),
        })
    }

    /// A prepared level from an already-parsed file (tests).
    #[cfg(test)]
    fn prepare_from_file_for_test(
        mut file: SceneFile,
        registry: &pulsar_class::ClassRegistry,
    ) -> PreparedLevelFile {
        migrate_scene_file(&mut file, registry);
        PreparedLevelFile {
            file,
            registry: registry.clone(),
        }
    }

    /// [`prepare`](Self::prepare) against the current project's classes. The
    /// class scan reads the disk, so this too belongs on a worker.
    pub fn prepare_for_project(path: &Path) -> Result<PreparedLevelFile, RuntimeLevelError> {
        Self::prepare(path, &project_class_registry())
    }

    /// Hydrate a prepared level into `world`. Pure in-memory work: no file is
    /// read and no asset is waited for (the renderer's per-frame pump binds
    /// them as they finish), so it is safe under a world write lock on a
    /// frame thread. Sublevels are requested in the background
    /// ([`crate::scene::sublevel_display::sync_level_instances_deferred`]).
    pub fn apply_prepared(
        prepared: PreparedLevelFile,
        world: &mut World,
    ) -> Result<LevelExtras, RuntimeLevelError> {
        profiling::profile_scope!("runtime_level::apply_prepared");
        let editor_camera = prepared.editor_camera();
        Self::hydrate_migrated(prepared.file, world, &prepared.registry)?;
        crate::scene::sublevel_display::sync_level_instances_deferred(world);
        Ok(LevelExtras { editor_camera })
    }
    /// Instantiate a level file's objects **under** `parent`: the file's origin
    /// is `parent`'s origin, its top-level objects become `parent`'s children
    /// (their transforms are the file's own, relative to its origin), and every
    /// object id gets `id_prefix` so a level can be placed more than once. This
    /// is how a level instance is shown (`SubLevelDisplayComponent`).
    ///
    /// Returns every object this created, class-generated children included.
    /// Call `engine_state::set_project_path` first, as for [`load_into`].
    /// Assets resolve through the host's normal asset pump. This path mutates
    /// a live shared world, so it must never wait for asset workers.
    pub fn load_under(
        path: &Path,
        world: &mut World,
        parent: Entity,
        id_prefix: &str,
    ) -> Result<Vec<Entity>, RuntimeLevelError> {
        let prepared = Self::prepare_for_project(path)?;
        Self::apply_prepared_under(&prepared, world, parent, id_prefix)
    }

    /// [`load_under`](Self::load_under) from a level prepared on a worker
    /// ([`prepare`](Self::prepare)); the same prepared level can be placed any
    /// number of times.
    pub fn apply_prepared_under(
        prepared: &PreparedLevelFile,
        world: &mut World,
        parent: Entity,
        id_prefix: &str,
    ) -> Result<Vec<Entity>, RuntimeLevelError> {
        let registry = &prepared.registry;
        let mut file = prepared.file.clone();
        let prefixed = |id: &str| format!("{id_prefix}{id}");
        let top_level: Vec<String> = file
            .objects
            .iter()
            .filter(|obj| obj.parent.is_none())
            .map(|obj| prefixed(&obj.id))
            .collect();
        for obj in &mut file.objects {
            obj.id = prefixed(&obj.id);
            obj.parent = obj.parent.as_deref().map(prefixed);
        }
        if let Some(map) = file.components.as_object_mut() {
            *map = std::mem::take(map)
                .into_iter()
                .map(|(id, value)| (prefixed(&id), value))
                .collect();
        }
        file.blueprint_bindings = std::mem::take(&mut file.blueprint_bindings)
            .into_iter()
            .map(|(id, bindings)| (prefixed(&id), bindings))
            .collect();

        let mut before: HashSet<Entity> =
            crate::scene::hierarchy_transform::descendants_of(world, parent)
                .into_iter()
                .collect();
        before.insert(parent);
        Self::hydrate_migrated(file, world, registry)?;
        for id in top_level {
            if let Some(entity) = world.entity_for(&id) {
                // The file's transform is already relative to its origin.
                let _ = world.reparent(entity, Some(parent));
            }
        }
        Ok(
            crate::scene::hierarchy_transform::descendants_of(world, parent)
                .into_iter()
                .filter(|entity| !before.contains(entity))
                .collect(),
        )
    }

    /// Hydrate from an already-parsed [`SceneFile`] into a fresh store
    /// (import/legacy callers that get their JSON from somewhere other than
    /// disk).
    pub fn from_scene_file(file: SceneFile) -> Result<Self, RuntimeLevelError> {
        Self::from_scene_file_with_classes(file, &project_class_registry())
    }

    /// [`from_scene_file`](Self::from_scene_file) with an explicit class
    /// registry.
    pub fn from_scene_file_with_classes(
        file: SceneFile,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<Self, RuntimeLevelError> {
        let editor_camera = editor_camera(&file.editor);
        let mut scene = crate::scene::new_scene();
        Self::hydrate_scene_file(file, &mut scene.world, registry)?;
        crate::scene::sublevel_display::sync_level_instances(&mut scene.world);
        crate::scene::resolve_level_assets(&mut scene.world);
        let extras = LevelExtras { editor_camera };
        Ok(Self {
            scene: Arc::new(RwLock::new(scene)),
            extras,
        })
    }

    /// Shared hydration core: version gate + class migration + objects +
    /// components + class instances into `world`.
    fn hydrate_scene_file(
        mut file: SceneFile,
        world: &mut World,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<(), RuntimeLevelError> {
        migrate_scene_file(&mut file, registry);
        Self::hydrate_migrated(file, world, registry)
    }

    /// Hydrate a file that already went through [`migrate_scene_file`] (a
    /// prepared level), all at once: [`LevelHydration`] run to the end.
    fn hydrate_migrated(
        file: SceneFile,
        world: &mut World,
        registry: &pulsar_class::ClassRegistry,
    ) -> Result<(), RuntimeLevelError> {
        let mut hydration = LevelHydration::new(file, registry.clone())?;
        while let HydrationProgress::More = hydration.step(world, std::time::Duration::MAX)? {}
        Ok(())
    }

    /// The shared, authoritative scene. Renderers and the tick loop all clone
    /// this handle.
    pub fn scene(&self) -> SharedScene {
        Arc::clone(&self.scene)
    }

    /// The level's extras: the editor camera seed.
    pub fn extras(&self) -> &LevelExtras {
        &self.extras
    }

    /// Editor camera saved with the level, if any.
    pub fn editor_camera(&self) -> Option<EditorCamera> {
        self.extras.editor_camera
    }
}

/// One component instance record -- either parsed from the file's persisted
/// components array (which carries its own enabled flag) or converted from
/// `component_instances_from_props`'s `(index, class_name, data)` tuples
/// (where presence means enabled).
#[derive(Clone, Debug)]
struct ComponentRecord {
    #[allow(dead_code)]
    index: usize,
    class_name: String,
    data: Value,
    enabled: bool,
}

fn load_scene_file(
    path: &Path,
    registry: &pulsar_class::ClassRegistry,
) -> Result<SceneFile, RuntimeLevelError> {
    let io_error = |message: String| RuntimeLevelError::Io {
        path: path.display().to_string(),
        message,
    };
    let parse_error = |message: String| RuntimeLevelError::Parse {
        path: path.display().to_string(),
        message,
    };
    let bytes = engine_fs::virtual_fs::read_file(path).map_err(|e| io_error(e.to_string()))?;
    let mut value: Value =
        serde_json::from_slice(&bytes).map_err(|e| parse_error(e.to_string()))?;
    // Migrate on the raw JSON first: it still sees fields the typed
    // `SceneFile` drops (a `Blueprint` object type, a flat `script_asset`).
    let report = pulsar_class::migrate::migrate_level_value(&mut value, registry);
    if report.changed() {
        tracing::info!(
            path = %path.display(),
            script_components = report.script_components.len(),
            bindings = report.bindings.len(),
            "Migrated level classes to ClassInstance"
        );
    }
    serde_json::from_value(value).map_err(|error| parse_error(error.to_string()))
}

/// Classes of the current project (`engine_state`'s project path); empty
/// when no project is set, which leaves every placed class unresolved.
fn project_class_registry() -> pulsar_class::ClassRegistry {
    match engine_state::get_project_path() {
        Some(root) => pulsar_class::ClassRegistry::scan(Path::new(&root)),
        None => pulsar_class::ClassRegistry::default(),
    }
}

/// Run the class migration over an already-parsed file: its objects'
/// component lists, the top-level `components` section and
/// `blueprint_bindings`.
fn migrate_scene_file(file: &mut SceneFile, registry: &pulsar_class::ClassRegistry) {
    let objects: Vec<Value> = file
        .objects
        .iter()
        .map(|obj| {
            let mut entry = serde_json::json!({ "id": obj.id, "props": obj.props });
            if let Some(instances) = &obj.component_instances {
                entry["component_instances"] = instances.clone();
            }
            entry
        })
        .collect();
    let mut value = serde_json::json!({
        "objects": objects,
        "components": file.components,
        "blueprint_bindings": file.blueprint_bindings,
    });
    let report = pulsar_class::migrate::migrate_level_value(&mut value, registry);
    if !report.changed() {
        return;
    }
    let records = &report.records;
    if records.changed() {
        tracing::info!(
            plugins = records.plugins.len(),
            nested = records.nested.len(),
            stripped_props = records.stripped_props.len(),
            retired_fields = records.retired_fields.len(),
            "Migrated legacy component records"
        );
    }
    if let Some(objects) = value.get("objects").and_then(Value::as_array) {
        for (obj, migrated) in file.objects.iter_mut().zip(objects) {
            if let Some(props) = migrated
                .get("props")
                .and_then(|p| serde_json::from_value(p.clone()).ok())
            {
                obj.props = props;
            }
            obj.component_instances = migrated.get("component_instances").cloned();
        }
    }
    file.components = value.get("components").cloned().unwrap_or(Value::Null);
    file.blueprint_bindings = value
        .get("blueprint_bindings")
        .and_then(|b| serde_json::from_value(b.clone()).ok())
        .unwrap_or_default();
}

/// The accepted level versions, as the editor's own loader: 1.x and 2.x.
fn check_version(version: &str) -> Result<(), RuntimeLevelError> {
    if !version.starts_with("1.") && !version.starts_with("2.") && version != "1" {
        return Err(RuntimeLevelError::UnsupportedVersion(version.to_owned()));
    }
    Ok(())
}

/// One object's component records: a persisted entry is authoritative,
/// including an explicit empty array (the object has no components), else the
/// records of its props.
fn component_records_of(
    persisted: &HashMap<String, Vec<ComponentRecord>>,
    obj: &pulsar_scene::format::SceneObject,
) -> Vec<ComponentRecord> {
    match persisted.get(&obj.id) {
        Some(records) => records.clone(),
        None => component_instances_from_props(&obj.props, obj.component_instances.as_ref())
            .into_iter()
            .map(|(index, class_name, data)| ComponentRecord {
                index,
                class_name,
                data,
                enabled: true,
            })
            .collect(),
    }
}

fn version_string(version: &Value) -> String {
    match version {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Spawn one file-format object into the world, with its props and
/// component-instance JSON attached.
fn spawn_file_object(
    world: &mut World,
    obj: &pulsar_scene::format::SceneObject,
) -> Result<Entity, RuntimeLevelError> {
    let parse_error = |message: String| RuntimeLevelError::Parse {
        path: String::new(),
        message,
    };
    let parent = match &obj.parent {
        Some(parent_id) => Some(world.entity_for(parent_id).ok_or_else(|| {
            parse_error(format!(
                "object '{}' references parent '{}' before it is available",
                obj.id, parent_id
            ))
        })?),
        None => None,
    };
    let entity = world
        .spawn_object(SpawnObject {
            stable_id: Some(obj.id.clone()),
            name: obj.name.clone(),
            parent,
            transform: Transform {
                position: obj.world_position(),
                rotation: obj.world_rotation(),
                scale: obj.world_scale(),
            },
            visibility: Visibility {
                visible: obj.visible,
                locked: obj.locked,
            },
            object_type: object_type(obj.object_type),
        })
        .map_err(|error| parse_error(error.to_string()))?;
    if let Some(mut props) = world.get_mut::<RenderProps>(entity) {
        props.props = obj.props.clone();
    }
    Ok(entity)
}

/// `pulsar_scene`'s loader-facing object classification ->
/// `engine_backend`'s store-facing one.
fn object_type(object_type: FileObjectType) -> ObjectType {
    match object_type {
        FileObjectType::Empty | FileObjectType::Unknown => ObjectType::Empty,
        FileObjectType::Folder => ObjectType::Folder,
        FileObjectType::Camera => ObjectType::Camera,
    }
}

/// Parse the editor's persisted components section:
/// `{ "<object_id>": [ { "class_name": ..., "data": ..., "enabled": ... } ] }`.
/// Lenient by design -- entries missing a class name are skipped; a missing
/// `enabled` flag means enabled (matching how the editor treats inline
/// instances).
fn persisted_components(components: &Value) -> HashMap<String, Vec<ComponentRecord>> {
    let mut out = HashMap::new();
    let Some(map) = components.as_object() else {
        return out;
    };
    for (object_id, entries) in map {
        let Some(array) = entries.as_array() else {
            continue;
        };
        let records = array
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                Some(ComponentRecord {
                    index: entry
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|value| value as usize)
                        .unwrap_or(index),
                    class_name: entry.get("class_name")?.as_str()?.to_string(),
                    data: entry.get("data").cloned().unwrap_or(Value::Null),
                    enabled: entry
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                })
            })
            .collect();
        out.insert(object_id.clone(), records);
    }
    out
}

fn validate_object(
    world: &World,
    seen: &mut HashSet<String>,
    obj: &pulsar_scene::format::SceneObject,
) -> Result<(), RuntimeLevelError> {
    if world.entity_for(&obj.id).is_some() || !seen.insert(obj.id.clone()) {
        return Err(RuntimeLevelError::Parse {
            path: String::new(),
            message: format!("duplicate stable id '{}'", obj.id),
        });
    }
    if let Some(parent) = &obj.parent {
        if !seen.contains(parent.as_str()) && world.entity_for(parent).is_none() {
            return Err(RuntimeLevelError::Parse {
                path: String::new(),
                message: format!(
                    "object '{}' references parent '{}' before it is available",
                    obj.id, parent
                ),
            });
        }
    }
    Ok(())
}
/// Attach `instances` to `entity` as component-instance entities, in order:
/// every record becomes its own instance, enabled
/// or not, with its stable id and parent link. A class this build does not
/// register is kept as an explicit unresolved payload; a registered class
/// whose data does not decode fails the load, naming the object and class.
fn hydrate_components(
    world: &mut World,
    entity: pulsar_scenedb::Entity,
    object_id: &str,
    instances: &[ComponentRecord],
) -> Result<(), RuntimeLevelError> {
    let records: Vec<pulsar_scene_model::ComponentInstance> = instances
        .iter()
        .map(|record| pulsar_scene_model::ComponentInstance {
            class_name: record.class_name.clone(),
            enabled: record.enabled,
            data: record.data.clone(),
        })
        .collect();
    let attached =
        pulsar_world_registry::attach_records(world, entity, &records).map_err(|error| {
            RuntimeLevelError::ComponentHydration {
                object_id: object_id.to_string(),
                class_name: String::new(),
                message: error.to_string(),
            }
        })?;
    for (record, instance) in records.iter().zip(attached) {
        let Some(unresolved) = world.get::<pulsar_scene_model::UnresolvedComponent>(instance)
        else {
            continue;
        };
        if pulsar_world_registry::component_id_for_class(&record.class_name).is_some() {
            return Err(RuntimeLevelError::ComponentHydration {
                object_id: object_id.to_string(),
                class_name: record.class_name.clone(),
                message: unresolved.reason.clone(),
            });
        }
    }
    Ok(())
}

/// Read `editor.camera` out of a level file's editor section, if present.
fn editor_camera(editor: &Value) -> Option<EditorCamera> {
    let cam = editor.get("camera")?;
    let pos = cam.get("position")?.as_array()?;
    if pos.len() < 3 {
        return None;
    }
    Some(EditorCamera {
        position: [pos[0].as_f64()?, pos[1].as_f64()?, pos[2].as_f64()?],
        yaw: cam.get("yaw").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        pitch: cam.get("pitch").and_then(Value::as_f64).unwrap_or(0.0) as f32,
    })
}

#[cfg(test)]
mod tests {
    // A class of these tests has the marker of a language (a kernel crate's
    // tests never link a plugin).
    pulsar_language::submit_test_language!("test.graph", ["graph_save.json"]);

    use super::*;
    use crate::scene::StableId;

    const SAMPLE_LEVEL: &str = r#"{
        "version": "2.1",
        "objects": [
            {
                "id": "sun", "name": "Sun", "object_type": {"Light": "Point"},
                "transform": {"position": [1.0, 5.0, 2.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
                "parent": null, "visible": true, "locked": false, "props": {}
            },
            {
                "id": "group", "name": "Group", "object_type": "Folder",
                "transform": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]},
                "parent": null, "visible": true, "locked": false, "props": {}
            },
            {
                "id": "cube", "name": "Cube", "object_type": {"Mesh": "Cube"},
                "transform": {"position": [4.0, 0.0, 0.0], "rotation": [0.0, 90.0, 0.0], "scale": [2.0, 2.0, 2.0]},
                "parent": "group", "visible": false, "locked": true, "props": {}
            }
        ],
        "components": {},
        "metadata": {},
        "editor": {"camera": {"position": [10.0, 20.0, 30.0], "yaw": 1.0, "pitch": -0.25}}
    }"#;

    fn sample_level() -> RuntimeLevel {
        let file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        RuntimeLevel::from_scene_file(file).expect("sample hydrates")
    }

    /// A level saved before transforms were parent-relative keeps every object
    /// where it was; a current level's values are already local.
    #[test]
    fn legacy_world_space_transforms_load_as_parent_relative() {
        let level = |version: &str| {
            let json = format!(
                r#"{{
                "version": "{version}",
                "objects": [
                    {{"id": "group", "name": "Group", "object_type": "Folder",
                      "transform": {{"position": [10.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}},
                      "parent": null, "visible": true, "locked": false, "props": {{}}}},
                    {{"id": "cube", "name": "Cube", "object_type": {{"Mesh": "Cube"}},
                      "transform": {{"position": [11.0, 2.0, 3.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0]}},
                      "parent": "group", "visible": true, "locked": false, "props": {{}}}}
                ],
                "components": {{}}, "metadata": {{}}
            }}"#
            );
            let file: SceneFile = serde_json::from_str(&json).expect("parses");
            RuntimeLevel::from_scene_file(file).expect("hydrates")
        };
        let local_of_cube = |level: &RuntimeLevel| {
            let scene = level.scene();
            let scene = scene.read();
            let cube = scene.world.entity_for("cube").unwrap();
            scene.world.get::<Transform>(cube).unwrap().position
        };
        assert_eq!(
            local_of_cube(&level("2.1")),
            [1.0, 2.0, 3.0],
            "2.1 held world values"
        );
        assert_eq!(
            local_of_cube(&level("2.2")),
            [11.0, 2.0, 3.0],
            "2.2 holds local values"
        );
    }

    /// Objects land in the shared store with transforms/hierarchy/
    /// visibility intact -- one copy of state, owned by SceneDB.
    #[test]
    fn load_hydrates_objects_hierarchy_and_visibility_into_the_store() {
        let level = sample_level();
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;

        let sun = world.entity_for("sun").expect("sun loaded");
        assert_eq!(
            world.get::<Transform>(sun).unwrap().position,
            [1.0, 5.0, 2.0]
        );
        assert_eq!(world.get::<Visibility>(sun).unwrap().visible, true);

        let cube = world.entity_for("cube").expect("cube loaded");
        assert_eq!(
            *world.get::<Visibility>(cube).unwrap(),
            Visibility {
                visible: false,
                locked: true
            }
        );
        let group = world.entity_for("group").unwrap();
        assert_eq!(world.parent_of(cube), Some(group));
        assert_eq!(world.children_of(Some(group)), vec![cube]);
    }

    /// A zero budget still advances one unit per step, the world is partial in
    /// between (what a frame thread shows while the level arrives), and the
    /// result is the level a single-shot load builds.
    #[test]
    fn stepped_hydration_makes_progress_and_matches_a_single_shot() {
        let file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        let registry = pulsar_class::ClassRegistry::default();
        let mut world = crate::scene::new_scene().world;
        let mut hydration = RuntimeLevel::prepare_from_file_for_test(file, &registry)
            .begin_hydration()
            .expect("begins");

        let mut steps = 0;
        let mut seen_partial = false;
        let extras = loop {
            steps += 1;
            assert!(steps < 100, "hydration never finished");
            match hydration
                .step(&mut world, std::time::Duration::ZERO)
                .expect("steps")
            {
                HydrationProgress::More => {
                    seen_partial |=
                        world.entity_for("sun").is_some() && world.entity_for("cube").is_none();
                }
                HydrationProgress::Done(extras) => break extras,
            }
        };
        assert!(steps > 3, "work was split into units, got {steps} steps");
        assert!(seen_partial, "objects arrive before the whole level does");
        assert!(extras.editor_camera.is_some());

        let whole = sample_level();
        let whole = whole.scene();
        let whole = whole.read();
        for id in ["sun", "group", "cube"] {
            let stepped = world.entity_for(id).expect("stepped has it");
            let single = whole.world.entity_for(id).unwrap();
            assert_eq!(
                world.get::<Transform>(stepped).unwrap().position,
                whole.world.get::<Transform>(single).unwrap().position
            );
        }
        let cube = world.entity_for("cube").unwrap();
        assert_eq!(world.parent_of(cube), world.entity_for("group"));
    }

    #[test]
    fn editor_camera_is_extracted_when_present() {
        let level = sample_level();
        assert_eq!(
            level.editor_camera(),
            Some(EditorCamera {
                position: [10.0, 20.0, 30.0],
                yaw: 1.0,
                pitch: -0.25
            })
        );
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let json = SAMPLE_LEVEL.replace("\"version\": \"2.1\"", "\"version\": \"9.9\"");
        let file: SceneFile = serde_json::from_str(&json).unwrap();
        // `.err().unwrap()` rather than `.unwrap_err()` -- the latter needs
        // `RuntimeLevel: Debug` (the `Ok` type), which it doesn't implement
        // (it holds a `SceneDb`, which doesn't either).
        assert_eq!(
            RuntimeLevel::from_scene_file(file).err().unwrap(),
            RuntimeLevelError::UnsupportedVersion("9.9".into())
        );
    }

    /// Stable ids survive exactly as authored (save/load identity), unlike
    /// raw Entity bits.
    #[test]
    fn stable_ids_round_trip_as_authored() {
        let level = sample_level();
        let scene = level.scene();
        let scene = scene.read();
        let world = &scene.world;
        let cube = world.entity_for("cube").unwrap();
        assert_eq!(
            world.get::<StableId>(cube).map(|s| s.0.clone()),
            Some("cube".into())
        );
    }

    /// Legacy `blueprint_bindings` are read only through the class
    /// migration: a binding becomes the object's `ClassInstance`, with its
    /// overrides as variable overrides, which the script driver follows.
    /// Nothing is left for hosts to apply.
    #[test]
    fn legacy_blueprint_bindings_load_as_class_instances() {
        let project = tempfile::tempdir().unwrap();
        let class_dir = project.path().join("src").join("classes").join("TickProbe");
        std::fs::create_dir_all(&class_dir).unwrap();
        std::fs::write(class_dir.join("graph_save.json"), "{}").unwrap();
        let registry = pulsar_class::ClassRegistry::scan(project.path());

        let mut file: SceneFile = serde_json::from_str(SAMPLE_LEVEL).expect("sample parses");
        file.blueprint_bindings.insert(
            "cube".to_string(),
            vec![pulsar_scene::BlueprintBinding {
                class_name: "TickProbe".to_string(),
                overrides: [("speed".to_string(), serde_json::json!(2.5))].into(),
            }],
        );
        let level = RuntimeLevel::from_scene_file_with_classes(file, &registry)
            .expect("bound shape hydrates");
        assert!(level.editor_camera().is_some(), "camera extras unchanged");
        let scene = level.scene();
        let scene = scene.read();
        let cube = scene.world.entity_for("cube").unwrap();
        let instance = pulsar_class::world::class_instance_of(&scene.world, cube)
            .expect("binding migrated to a ClassInstance");
        assert_eq!(instance.class_name, "TickProbe");
        assert_eq!(instance.class, registry.by_name("TickProbe").unwrap().id);
        assert_eq!(instance.variable_overrides["speed"], serde_json::json!(2.5));
    }
}
