//! Object Header Section - Name, visibility, and lock status
//!
//! This component provides editable fields for basic object properties that appear
//! at the top of the properties panel.

use gpui::{prelude::*, *};
use std::sync::Arc;
use ui::{h_flex, v_flex, ActiveTheme};

use super::super::bindings::bound_field::{BoolBoundField, StringBoundField};
use crate::level_editor::core::commands::{execute_command, SceneCommand};
use crate::level_editor::scene_database::SceneDatabase;
use crate::level_editor::state::LevelEditorState;

/// Object header section showing name, visibility, and locked status
pub struct ObjectHeaderSection {
    name_field: Entity<StringBoundField>,
    visible_field: Entity<BoolBoundField>,
    locked_field: Entity<BoolBoundField>,
    object_id: String,
}

impl ObjectHeaderSection {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use super::super::bindings::field_bindings::{BoolFieldBinding, StringFieldBinding};

        // Name/visible/locked all go through `SceneCommand`/`execute_command`
        // now (Pulsar-Native#561), not `SceneDatabase::update_object`
        // (whole-object overwrite, and -- despite a since-removed comment
        // claiming otherwise -- never actually undo-tracked).

        // Name field. Getters use the targeted component reads (see
        // `SceneDatabase::get_object_name`'s doc) — these run on every scene
        // revision bump while this object is selected, and a whole-object
        // read per bump scaled the panel's cost with component count.
        let name_field = cx.new(|cx| {
            let state_arc = state_arc.clone();
            StringBoundField::new(
                StringFieldBinding::new_with_db(
                    |id, db| db.get_object_name(id),
                    move |id, name, _db| {
                        execute_command(
                            &mut state_arc.write(),
                            SceneCommand::SetName {
                                id: id.clone(),
                                name,
                            },
                        )
                        .changed
                    },
                ),
                "Name",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Visible field
        let visible_field = cx.new(|cx| {
            let state_arc = state_arc.clone();
            BoolBoundField::new(
                BoolFieldBinding::new_with_db(
                    |id, db| db.get_object_visibility(id).map(|(visible, _)| visible),
                    move |id, visible, _db| {
                        execute_command(
                            &mut state_arc.write(),
                            SceneCommand::SetVisibility {
                                id: id.clone(),
                                visible: Some(visible),
                                locked: None,
                            },
                        )
                        .changed
                    },
                ),
                "Visible",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        // Locked field
        let locked_field = cx.new(|cx| {
            BoolBoundField::new(
                BoolFieldBinding::new_with_db(
                    |id, db| db.get_object_visibility(id).map(|(_, locked)| locked),
                    move |id, locked, _db| {
                        execute_command(
                            &mut state_arc.write(),
                            SceneCommand::SetVisibility {
                                id: id.clone(),
                                visible: None,
                                locked: Some(locked),
                            },
                        )
                        .changed
                    },
                ),
                "Locked",
                object_id.clone(),
                scene_db.clone(),
                window,
                cx,
            )
        });

        Self {
            name_field,
            visible_field,
            locked_field,
            object_id,
        }
    }

    /// Refresh all fields when scene data changes externally
    pub fn refresh(&self, window: &mut Window, cx: &mut App) {
        self.name_field
            .update(cx, |field, cx| field.refresh(window, cx));
        self.visible_field
            .update(cx, |field, cx| field.refresh(window, cx));
        self.locked_field
            .update(cx, |field, cx| field.refresh(window, cx));
    }
}

impl Render for ObjectHeaderSection {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .p_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            // Name field
            .child(self.name_field.clone())
            // Visibility and Locked toggles in a row
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .child(div().flex_1().child(self.visible_field.clone()))
                    .child(div().flex_1().child(self.locked_field.clone())),
            )
    }
}
