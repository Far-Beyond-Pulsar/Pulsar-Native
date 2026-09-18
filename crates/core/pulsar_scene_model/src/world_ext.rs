//! Scene operations expressed directly on `pulsar_scenedb::World`.
//!
//! Nothing here owns state. Identity lookup, hierarchy and selection are all
//! derived from the components in [`crate::components`], so there is no second
//! source of truth to keep in step with the world (and nothing to rebuild when a
//! world is swapped, undone or reloaded).

use std::sync::atomic::{AtomicU64, Ordering};

use pulsar_scenedb::{Entity, World};

use crate::components::{
    Name, Parent, RenderProps, Selected, SiblingIndex, StableId, Transform, Visibility,
};
use crate::ObjectType;

/// Monotonic source for [`SiblingIndex`] values and generated ids. Process
/// wide on purpose: it only has to keep spawn order stable and ids unique, and
/// it never needs to agree between worlds.
static NEXT_ORDINAL: AtomicU64 = AtomicU64::new(1);

fn next_ordinal() -> u64 {
    NEXT_ORDINAL.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SceneError {
    #[error("stable id '{0}' is already in use")]
    DuplicateId(String),
    #[error("reparenting '{0}' under '{1}' would create a cycle")]
    WouldCreateCycle(String, String),
    #[error("object '{1}' references parent '{0}', which hasn't been loaded yet")]
    UnknownParent(String, String),
}

/// Everything needed to spawn one scene object.
#[derive(Clone, Debug)]
pub struct SpawnObject {
    /// Generated (`object_N`) when `None`.
    pub stable_id: Option<String>,
    pub name: String,
    pub parent: Option<Entity>,
    pub transform: Transform,
    pub visibility: Visibility,
    pub object_type: ObjectType,
}

impl SpawnObject {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            stable_id: None,
            name: name.into(),
            parent: None,
            transform: Transform::default(),
            visibility: Visibility::default(),
            object_type: ObjectType::Empty,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.stable_id = Some(id.into());
        self
    }

    pub fn with_parent(mut self, parent: Option<Entity>) -> Self {
        self.parent = parent;
        self
    }

    pub fn with_object_type(mut self, object_type: ObjectType) -> Self {
        self.object_type = object_type;
        self
    }

    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    pub fn with_visibility(mut self, visibility: Visibility) -> Self {
        self.visibility = visibility;
        self
    }
}

/// Scene helpers on `World`. Import the trait to use them.
pub trait SceneWorldExt {
    // ── Identity ────────────────────────────────────────────────────────
    fn entity_for(&self, stable_id: &str) -> Option<Entity>;
    fn stable_id_of(&self, entity: Entity) -> Option<&str>;

    // ── Hierarchy ───────────────────────────────────────────────────────
    fn parent_of(&self, entity: Entity) -> Option<Entity>;
    /// Ordered children of `parent`, or the root objects for `None`.
    fn children_of(&self, parent: Option<Entity>) -> Vec<Entity>;
    fn reparent(&mut self, entity: Entity, new_parent: Option<Entity>) -> Result<(), SceneError>;
    fn reorder_sibling(&mut self, entity: Entity, target: Entity) -> bool;
    fn move_sibling_up(&mut self, entity: Entity) -> bool;
    fn move_sibling_down(&mut self, entity: Entity) -> bool;

    // ── Lifecycle ───────────────────────────────────────────────────────
    fn spawn_object(&mut self, spec: SpawnObject) -> Result<Entity, SceneError>;
    /// Despawn `entity` and, recursively, its children.
    fn despawn_tree(&mut self, entity: Entity);

    // ── Selection ───────────────────────────────────────────────────────
    /// Single selection: clears every other [`Selected`] marker.
    fn select(&mut self, entity: Option<Entity>);
    fn selected_entity(&self) -> Option<Entity>;
    fn selected_id(&self) -> Option<String>;

    // ── Queries ─────────────────────────────────────────────────────────
    fn camera_entities(&self) -> Vec<Entity>;
}

impl SceneWorldExt for World {
    fn entity_for(&self, stable_id: &str) -> Option<Entity> {
        self.query::<&StableId>()
            .find(|(_, id)| id.0 == stable_id)
            .map(|(entity, _)| entity)
    }

    fn stable_id_of(&self, entity: Entity) -> Option<&str> {
        self.get::<StableId>(entity).map(|id| id.0.as_str())
    }

    fn parent_of(&self, entity: Entity) -> Option<Entity> {
        self.get::<Parent>(entity).map(|parent| parent.0)
    }

    fn children_of(&self, parent: Option<Entity>) -> Vec<Entity> {
        let mut children: Vec<(SiblingIndex, Entity)> = self
            .query::<&StableId>()
            .map(|(entity, _)| entity)
            .filter(|entity| self.parent_of(*entity) == parent)
            .map(|entity| {
                let order = self.get::<SiblingIndex>(entity).copied().unwrap_or_default();
                (order, entity)
            })
            .collect();
        children.sort_by_key(|(order, entity)| (*order, entity.bits()));
        children.into_iter().map(|(_, entity)| entity).collect()
    }

    fn reparent(&mut self, entity: Entity, new_parent: Option<Entity>) -> Result<(), SceneError> {
        if let Some(target) = new_parent {
            let mut cursor = Some(target);
            while let Some(current) = cursor {
                if current == entity {
                    return Err(SceneError::WouldCreateCycle(
                        self.stable_id_of(entity).unwrap_or_default().to_string(),
                        self.stable_id_of(target).unwrap_or_default().to_string(),
                    ));
                }
                cursor = self.parent_of(current);
            }
        }
        match new_parent {
            Some(parent) => {
                self.insert(entity, Parent(parent));
            }
            None => {
                self.remove::<Parent>(entity);
            }
        }
        // Appended after its new siblings.
        self.insert(entity, SiblingIndex(next_ordinal()));
        Ok(())
    }

    fn reorder_sibling(&mut self, entity: Entity, target: Entity) -> bool {
        if self.parent_of(entity) != self.parent_of(target) {
            return false;
        }
        let (Some(a), Some(b)) = (
            self.get::<SiblingIndex>(entity).copied(),
            self.get::<SiblingIndex>(target).copied(),
        ) else {
            return false;
        };
        // Equal indices would make the swap a no-op; give one side a fresh value.
        let a = if a == b {
            SiblingIndex(next_ordinal())
        } else {
            a
        };
        self.insert(entity, b);
        self.insert(target, a);
        true
    }

    fn move_sibling_up(&mut self, entity: Entity) -> bool {
        let siblings = self.children_of(self.parent_of(entity));
        match siblings.iter().position(|e| *e == entity) {
            Some(index) if index > 0 => self.reorder_sibling(entity, siblings[index - 1]),
            _ => false,
        }
    }

    fn move_sibling_down(&mut self, entity: Entity) -> bool {
        let siblings = self.children_of(self.parent_of(entity));
        match siblings.iter().position(|e| *e == entity) {
            Some(index) if index + 1 < siblings.len() => {
                self.reorder_sibling(entity, siblings[index + 1])
            }
            _ => false,
        }
    }

    fn spawn_object(&mut self, spec: SpawnObject) -> Result<Entity, SceneError> {
        let stable_id = match spec.stable_id {
            Some(id) => {
                if self.entity_for(&id).is_some() {
                    return Err(SceneError::DuplicateId(id));
                }
                id
            }
            None => loop {
                let candidate = format!("object_{}", next_ordinal());
                if self.entity_for(&candidate).is_none() {
                    break candidate;
                }
            },
        };
        // One archetype transition for the whole object, not one per component.
        let entity = self.spawn_bundle((
            StableId(stable_id),
            Name(spec.name),
            spec.transform,
            spec.visibility,
            spec.object_type,
            SiblingIndex(next_ordinal()),
            RenderProps::default(),
        ));
        if let Some(parent) = spec.parent {
            self.insert(entity, Parent(parent));
        }
        Ok(entity)
    }

    fn despawn_tree(&mut self, entity: Entity) {
        for child in self.children_of(Some(entity)) {
            self.despawn_tree(child);
        }
        self.despawn(entity);
    }

    fn select(&mut self, entity: Option<Entity>) {
        let previous: Vec<Entity> = self.query::<&Selected>().map(|(e, _)| e).collect();
        for other in previous {
            if Some(other) != entity {
                self.remove::<Selected>(other);
            }
        }
        if let Some(entity) = entity {
            if self.is_alive(entity) && self.get::<Selected>(entity).is_none() {
                self.insert(entity, Selected);
            }
        }
    }

    fn selected_entity(&self) -> Option<Entity> {
        self.query::<&Selected>().map(|(entity, _)| entity).next()
    }

    fn selected_id(&self) -> Option<String> {
        self.selected_entity()
            .and_then(|entity| self.stable_id_of(entity))
            .map(str::to_string)
    }

    fn camera_entities(&self) -> Vec<Entity> {
        self.query::<&ObjectType>()
            .filter(|(_, object_type)| matches!(object_type, ObjectType::Camera))
            .map(|(entity, _)| entity)
            .collect()
    }
}
