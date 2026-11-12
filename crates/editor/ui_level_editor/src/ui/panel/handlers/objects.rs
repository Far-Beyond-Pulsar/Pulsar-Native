use super::*;

impl LevelEditorPanel {
    pub(in crate::ui::panel) fn on_add_object(
        &mut self,
        _: &AddObject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::commands::{execute_command, SceneCommand};
        let mut state = self.shared_state.write();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: "New Object".to_string(),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
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
        drop(state);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_add_object_of_type(
        &mut self,
        action: &AddObjectOfType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.shared_state.write();
        if let Some(position) = action.world_position {
            crate::scene_edit::create::create_at(&mut state, &action.object_type, position);
        } else {
            crate::scene_edit::create::create(&mut state, &action.object_type);
        }
        drop(state);
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_delete_object(
        &mut self,
        _: &DeleteObject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::commands::{execute_command, SceneCommand};
        let selected = self.shared_state.read().scene.selected_object();
        if let Some(id) = selected {
            let mut state = self.shared_state.write();
            execute_command(&mut state, SceneCommand::RemoveObject { id });
            drop(state);
            self.sync_gizmo_to_helio();
        }
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_duplicate_object(
        &mut self,
        _: &DuplicateObject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.shared_state.read().scene.selected_object();
        if let Some(id) = selected {
            let mut state = self.shared_state.write();
            crate::commands::duplicate_and_select(&mut state, &id);
            drop(state);
            self.sync_gizmo_to_helio();
        }
        cx.notify();
    }

    /// Undo the last mutating scene command (Pulsar-Native#554). The restore
    /// writes through the World like any edit, so the renderer follows it
    /// from the revision; only the gizmo needs re-syncing.
    pub(in crate::ui::panel) fn on_undo(
        &mut self,
        _: &crate::ui::actions::Undo,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.shared_state.write();
        crate::extend::finish_interactions(&mut state, true);
        if state.scene.undo() {
            state.scene.bump_revision(true);
            drop(state);
            self.sync_gizmo_to_helio();
        }
        cx.notify();
    }

    /// Redo the last undone scene command. See [`Self::on_undo`]'s doc.
    pub(in crate::ui::panel) fn on_redo(
        &mut self,
        _: &crate::ui::actions::Redo,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = self.shared_state.write();
        if state.scene.redo() {
            state.scene.bump_revision(true);
            drop(state);
            self.sync_gizmo_to_helio();
        }
        cx.notify();
    }

    /// The Edit menu's Undo, for a plugin's panel (the spline panel's
    /// buttons).
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.on_undo(&crate::ui::actions::Undo, window, cx);
    }

    /// The Edit menu's Redo, for a plugin's panel.
    pub fn redo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.on_redo(&crate::ui::actions::Redo, window, cx);
    }

    pub(in crate::ui::panel) fn on_select_object(
        &mut self,
        action: &SelectObject,
        _: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .scene
            .select_object(Some(action.object_id.clone()));
        self.sync_gizmo_to_helio(); // Sync gizmo to follow selected object
                                    // Selection is observed by the hierarchy and properties panels' own
                                    // frame pumps. Notifying this root view rebuilds the entire dock tree
                                    // for a change those panels can handle locally.
    }

    pub(in crate::ui::panel) fn on_toggle_object_expanded(
        &mut self,
        action: &ToggleObjectExpanded,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .hierarchy
            .toggle_object_expanded(&action.object_id);
        cx.notify();
    }
}
