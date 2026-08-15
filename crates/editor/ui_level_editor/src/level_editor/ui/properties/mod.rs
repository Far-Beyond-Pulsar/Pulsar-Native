pub mod component_hierarchy;
pub mod object_header_section;
pub mod object_type_fields;
pub mod transform_section;

pub use component_hierarchy::ComponentHierarchyPanel;
pub use object_header_section::ObjectHeaderSection;
pub use object_type_fields::ObjectTypeFieldsSection;
pub use transform_section::TransformSection;

use gpui::{prelude::*, *};
use rust_i18n::t;
use std::collections::HashSet;
use std::sync::Arc;
use ui::{
    button::Button,
    h_flex,
    input::{InputState, TextInput},
    scroll::ScrollbarAxis,
    v_flex, ActiveTheme, CollapsibleSection, IconName, Sizable, StyledExt,
};
use ui_common::properties_inspector;

use crate::level_editor::scene_database::{ObjectType, Transform};
use crate::level_editor::state::LevelEditorState;
use crate::level_editor::workspace::panels::PropertiesPanelWrapper;
use crate::level_editor::SceneObjectData;

/// Properties Panel - Inspector showing properties of the selected object
pub struct PropertiesPanel;

impl PropertiesPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        editing_property: &Option<String>,
        property_input: &Entity<InputState>,
        collapsed_sections: &HashSet<String>,
        object_header_section: &Option<Entity<super::ObjectHeaderSection>>,
        transform_section: &Option<Entity<super::TransformSection>>,
        object_type_fields_section: &Option<Entity<super::ObjectTypeFieldsSection>>,
        window: &mut Window,
        cx: &mut Context<PropertiesPanelWrapper>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Professional header
            .child(self.render_header(state, cx))
            // Main content area
            .child(div().flex_1().overflow_hidden().w_full().child(
                div().size_full().scrollable(ScrollbarAxis::Vertical).child(
                    if let Some(_selected) = state.scene.get_selected_object() {
                        let mut flex = v_flex().w_full().p_3().gap_4().min_w_full();

                        // Render new ObjectHeaderSection if available (new binding system)
                        if let Some(ref section) = object_header_section {
                            flex = flex.child(section.clone());
                        }

                        // Render new TransformSection if available (new binding system)
                        if let Some(ref section) = transform_section {
                            flex = flex.child(section.clone());
                        }

                        // Reflection-backed object type properties — always present.
                        if let Some(ref section) = object_type_fields_section {
                            flex = flex.child(section.clone());
                        }

                        flex.into_any_element()
                    } else {
                        Self::render_empty_state(cx).into_any_element()
                    },
                ),
            ))
    }

    fn render_header(
        &self,
        state: &LevelEditorState,
        cx: &Context<PropertiesPanelWrapper>,
    ) -> impl IntoElement {
        let has_selection = state.scene.get_selected_object().is_some();

        properties_inspector::render_header(
            t!("LevelEditor.Properties.Title").to_string(),
            has_selection,
            t!("LevelEditor.Properties.Selected").to_string(),
            "level-editor-properties-more",
            cx,
        )
    }

    fn render_empty_state(cx: &Context<PropertiesPanelWrapper>) -> impl IntoElement {
        properties_inspector::render_empty_state(
            IconName::CursorPointer,
            t!("LevelEditor.Properties.NoSelection").to_string(),
            t!("LevelEditor.Properties.NoSelectionDesc").to_string(),
            cx,
        )
    }
}
