//! Per-component property card rendering.
//!
//! For each [`ComponentInstance`] attached to the selected object, this module:
//!   1. Creates instances via the reflection registry to read property metadata.
//!   2. Reads current values from the scene database.
//!   3. Ensures the appropriate widget state (numeric input, colour picker, asset picker).
//!   4. Delegates row rendering to [`ui_common::render_property_row_runtime`].
//!   5. Groups rows into collapsible category sections via [`category_section`].

use engine_backend::scene::ComponentInstance;
use gpui::{prelude::*, *};
use pulsar_reflection::{TypeStructure, REGISTRY, RUNTIME_TYPE_REGISTRY};
use serde_json::Value;
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

                // ── Build callbacks (shared across all props for this component) ──

                let db_bool = self.scene_db.clone();
                let oid_bool = self.object_id.clone();
                let cls_bool = class_name.to_string();
                let on_bool_toggle = Arc::new(
                    move |prop_name: &str, checked: bool, _w: &mut Window, _cx: &mut App| {
                        db_bool.update_component_property(
                            &oid_bool,
                            &cls_bool,
                            prop_name,
                            Value::from(checked),
                        );
                    },
                );

                let db_enum = self.scene_db.clone();
                let oid_enum = self.object_id.clone();
                let cls_enum = class_name.to_string();
                let on_enum_select = Arc::new(
                    move |prop_name: &str, ix: usize, _w: &mut Window, _cx: &mut App| {
                        db_enum.update_component_property(
                            &oid_enum,
                            &cls_enum,
                            prop_name,
                            Value::from(ix as u64),
                        );
                    },
                );

                // ── Per-property widget state + row rendering ──────────────────

                let mut row_data: Vec<(
                    AnyElement,
                    Option<String>,
                    Option<String>,
                    bool,
                    Option<usize>,
                )> = Vec::new();

                for prop in &properties {
                    let default_any = (prop.getter)(instance.as_ref());
                    let default_json = RUNTIME_TYPE_REGISTRY
                        .serialize_json_for_any(default_any.as_ref())
                        .unwrap_or(serde_json::json!(null));
                    let current_json =
                        self.read_property_json(class_name, prop.name, &default_json);

                    // Numeric inputs
                    match &prop.type_info.structure {
                        TypeStructure::Primitive if prop.type_info.base_name() == "f32" => {
                            let v = current_json.as_f64().unwrap_or(0.0) as f32;
                            let cls = class_name.to_string();
                            let pn = prop.name.to_string();
                            let db = self.scene_db.clone();
                            let oid = self.object_id.clone();
                            self.property_state.ensure_f32_input(
                                class_name,
                                prop.name,
                                v,
                                1.0,
                                move |new_val| {
                                    db.update_component_property(
                                        &oid,
                                        &cls,
                                        &pn,
                                        Value::from(new_val),
                                    );
                                },
                                window,
                                cx,
                            );
                        }
                        TypeStructure::Primitive if prop.type_info.base_name() == "i32" => {
                            let v = current_json.as_i64().unwrap_or(0) as i32;
                            let cls = class_name.to_string();
                            let pn = prop.name.to_string();
                            let db = self.scene_db.clone();
                            let oid = self.object_id.clone();
                            self.property_state.ensure_i32_input(
                                class_name,
                                prop.name,
                                v,
                                move |new_val| {
                                    db.update_component_property(
                                        &oid,
                                        &cls,
                                        &pn,
                                        Value::from(new_val),
                                    );
                                },
                                window,
                                cx,
                            );
                        }
                        _ => {}
                    }

                    // Mesh asset picker (by type name or conventional field name)
                    if prop.type_info.type_name == "MeshAssetPath"
                        || (prop.type_info.is_string() && prop.name == "mesh_asset")
                    {
                        let v = current_json.as_str().unwrap_or("");
                        let cls = class_name.to_string();
                        let pn = prop.name.to_string();
                        let db = self.scene_db.clone();
                        let oid = self.object_id.clone();
                        self.property_state.ensure_mesh_asset_picker(
                            class_name,
                            prop.name,
                            v,
                            move |new_val| {
                                db.update_component_property(
                                    &oid,
                                    &cls,
                                    &pn,
                                    Value::String(new_val),
                                );
                            },
                            window,
                            cx,
                        );
                    }

                    // Colour picker (`[f32; 4]` primitive or colour-named field)
                    let is_color = matches!(
                        &prop.type_info.structure,
                        TypeStructure::Primitive if prop.type_info.base_name() == "[f32; 4]"
                    ) || ui_common::reflected_properties_panel::is_color_field_name(prop.name);

                    if is_color {
                        let rgba = current_json
                            .as_array()
                            .and_then(|arr| {
                                (arr.len() == 4).then(|| {
                                    [
                                        arr[0].as_f64().unwrap_or(1.0) as f32,
                                        arr[1].as_f64().unwrap_or(1.0) as f32,
                                        arr[2].as_f64().unwrap_or(1.0) as f32,
                                        arr[3].as_f64().unwrap_or(1.0) as f32,
                                    ]
                                })
                            })
                            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
                        let cls = class_name.to_string();
                        let pn = prop.name.to_string();
                        let db = self.scene_db.clone();
                        let oid = self.object_id.clone();
                        self.property_state.ensure_color_picker(
                            class_name,
                            prop.name,
                            rgba,
                            move |rgba| {
                                db.update_component_property(
                                    &oid,
                                    &cls,
                                    &pn,
                                    serde_json::json!(rgba),
                                );
                            },
                            window,
                            cx,
                        );
                    }

                    // Render the row using the shared runtime renderer
                    let widgets = self.property_state.widget_map_for(class_name, prop.name);

                    let prop_bool = prop.name.to_string();
                    let on_bool = on_bool_toggle.clone();
                    let bool_cb = Arc::new(
                        move |checked: bool, window: &mut Window, cx: &mut App| {
                            (on_bool)(&prop_bool, checked, window, cx);
                        },
                    );

                    let prop_enum = prop.name.to_string();
                    let on_enum = on_enum_select.clone();
                    let enum_cb =
                        Arc::new(move |ix: usize, window: &mut Window, cx: &mut App| {
                            (on_enum)(&prop_enum, ix, window, cx);
                        });

                    let row = ui_common::render_property_row_runtime(
                        "level",
                        class_name,
                        &prop.display_name,
                        prop.name,
                        prop.type_info,
                        &current_json,
                        widgets,
                        bool_cb,
                        enum_cb,
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

                let category_elements =
                    self.render_categorized_rows(class_name, categorized, cx);

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
