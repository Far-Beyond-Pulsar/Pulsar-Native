//! The level editor's selection operations: select all, none and inverse,
//! and the ways to expand a selection (more/less along the hierarchy, the
//! hierarchy relatives, and objects similar to the selection). The keyboard
//! shortcuts (Ctrl/Cmd+A, Ctrl/Cmd+Shift+A), the viewport's and the
//! hierarchy's "Select" menu all run [`select`], so they agree.
//!
//! Only objects that can be edited on their own take part: class-generated
//! children and instance members are rebuilt from their source, so they are
//! never selected by these operations.
//!
//! An operation that adds objects keeps the current selection, and its
//! primary stays the primary; new objects are added in outline order.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, Context, Window};
use rust_i18n::t;
use ui::menu::popup_menu::PopupMenu;

use crate::scene_edit::{components, objects as scene_objects, SceneObjectData};
use crate::state::LevelEditorState;

/// What [`SelectOp::Similar`] compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Similarity {
    /// Everything but placement: object type, type props, and the same
    /// components with the same enabled flags and values.
    Settings,
    /// The same object type and the same component classes, whatever
    /// their values.
    ComponentTypes,
    /// At least one component (class and value) in common.
    SharedComponent,
    /// The same name once a trailing number is dropped ("Tree 12" is "Tree").
    Name,
}

/// A selection operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectOp {
    All,
    None,
    Invert,
    /// Add each selected object's parent and direct children.
    More,
    /// Drop selected objects whose parent or a child is not selected (the
    /// edge of each selected part of the tree). Objects with no parent and
    /// no children are kept.
    Less,
    /// Add every descendant.
    Children,
    /// Add direct children.
    ImmediateChildren,
    /// Add each selected object's parent.
    Parent,
    /// Add the objects that share a parent with a selected object.
    Siblings,
    Similar(Similarity),
}

/// The level's objects as the selection operations need them.
struct Outline {
    /// Every object, in tree order.
    order: Vec<String>,
    parent: HashMap<String, String>,
    children: HashMap<String, Vec<String>>,
    /// Objects that can be selected on their own.
    selectable: HashSet<String>,
}

fn outline(state: &LevelEditorState) -> Outline {
    let world = state.scene.world();
    let (objects, _) = scene_objects::get_hierarchy_projection(&world);
    let mut outline = Outline {
        order: Vec::with_capacity(objects.len()),
        parent: HashMap::new(),
        children: HashMap::new(),
        selectable: HashSet::new(),
    };
    for object in objects {
        for child in &object.children {
            outline.parent.insert(child.clone(), object.id.clone());
        }
        if !crate::scene_edit::classes::is_generated_child(&world, &object.id)
            && !scene_objects::is_instance_member(&world, &object.id)
        {
            outline.selectable.insert(object.id.clone());
        }
        outline.order.push(object.id.clone());
        outline.children.insert(object.id, object.children);
    }
    outline
}

/// Run `op` on the level's selection. Returns how many objects are selected
/// afterwards.
pub(crate) fn select(state: &mut LevelEditorState, op: SelectOp) -> usize {
    let current = {
        let world = state.scene.world();
        scene_objects::selected_object_ids(&world)
    };
    let outline = outline(state);
    let selected: HashSet<&str> = current.iter().map(String::as_str).collect();
    let ids: Vec<String> = match op {
        SelectOp::All => outline
            .order
            .iter()
            .filter(|id| outline.selectable.contains(*id))
            .cloned()
            .collect(),
        SelectOp::None => Vec::new(),
        SelectOp::Invert => outline
            .order
            .iter()
            .filter(|id| outline.selectable.contains(*id) && !selected.contains(id.as_str()))
            .cloned()
            .collect(),
        SelectOp::Less => {
            let is_selected = |id: &String| selected.contains(id.as_str());
            current
                .iter()
                .filter(|id| {
                    outline.parent.get(*id).is_none_or(is_selected)
                        && outline
                            .children
                            .get(*id)
                            .is_none_or(|kids| kids.iter().all(is_selected))
                })
                .cloned()
                .collect()
        }
        _ => {
            if current.is_empty() {
                return 0;
            }
            let added = expansion(state, &outline, &current, op);
            with_added(&outline, &current, &added)
        }
    };
    state.scene.select_objects(&ids);
    ids.len()
}

/// The objects an expanding `op` adds to `current`.
fn expansion(
    state: &LevelEditorState,
    outline: &Outline,
    current: &[String],
    op: SelectOp,
) -> HashSet<String> {
    let mut added = HashSet::new();
    let children = |id: &String| outline.children.get(id).into_iter().flatten();
    match op {
        SelectOp::More => {
            for id in current {
                added.extend(outline.parent.get(id).cloned());
                added.extend(children(id).cloned());
            }
        }
        SelectOp::Children => {
            let mut stack: Vec<&String> = current.iter().collect();
            while let Some(id) = stack.pop() {
                for child in children(id) {
                    if added.insert(child.clone()) {
                        stack.push(child);
                    }
                }
            }
        }
        SelectOp::ImmediateChildren => {
            for id in current {
                added.extend(children(id).cloned());
            }
        }
        SelectOp::Parent => {
            added.extend(
                current
                    .iter()
                    .filter_map(|id| outline.parent.get(id).cloned()),
            );
        }
        SelectOp::Siblings => {
            let parents: HashSet<Option<&String>> =
                current.iter().map(|id| outline.parent.get(id)).collect();
            added.extend(
                outline
                    .order
                    .iter()
                    .filter(|id| parents.contains(&outline.parent.get(*id)))
                    .cloned(),
            );
        }
        SelectOp::Similar(similarity) => {
            let world = state.scene.world();
            let keys = |id: &str| similarity_keys(&world, id, similarity);
            let wanted: HashSet<String> = current.iter().flat_map(|id| keys(id)).collect();
            added.extend(
                outline
                    .order
                    .iter()
                    .filter(|id| outline.selectable.contains(*id))
                    .filter(|id| keys(id).iter().any(|key| wanted.contains(key)))
                    .cloned(),
            );
        }
        SelectOp::All | SelectOp::None | SelectOp::Invert | SelectOp::Less => {}
    }
    added
}

/// `current` plus the selectable objects of `added`, new ones first in tree
/// order, so the existing primary (the last) stays the primary.
fn with_added(outline: &Outline, current: &[String], added: &HashSet<String>) -> Vec<String> {
    let have: HashSet<&str> = current.iter().map(String::as_str).collect();
    let mut ids: Vec<String> = outline
        .order
        .iter()
        .filter(|id| {
            added.contains(*id) && outline.selectable.contains(*id) && !have.contains(id.as_str())
        })
        .cloned()
        .collect();
    ids.extend(current.iter().cloned());
    ids
}

/// The keys an object is compared by: two objects are similar when they
/// share one.
fn similarity_keys(world: &pulsar_scenedb::World, id: &str, similarity: Similarity) -> Vec<String> {
    let Some(object) = scene_objects::get_object(world, id) else {
        return Vec::new();
    };
    match similarity {
        Similarity::Name => vec![base_name(&object.name).to_owned()],
        Similarity::ComponentTypes => {
            let mut classes = components::get_component_class_names(world, id);
            classes.sort();
            vec![serde_json::json!([format!("{:?}", object.object_type), classes]).to_string()]
        }
        Similarity::SharedComponent => components::get_components(world, id)
            .into_iter()
            .map(|component| component_key(component).to_string())
            .collect(),
        Similarity::Settings => vec![settings_signature(
            &object,
            components::get_components(world, id),
        )],
    }
}

/// `name` without a trailing number and the separator before it.
fn base_name(name: &str) -> &str {
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.len() == name.len() {
        return name.trim();
    }
    let base = trimmed.trim_end_matches([' ', '_', '-', '.', '(']);
    if base.is_empty() {
        name.trim()
    } else {
        base
    }
}

/// A component as settings: its class, enabled flag and value, without
/// the record metadata that is per instance (its id) or placement (a slot
/// component's local transform).
fn component_key(component: engine_backend::ComponentInstance) -> serde_json::Value {
    use pulsar_world_registry::instances::{INSTANCE_ID_KEY, TRANSFORM_KEY};
    let mut data = component.data;
    if let Some(map) = data.as_object_mut() {
        map.remove(INSTANCE_ID_KEY);
        map.remove(TRANSFORM_KEY);
    }
    serde_json::json!([component.class_name, component.enabled, data])
}

/// Everything about an object except where it is and what it is called, as
/// one string. Its pivot offset is placement (it moves with the transform
/// and differs between copies by rounding), so it is left out.
fn settings_signature(
    object: &SceneObjectData,
    components: Vec<engine_backend::ComponentInstance>,
) -> String {
    let props: BTreeMap<&String, &serde_json::Value> = object
        .props
        .iter()
        .filter(|(key, _)| key.as_str() != engine_backend::scene::pivots::PIVOT_OFFSET_PROP)
        .collect();
    let components: Vec<serde_json::Value> = components.into_iter().map(component_key).collect();
    serde_json::json!([format!("{:?}", object.object_type), props, components]).to_string()
}

/// An object's archetype for mass editing: its object type and its
/// component classes in order (with each component's parent), so component
/// index N is the same kind of component on every object that shares it.
fn archetype(world: &pulsar_scenedb::World, id: &str) -> Option<String> {
    use pulsar_world_registry::instances::PARENT_INDEX_KEY;
    let object_type = scene_objects::get_object(world, id)?.object_type;
    let components: Vec<serde_json::Value> = components::get_components_metadata(world, id)
        .into_iter()
        .map(|c| serde_json::json!([c.class_name, c.data.get(PARENT_INDEX_KEY)]))
        .collect();
    Some(serde_json::json!([format!("{object_type:?}"), components]).to_string())
}

/// How the properties panel edits the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MassEdit {
    /// One object (or none) selected: edits go to it.
    Single,
    /// Several objects sharing one archetype: an edit goes to all of them.
    /// Holds every selected object other than the primary.
    All(Vec<String>),
    /// Several objects of different archetypes: the panel shows and edits
    /// only the primary. Holds the selection's size.
    PrimaryOnly(usize),
}

/// Whether a property edit on `shown` (the object the properties panel
/// shows, the primary) also applies to the rest of the selection: it does
/// when `shown` is selected and every selected object shares its archetype
/// ([`archetype`]).
pub(crate) fn mass_edit(state: &LevelEditorState, shown: &str) -> MassEdit {
    let world = state.scene.world();
    let selection = scene_objects::selected_object_ids(&world);
    if selection.len() < 2 || !selection.iter().any(|id| id == shown) {
        return MassEdit::Single;
    }
    let key = archetype(&world, shown);
    let rest: Vec<String> = selection
        .iter()
        .filter(|id| *id != shown)
        .cloned()
        .collect();
    if key.is_some() && rest.iter().all(|id| archetype(&world, id) == key) {
        MassEdit::All(rest)
    } else {
        MassEdit::PrimaryOnly(selection.len())
    }
}

type SharedState = Arc<parking_lot::RwLock<LevelEditorState>>;

/// Add the "Select" submenu: select all, deselect all, invert, and
/// "Expand Selection" with the ways to grow or shrink it (disabled while
/// nothing is selected).
pub(crate) fn select_submenu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    state: &SharedState,
) -> PopupMenu {
    let has_selection = !state.read().scene.selected_objects().is_empty();
    let state = state.clone();
    menu.submenu(
        t!("LevelEditor.Select.Menu"),
        window,
        cx,
        move |submenu, window, cx| {
            let run = |op: SelectOp| {
                let state = state.clone();
                move |_: &mut Window, _: &mut App| {
                    select(&mut state.write(), op);
                }
            };
            let expand_state = state.clone();
            submenu
                .menu_handler(t!("LevelEditor.Select.All"), run(SelectOp::All))
                .menu_handler(t!("LevelEditor.Select.None"), run(SelectOp::None))
                .menu_handler(t!("LevelEditor.Select.Invert"), run(SelectOp::Invert))
                .separator()
                .submenu_with_disabled(
                    t!("LevelEditor.Select.Expand"),
                    !has_selection,
                    window,
                    cx,
                    move |menu, _, _| expand_menu(menu, &expand_state),
                )
        },
    )
}

/// The "Expand Selection" submenu.
fn expand_menu(menu: PopupMenu, state: &SharedState) -> PopupMenu {
    let run = |op: SelectOp| {
        let state = state.clone();
        move |_: &mut Window, _: &mut App| {
            select(&mut state.write(), op);
        }
    };
    menu.menu_handler(t!("LevelEditor.Select.More"), run(SelectOp::More))
        .menu_handler(t!("LevelEditor.Select.Less"), run(SelectOp::Less))
        .separator()
        .label(t!("LevelEditor.Select.SimilarHeading"))
        .menu_handler(
            t!("LevelEditor.Select.SameSettings"),
            run(SelectOp::Similar(Similarity::Settings)),
        )
        .menu_handler(
            t!("LevelEditor.Select.SameComponents"),
            run(SelectOp::Similar(Similarity::ComponentTypes)),
        )
        .menu_handler(
            t!("LevelEditor.Select.SharedComponent"),
            run(SelectOp::Similar(Similarity::SharedComponent)),
        )
        .menu_handler(
            t!("LevelEditor.Select.SameName"),
            run(SelectOp::Similar(Similarity::Name)),
        )
        .separator()
        .label(t!("LevelEditor.Select.HierarchyHeading"))
        .menu_handler(t!("LevelEditor.Select.Children"), run(SelectOp::Children))
        .menu_handler(
            t!("LevelEditor.Select.ImmediateChildren"),
            run(SelectOp::ImmediateChildren),
        )
        .menu_handler(t!("LevelEditor.Select.Parent"), run(SelectOp::Parent))
        .menu_handler(t!("LevelEditor.Select.Siblings"), run(SelectOp::Siblings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{execute_command, SceneCommand};
    use crate::scene_edit::{ObjectType, Transform};

    fn add(
        state: &mut LevelEditorState,
        name: &str,
        kind: ObjectType,
        x: f32,
        parent: Option<&str>,
    ) -> String {
        let mut props = std::collections::HashMap::new();
        // Copies differ in their pivot offset by rounding; it is placement.
        props.insert(
            engine_backend::scene::pivots::PIVOT_OFFSET_PROP.to_string(),
            serde_json::json!([x * 1e-7, 3.3, 0.0]),
        );
        let data = SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type: kind,
            transform: Transform {
                position: [f64::from(x), 0.0, 0.0],
                rotation: [0.0, x * 10.0, 0.0],
                scale: [1.0 + x, 1.0, 1.0],
            },
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props,
            component_instances: None,
        };
        execute_command(
            state,
            SceneCommand::AddObject {
                data,
                parent_id: parent.map(str::to_string),
            },
        )
        .affected_ids[0]
            .clone()
    }

    fn selected(state: &LevelEditorState) -> HashSet<String> {
        state.scene.selected_objects().into_iter().collect()
    }

    fn set(ids: &[&String]) -> HashSet<String> {
        ids.iter().map(|id| (*id).clone()).collect()
    }

    #[test]
    fn same_settings_ignores_placement_and_keeps_the_primary() {
        let mut state = LevelEditorState::new();
        let a = add(&mut state, "Tree 1", ObjectType::Empty, 0.0, None);
        let b = add(&mut state, "Tree 2", ObjectType::Empty, 5.0, None);
        let c = add(&mut state, "Tree 3", ObjectType::Empty, -3.0, None);
        let camera = add(&mut state, "Camera", ObjectType::Camera, 1.0, None);
        let folder = add(&mut state, "Folder", ObjectType::Folder, 2.0, None);

        state.scene.select_objects(std::slice::from_ref(&b));
        assert_eq!(
            select(&mut state, SelectOp::Similar(Similarity::Settings)),
            3
        );
        assert_eq!(selected(&state), set(&[&a, &b, &c]));
        assert!(!selected(&state).contains(&camera) && !selected(&state).contains(&folder));
        assert_eq!(state.scene.selected_object(), Some(b));
    }

    #[test]
    fn expanding_nothing_selects_nothing() {
        let mut state = LevelEditorState::new();
        add(&mut state, "Rock", ObjectType::Empty, 0.0, None);
        state.scene.select_objects(&[]);
        for op in [
            SelectOp::Similar(Similarity::Settings),
            SelectOp::More,
            SelectOp::Children,
            SelectOp::Siblings,
        ] {
            assert_eq!(select(&mut state, op), 0, "{op:?}");
            assert!(state.scene.selected_objects().is_empty());
        }
    }

    #[test]
    fn all_none_and_invert() {
        let mut state = LevelEditorState::new();
        let a = add(&mut state, "A", ObjectType::Empty, 0.0, None);
        let b = add(&mut state, "B", ObjectType::Empty, 1.0, None);
        let c = add(&mut state, "C", ObjectType::Empty, 2.0, None);
        assert_eq!(select(&mut state, SelectOp::All), 3);
        state.scene.select_objects(std::slice::from_ref(&a));
        assert_eq!(select(&mut state, SelectOp::Invert), 2);
        assert_eq!(selected(&state), set(&[&b, &c]));
        assert_eq!(select(&mut state, SelectOp::None), 0);
        assert!(selected(&state).is_empty());
    }

    #[test]
    fn hierarchy_expansions_and_more_less() {
        let mut state = LevelEditorState::new();
        let root = add(&mut state, "Root", ObjectType::Folder, 0.0, None);
        let mid = add(&mut state, "Mid", ObjectType::Empty, 1.0, Some(&root));
        let leaf = add(&mut state, "Leaf", ObjectType::Empty, 2.0, Some(&mid));
        let sibling = add(&mut state, "Sibling", ObjectType::Empty, 3.0, Some(&root));

        state.scene.select_objects(std::slice::from_ref(&root));
        select(&mut state, SelectOp::Children);
        assert_eq!(selected(&state), set(&[&root, &mid, &leaf, &sibling]));

        state.scene.select_objects(std::slice::from_ref(&root));
        select(&mut state, SelectOp::ImmediateChildren);
        assert_eq!(selected(&state), set(&[&root, &mid, &sibling]));

        state.scene.select_objects(std::slice::from_ref(&leaf));
        select(&mut state, SelectOp::Parent);
        assert_eq!(selected(&state), set(&[&leaf, &mid]));

        state.scene.select_objects(std::slice::from_ref(&mid));
        select(&mut state, SelectOp::Siblings);
        assert_eq!(selected(&state), set(&[&mid, &sibling]));

        state.scene.select_objects(std::slice::from_ref(&mid));
        select(&mut state, SelectOp::More);
        assert_eq!(selected(&state), set(&[&root, &mid, &leaf]));
        // Root is at the edge (its child Sibling is not selected); Mid and
        // Leaf have every relative selected.
        select(&mut state, SelectOp::Less);
        assert_eq!(selected(&state), set(&[&mid, &leaf]));
    }

    #[test]
    fn same_name_drops_the_trailing_number() {
        assert_eq!(base_name("Tree 12"), "Tree");
        assert_eq!(base_name("Tree_3"), "Tree");
        assert_eq!(base_name("Tree"), "Tree");
        assert_eq!(base_name("42"), "42");
        let mut state = LevelEditorState::new();
        let a = add(&mut state, "Tree 1", ObjectType::Empty, 0.0, None);
        let b = add(&mut state, "Tree 20", ObjectType::Camera, 1.0, None);
        add(&mut state, "Rock 1", ObjectType::Empty, 2.0, None);
        state.scene.select_objects(std::slice::from_ref(&a));
        select(&mut state, SelectOp::Similar(Similarity::Name));
        assert_eq!(selected(&state), set(&[&a, &b]));
    }
}
