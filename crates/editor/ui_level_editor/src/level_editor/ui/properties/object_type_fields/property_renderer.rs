//! Per-component property card rendering.
//!
//! For each [`ComponentInstance`] attached to the selected object, this module:
//!   1. Looks up cached property metadata (populated lazily, reused across frames).
//!   2. Reads current values from the scene database.
//!   3. Delegates row rendering to [`ui_common::render_property_row_runtime`],
//!      which picks the editor registered for each property's type.
//!   4. Groups rows into collapsible category sections via [`category_section`].
//!
//! Performance notes:
//! - The metadata cache eliminates the per-frame `create_instance()` +
//!   `get_properties()` allocation — previously the single biggest cost.
//! - `with_world_component` holds one `store.read()` per component,
//!   reading all property values inside that lock instead of acquiring
//!   a separate lock per property.

use engine_backend::scene::ComponentInstance;
use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyMetadata, REGISTRY, RUNTIME_TYPE_REGISTRY};
use std::any::Any;
use std::sync::Arc;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use crate::level_editor::core::commands::{execute_command, SceneCommand};
use crate::level_editor::core::scene_database::PropertyChangeSet;
use super::category_section::group_rows_by_category;
use super::{ObjectTypeFieldsSection, PropertyMetadataCacheEntry};

/// Read a property value from the live World, with JSON and default-instance
/// fallbacks.  Used only when the batch read (via `with_world_component`)
/// fails — e.g. the entity doesn't exist in the World.
fn read_property_from_world(
    scene_db: &crate::level_editor::scene_database::SceneDatabase,
    object_id: &crate::level_editor::scene_database::ObjectId,
    class_name: &str,
    prop: &PropertyMetadata,
    component: &ComponentInstance,
    default_instance: &dyn pulsar_reflection::EngineClass,
) -> Box<dyn Any> {
    scene_db
        .read_live_component_property(object_id, class_name, prop.name)
        .or_else(|| {
            component
                .data
                .get(prop.name)
                .filter(|json| !json.is_null())
                .and_then(|json| {
                    RUNTIME_TYPE_REGISTRY
                        .deserialize_json_for_type(prop.type_info, json.clone())
                        .ok()
                })
        })
        .unwrap_or_else(|| (prop.getter)(default_instance))
}

impl ObjectTypeFieldsSection {
    /// Ensure the metadata cache is populated for `class_name`.
    fn ensure_metadata_cached(&mut self, class_name: &str) -> Option<()> {
        if self.property_metadata_cache.contains_key(class_name) {
            return Some(());
        }
        let instance = REGISTRY.create_instance(class_name)?;
        let properties = instance.get_properties();
        if properties.is_empty() {
            return None;
        }
        self.property_metadata_cache.insert(
            class_name.to_string(),
            Arc::new(PropertyMetadataCacheEntry {
                properties: Arc::new(properties),
                _default_instance: instance,
            }),
        );
        Some(())
    }

    /// Builds a property-card element for every attached component that has at
    /// least one reflected property present in the registry.
    ///
    /// Components whose class is not in the registry are silently skipped — the
    /// diagnostic banner in [`super`] already surfaces that condition.
    ///
    /// ## Performance
    /// Uses `with_world_component` to read ALL property values of a component
    /// under a single `store.read()` acquisition (one RwLock READ per component
    /// instead of one per property).
    pub(super) fn render_component_sections(
        &mut self,
        attached: &[ComponentInstance],
        _property_changes: &PropertyChangeSet,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Deliberately not logged per render: this runs for every row rebuild
        // of the panel, and `REGISTRY.get_class_names()` alone allocated a
        // full registry-size Vec each time.
        tracing::trace!(
            "[ObjectTypeFieldsSection] object_id={} attached={}",
            self.object_id,
            attached.len(),
        );

        let class_names: Vec<String> = attached.iter().map(|c| c.class_name.clone()).collect();
        for name in &class_names {
            let _ = self.ensure_metadata_cached(name);
        }

        let scene_db = self.scene_db.clone();
        let object_id = self.object_id.clone();

        attached
            .iter()
            .enumerate()
            .filter_map(|(idx, component)| {
                let class_name = &class_names[idx];

                let properties = {
                    let cached = self.property_metadata_cache.get(class_name.as_str())?;
                    Arc::clone(&cached.properties)
                };
                if properties.is_empty() {
                    return None;
                }

                let default_inst: &dyn pulsar_reflection::EngineClass = &*self
                    .property_metadata_cache
                    .get(class_name.as_str())?
                    ._default_instance;

                // ── Single lock acquisition: read ALL property values ─────
                // The closure holds `store.read()` for the entire duration
                // and calls each getter against the live World component.
                // Values are consumed via `into_iter()` so each `Box<dyn Any>`
                // is used exactly once — no Clone needed.
                let batch_values: Option<Vec<Box<dyn Any>>> =
                    scene_db.with_world_component(&object_id, class_name, |instance| {
                        properties
                            .iter()
                            .map(|prop| (prop.getter)(instance))
                            .collect()
                    });

                let mut row_data: Vec<(
                    AnyElement,
                    Option<String>,
                    Option<String>,
                    bool,
                    Option<usize>,
                )> = Vec::new();

                if let Some(values) = batch_values {
                    // Batch succeeded — consume values via into_iter().
                    for (prop, value) in properties.iter().zip(values.into_iter()) {
                        let write_back = {
                            let state_arc = self.state_arc.clone();
                            let oid = object_id.clone();
                            let cls = class_name.clone();
                            let pn = prop.name.to_string();
                            Arc::new(
                                move |new_val: Box<dyn Any + Send>,
                                      _window: &mut Window,
                                      _cx: &mut App| {
                                    execute_command(
                                        &mut state_arc.write(),
                                        SceneCommand::SetComponentProperty {
                                            id: oid.clone(),
                                            class_name: cls.clone(),
                                            prop_name: pn.clone(),
                                            value: new_val,
                                        },
                                    );
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
                            value.as_ref(),
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
                } else {
                    // Batch miss — entity not in World or class not hydrated.
                    // Fall back to per-property reads (JSON → default).
                    for prop in properties.iter() {
                        let current_any = read_property_from_world(
                            &scene_db,
                            &object_id,
                            class_name,
                            prop,
                            component,
                            default_inst,
                        );

                        let write_back = {
                            let state_arc = self.state_arc.clone();
                            let oid = object_id.clone();
                            let cls = class_name.clone();
                            let pn = prop.name.to_string();
                            Arc::new(
                                move |new_val: Box<dyn Any + Send>,
                                      _window: &mut Window,
                                      _cx: &mut App| {
                                    execute_command(
                                        &mut state_arc.write(),
                                        SceneCommand::SetComponentProperty {
                                            id: oid.clone(),
                                            class_name: cls.clone(),
                                            prop_name: pn.clone(),
                                            value: new_val,
                                        },
                                    );
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
                }

                let (mut uncategorized, categorized) = group_rows_by_category(row_data);
                let category_elements =
                    self.render_categorized_rows(class_name, categorized, cx);
                uncategorized.extend(category_elements);

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
                                        .child(class_name.clone()),
                                ),
                        )
                        .children(uncategorized)
                        .into_any_element(),
                )
            })
            .collect()
    }
}
