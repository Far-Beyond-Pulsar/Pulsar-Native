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
        use crate::commands::{execute_command, SceneCommand};
        let mut state = self.shared_state.write();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: format!("New {}", action.object_type),
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
        use crate::commands::{execute_command, SceneCommand};
        let selected = self.shared_state.read().scene.selected_object();
        if let Some(id) = selected {
            let mut state = self.shared_state.write();
            execute_command(
                &mut state,
                SceneCommand::DuplicateObject {
                    source_id: id,
                    count: 1,
                    position_offset: None,
                },
            );
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
        crate::state::voxel::VoxelStroke::finish(&mut state, true);
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

    pub(in crate::ui::panel) fn on_select_object(
        &mut self,
        action: &SelectObject,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .scene
            .select_object(Some(action.object_id.clone()));
        self.sync_gizmo_to_helio(); // Sync gizmo to follow selected object
        cx.notify();
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
