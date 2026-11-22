//! The properties panel (`PropertiesPanel`): object header, transform and the component
//! hierarchy.

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
use std::sync::Arc;
use ui::{input::InputState, v_flex, ActiveTheme, IconName};
use ui_common::properties_inspector;

use crate::state::LevelEditorState;
use crate::workspace::panels::PropertiesPanelWrapper;

/// Properties Panel - Inspector showing properties of the selected object
pub struct PropertiesPanel;

impl PropertiesPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        state: &LevelEditorState,
        _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _editing_property: &Option<String>,
        _property_input: &Entity<InputState>,
        _object_header_section: &Option<Entity<super::ObjectHeaderSection>>,
        _transform_section: &Option<Entity<super::TransformSection>>,
        object_type_fields_section: &Option<Entity<super::ObjectTypeFieldsSection>>,
        _window: &mut Window,
        cx: &mut Context<PropertiesPanelWrapper>,
    ) -> impl IntoElement {
        let _scope = gpui::render_stats::scope("properties: inner render");
        let _header_scope = gpui::render_stats::scope("properties: header");
        let primary = state.scene.selected_object();
        let has_selection = primary.is_some();
        let mass_edit = primary
            .as_deref()
            .map(|primary| crate::ui::selection::mass_edit(state, primary));
        drop(_header_scope);
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Professional header
            .child(self.render_header(has_selection, cx))
            .children(mass_edit.and_then(|mode| Self::render_mass_edit_banner(&mode, cx)))
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

    /// With several objects selected: whether a property edit goes to all of
    /// them (they share one archetype) or only to the primary shown below.
    fn render_mass_edit_banner(
        mode: &crate::ui::selection::MassEdit,
        cx: &Context<PropertiesPanelWrapper>,
    ) -> Option<AnyElement> {
        use crate::ui::selection::MassEdit;
        let (text, color) = match mode {
            MassEdit::Single => return None,
            MassEdit::All(peers) => (
                t!("LevelEditor.Properties.MassEdit", count = peers.len() + 1).to_string(),
                cx.theme().accent_foreground,
            ),
            MassEdit::PrimaryOnly(count) => (
                t!("LevelEditor.Properties.MixedSelection", count = count).to_string(),
                cx.theme().muted_foreground,
            ),
        };
        let background = match mode {
            MassEdit::All(_) => cx.theme().accent,
            _ => cx.theme().muted,
        };
        Some(
            div()
                .w_full()
                .px_3()
                .py_1()
                .bg(background)
                .text_xs()
                .text_color(color)
                .child(text)
                .into_any_element(),
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
