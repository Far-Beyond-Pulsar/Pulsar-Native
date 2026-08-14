//! `World`/`Entity`-backed scene store (Phase B1, tracked at
//! [Far-Beyond-Pulsar/Pulsar-Native#553](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/553)).
//!
//! Replaces `SceneDb`/`SceneMetadataDb`/`ComponentDb` (this module's
//! siblings) with `pulsar_scenedb::World` as the live, authoritative scene
//! store. This is new, additive infrastructure for now -- `SceneDatabase`
//! (`ui_level_editor`, the ~250-call-site facade everything else in the
//! editor actually calls) hasn't been repointed at this yet. That's the
//! remaining, much larger mechanical step tracked in the same issue; this
//! module is the foundation it gets built on.
//!
//! ## Design decisions (from Pulsar-Native#553, carried into this module)
//!
//! - `Entity` is the live identity, replacing the old system's
//!   `ObjectId`/`EditorObjectId: String`. [`StableId`] is what survives a
//!   save/load round trip and what cross-reference fields (parent, portal
//!   links, ...) will eventually serialize as -- never a raw `Entity`'s bits.
//!   There is no public, non-crate-private `pulsar_scenedb` API for placing
//!   an `Entity` at a caller-chosen index/generation on load (the only thing
//!   that can, `World::force_spawn_in_archetype`, is `pub(crate)`, reachable
//!   only through the replication crate's `Snapshot`/`Delta::apply`, built
//!   for network resync -- not "load an arbitrary saved file fresh").
//! - [`Parent`] is a real `World` component, so it's versioned/tracked like
//!   everything else. The children-reverse-index (`WorldSceneStore`'s own
//!   `children` map) is auxiliary bookkeeping maintained alongside `World`,
//!   not a parallel authority -- the same shape `helio-scenedb`'s
//!   `HelioRenderSubsystem` already uses for its own `material_ids`/
//!   `sectioned_ids` maps. `pulsar_scenedb::relation::RelationIndex` was
//!   confirmed the wrong tool for this: it's built for *reciprocal pairwise*
//!   links (portal pairs), and its own reciprocity invariant actively
//!   misclassifies one-to-many parent-to-children as conflicts.
//! - Transform is flat, not hierarchy-derived -- confirmed by reading
//!   `pulsar_scene::format::SceneObject::world_position()`/etc.: a v1/v2
//!   file-format migration shim (prefer nested `transform`, fall back to
//!   legacy flat fields), already-resolved, already flat per-object, no
//!   runtime parent-chain composition happens anywhere today. A flat
//!   per-`Entity` [`Transform`] component is a direct fit, matching
//!   `HierarchyManager`'s own doc that it's *"independent of Helio Scene's
//!   transform hierarchy."*
//! - No `Pod`/`Copy` bound is required for a type to live in `World` at all
//!   -- `pulsar_scenedb::Component` is a blanket impl for
//!   `Any + Send + Sync + 'static`. That bound only applies to the
//!   *opt-in* `#[gpu(buffer = "...")]`/`scene_store` GPU-mirror path (Phase
//!   A), which is irrelevant to this module.
//!
//! ## What this module deliberately does NOT do yet
//!
//! - **Lock-free hot-path transform reads.** `SceneEntry`'s atomic
//!   `position`/`rotation`/`scale` fields exist so the render thread can
//!   read transforms without ever taking a lock. `World::get`/`get_mut`
//!   here go through the normal borrow-checked path instead. Whether that
//!   render-thread contract needs to be preserved (and how, if so -- e.g. a
//!   double-buffered snapshot published once per frame) is a real, separate
//!   design question for whoever wires this into the actual renderer sync
//!   path, not silently dropped or assumed away here.
//! - **Component instances** (the `ComponentInstance`/JSON-per-component
//!   data `ComponentDb` stores today). Phase B4/B5 (Pulsar-Native#555/#556)
//!   cover migrating real `#[engine_class]` component data onto this store
//!   via `world_mut().insert(entity, SomeRealComponent { .. })` directly --
//!   deliberately not re-invented as a separate concept here.
//! - **Wiring into `SceneDatabase`.** This module is usable standalone
//!   (proven by its own tests below) but nothing in `ui_level_editor` reads
//!   from it yet.

use crate::scene::{GizmoAxis, GizmoState, GizmoType, ObjectDirtyFlags};
use pulsar_scenedb::{Entity, World};
use std::collections::HashMap;

// ── Components ──────────────────────────────────────────────────────────────

/// Stable, human-readable identity for a scene object -- survives save/load
/// (unlike the raw `Entity` bits, which are only meaningful within one live
/// `World`'s lifetime). Every entity spawned through [`WorldSceneStore`] gets
/// exactly one, and it's unique within that store (enforced at spawn time).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StableId(pub String);

impl StableId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Parent-child relationship for the editor outliner. Confirmed independent
/// of transform resolution (see this module's top doc) -- purely
/// organizational, matching `HierarchyManager`'s existing, well-tested
/// design, just rekeyed from `String` to `Entity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parent(pub Entity);

/// Flat per-entity world-space transform. See this module's top doc for why
/// this is flat rather than composed from a parent chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self { position: [0.0; 3], rotation: [0.0; 3], scale: [1.0; 3] }
    }
}

/// Display name -- independent of [`StableId`], which is an opaque identity
/// string, not necessarily the human-editable display name (mirrors
/// `SceneEntryMeta::name` in the system this replaces).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name(pub String);

/// Editor visibility/lock flags. Mirrors `SceneEntry`'s atomic
/// `visible`/`locked` fields today, minus the lock-free-read requirement --
/// see this module's top doc ("What this module deliberately does NOT do
/// yet") for that gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visibility {
    pub visible: bool,
    pub locked: bool,
}

impl Default for Visibility {
    fn default() -> Self {
        Self { visible: true, locked: false }
    }
}

// ── WorldSceneStore ─────────────────────────────────────────────────────────

/// Errors from [`WorldSceneStore`] mutations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorldSceneStoreError {
    #[error("stable id '{0}' is already in use")]
    DuplicateId(String),
    #[error("reparenting '{0}' under '{1}' would create a cycle")]
    WouldCreateCycle(String, String),
    #[error("object '{1}' references parent '{0}', which hasn't been loaded yet")]
    UnknownParent(String, String),
}

/// `World`/`Entity`-backed live scene store. See this module's top doc for
/// the design decisions behind it and what it deliberately doesn't cover yet.
pub struct WorldSceneStore {
    world: World,
    /// `StableId` <-> `Entity`, both directions -- the save/load and
    /// cross-reference resolution bridge (Pulsar-Native#553 decision #2).
    /// The forward direction lives here; the reverse direction is just
    /// `world.get::<StableId>(entity)`, so it isn't duplicated in a second
    /// map.
    by_stable_id: HashMap<String, Entity>,
    /// Children reverse-index, auxiliary bookkeeping alongside `Parent`
    /// components (see this module's top doc). Key `None` = root-level.
    children: HashMap<Option<Entity>, Vec<Entity>>,
    /// Currently selected entity, if any. Mirrors `SceneDbInner::selected`.
    selected: Option<Entity>,
    /// Gizmo state for the level editor. Mirrors `SceneDbInner::gizmo_state`
    /// -- reuses `crate::scene`'s existing `GizmoState`/`GizmoType`/
    /// `GizmoAxis` types rather than redefining them, since this store lives
    /// in the same crate and nothing about those types is `SceneDb`-specific.
    gizmo_state: GizmoState,
    /// Per-entity accumulated dirty flags since the last
    /// [`Self::take_dirty_flags`]/[`Self::drain_dirty`] for that entity.
    /// Entity-keyed internally (the real identity); the public API mirrors
    /// `SceneDb`'s stable-id-string-keyed shape (see this module's "What
    /// this deliberately does NOT do yet" doc re: lock-free reads -- this
    /// whole store is meant to live behind one `RwLock` shared with the
    /// renderer, not per-field atomics, so a plain `HashMap` here is
    /// consistent with that design rather than a regression from it).
    dirty: HashMap<Entity, ObjectDirtyFlags>,
    /// Monotonic counter, bumped once per [`Self::publish`] call (i.e. once
    /// per mutation that should cause a render-thread resync). Mirrors
    /// `SceneDb::dirty_gen()`.
    dirty_gen: u64,
    /// Stable ids of entities despawned since the last
    /// [`Self::take_removed_ids`]. Mirrors `SceneDb::take_removed_ids()`.
    removed_since_drain: Vec<String>,
    /// Monotonic counter, bumped on every mutation. Mirrors
    /// `SceneDb::render_revision()` -- the renderer's "has anything changed
    /// since I last synced" check.
    render_revision: u64,
}

impl WorldSceneStore {
    pub fn new() -> Self {
        Self {
            world: World::new(),
            by_stable_id: HashMap::new(),
            children: HashMap::new(),
            selected: None,
            gizmo_state: GizmoState::default(),
            dirty: HashMap::new(),
            dirty_gen: 0,
            removed_since_drain: Vec::new(),
            render_revision: 0,
        }
    }

    /// Record that `entity` changed in a way the renderer cares about, and
    /// bump both revision counters. Mirrors `SceneDb::publish` -- called from
    /// every mutator below, not something callers invoke directly.
    fn publish(&mut self, entity: Entity, flags: ObjectDirtyFlags) {
        self.dirty.entry(entity).or_insert_with(ObjectDirtyFlags::empty).insert(flags);
        self.dirty_gen = self.dirty_gen.saturating_add(1);
        self.render_revision = self.render_revision.saturating_add(1);
    }

    /// Direct access to the underlying `World`, for callers that need to
    /// `insert`/`get`/`get_mut` real component types (e.g. `pulsar_rendering`
    /// components, once Phase B4/B5 land) beyond this store's own CRUD API.
    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    // ── Object creation / deletion ──────────────────────────────────────

    /// Spawn a new object with the given stable id (auto-generated if
    /// `None`), name, and parent. Mirrors `SceneDb::add_object`'s contract.
    pub fn spawn(
        &mut self,
        stable_id: Option<String>,
        name: impl Into<String>,
        parent: Option<Entity>,
    ) -> Result<Entity, WorldSceneStoreError> {
        let stable_id = match stable_id {
            Some(id) => {
                if self.by_stable_id.contains_key(&id) {
                    return Err(WorldSceneStoreError::DuplicateId(id));
                }
                id
            }
            None => self.generate_stable_id(),
        };

        // `World::spawn()` + individual `insert()` calls rather than
        // `spawn_bundle` -- this workspace's currently-pinned `pulsar_scenedb`
        // rev (root `Cargo.toml`, predates SceneDB#44/the `Bundle` trait's
        // introduction) doesn't have `spawn_bundle` yet. Costs a few extra
        // archetype migrations per spawn versus the single-migration bundle
        // path (see `pulsar_scenedb`'s own `bundle.rs` doc) -- an acceptable
        // tradeoff for new, unproven infrastructure; revisit once the pin
        // catches up (tracked at Pulsar-Native#560).
        let entity = self.world.spawn();
        self.world.insert(entity, StableId(stable_id.clone()));
        self.world.insert(entity, Name(name.into()));
        self.world.insert(entity, Transform::default());
        self.world.insert(entity, Visibility::default());

        if let Some(parent_entity) = parent {
            self.world.insert(entity, Parent(parent_entity));
        }

        self.by_stable_id.insert(stable_id, entity);
        self.children.entry(parent).or_default().push(entity);
        self.publish(entity, ObjectDirtyFlags::all());

        Ok(entity)
    }

    /// Remove an object and recursively its children. Mirrors
    /// `SceneDb::remove_object`/`HierarchyManager`'s recursive-delete contract
    /// (that one leaves recursion to the caller; this one does it directly,
    /// since it owns both the hierarchy index and the entities themselves).
    pub fn despawn(&mut self, entity: Entity) {
        // Collect children first -- despawning mutates `self.children`.
        let kids = self.children.remove(&Some(entity)).unwrap_or_default();
        for child in kids {
            self.despawn(child);
        }

        // Detach from parent's child list.
        let parent = self.world.get::<Parent>(entity).map(|p| p.0);
        if let Some(siblings) = self.children.get_mut(&parent) {
            siblings.retain(|&e| e != entity);
        }

        if let Some(id) = self.world.get::<StableId>(entity) {
            let id = id.0.clone();
            self.by_stable_id.remove(&id);
            self.removed_since_drain.push(id);
        }
        self.dirty.remove(&entity);
        if self.selected == Some(entity) {
            self.selected = None;
        }
        self.dirty_gen = self.dirty_gen.saturating_add(1);
        self.render_revision = self.render_revision.saturating_add(1);

        self.world.despawn(entity);
    }

    // ── Lookup ───────────────────────────────────────────────────────────

    pub fn entity_for(&self, stable_id: &str) -> Option<Entity> {
        self.by_stable_id.get(stable_id).copied()
    }

    pub fn stable_id_of(&self, entity: Entity) -> Option<&str> {
        self.world.get::<StableId>(entity).map(|id| id.0.as_str())
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        self.world.is_alive(entity)
    }

    // ── Hierarchy ────────────────────────────────────────────────────────

    pub fn parent_of(&self, entity: Entity) -> Option<Entity> {
        self.world.get::<Parent>(entity).map(|p| p.0)
    }

    /// Ordered children of `parent`, or root-level entities if `None`.
    pub fn children_of(&self, parent: Option<Entity>) -> &[Entity] {
        self.children.get(&parent).map(Vec::as_slice).unwrap_or(&[])
    }

    fn is_ancestor_of(&self, potential_ancestor: Entity, of: Entity) -> bool {
        let mut current = of;
        while let Some(parent) = self.parent_of(current) {
            if parent == potential_ancestor {
                return true;
            }
            current = parent;
        }
        false
    }

    /// Reparent `entity` under `new_parent` (`None` = root). Cycle-checked
    /// (including self-parenting), same contract as
    /// `HierarchyManager::reparent_object`/`SceneDb::reparent_object`.
    pub fn reparent(
        &mut self,
        entity: Entity,
        new_parent: Option<Entity>,
    ) -> Result<(), WorldSceneStoreError> {
        if let Some(new_parent_entity) = new_parent {
            if new_parent_entity == entity || self.is_ancestor_of(entity, new_parent_entity) {
                let entity_id = self.stable_id_of(entity).unwrap_or_default().to_string();
                let parent_id = self.stable_id_of(new_parent_entity).unwrap_or_default().to_string();
                return Err(WorldSceneStoreError::WouldCreateCycle(entity_id, parent_id));
            }
        }

        let old_parent = self.parent_of(entity);
        if let Some(siblings) = self.children.get_mut(&old_parent) {
            siblings.retain(|&e| e != entity);
        }

        match new_parent {
            Some(p) => {
                self.world.insert(entity, Parent(p));
            }
            None => {
                self.world.remove::<Parent>(entity);
            }
        }
        self.children.entry(new_parent).or_default().push(entity);
        self.publish(entity, ObjectDirtyFlags::HIERARCHY | ObjectDirtyFlags::TRANSFORM);

        Ok(())
    }

    // ── Transform / name / visibility ───────────────────────────────────

    pub fn transform(&self, entity: Entity) -> Option<Transform> {
        self.world.get::<Transform>(entity).copied()
    }

    pub fn set_transform(&mut self, entity: Entity, transform: Transform) -> bool {
        match self.world.get_mut::<Transform>(entity) {
            Some(t) => {
                *t = transform;
                true
            }
            None => return false,
        };
        self.publish(entity, ObjectDirtyFlags::TRANSFORM);
        true
    }

    pub fn name(&self, entity: Entity) -> Option<&str> {
        self.world.get::<Name>(entity).map(|n| n.0.as_str())
    }

    pub fn set_name(&mut self, entity: Entity, name: impl Into<String>) -> bool {
        match self.world.get_mut::<Name>(entity) {
            Some(n) => {
                n.0 = name.into();
                true
            }
            None => return false,
        };
        self.publish(entity, ObjectDirtyFlags::NAME);
        true
    }

    pub fn visibility(&self, entity: Entity) -> Option<Visibility> {
        self.world.get::<Visibility>(entity).copied()
    }

    pub fn set_visibility(&mut self, entity: Entity, visibility: Visibility) -> bool {
        match self.world.get_mut::<Visibility>(entity) {
            Some(v) => {
                *v = visibility;
                true
            }
            None => return false,
        };
        self.publish(entity, ObjectDirtyFlags::VISIBILITY);
        true
    }

    // ── Selection ────────────────────────────────────────────────────────

    /// Select an object by stable id (`None` deselects). Mirrors
    /// `SceneDb::select_object`. Unknown ids are treated as a deselect --
    /// same permissive contract as `SceneDb` (selecting a since-removed id
    /// is a normal race between the UI and render threads, not an error).
    pub fn select_object(&mut self, stable_id: Option<String>) {
        self.selected = stable_id.and_then(|id| self.entity_for(&id));
    }

    /// Mirrors `SceneDb::get_selected_id`.
    pub fn get_selected_id(&self) -> Option<String> {
        self.selected.and_then(|e| self.stable_id_of(e)).map(str::to_string)
    }

    pub fn get_selected_entity(&self) -> Option<Entity> {
        self.selected
    }

    // ── Gizmo state ──────────────────────────────────────────────────────

    /// Mirrors `SceneDb::get_gizmo_state`.
    pub fn get_gizmo_state(&self) -> GizmoState {
        self.gizmo_state.clone()
    }

    /// Mirrors `SceneDb::set_gizmo_type`.
    pub fn set_gizmo_type(&mut self, gizmo_type: GizmoType) {
        self.gizmo_state.gizmo_type = gizmo_type;
    }

    /// Mirrors `SceneDb::set_gizmo_highlighted_axis`.
    pub fn set_gizmo_highlighted_axis(&mut self, axis: Option<GizmoAxis>) {
        self.gizmo_state.highlighted_axis = axis;
    }

    // ── Dirty / revision tracking ────────────────────────────────────────
    //
    // String-id-keyed, mirroring `SceneDb`'s exact shape -- these exist
    // because the renderer's sync loop (`HelioRenderer::sync_scene`/
    // `sync_scene_delta`) only ever holds stable-id strings, never `Entity`
    // values, across a frame boundary (it re-resolves ids from snapshots
    // every pass today). See this module's top doc for why this is a plain
    // `HashMap` behind one lock rather than `SceneDb`'s per-entry atomics.

    /// Mirrors `SceneDb::render_revision`.
    pub fn render_revision(&self) -> u64 {
        self.render_revision
    }

    /// Mirrors `SceneDb::dirty_gen`.
    pub fn dirty_gen(&self) -> u64 {
        self.dirty_gen
    }

    /// Mirrors `SceneDb::mark_dirty`. No-op if `id` doesn't resolve.
    pub fn mark_dirty(&mut self, id: &str, flags: ObjectDirtyFlags) {
        if let Some(entity) = self.entity_for(id) {
            self.publish(entity, flags);
        }
    }

    /// Mirrors `SceneDb::take_dirty_flags` -- returns and clears the flags
    /// accumulated for one object since the last call.
    pub fn take_dirty_flags(&mut self, id: &str) -> ObjectDirtyFlags {
        match self.entity_for(id) {
            Some(entity) => self.dirty.remove(&entity).unwrap_or_else(ObjectDirtyFlags::empty),
            None => ObjectDirtyFlags::empty(),
        }
    }

    /// Mirrors `SceneDb::drain_dirty` -- returns and clears every object's
    /// accumulated dirty flags since the last call.
    pub fn drain_dirty(&mut self) -> Vec<(String, ObjectDirtyFlags)> {
        std::mem::take(&mut self.dirty)
            .into_iter()
            .filter_map(|(entity, flags)| {
                self.stable_id_of(entity).map(|id| (id.to_string(), flags))
            })
            .collect()
    }

    /// Mirrors `SceneDb::take_removed_ids` -- stable ids despawned since the
    /// last call.
    pub fn take_removed_ids(&mut self) -> Vec<String> {
        std::mem::take(&mut self.removed_since_drain)
    }

    // ── String-id convenience wrappers ───────────────────────────────────
    //
    // `SceneDatabase`'s public API and the renderer's sync loop both operate
    // on stable-id strings, not raw `Entity` values -- these exist so that
    // rewiring, e.g. `HelioRenderer::handle_left_release`'s
    // `scene_db.apply_transform(&scene_id, pos, rot, scale)` call, is a
    // type/name swap rather than a control-flow rewrite. Callers that
    // already have an `Entity` in hand (most of `WorldSceneStore`'s own
    // internals, and anything iterating `to_snapshots()`) should keep using
    // the `Entity`-keyed primitives above instead -- these wrappers pay an
    // extra `HashMap` lookup each call.

    /// Mirrors `SceneDb::get_object`.
    pub fn get_object(&self, id: &str) -> Option<ObjectSnapshot> {
        let entity = self.entity_for(id)?;
        let parent = self.parent_of(entity).and_then(|p| self.stable_id_of(p)).map(str::to_string);
        Some(ObjectSnapshot {
            stable_id: id.to_string(),
            name: self.name(entity).unwrap_or_default().to_string(),
            parent,
            transform: self.transform(entity).unwrap_or_default(),
            visibility: self.visibility(entity).unwrap_or_default(),
        })
    }

    /// Mirrors `SceneDb::get_all_snapshots`. Same as [`Self::to_snapshots`]
    /// under a name that matches the call sites being migrated.
    pub fn get_all_snapshots(&self) -> Vec<ObjectSnapshot> {
        self.to_snapshots()
    }

    /// Mirrors `SceneDb::apply_transform`. No-op (returns `false`) if `id`
    /// doesn't resolve to a live entity.
    pub fn apply_transform(
        &mut self,
        id: &str,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    ) -> bool {
        match self.entity_for(id) {
            Some(entity) => self.set_transform(entity, Transform { position, rotation, scale }),
            None => false,
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn generate_stable_id(&self) -> String {
        // Matches SceneDb::add_object's `object_{n}` scheme closely enough
        // to feel like a drop-in replacement; not required to be
        // byte-identical to it -- callers only ever treat this as an opaque
        // string, never parse the number back out.
        let mut n = self.by_stable_id.len() as u64 + 1;
        loop {
            let candidate = format!("object_{n}");
            if !self.by_stable_id.contains_key(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    // ── Load / save bridge (Pulsar-Native#553 decision #2) ──────────────

    /// Build a fresh store from a flat list of snapshots. `snapshots` MUST be
    /// in parent-before-child order (matching `SceneDb::collect_dfs`'s
    /// existing contract, and what [`Self::to_snapshots`] itself produces) --
    /// a child referencing a parent not yet spawned is a real error, not
    /// silently dropped or reordered.
    ///
    /// This is the same bridge B6 (Pulsar-Native#557) needs to unify
    /// `pulsar_scene::format::SceneFile` and the editor's `LevelFile` onto
    /// one `World <-> JSON` path -- built here, generically over
    /// [`ObjectSnapshot`] rather than either concrete file format, so both
    /// can convert into/out of `ObjectSnapshot` and share this rather than
    /// duplicating it.
    pub fn load_from_snapshots(
        snapshots: &[ObjectSnapshot],
    ) -> Result<Self, WorldSceneStoreError> {
        let mut store = Self::new();
        for snap in snapshots {
            let parent = match &snap.parent {
                Some(parent_id) => Some(store.entity_for(parent_id).ok_or_else(|| {
                    WorldSceneStoreError::UnknownParent(parent_id.clone(), snap.stable_id.clone())
                })?),
                None => None,
            };
            let entity = store.spawn(Some(snap.stable_id.clone()), snap.name.clone(), parent)?;
            store.set_transform(entity, snap.transform);
            store.set_visibility(entity, snap.visibility);
        }
        Ok(store)
    }

    /// Serialize back out to the same flat shape, depth-first with parents
    /// before children -- so a round trip through [`Self::load_from_snapshots`]
    /// always succeeds on its own output.
    pub fn to_snapshots(&self) -> Vec<ObjectSnapshot> {
        let mut out = Vec::new();
        self.collect_snapshots_dfs(None, &mut out);
        out
    }

    fn collect_snapshots_dfs(&self, parent: Option<Entity>, out: &mut Vec<ObjectSnapshot>) {
        let parent_stable_id = parent.and_then(|p| self.stable_id_of(p)).map(str::to_string);
        for &entity in self.children_of(parent) {
            out.push(ObjectSnapshot {
                stable_id: self.stable_id_of(entity).unwrap_or_default().to_string(),
                name: self.name(entity).unwrap_or_default().to_string(),
                parent: parent_stable_id.clone(),
                transform: self.transform(entity).unwrap_or_default(),
                visibility: self.visibility(entity).unwrap_or_default(),
            });
            self.collect_snapshots_dfs(Some(entity), out);
        }
    }
}

impl Default for WorldSceneStore {
    fn default() -> Self {
        Self::new()
    }
}

/// A flat, ordered (parent-before-child) snapshot of one object -- the
/// interchange shape [`WorldSceneStore::load_from_snapshots`]/
/// [`WorldSceneStore::to_snapshots`] use to bridge `World` and any
/// serializable format (JSON today, whatever tomorrow). Deliberately close
/// in shape to the pre-B1 `SceneObjectSnapshot` (`scene::SceneObjectSnapshot`)
/// so callers migrating off that type have a direct field-by-field mapping,
/// not a redesign to learn too. `scene_path` isn't modeled here -- it's a
/// value *derived* from the parent chain (see `SceneDb::update_subtree_path`),
/// recomputable from `parent` at the point something actually needs it,
/// not independent data worth carrying through this bridge.
///
/// `props`/`component_instances` (real component data) are also NOT modeled
/// here -- that's Phase B4/B5's job (Pulsar-Native#555/#556), which inserts
/// real typed components into `World` directly rather than reinventing a
/// JSON side-channel on top of this bridge.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectSnapshot {
    pub stable_id: String,
    pub name: String,
    pub parent: Option<String>,
    pub transform: Transform,
    pub visibility: Visibility,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_assigns_a_stable_id_and_defaults() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(None, "Object", None).unwrap();
        assert_eq!(store.stable_id_of(e), Some("object_1"));
        assert_eq!(store.name(e), Some("Object"));
        assert_eq!(store.transform(e), Some(Transform::default()));
        assert_eq!(store.visibility(e), Some(Visibility::default()));
        assert_eq!(store.parent_of(e), None);
    }

    #[test]
    fn spawn_rejects_a_duplicate_stable_id() {
        let mut store = WorldSceneStore::new();
        store.spawn(Some("dup".into()), "A", None).unwrap();
        let err = store.spawn(Some("dup".into()), "B", None).unwrap_err();
        assert_eq!(err, WorldSceneStoreError::DuplicateId("dup".into()));
    }

    #[test]
    fn entity_for_and_stable_id_of_round_trip() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(Some("earth".into()), "Earth", None).unwrap();
        assert_eq!(store.entity_for("earth"), Some(e));
        assert_eq!(store.stable_id_of(e), Some("earth"));
    }

    #[test]
    fn despawn_removes_the_entity_and_its_stable_id() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(Some("gone".into()), "Gone", None).unwrap();
        store.despawn(e);
        assert!(!store.is_alive(e));
        assert_eq!(store.entity_for("gone"), None);
    }

    #[test]
    fn despawn_is_recursive_over_children() {
        let mut store = WorldSceneStore::new();
        let parent = store.spawn(Some("parent".into()), "Parent", None).unwrap();
        let child = store.spawn(Some("child".into()), "Child", Some(parent)).unwrap();
        let grandchild =
            store.spawn(Some("grandchild".into()), "Grandchild", Some(child)).unwrap();

        store.despawn(parent);

        assert!(!store.is_alive(parent));
        assert!(!store.is_alive(child));
        assert!(!store.is_alive(grandchild));
    }

    #[test]
    fn despawning_a_child_does_not_touch_its_siblings() {
        let mut store = WorldSceneStore::new();
        let parent = store.spawn(Some("parent".into()), "Parent", None).unwrap();
        let a = store.spawn(Some("a".into()), "A", Some(parent)).unwrap();
        let b = store.spawn(Some("b".into()), "B", Some(parent)).unwrap();

        store.despawn(a);

        assert!(!store.is_alive(a));
        assert!(store.is_alive(b));
        assert_eq!(store.children_of(Some(parent)), &[b]);
    }

    #[test]
    fn children_of_reflects_spawn_and_reparent() {
        let mut store = WorldSceneStore::new();
        let root = store.spawn(Some("root".into()), "Root", None).unwrap();
        let a = store.spawn(Some("a".into()), "A", Some(root)).unwrap();
        let b = store.spawn(Some("b".into()), "B", Some(root)).unwrap();
        assert_eq!(store.children_of(Some(root)), &[a, b]);
        assert_eq!(store.children_of(None), &[root]);

        store.reparent(a, None).unwrap();
        assert_eq!(store.children_of(Some(root)), &[b]);
        assert_eq!(store.children_of(None), &[root, a]);
        assert_eq!(store.parent_of(a), None);
    }

    #[test]
    fn reparent_rejects_a_cycle() {
        let mut store = WorldSceneStore::new();
        let parent = store.spawn(Some("parent".into()), "Parent", None).unwrap();
        let child = store.spawn(Some("child".into()), "Child", Some(parent)).unwrap();

        let err = store.reparent(parent, Some(child)).unwrap_err();
        assert_eq!(
            err,
            WorldSceneStoreError::WouldCreateCycle("parent".into(), "child".into())
        );
        // Must be a no-op on failure -- hierarchy stays exactly as it was.
        assert_eq!(store.parent_of(parent), None);
        assert_eq!(store.children_of(Some(parent)), &[child]);
    }

    #[test]
    fn reparent_rejects_self_parenting() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(Some("self".into()), "Self", None).unwrap();
        let err = store.reparent(e, Some(e)).unwrap_err();
        assert_eq!(err, WorldSceneStoreError::WouldCreateCycle("self".into(), "self".into()));
    }

    #[test]
    fn set_transform_and_name_and_visibility_round_trip() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(None, "Object", None).unwrap();

        let t = Transform { position: [1.0, 2.0, 3.0], rotation: [0.0; 3], scale: [1.0; 3] };
        assert!(store.set_transform(e, t));
        assert_eq!(store.transform(e), Some(t));

        assert!(store.set_name(e, "Renamed"));
        assert_eq!(store.name(e), Some("Renamed"));

        let v = Visibility { visible: false, locked: true };
        assert!(store.set_visibility(e, v));
        assert_eq!(store.visibility(e), Some(v));
    }

    #[test]
    fn mutators_on_a_despawned_entity_return_false_or_none() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(None, "Gone", None).unwrap();
        store.despawn(e);

        assert!(!store.set_transform(e, Transform::default()));
        assert!(!store.set_name(e, "x"));
        assert_eq!(store.transform(e), None);
        assert_eq!(store.name(e), None);
    }

    #[test]
    fn deep_hierarchy_reparent_keeps_descendants_attached() {
        // Moving a subtree (not just a leaf) must carry its own children
        // along -- only the moved node's parent link changes.
        let mut store = WorldSceneStore::new();
        let root_a = store.spawn(Some("root_a".into()), "RootA", None).unwrap();
        let root_b = store.spawn(Some("root_b".into()), "RootB", None).unwrap();
        let mid = store.spawn(Some("mid".into()), "Mid", Some(root_a)).unwrap();
        let leaf = store.spawn(Some("leaf".into()), "Leaf", Some(mid)).unwrap();

        store.reparent(mid, Some(root_b)).unwrap();

        assert_eq!(store.parent_of(mid), Some(root_b));
        assert_eq!(store.children_of(Some(root_a)), &[] as &[Entity]);
        assert_eq!(store.children_of(Some(root_b)), &[mid]);
        // `leaf` never moved directly -- still under `mid`, wherever `mid` is.
        assert_eq!(store.parent_of(leaf), Some(mid));
        assert_eq!(store.children_of(Some(mid)), &[leaf]);
    }

    // ── Load / save bridge ──────────────────────────────────────────────

    fn snap(stable_id: &str, parent: Option<&str>) -> ObjectSnapshot {
        ObjectSnapshot {
            stable_id: stable_id.to_string(),
            name: stable_id.to_string(),
            parent: parent.map(str::to_string),
            transform: Transform::default(),
            visibility: Visibility::default(),
        }
    }

    #[test]
    fn load_from_snapshots_reconstructs_the_hierarchy() {
        let snapshots = vec![
            snap("root", None),
            snap("child_a", Some("root")),
            snap("child_b", Some("root")),
            snap("grandchild", Some("child_a")),
        ];

        let store = WorldSceneStore::load_from_snapshots(&snapshots).unwrap();

        let root = store.entity_for("root").unwrap();
        let child_a = store.entity_for("child_a").unwrap();
        let child_b = store.entity_for("child_b").unwrap();
        let grandchild = store.entity_for("grandchild").unwrap();

        assert_eq!(store.children_of(None), &[root]);
        assert_eq!(store.children_of(Some(root)), &[child_a, child_b]);
        assert_eq!(store.children_of(Some(child_a)), &[grandchild]);
        assert_eq!(store.parent_of(grandchild), Some(child_a));
    }

    #[test]
    fn load_from_snapshots_rejects_a_forward_reference() {
        // "child" lists "root" as its parent, but "root" hasn't been loaded
        // yet -- must error, not silently treat it as root-level or panic.
        let snapshots = vec![snap("child", Some("root")), snap("root", None)];

        // `.err().unwrap()` rather than `.unwrap_err()` -- the latter needs
        // `WorldSceneStore: Debug` (the `Ok` type) too, which it doesn't
        // implement (it holds a `World`, which doesn't either).
        let err = WorldSceneStore::load_from_snapshots(&snapshots).err().unwrap();
        assert_eq!(err, WorldSceneStoreError::UnknownParent("root".into(), "child".into()));
    }

    #[test]
    fn to_snapshots_round_trips_through_load_from_snapshots() {
        // True DFS pre-order (root, child_a, *child_a's own subtree*, then
        // child_b) -- what `to_snapshots` actually produces (visits a
        // child's whole subtree before its next sibling). `load_from_
        // snapshots` itself only requires "parent before child" (any such
        // ordering works, proven by `load_from_snapshots_reconstructs_the_
        // hierarchy`'s different ordering above) -- this test specifically
        // checks the round trip is exact, which requires matching
        // `to_snapshots`'s own canonical order up front.
        let original = vec![
            snap("root", None),
            snap("child_a", Some("root")),
            snap("grandchild", Some("child_a")),
            snap("child_b", Some("root")),
        ];

        let store = WorldSceneStore::load_from_snapshots(&original).unwrap();
        let round_tripped = store.to_snapshots();

        assert_eq!(round_tripped, original);

        // And the round-tripped output must itself load cleanly -- proves
        // `to_snapshots` really does emit parent-before-child order, not
        // just "the same set in some order".
        let reloaded = WorldSceneStore::load_from_snapshots(&round_tripped).unwrap();
        assert_eq!(reloaded.to_snapshots(), round_tripped);
    }

    #[test]
    fn to_snapshots_preserves_transform_and_visibility_edits() {
        let mut store = WorldSceneStore::load_from_snapshots(&[snap("obj", None)]).unwrap();
        let entity = store.entity_for("obj").unwrap();
        let t = Transform { position: [4.0, 5.0, 6.0], rotation: [0.0; 3], scale: [2.0; 3] };
        let v = Visibility { visible: false, locked: true };
        store.set_transform(entity, t);
        store.set_visibility(entity, v);

        let snaps = store.to_snapshots();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].transform, t);
        assert_eq!(snaps[0].visibility, v);
    }

    // ── Selection ────────────────────────────────────────────────────────

    #[test]
    fn select_object_by_stable_id_round_trips() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(Some("obj".into()), "Object", None).unwrap();

        store.select_object(Some("obj".into()));
        assert_eq!(store.get_selected_id(), Some("obj".to_string()));
        assert_eq!(store.get_selected_entity(), Some(e));

        store.select_object(None);
        assert_eq!(store.get_selected_id(), None);
        assert_eq!(store.get_selected_entity(), None);
    }

    #[test]
    fn selecting_an_unknown_id_deselects() {
        let mut store = WorldSceneStore::new();
        store.spawn(Some("obj".into()), "Object", None).unwrap();
        store.select_object(Some("obj".into()));

        store.select_object(Some("does_not_exist".into()));

        assert_eq!(store.get_selected_id(), None);
    }

    #[test]
    fn despawning_the_selected_entity_clears_selection() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(Some("obj".into()), "Object", None).unwrap();
        store.select_object(Some("obj".into()));

        store.despawn(e);

        assert_eq!(store.get_selected_id(), None);
        assert_eq!(store.get_selected_entity(), None);
    }

    // ── Gizmo state ──────────────────────────────────────────────────────

    #[test]
    fn gizmo_state_defaults_and_round_trips() {
        let mut store = WorldSceneStore::new();
        assert_eq!(store.get_gizmo_state().gizmo_type, GizmoType::None);
        assert_eq!(store.get_gizmo_state().highlighted_axis, None);

        store.set_gizmo_type(GizmoType::Rotate);
        store.set_gizmo_highlighted_axis(Some(GizmoAxis::Y));

        let state = store.get_gizmo_state();
        assert_eq!(state.gizmo_type, GizmoType::Rotate);
        assert_eq!(state.highlighted_axis, Some(GizmoAxis::Y));
    }

    // ── Dirty / revision tracking ────────────────────────────────────────

    #[test]
    fn spawn_marks_the_new_entity_fully_dirty() {
        let mut store = WorldSceneStore::new();
        store.spawn(Some("obj".into()), "Object", None).unwrap();

        assert_eq!(store.take_dirty_flags("obj"), ObjectDirtyFlags::all());
        // Draining clears it -- a second take is empty.
        assert_eq!(store.take_dirty_flags("obj"), ObjectDirtyFlags::empty());
    }

    #[test]
    fn apply_transform_marks_transform_dirty_and_bumps_revision() {
        let mut store = WorldSceneStore::new();
        store.spawn(Some("obj".into()), "Object", None).unwrap();
        store.take_dirty_flags("obj"); // clear the spawn-time dirty flags
        let revision_before = store.render_revision();

        let ok = store.apply_transform("obj", [1.0, 2.0, 3.0], [0.0; 3], [1.0; 3]);

        assert!(ok);
        assert_eq!(store.take_dirty_flags("obj"), ObjectDirtyFlags::TRANSFORM);
        assert!(store.render_revision() > revision_before);
        assert_eq!(
            store.get_object("obj").unwrap().transform,
            Transform { position: [1.0, 2.0, 3.0], rotation: [0.0; 3], scale: [1.0; 3] }
        );
    }

    #[test]
    fn apply_transform_on_an_unknown_id_is_a_no_op() {
        let mut store = WorldSceneStore::new();
        assert!(!store.apply_transform("nope", [1.0; 3], [0.0; 3], [1.0; 3]));
    }

    #[test]
    fn drain_dirty_returns_every_object_and_clears_all_of_them() {
        let mut store = WorldSceneStore::new();
        store.spawn(Some("a".into()), "A", None).unwrap();
        store.spawn(Some("b".into()), "B", None).unwrap();

        let drained = store.drain_dirty();
        let ids: std::collections::HashSet<_> = drained.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(ids, ["a".to_string(), "b".to_string()].into_iter().collect());

        assert!(store.drain_dirty().is_empty());
        assert_eq!(store.take_dirty_flags("a"), ObjectDirtyFlags::empty());
    }

    #[test]
    fn take_removed_ids_reports_despawned_entities_once() {
        let mut store = WorldSceneStore::new();
        let e = store.spawn(Some("gone".into()), "Gone", None).unwrap();
        store.despawn(e);

        assert_eq!(store.take_removed_ids(), vec!["gone".to_string()]);
        assert!(store.take_removed_ids().is_empty());
    }

    #[test]
    fn despawn_reports_every_removed_descendant() {
        let mut store = WorldSceneStore::new();
        let parent = store.spawn(Some("parent".into()), "Parent", None).unwrap();
        store.spawn(Some("child".into()), "Child", Some(parent)).unwrap();

        store.despawn(parent);

        let removed = store.take_removed_ids();
        let removed: std::collections::HashSet<_> = removed.into_iter().collect();
        assert_eq!(removed, ["parent".to_string(), "child".to_string()].into_iter().collect());
    }

    #[test]
    fn mark_dirty_on_an_unknown_id_is_a_no_op() {
        let mut store = WorldSceneStore::new();
        let revision_before = store.render_revision();
        store.mark_dirty("nope", ObjectDirtyFlags::TRANSFORM);
        assert_eq!(store.render_revision(), revision_before);
    }

    // ── String-id convenience wrappers ───────────────────────────────────

    #[test]
    fn get_object_resolves_parent_as_a_stable_id() {
        let mut store = WorldSceneStore::new();
        let parent = store.spawn(Some("parent".into()), "Parent", None).unwrap();
        store.spawn(Some("child".into()), "Child", Some(parent)).unwrap();

        let child = store.get_object("child").unwrap();
        assert_eq!(child.parent, Some("parent".to_string()));
        assert_eq!(child.name, "Child");
    }

    #[test]
    fn get_object_on_an_unknown_id_is_none() {
        let store = WorldSceneStore::new();
        assert_eq!(store.get_object("nope"), None);
    }

    #[test]
    fn get_all_snapshots_matches_to_snapshots() {
        let mut store = WorldSceneStore::new();
        store.spawn(Some("a".into()), "A", None).unwrap();
        store.spawn(Some("b".into()), "B", None).unwrap();
        assert_eq!(store.get_all_snapshots(), store.to_snapshots());
    }
}
