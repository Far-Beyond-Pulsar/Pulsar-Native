//! The browser's rows: the index flattened for one of its four views, with
//! the current search and what is open. The list draws only the rows on
//! screen.

use std::collections::{HashMap, HashSet};

use pulsar_scenedb::{ComponentId, Entity};

use super::index::SceneIndex;
use super::query::Query;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Scene objects as the level's tree.
    #[default]
    Objects,
    /// Every entity, flat.
    Entities,
    /// Component types, each opening to the entities that have it.
    Components,
    /// Archetypes, each opening to its entities.
    Archetypes,
}

impl Mode {
    pub const ALL: [Self; 4] = [Self::Objects, Self::Entities, Self::Components, Self::Archetypes];

    pub fn label(self) -> &'static str {
        match self {
            Self::Objects => "Objects",
            Self::Entities => "Entities",
            Self::Components => "Types",
            Self::Archetypes => "Archetypes",
        }
    }
}

/// A row that can be opened or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Object(Entity),
    Type(ComponentId),
    Archetype(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Entity {
        /// Index into [`SceneIndex::entities`].
        index: u32,
        depth: u16,
        /// `Some(open)` for an object with children.
        open: Option<bool>,
        /// The component type the row is listed under, which selecting it
        /// should show.
        under: Option<ComponentId>,
        /// In a filtered tree, `false` for an ancestor shown only to place a
        /// match.
        matched: bool,
    },
    Type {
        /// Index into [`SceneIndex::types`].
        index: u32,
        open: bool,
    },
    Archetype {
        /// Index into [`SceneIndex::archetypes`].
        index: u32,
        open: bool,
        /// Entities listed under it (those matching the search).
        shown: u32,
    },
}

impl Row {
    pub fn node(&self, index: &SceneIndex) -> Option<Node> {
        match *self {
            Row::Entity {
                index: row, open, ..
            } => open.map(|_| Node::Object(index.entities[row as usize].entity)),
            Row::Type { index: row, .. } => Some(Node::Type(index.types[row as usize].id)),
            Row::Archetype { index: row, .. } => {
                Some(Node::Archetype(index.archetypes[row as usize].id))
            }
        }
    }
}

/// What is open. Objects start open (a level's tree is read open); types and
/// archetypes start closed (each can hold the whole scene).
#[derive(Debug, Default)]
pub struct Expansion {
    flipped: HashSet<Node>,
}

impl Expansion {
    pub fn is_open(&self, node: Node) -> bool {
        matches!(node, Node::Object(_)) != self.flipped.contains(&node)
    }

    pub fn set(&mut self, node: Node, open: bool) {
        if open == matches!(node, Node::Object(_)) {
            self.flipped.remove(&node);
        } else {
            self.flipped.insert(node);
        }
    }

    pub fn toggle(&mut self, node: Node) {
        let open = self.is_open(node);
        self.set(node, !open);
    }
}

pub fn rows(index: &SceneIndex, mode: Mode, query: &Query, open: &Expansion) -> Vec<Row> {
    match mode {
        Mode::Objects => object_rows(index, query, open),
        Mode::Entities => {
            let matched = query.matches(index);
            let mut order: Vec<u32> = (0..index.entities.len() as u32)
                .filter(|&ix| matched[ix as usize])
                .collect();
            order.sort_unstable_by_key(|&ix| {
                let entity = index.entities[ix as usize].entity;
                (entity.index(), entity.generation())
            });
            order.into_iter().map(|ix| flat(ix, 0, None)).collect()
        }
        Mode::Components => {
            let mut rows = Vec::new();
            for (type_ix, row) in index.types.iter().enumerate() {
                if !query.matches_type(row) {
                    continue;
                }
                let is_open = open.is_open(Node::Type(row.id));
                rows.push(Row::Type {
                    index: type_ix as u32,
                    open: is_open,
                });
                if is_open {
                    rows.extend(
                        index
                            .entities_with(row.id)
                            .into_iter()
                            .map(|ix| flat(ix, 1, Some(row.id))),
                    );
                }
            }
            rows
        }
        Mode::Archetypes => {
            let matched = query.matches(index);
            let mut rows = Vec::new();
            for (archetype_ix, archetype) in index.archetypes.iter().enumerate() {
                let shown: Vec<u32> = archetype
                    .entities
                    .iter()
                    .copied()
                    .filter(|&ix| matched[ix as usize])
                    .collect();
                // Archetypes stay in the store when their last entity leaves;
                // empty ones and ones the search empties are not listed.
                if shown.is_empty() {
                    continue;
                }
                let is_open = open.is_open(Node::Archetype(archetype.id));
                rows.push(Row::Archetype {
                    index: archetype_ix as u32,
                    open: is_open,
                    shown: shown.len() as u32,
                });
                if is_open {
                    rows.extend(shown.into_iter().map(|ix| flat(ix, 1, None)));
                }
            }
            rows
        }
    }
}

fn flat(index: u32, depth: u16, under: Option<ComponentId>) -> Row {
    Row::Entity {
        index,
        depth,
        open: None,
        under,
        matched: true,
    }
}

fn object_rows(index: &SceneIndex, query: &Query, open: &Expansion) -> Vec<Row> {
    // While searching, the tree is the matches and their ancestors, all open.
    let search = (!query.is_empty()).then(|| {
        let matched = query.matches(index);
        let mut keep: HashMap<u32, bool> = HashMap::new();
        for (ix, row) in index.entities.iter().enumerate() {
            if !matched[ix] || row.kind != super::index::EntityKind::Object {
                continue;
            }
            keep.insert(ix as u32, true);
            let mut parent = row.parent;
            while let Some(&parent_ix) = parent.as_ref().and_then(|p| index.by_entity.get(p)) {
                if keep.contains_key(&parent_ix) {
                    break;
                }
                keep.insert(parent_ix, matched[parent_ix as usize]);
                parent = index.entities[parent_ix as usize].parent;
            }
        }
        keep
    });

    let mut rows = Vec::new();
    // Depth first without recursion: a deep chain must not overflow.
    let mut stack: Vec<(u32, u16)> = index.roots.iter().rev().map(|&ix| (ix, 0)).collect();
    while let Some((ix, depth)) = stack.pop() {
        let matched = match &search {
            Some(keep) => match keep.get(&ix) {
                Some(&matched) => matched,
                None => continue,
            },
            None => true,
        };
        let children = index.children.get(&ix);
        let has_children = children.is_some_and(|children| {
            search
                .as_ref()
                .is_none_or(|keep| children.iter().any(|child| keep.contains_key(child)))
        });
        let is_open = has_children
            && (search.is_some() || open.is_open(Node::Object(index.entities[ix as usize].entity)));
        rows.push(Row::Entity {
            index: ix,
            depth,
            open: has_children.then_some(is_open),
            under: None,
            matched,
        });
        if is_open {
            if let Some(children) = children {
                stack.extend(
                    children
                        .iter()
                        .rev()
                        .map(|&child| (child, depth.saturating_add(1))),
                );
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::index::scan_world;
    use pulsar_scene_model::{Name, Parent, StableId};
    use pulsar_scenedb::World;

    fn object(world: &mut World, name: &str, parent: Option<Entity>) -> Entity {
        let entity = world.spawn();
        world.insert(entity, StableId(name.into()));
        world.insert(entity, Name(name.into()));
        if let Some(parent) = parent {
            world.insert(entity, Parent(parent));
        }
        entity
    }

    fn shown(index: &SceneIndex, rows: &[Row]) -> Vec<(String, u16, bool)> {
        rows.iter()
            .filter_map(|row| match *row {
                Row::Entity {
                    index: ix,
                    depth,
                    matched,
                    ..
                } => Some((index.entities[ix as usize].label.to_string(), depth, matched)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_object_tree_closes_and_a_search_keeps_ancestors() {
        let mut world = World::new();
        let root = object(&mut world, "root", None);
        let middle = object(&mut world, "middle", Some(root));
        object(&mut world, "leaf", Some(middle));
        object(&mut world, "other", None);
        let index = scan_world(&world);
        let mut open = Expansion::default();

        let all = rows(&index, Mode::Objects, &Query::default(), &open);
        assert_eq!(all.len(), 4);

        open.set(Node::Object(root), false);
        let closed = rows(&index, Mode::Objects, &Query::default(), &open);
        assert_eq!(closed.len(), 2);

        // The search opens the way to its match, whatever is closed.
        let found = rows(&index, Mode::Objects, &Query::parse("leaf"), &open);
        assert_eq!(
            shown(&index, &found),
            [
                ("root".to_string(), 0, false),
                ("middle".to_string(), 1, false),
                ("leaf".to_string(), 2, true),
            ]
        );
    }

    #[test]
    fn type_and_archetype_views_open_to_their_entities() {
        let mut world = World::new();
        object(&mut world, "a", None);
        object(&mut world, "b", None);
        let index = scan_world(&world);
        let mut open = Expansion::default();
        let closed = rows(&index, Mode::Components, &Query::default(), &open);
        assert_eq!(closed.len(), 2, "StableId and Name");
        let name = index.types.iter().find(|t| &*t.name == "Name").unwrap().id;
        open.toggle(Node::Type(name));
        assert_eq!(rows(&index, Mode::Components, &Query::default(), &open).len(), 4);

        let archetypes = rows(&index, Mode::Archetypes, &Query::default(), &open);
        assert_eq!(archetypes.len(), 1, "emptied archetypes are not listed");
        assert!(matches!(archetypes[0], Row::Archetype { shown: 2, .. }));
    }
}
