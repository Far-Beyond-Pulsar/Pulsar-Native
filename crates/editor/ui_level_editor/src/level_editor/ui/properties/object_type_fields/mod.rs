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
use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use ui::button::ButtonVariants as _;
use ui::dropdown::{SearchableList, SearchableListEvent};
use ui::{v_flex, ActiveTheme};
use ui_common::{MeshAssetPicker, PropertyStateManager};

use crate::level_editor::scene_database::SceneDatabase;
use crate::level_editor::state::LevelEditorState;

mod category_section;
mod icon_picker;
mod property_renderer;

pub struct ObjectTypeFieldsSection {
    pub(super) object_id: String,
    pub(super) scene_db: SceneDatabase,
    /// Currently selected component index (reserved for future highlight use).
    pub(super) selected_component: Option<usize>,
    /// Searchable component list for the add-component popover.
    pub(super) component_list: Entity<SearchableList<String>>,
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
        let mut items: Vec<String> = REGISTRY
            .get_class_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        if let Some(pm) = plugin_manager::global() {
            let pm = pm.read();
            let plugin_defs = pm.get_all_component_definitions();
            for def in &plugin_defs {
                if !items.contains(&def.id) {
                    items.push(def.id.clone());
                }
            }
        }
        items.sort();

        let component_list = cx.new(|cx| {
            SearchableList::new(window, cx, items, |name| name.clone())
                .with_empty_text("No components found")
                .with_max_width(px(240.0))
                .with_max_height(px(320.0))
                .with_icon_getter(|_| ui::IconName::Component)
        });

        let scene_db_for_add = scene_db.clone();
        let object_id_for_add = object_id.clone();
        cx.subscribe(
            &component_list,
            move |_this, _, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(class_name) = event {
                    Self::add_component(
                        &scene_db_for_add,
                        &object_id_for_add,
                        class_name,
                        cx,
                    );
                    cx.notify();
                }
            },
        )
        .detach();

        Self {
            object_id,
            scene_db,
            selected_component: None,
            component_list,
            state_arc,
            property_state: PropertyStateManager::new(),
            icon_asset_picker: None,
            collapsed_property_categories: HashSet::new(),
            expanded_property_categories: HashSet::new(),
        }
    }

    fn add_component(
        scene_db: &SceneDatabase,
        object_id: &String,
        class_name: &str,
        _cx: &mut Context<Self>,
    ) {
        let class_name = class_name.to_string();
        if REGISTRY.has_class(&class_name) {
            if let Some(mut instance) = REGISTRY.create_instance(&class_name) {
                let props = instance.get_properties();
                let mut map = serde_json::Map::new();
                for prop in &props {
                    let v = (prop.getter)(instance.as_ref());
                    let json_value = RUNTIME_TYPE_REGISTRY
                        .serialize_json_for_any(v.as_ref())
                        .unwrap_or(serde_json::json!(null));
                    map.insert(prop.name.to_string(), json_value);
                }
                scene_db.add_component(object_id, class_name, Value::Object(map));
            }
        } else if let Some(instance) = engine_backend::EngineBackend::global().and_then(|b| {
            let guard = b.read();
            guard.plugin_components().create_instance(&class_name)
        }) {
            let props = instance.get_properties();
            let mut map = serde_json::Map::new();
            for prop in &props {
                let v = (prop.getter)(instance.as_ref());
                let json_value = RUNTIME_TYPE_REGISTRY
                    .serialize_json_for_any(v.as_ref())
                    .unwrap_or(serde_json::json!(null));
                map.insert(prop.name.to_string(), json_value);
            }
            scene_db.add_component(object_id, class_name, Value::Object(map));
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
        use ui::{IconName, Sizable as _};

        // ── Object icon picker row ─────────────────────────────────────────
        let icon_row = self.render_icon_row(window, cx);

        // ── Component hierarchy panel (tree + add-component button) ────────
        let list = self.component_list.clone();
        let add_popover = Popover::<SearchableList<String>>::new("add-component-picker")
            .anchor(Corner::TopRight)
            .trigger(
                ui::button::Button::new("add-component-btn")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost(),
            )
            .content(move |_window, _cx| list.clone())
            .into_any_element();

        // Fetched once and shared by every consumer below — `get_components`
        // deep-clones each component's JSON, so one call per frame is the budget.
        let attached = self.scene_db.get_components(&self.object_id);

        let component_hierarchy =
            ComponentHierarchyPanel::new(self.object_id.clone(), self.scene_db.clone());
        let state = self.state_arc.read();
        let component_panel = component_hierarchy
            .render(&attached, &state, self.state_arc.clone(), add_popover, cx)
            .into_any_element();
        drop(state);

        // ── Diagnostic banner (no components / registry mismatch) ──────────
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
