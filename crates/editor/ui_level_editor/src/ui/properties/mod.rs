pub mod bindings;
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
    v_flex, ActiveTheme, CollapsibleSection, IconName, Sizable, StyledExt,
};
use ui_common::properties_inspector;

use crate::scene_edit::{ObjectType, Transform};
use crate::state::LevelEditorState;
use crate::workspace::panels::PropertiesPanelWrapper;
use crate::SceneObjectData;

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
        let _scope = gpui::render_stats::scope("properties: inner render");
        let _header_scope = gpui::render_stats::scope("properties: header");
        let has_selection = state.scene.selected_object().is_some();
        drop(_header_scope);
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Professional header
            .child(self.render_header(has_selection, cx))
            // Main content area
            .child(
                div().flex_1().overflow_hidden().w_full().child(
                    div()
                        .size_full()
                        .child(match (has_selection, object_type_fields_section) {
                            // The section is one virtual list holding the object
                            // header, the transform and every component, so it
                            // scrolls itself.
                            (true, Some(section)) => AnyView::from(section.clone())
                                .cached(StyleRefinement::default().size_full())
                                .into_any_element(),
                            (true, None) => div().into_any_element(),
                            (false, _) => Self::render_empty_state(cx).into_any_element(),
                        }),
                ),
            )
    }

    /// A section view as its own cached view whose height follows its content,
    /// so a hover or value push elsewhere in the panel replays it instead of
    /// rebuilding it.
    pub(super) fn cached_section_view(view: &AnyView) -> AnyElement {
        view.clone()
            .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0())
            .into_any_element()
    }

    /// A section that refreshes by itself, like the viewport's overlays: a
    /// transform pushed into it while an object is dragged re-renders just
    /// this section, in place. Only for a section whose size does not follow
    /// what changes; see `AnyView::isolated`.
    pub(super) fn isolated_section_view(view: &AnyView) -> AnyElement {
        view.clone()
            .cached_auto_height(StyleRefinement::default().w_full().flex_shrink_0())
            .isolated()
            .into_any_element()
    }

    fn render_header(
        &self,
        has_selection: bool,
        cx: &Context<PropertiesPanelWrapper>,
    ) -> impl IntoElement {
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
