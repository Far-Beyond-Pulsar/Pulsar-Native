//! Per-component property card rendering.
//!
//! For each [`ComponentInstance`] attached to the selected object, this module:
//!   1. Creates instances via the reflection registry to read property metadata.
//!   2. Reads current values from the scene database.
//!   3. Delegates row rendering to [`ui_common::render_property_row_runtime`],
//!      which picks the editor registered for each property's type.
//!   4. Groups rows into collapsible category sections via [`category_section`].

use engine_backend::scene::ComponentInstance;
use gpui::{prelude::*, *};
use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use serde_json::Value;
use std::any::Any;
use std::sync::Arc;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use super::category_section::group_rows_by_category;
use super::ObjectTypeFieldsSection;

impl ObjectTypeFieldsSection {
    /// Reads a property value for `class_name::property_name` from the scene
    /// database, falling back to `default_json` if no value is stored yet.
    pub(super) fn read_property_json(
        &self,
        class_name: &str,
        property_name: &str,
        default_json: &Value,
    ) -> Value {
        let components = self.scene_db.get_components(&self.object_id);
        components
            .iter()
            .find(|c| c.class_name == class_name)
            .and_then(|c| c.data.get(property_name).cloned())
            .unwrap_or_else(|| default_json.clone())
    }

    /// Builds a property-card element for every attached component that has at
    /// least one reflected property present in the registry.
    ///
    /// Components whose class is not in the registry are silently skipped — the
    /// diagnostic banner in [`super`] already surfaces that condition.
    pub(super) fn render_component_sections(
        &mut self,
        attached: &[ComponentInstance],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        tracing::debug!(
            "[ObjectTypeFieldsSection] object_id={} attached={} registry={}",
            self.object_id,
            attached.len(),
            REGISTRY.get_class_names().len(),
        );

        attached
            .iter()
            .filter_map(|component| {
                let class_name = component.class_name.as_str();
                let instance = REGISTRY.create_instance(class_name)?;
                let properties = instance.get_properties();
                if properties.is_empty() {
                    return None;
                }

                // ── Per-property widget state + row rendering ──────────────────

                let mut row_data: Vec<(
                    AnyElement,
                    Option<String>,
                    Option<String>,
                    bool,
                    Option<usize>,
                )> = Vec::new();

                let scene_db_for_props = self.scene_db.clone();
                let object_id_for_props = self.object_id.clone();

                for prop in &properties {
                    let default_any = (prop.getter)(instance.as_ref());
                    let current_json =
                        self.read_property_json(class_name, prop.name, &Value::Null);
                    let current_any: Box<dyn Any> = if current_json.is_null() {
                        default_any
                    } else {
                        RUNTIME_TYPE_REGISTRY
                            .deserialize_json_for_type(prop.type_info, current_json.clone())
                            .unwrap_or_else(|_| default_any)
                    };

                    // ── Write-back closure for the runtime renderer ──────────
                    let write_back = {
                        let db = scene_db_for_props.clone();
                        let oid = object_id_for_props.clone();
                        let cls = class_name.to_string();
                        let pn = prop.name.to_string();
                        Arc::new(
                            move |new_val: Box<dyn Any + Send>,
                                  _window: &mut Window,
                                  _cx: &mut App| {
                                if let Ok(json) =
                                    RUNTIME_TYPE_REGISTRY.serialize_json_for_any(new_val.as_ref())
                                {
                                    db.update_component_property(&oid, &cls, &pn, json);
                                }
                            },
                        )
                    };

                    let row = ui_common::render_property_row_runtime(
                        &mut self.property_state,
                        "level",
                        class_name,
                        &prop.display_name,
                        prop.name,
                        prop.type_info,
                        current_any.as_ref(),
                        write_back,
                        window,
                        cx,
                    );

                    row_data.push((
                        row,
                        prop.category.map(str::to_string),
                        prop.category_color.map(str::to_string),
                        prop.category_default_collapsed,
                        prop.category_order,
                    ));
                }

                // ── Group into uncategorised + categorised buckets ─────────────

                let (mut uncategorized, categorized) = group_rows_by_category(row_data);

                let category_elements = self.render_categorized_rows(class_name, categorized, cx);

                uncategorized.extend(category_elements);

                // ── Wrap in a named component card ─────────────────────────────

                Some(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .p_3()
                        .bg(cx.theme().sidebar)
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_2()
                                .child(Icon::new(IconName::Component).small())
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(cx.theme().foreground)
                                        .child(class_name.to_string()),
                                ),
                        )
                        .children(uncategorized)
                        .into_any_element(),
                )
            })
            .collect()
    }
}
