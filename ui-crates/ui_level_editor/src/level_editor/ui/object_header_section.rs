//! Object Header Section - Name, visibility, and lock status
//!
//! This component provides editable fields for basic object properties that appear
//! at the top of the properties panel.

use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt,
    checkbox::Checkbox,
};

use crate::level_editor::scene_database::SceneDatabase;
use super::bound_field::{StringBoundField, BoolBoundField};

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        use super::field_bindings::{StringFieldBinding, BoolFieldBinding};

        // Name field
        let name_field = cx.new(|cx| {
            StringBoundField::new(
                StringFieldBinding::new(
                    |obj| obj.name.clone(),
                    |obj, val| obj.name = val,
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
            BoolBoundField::new(
                BoolFieldBinding::new(
                    |obj| obj.visible,
                    |obj, val| obj.visible = val,
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
                BoolFieldBinding::new(
                    |obj| obj.locked,
                    |obj, val| obj.locked = val,
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
        self.name_field.update(cx, |field, cx| field.refresh(window, cx));
        self.visible_field.update(cx, |field, cx| field.refresh(window, cx));
        self.locked_field.update(cx, |field, cx| field.refresh(window, cx));
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
                    .child(
                        div()
                            .flex_1()
                            .child(self.visible_field.clone())
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(self.locked_field.clone())
                    )
            )
    }
}
