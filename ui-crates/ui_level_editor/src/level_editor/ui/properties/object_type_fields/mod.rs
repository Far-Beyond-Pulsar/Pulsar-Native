//! Inspector section for a selected scene object.
//!
//! Each concern lives in its own sub-module:
//!
//! | Module               | Responsibility                                              |
//! |----------------------|-------------------------------------------------------------|
//! | [`icon_picker`]      | Object-level icon-asset picker (stored as a plain prop).   |
//! | [`property_renderer`]| Per-component property cards from the reflection registry. |
//! | [`category_section`] | Collapsible category group headers and row layout.         |
//!
//! The legacy "Object Type" card that hard-coded `ObjectType` enum variants
//! has been removed.  Component behaviour now drives all object logic.

use engine_backend::scene::ComponentInstance;
use gpui::{prelude::*, *};
use pulsar_reflection::REGISTRY;
use std::collections::HashSet;
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::{v_flex, ActiveTheme};
use ui_common::{MeshAssetPicker, PropertyStateManager};

use super::super::dialogs::add_component_dialog::AddComponentDialog;
use super::super::state::LevelEditorState;
use crate::level_editor::scene_database::SceneDatabase;

mod category_section;
mod icon_picker;
mod property_renderer;

pub struct ObjectTypeFieldsSection {
    pub(super) object_id: String,
    pub(super) scene_db: SceneDatabase,
    /// Currently selected component index (reserved for future highlight use).
    pub(super) selected_component: Option<usize>,
    /// Add-component dialog entity.
    pub(super) add_component_dialog: Entity<AddComponentDialog>,
    /// Shared level-editor state (expand/collapse, selection).
    pub(super) state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
    /// Shared property widget state (numeric inputs, colour pickers, asset pickers).
    pub(super) property_state: PropertyStateManager,
    /// Asset picker for the object-level icon prop.
    pub(super) icon_asset_picker: Option<Entity<MeshAssetPicker>>,
    /// Categories the user has explicitly collapsed this session.
    pub(super) collapsed_property_categories: HashSet<(String, String)>,
    /// Categories the user has explicitly expanded, overriding the default-collapsed flag.
    pub(super) expanded_property_categories: HashSet<(String, String)>,
}

impl ObjectTypeFieldsSection {
    pub fn new(
        object_id: String,
        scene_db: SceneDatabase,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dialog_object_id = object_id.clone();
        let dialog_scene_db = scene_db.clone();
        let add_component_dialog =
            cx.new(|cx| AddComponentDialog::new(dialog_object_id, dialog_scene_db, window, cx));

        // Refresh the inspector whenever a component is added.
        cx.subscribe(
            &add_component_dialog,
            |_this, _dialog, _event: &super::super::dialogs::add_component_dialog::ComponentAddedEvent, cx| {
                cx.notify();
            },
        )
        .detach();

        Self {
            object_id,
            scene_db,
            selected_component: None,
            add_component_dialog,
            state_arc,
            property_state: PropertyStateManager::new(),
            icon_asset_picker: None,
            collapsed_property_categories: HashSet::new(),
            expanded_property_categories: HashSet::new(),
        }
    }

    /// Returns a diagnostic banner element when no components are attached or
    /// none of the attached components can be found in the reflection registry.
    fn render_diag_card(
        &self,
        attached: &[ComponentInstance],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if attached.is_empty() {
            Some(self.diag_card_element("⚠ No components attached", cx))
        } else if attached
            .iter()
            .all(|c| !REGISTRY.has_class(c.class_name.as_str()))
        {
            Some(self.diag_card_element("⚠ Components not found in registry", cx))
        } else {
            None
        }
    }

    fn diag_card_element(&self, message: &str, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .gap_1()
            .p_3()
            .bg(cx.theme().sidebar)
            .rounded(px(8.0))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(message.to_string()),
            )
            .into_any_element()
    }
}

impl Render for ObjectTypeFieldsSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use super::ComponentHierarchyPanel;
        use ui::popover::Popover;
        use ui::{IconName, Sizable};

        // ── Object icon picker row ─────────────────────────────────────────
        let icon_row = self.render_icon_row(window, cx);

        // ── Component hierarchy panel (tree + add-component button) ────────
        let dialog = self.add_component_dialog.clone();
        let add_popover = Popover::<AddComponentDialog>::new("add-component-picker")
            .anchor(Corner::TopRight)
            .trigger(
                ui::button::Button::new("add-component-btn")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost(),
            )
            .content(move |_window, _cx| dialog.clone())
            .into_any_element();

        let component_hierarchy =
            ComponentHierarchyPanel::new(self.object_id.clone(), self.scene_db.clone());
        let state = self.state_arc.read();
        let component_panel = component_hierarchy
            .render(&state, self.state_arc.clone(), add_popover, cx)
            .into_any_element();
        drop(state);

        // ── Diagnostic banner (no components / registry mismatch) ──────────
        let attached = self.scene_db.get_components(&self.object_id);
        let diag_card = self.render_diag_card(&attached, cx);

        // ── Per-component property cards ───────────────────────────────────
        let component_sections = self.render_component_sections(&attached, window, cx);

        v_flex()
            .w_full()
            .gap_3()
            .child(icon_row)
            .child(component_panel)
            .children(diag_card)
            .children(component_sections)
            .into_any_element()
    }
}
