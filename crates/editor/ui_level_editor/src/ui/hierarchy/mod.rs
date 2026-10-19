use crate::scene_edit::ObjectType;
use crate::scene_edit::SceneObjectData;
use crate::state::{HierarchyDragPayload, LevelEditorState};
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::{prelude::*, *};
use rust_i18n::t;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    hierarchical_tree::tree_colors,
    menu::popup_menu::PopupMenu,
    ActiveTheme, HierarchicalTreeView, HierarchyConfig, HierarchyItem, HierarchyLayout, Icon,
    IconName, Sizable, StyledExt,
};

/// GPUI Render impl for the hierarchy drag ghost label.
impl Render for HierarchyDragPayload {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_1()
            .rounded(px(4.0))
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(cx.theme().foreground)
            .child(self.object_name.clone())
    }
}

// ── Scene Object Item ─────────────────────────────────────────────────────────

/// How an object relates to a placed class (#921).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassRole {
    None,
    /// A placed class instance: shown as one class object.
    Root,
    /// A child object the class generated (rebuilt from the class, not
    /// saved on its own): nested under its root and marked class-owned.
    Owned,
    /// Generated content of a level instance (shown by a sublevel display, or
    /// added to an in-memory instance): rebuilt from its source, never saved
    /// with this level, edited by changing the source.
    Instanced,
}

#[derive(Clone)]
struct SceneObjectItem {
    object: Rc<crate::scene_edit::objects::HierarchyObjectProjection>,
    state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    is_selected: bool,
    is_folder: bool,
    class_role: ClassRole,
    /// The object's authored movability (#837,
    /// `helio_component::components::object_movability`); `None` for objects
    /// with no mesh or light.
    movability: Option<helio::Movability>,
}

/// Object ids in the order the tree shows them (expanded branches only).
fn visible_order(state: &LevelEditorState) -> Vec<String> {
    let (objects, root_ids) = {
        let world = state.scene.world();
        crate::scene_edit::objects::get_hierarchy_projection(&world)
    };
    let children: std::collections::HashMap<&str, &Vec<String>> = objects
        .iter()
        .map(|object| (object.id.as_str(), &object.children))
        .collect();
    let mut order = Vec::with_capacity(objects.len());
    let mut stack: Vec<&String> = root_ids.iter().rev().collect();
    while let Some(id) = stack.pop() {
        order.push(id.clone());
        if state.hierarchy.is_object_expanded(id) {
            if let Some(kids) = children.get(id.as_str()) {
                stack.extend(kids.iter().rev());
            }
        }
    }
    order
}

/// Row click with the held modifiers: plain selects one row, ctrl/cmd toggles
/// a row in the selection, shift selects the range from the anchor.
fn apply_row_selection(state: &mut LevelEditorState, id: &str, modifiers: &Modifiers) {
    let primary = state.scene.selected_object();
    if modifiers.shift {
        let order = visible_order(state);
        let anchor = state
            .hierarchy
            .selection_anchor
            .clone()
            .or_else(|| primary.clone())
            .unwrap_or_else(|| id.to_string());
        let from = order.iter().position(|row| *row == anchor);
        let to = order.iter().position(|row| row == id);
        if let (Some(from), Some(to)) = (from, to) {
            let (lo, hi) = (from.min(to), from.max(to));
            state.hierarchy.multi_selected = order[lo..=hi].to_vec();
            state.hierarchy.selection_anchor = Some(anchor);
            state.scene.select_object(Some(id.to_string()));
            return;
        }
    }
    if modifiers.secondary() {
        if state.hierarchy.multi_selected.is_empty() {
            state.hierarchy.multi_selected.extend(primary);
        }
        state.hierarchy.toggle_multi_selected(id);
        let next_primary = if state.hierarchy.is_multi_selected(id) {
            Some(id.to_string())
        } else {
            state.hierarchy.multi_selected.last().cloned()
        };
        state.scene.select_object(next_primary);
        return;
    }
    state.hierarchy.clear_multi_selection();
    state.hierarchy.selection_anchor = Some(id.to_string());
    state.scene.select_object(Some(id.to_string()));
}

/// Put the selected objects into a new folder, placed under the first
/// object's parent, and select the folder. Objects under another selected
/// object travel with it, and class-generated children are left alone.
/// Returns whether a folder was made.
fn group_selection_into_folder(state: &mut LevelEditorState) -> bool {
    use crate::commands::{execute_command, SceneCommand};
    use engine_backend::scene::SceneWorldExt;

    let mut picked = state.hierarchy.multi_selected.clone();
    if picked.is_empty() {
        picked.extend(state.scene.selected_object());
    }
    let order = visible_order(state);
    // Ancestors of an object, nearest first.
    let ancestors = |state: &LevelEditorState, id: &str| -> Vec<String> {
        let world = state.scene.world();
        let mut chain = Vec::new();
        let mut at = world.entity_for(id).and_then(|e| world.parent_of(e));
        while let Some(entity) = at {
            if let Some(stable) = world.stable_id_of(entity) {
                chain.push(stable.to_string());
            }
            at = world.parent_of(entity);
        }
        chain
    };
    let mut roots: Vec<String> = picked
        .iter()
        .filter(|id| {
            let world = state.scene.world();
            !crate::scene_edit::classes::is_generated_child(&world, id)
                && !crate::scene_edit::objects::is_instance_member(&world, id)
                && !ancestors(state, id).iter().any(|a| picked.contains(a))
        })
        .cloned()
        .collect();
    roots.sort_by_key(|id| order.iter().position(|row| row == id).unwrap_or(usize::MAX));
    let Some(first) = roots.first().cloned() else {
        return false;
    };
    let parent_id = ancestors(state, &first).into_iter().next();

    // The folder sits at the centre of what it holds, so its gizmo is on the
    // group. Reparenting keeps each object's place in the world, so nothing
    // jumps; from then on moving the folder moves them. The folder's own
    // transform is relative to its parent, like any object's.
    let mut transform = crate::scene_edit::Transform::default();
    {
        use engine_backend::scene::hierarchy_transform::{local_for_world, world_transform_of};
        let world = state.scene.world();
        let positions: Vec<[f64; 3]> = roots
            .iter()
            .filter_map(|id| world.entity_for(id))
            .filter_map(|entity| world_transform_of(&world, entity))
            .map(|pose| pose.position)
            .collect();
        if !positions.is_empty() {
            let n = positions.len() as f64;
            let mut centre = engine_backend::scene::Transform::default();
            for axis in 0..3 {
                centre.position[axis] = positions.iter().map(|p| p[axis]).sum::<f64>() / n;
            }
            let parent = parent_id.as_deref().and_then(|id| world.entity_for(id));
            transform = local_for_world(&world, parent, &centre).into();
        }
    }

    let created = execute_command(
        state,
        SceneCommand::AddObject {
            data: SceneObjectData {
                id: String::new(),
                name: "New Folder".to_string(),
                object_type: ObjectType::Folder,
                transform,
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            parent_id,
        },
    );
    let Some(folder) = created.affected_ids.first().cloned() else {
        return false;
    };
    for id in roots {
        execute_command(
            state,
            SceneCommand::ReparentObject {
                id,
                new_parent_id: Some(folder.clone()),
            },
        );
    }
    state.hierarchy.expanded_objects.insert(folder.clone());
    state.hierarchy.multi_selected = vec![folder.clone()];
    state.hierarchy.selection_anchor = Some(folder.clone());
    state.scene.select_object(Some(folder));
    true
}

/// `id` and every object under it, for the subtree actions.
fn subtree_ids(state: &LevelEditorState, id: &str) -> Vec<String> {
    use engine_backend::scene::SceneWorldExt;
    let world = state.scene.world();
    let Some(root) = world.entity_for(id) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Some(stable) = world.stable_id_of(entity) {
            ids.push(stable.to_string());
        }
        stack.extend(world.children_of(Some(entity)));
    }
    ids
}

impl HierarchyItem for SceneObjectItem {
    type Id = String;
    type DragPayload = HierarchyDragPayload;

    fn id(&self) -> Self::Id {
        self.object.id.clone()
    }

    fn name(&self) -> String {
        self.object.name.clone()
    }

    fn icon(&self) -> IconName {
        if self.class_role == ClassRole::Root {
            return IconName::Code;
        }
        if self.object.icon_asset.is_some() {
            return IconName::Image;
        }
        HierarchyPanel::get_icon_for_object_type(self.object.object_type)
    }

    fn icon_color<V>(&self, cx: &Context<V>) -> Hsla
    where
        V: Render,
    {
        match self.class_role {
            ClassRole::Root => return cx.theme().primary,
            ClassRole::Owned | ClassRole::Instanced => return cx.theme().muted_foreground,
            ClassRole::None => {}
        }
        if self.object.icon_asset.is_some() {
            return tree_colors::DOC_TEAL;
        }
        HierarchyPanel::get_icon_color_for_type(self.object.object_type, cx)
    }

    fn children_ids(&self) -> Vec<Self::Id> {
        self.object.children.clone()
    }

    fn is_selected(&self) -> bool {
        self.is_selected
    }

    fn create_drag_payload(&self) -> Self::DragPayload {
        HierarchyDragPayload {
            object_id: self.object.id.clone(),
            object_name: self.object.name.clone(),
        }
    }

    fn drag_drop_id(&self) -> String {
        format!("hierarchy-{}", self.object.id)
    }

    fn extra_row_content<V>(&self, cx: &mut Context<V>) -> Option<AnyElement>
    where
        V: Render,
    {
        let visibility_id = self.object.id.clone();
        let visibility_state = self.state_arc.clone();
        let is_visible = self.object.visible;

        let duplicate_id = self.object.id.clone();
        let duplicate_state = self.state_arc.clone();
        let delete_id = self.object.id.clone();
        let delete_state = self.state_arc.clone();

        let visibility_button = Button::new(format!("scene-visibility-{}", visibility_id))
            .ghost()
            .xsmall()
            .icon(if is_visible {
                IconName::Eye
            } else {
                IconName::EyeOff
            })
            .tooltip(if is_visible {
                "Hide object"
            } else {
                "Show object"
            })
            .on_click(move |_, _, cx| {
                use crate::commands::{execute_command, SceneCommand};
                let mut state = visibility_state.write();
                let found = {
                    let world = state.scene.world();
                    crate::scene_edit::objects::get_object(&world, &visibility_id)
                };
                if let Some(mut obj) = found {
                    obj.visible = !obj.visible;
                    execute_command(&mut state, SceneCommand::UpdateObject { data: obj });
                }
                drop(state);
                cx.stop_propagation();
            });

        let duplicate_button = Button::new(format!("scene-duplicate-{}", duplicate_id))
            .ghost()
            .xsmall()
            .icon(IconName::Copy)
            .tooltip("Duplicate object")
            .on_click(move |_, _, cx| {
                let mut state = duplicate_state.write();
                crate::commands::duplicate_and_select(&mut state, &duplicate_id);
                drop(state);
                cx.stop_propagation();
            });

        let delete_button = Button::new(format!("scene-delete-{}", delete_id))
            .ghost()
            .xsmall()
            .icon(IconName::Trash)
            .tooltip("Delete object")
            .on_click(move |_, _, cx| {
                use crate::commands::{execute_command, SceneCommand};
                let mut state = delete_state.write();
                execute_command(
                    &mut state,
                    SceneCommand::RemoveObject {
                        id: delete_id.clone(),
                    },
                );
                drop(state);
                cx.stop_propagation();
            });

        let badge = |text: &'static str, cx: &mut Context<V>| {
            div()
                .px_1()
                .rounded(px(3.0))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .border_1()
                .border_color(cx.theme().border)
                .child(text)
        };
        let row = h_flex()
            .gap_0p5()
            .when(self.movability == Some(helio::Movability::Static), |row| {
                row.child(badge("static", cx))
            });
        Some(match self.class_role {
            // Class-owned children come and go with their class: no
            // duplicate/delete of their own.
            ClassRole::Owned => row
                .child(badge("class-owned", cx))
                .child(visibility_button)
                .into_any_element(),
            // Content of a shown level comes and goes with the display that shows it.
            ClassRole::Instanced => row
                .child(badge("level instance", cx))
                .child(visibility_button)
                .into_any_element(),
            ClassRole::Root => row
                .child(badge("class", cx))
                .child(visibility_button)
                .child(duplicate_button)
                .child(delete_button)
                .into_any_element(),
            ClassRole::None => row
                .child(visibility_button)
                .child(duplicate_button)
                .child(delete_button)
                .into_any_element(),
        })
    }

    fn build_context_menu(
        &self,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        use crate::commands::{execute_command, SceneCommand};

        if matches!(self.class_role, ClassRole::Owned | ClassRole::Instanced) {
            return menu;
        }
        let is_rigid_frame = {
            use engine_backend::scene::SceneWorldExt;
            let state = self.state_arc.read();
            let world = state.scene.world();
            world
                .entity_for(&self.object.id)
                .is_some_and(|entity| world.get::<engine_backend::scene::frames::RigidFrame>(entity).is_some())
        };
        let duplicate_id = self.object.id.clone();
        let delete_id = self.object.id.clone();
        let duplicate_state = self.state_arc.clone();
        let delete_state = self.state_arc.clone();
        // "Mark selection Static" (#837): the object and everything under
        // it, one undo step. Objects with no mesh or light are skipped.
        let set_movability = |movability: helio_component::components::ObjectMovability| {
            let id = self.object.id.clone();
            let state_arc = self.state_arc.clone();
            move |_: &mut Window, _: &mut App| {
                let mut state = state_arc.write();
                let ids = subtree_ids(&state, &id);
                execute_command(&mut state, SceneCommand::SetMovability { ids, movability });
            }
        };
        use helio_component::components::ObjectMovability;

        menu.menu_handler_with_icon(
            "Mark Static (with children)",
            IconName::Pin,
            set_movability(ObjectMovability::Static),
        )
        .menu_handler_with_icon(
            "Mark Movable (with children)",
            IconName::Drag,
            set_movability(ObjectMovability::Movable),
        )
        .separator()
        // A rigid frame draws its contents through one matrix: moving it costs
        // nothing per member. An ordinary group composes its members instead.
        .menu_handler_with_icon(
            if is_rigid_frame {
                "Make Ordinary Group"
            } else {
                "Make Rigid Frame"
            },
            IconName::Box,
            {
                let id = self.object.id.clone();
                let state_arc = self.state_arc.clone();
                move |_: &mut Window, _: &mut App| {
                    let mut state = state_arc.write();
                    let found = {
                        let world = state.scene.world();
                        crate::scene_edit::objects::get_object(&world, &id)
                    };
                    if let Some(mut obj) = found {
                        if is_rigid_frame {
                            obj.props.remove(engine_backend::scene::frames::FRAME_PROP);
                        } else {
                            obj.props.insert(
                                engine_backend::scene::frames::FRAME_PROP.to_string(),
                                serde_json::Value::Bool(true),
                            );
                        }
                        execute_command(&mut state, SceneCommand::UpdateObject { data: obj });
                    }
                }
            },
        )
        .when(
            {
                let state = self.state_arc.read();
                state.hierarchy.multi_selected.len() > 1
                    && state.hierarchy.is_multi_selected(&self.object.id)
            },
            |menu| {
                let group_state = self.state_arc.clone();
                menu.menu_handler_with_icon(
                    "Group into Folder",
                    IconName::FolderPlus,
                    move |_, _| {
                        group_selection_into_folder(&mut group_state.write());
                    },
                )
                .separator()
            },
        )
        .menu_handler_with_icon("Duplicate", IconName::Copy, move |_, app| {
            let _ = app;
            let mut state = duplicate_state.write();
            crate::commands::duplicate_and_select(&mut state, &duplicate_id);
        })
        .menu_handler_with_icon("Delete", IconName::Trash, move |_, app| {
            let _ = app;
            let mut state = delete_state.write();
            execute_command(
                &mut state,
                SceneCommand::RemoveObject {
                    id: delete_id.clone(),
                },
            );
        })
    }

    fn on_click_custom(&self) -> Option<Arc<dyn Fn()>> {
        if self.is_folder {
            // Folders use expand/collapse instead of selection
            None
        } else {
            None
        }
    }
}

/// Hierarchy Panel - Scene outliner showing all objects in a tree structure
pub struct HierarchyPanel {
    /// `(store_revision, selected)` the cached items were built from. Keyed on
    /// the world's own mutation counter (`World::revision`)
    /// rather than `scene.revision`: the render thread writes gizmo-drag
    /// results and click-selects straight into the store without ever touching
    /// the UI-side revision counter.
    cache_key: Option<(u64, Option<String>, Vec<String>)>,
    cached_items: Vec<SceneObjectItem>,
    cached_root_ids: Vec<String>,
}

impl HierarchyPanel {
    pub fn new() -> Self {
        Self {
            cache_key: None,
            cached_items: Vec::new(),
            cached_root_ids: Vec::new(),
        }
    }

    fn refresh_cache(
        &mut self,
        state: &LevelEditorState,
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
    ) {
        let _scope = gpui::render_stats::scope("hierarchy: refresh cache");
        let selected = state.scene.selected_object();
        let multi = state.hierarchy.multi_selected.clone();
        let key = (state.scene.world_revision(), selected.clone(), multi.clone());
        if self.cache_key.as_ref() == Some(&key) {
            return;
        }

        let world = state.scene.world();
        let _snapshot_scope = gpui::render_stats::scope("hierarchy: ECS snapshot");
        let (all_objects, root_ids) = crate::scene_edit::objects::get_hierarchy_projection(&world);
        drop(_snapshot_scope);
        self.cached_items = all_objects
            .into_iter()
            .map(|obj| {
                let is_selected = selected.as_deref() == Some(obj.id.as_str())
                    || multi.iter().any(|id| *id == obj.id);
                let is_folder = matches!(obj.object_type, ObjectType::Folder);
                let class_role = if crate::scene_edit::classes::is_class_root(&world, &obj.id) {
                    ClassRole::Root
                } else if crate::scene_edit::classes::is_generated_child(&world, &obj.id) {
                    ClassRole::Owned
                } else if crate::scene_edit::objects::is_instance_member(&world, &obj.id) {
                    ClassRole::Instanced
                } else {
                    ClassRole::None
                };
                let movability = {
                    use engine_backend::scene::SceneWorldExt;
                    world.entity_for(&obj.id).and_then(|entity| {
                        helio_component::components::object_movability(&world, entity)
                    })
                };
                SceneObjectItem {
                    object: Rc::new(obj),
                    state_arc: state_arc.clone(),
                    is_selected,
                    is_folder,
                    class_role,
                    movability,
                }
            })
            .collect();
        drop(world);
        self.cached_root_ids = root_ids;
        self.cache_key = Some(key);
    }

    pub fn render<V>(
        &mut self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        wrapper_entity: WeakEntity<V>,
        add_button: AnyElement,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<PanelEvent> + Render,
    {
        self.refresh_cache(state, &state_arc);
        let items = self.cached_items.clone();

        // Root-level objects (those without parents)
        let root_ids = self.cached_root_ids.clone();

        let state_arc_for_expand = state_arc.clone();
        let state_arc_for_toggle = state_arc.clone();
        let wrapper_for_toggle = wrapper_entity.clone();
        let state_arc_for_select = state_arc.clone();
        let wrapper_for_select = wrapper_entity.clone();
        let state_arc_for_drop = state_arc.clone();
        let wrapper_for_drop = wrapper_entity.clone();
        let state_arc_for_root_click = state_arc.clone();

        // Header buttons
        let header_buttons = vec![
            add_button,
            {
                let state_clone = state_arc.clone();
                let gpu_engine = gpu_engine.clone();
                Button::new("add_folder")
                    .icon(IconName::FolderPlus)
                    .ghost()
                    .xsmall()
                    .tooltip(t!("LevelEditor.Hierarchy.AddFolder"))
                    .on_click(move |_, _, _| {
                        use crate::commands::{execute_command, SceneCommand};
                        use crate::scene_edit::{ObjectType, SceneObjectData};
                        let transform = editor_camera_spawn_transform(&gpu_engine);
                        let mut state = state_clone.write();
                        execute_command(
                            &mut state,
                            SceneCommand::AddObject {
                                data: SceneObjectData {
                                    id: String::new(),
                                    name: "New Folder".to_string(),
                                    object_type: ObjectType::Folder,
                                    transform,
                                    visible: true,
                                    locked: false,
                                    parent: None,
                                    children: vec![],
                                    scene_path: String::new(),
                                    props: Default::default(),
                                    component_instances: None,
                                },
                                parent_id: None,
                            },
                        );
                    })
                    .into_any_element()
            },
            {
                let state_clone = state_arc.clone();
                let wrapper = wrapper_entity.clone();
                Button::new("group_selection")
                    .icon(IconName::Folder)
                    .ghost()
                    .xsmall()
                    .tooltip("Group selection into folder (ctrl/shift-click to select several)")
                    .on_click(move |_, _, cx| {
                        group_selection_into_folder(&mut state_clone.write());
                        if let Some(wrapper) = wrapper.upgrade() {
                            cx.notify(wrapper.entity_id());
                        }
                    })
                    .into_any_element()
            },
            {
                let state_clone = state_arc.clone();
                Button::new("delete_object")
                    .icon(IconName::Trash)
                    .ghost()
                    .xsmall()
                    .tooltip(t!("LevelEditor.Hierarchy.DeleteSelected"))
                    .on_click(move |_, _, _| {
                        use crate::commands::{execute_command, SceneCommand};
                        if let Some(id) = state_clone.read().scene.selected_object() {
                            let mut state = state_clone.write();
                            execute_command(&mut state, SceneCommand::RemoveObject { id });
                        }
                    })
                    .into_any_element()
            },
        ];

        let config = HierarchyConfig {
            items,
            root_ids,
            layout: HierarchyLayout::Panel,

            // Panel header
            title: Some(t!("LevelEditor.Hierarchy.Title").to_string()),
            header_buttons,

            // Root drop zone - NOTE: This callback doesn't have access to cx, so it still uses RwLock
            // TODO: Update DropArea to pass window and cx to callbacks
            root_drop_zone: Some((
                "Root".to_string(),
                Arc::new(move |payload: HierarchyDragPayload| {
                    use crate::commands::{execute_command, SceneCommand};
                    let mut state = state_arc.write();
                    execute_command(
                        &mut state,
                        SceneCommand::ReparentObject {
                            id: payload.object_id,
                            new_parent_id: None,
                        },
                    );
                }),
            )),

            // Widget config (not used in Panel mode)
            widget_title: None,
            widget_icon: None,
            widget_add_button: None,
            empty_message: String::new(),

            // Drag-and-drop options
            disable_nesting: false, // Allow full nesting in hierarchy

            // Callbacks
            is_expanded: Arc::new(move |id: &String| {
                state_arc_for_expand.read().hierarchy.is_object_expanded(id)
            }),
            on_toggle_expand: Arc::new(move |id: &String, _window, cx| {
                let id = id.clone();
                let state = state_arc_for_toggle.clone();
                let wrapper = wrapper_for_toggle.clone();
                cx.defer(move |cx| {
                    let mut state = state.write();
                    if state.hierarchy.is_object_expanded(&id) {
                        state.hierarchy.expanded_objects.remove(&id);
                    } else {
                        state.hierarchy.expanded_objects.insert(id);
                    }
                    drop(state);
                    if let Some(wrapper) = wrapper.upgrade() {
                        cx.notify(wrapper.entity_id());
                    }
                });
            }),
            on_select: Arc::new(move |id: &String, window, cx| {
                let id = id.clone();
                let modifiers = window.modifiers();
                let state = state_arc_for_select.clone();
                let wrapper = wrapper_for_select.clone();
                cx.defer(move |cx| {
                    {
                        let mut guard = state.write();
                        apply_row_selection(&mut guard, &id, &modifiers);
                    }
                    if let Some(wrapper) = wrapper.upgrade() {
                        cx.notify(wrapper.entity_id());
                    }
                });
            }),
            on_drop: Arc::new(
                move |payload: HierarchyDragPayload,
                      target_id: &String,
                      modifiers: &Modifiers,
                      _window,
                      cx| {
                    use crate::commands::{execute_command, SceneCommand};
                    if payload.object_id == *target_id {
                        return;
                    }
                    let object_id = payload.object_id.clone();
                    let target_id = target_id.clone();
                    let mods = modifiers.clone();
                    let state = state_arc_for_drop.clone();
                    let wrapper = wrapper_for_drop.clone();

                    cx.defer(move |cx| {
                        let mut state = state.write();
                        if mods.shift {
                            execute_command(
                                &mut state,
                                SceneCommand::ReparentObject {
                                    id: object_id,
                                    new_parent_id: None,
                                },
                            );
                        } else if mods.alt {
                            // Reorder doesn't fit a SceneCommand variant yet — call
                            // directly. The store's own mutation counter
                            // (`store_revision`) advances, so the hierarchy panel's
                            // frame pump picks the change up without any manual
                            // revision bookkeeping crate::scene_edit::objects::reorder_object_siblings(&mut here.state.scene.world_mut(), &object_id, &target_id);
                        } else {
                            let result = execute_command(
                                &mut state,
                                SceneCommand::ReparentObject {
                                    id: object_id,
                                    new_parent_id: Some(target_id.clone()),
                                },
                            );
                            if result.changed {
                                state.hierarchy.expanded_objects.insert(target_id);
                            }
                        }
                        drop(state);
                        if let Some(wrapper) = wrapper.upgrade() {
                            cx.notify(wrapper.entity_id());
                        }
                    });
                },
            ),
        };

        HierarchicalTreeView::new(config).render(cx)
    }

    pub fn get_icon_for_object_type(object_type: ObjectType) -> IconName {
        match object_type {
            ObjectType::Camera => IconName::Camera,
            ObjectType::Folder => IconName::Folder,
            ObjectType::Light(_) => IconName::LightBulb,
            ObjectType::Mesh(_) => IconName::Box,
            ObjectType::Empty => IconName::Circle,
            ObjectType::ParticleSystem => IconName::Sparks,
            ObjectType::AudioSource => IconName::MusicNote,
            ObjectType::Blueprint => IconName::Code,
        }
    }

    pub fn get_icon_color_for_type<V>(object_type: ObjectType, cx: &Context<V>) -> Hsla
    where
        V: Render,
    {
        match object_type {
            ObjectType::Camera => tree_colors::CODE_BLUE,
            ObjectType::Folder => tree_colors::FOLDER,
            ObjectType::Light(_) => tree_colors::SPECIAL_YELLOW,
            ObjectType::Mesh(_) => tree_colors::CODE_PURPLE,
            ObjectType::Empty => cx.theme().muted_foreground,
            ObjectType::ParticleSystem => tree_colors::EFFECT_ORANGE,
            ObjectType::AudioSource => tree_colors::DOC_TEAL,
            ObjectType::Blueprint => tree_colors::CODE_BLUE,
        }
    }
}

/// Put hierarchy-created scene objects at the live editor camera position.
/// The camera pose is read only when the user clicks, so moving the camera
/// while the hierarchy is open does not leave newly created objects at a
/// stale position.
pub(crate) fn editor_camera_spawn_transform(
    gpu_engine: &Arc<std::sync::Mutex<GpuRenderer>>,
) -> crate::scene_edit::Transform {
    let mut transform = crate::scene_edit::Transform::default();
    if let Some(camera) = gpu_engine
        .lock()
        .ok()
        .and_then(|engine| engine.editor_camera_state())
    {
        transform.position = camera.position;
    }
    transform
}

use ui::dock::PanelEvent;
