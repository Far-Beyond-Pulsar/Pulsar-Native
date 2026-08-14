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
}

impl WorldSceneStore {
    pub fn new() -> Self {
        Self { world: World::new(), by_stable_id: HashMap::new(), children: HashMap::new() }
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
        }

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
            None => false,
        }
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
            None => false,
        }
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
}

impl Default for WorldSceneStore {
    fn default() -> Self {
        Self::new()
    }
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
}
