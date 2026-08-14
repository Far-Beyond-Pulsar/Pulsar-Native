//! Per-component property card rendering.
//!
//! For each [`ComponentInstance`] attached to the selected object, this module:
//!   1. Creates instances via the reflection registry to read property metadata.
//!   2. Reads current values from the scene database.
//!   3. Delegates row rendering to [`ui_common::render_property_row_runtime`],
//!      which picks the editor registered for each property's type.
//!   4. Groups rows into collapsible category sections via [`category_section`].
//!
//! TODO(Pulsar-Native#575, SceneDB#47): the read side (`current_any` below)
//! calls `SceneDatabase::read_live_component_property` -- a fresh
//! `World` lookup per property, unconditionally, every time this section
//! renders. Correct, but polling: `World` has no way to signal "this
//! component actually changed" yet, so there's nothing better to do today.
//! Once SceneDB gains an entity/component listener/subscription system
//! (Far-Beyond-Pulsar/SceneDB#47), this should subscribe per attached
//! component instead and only re-pull on a real signal, caching in between.

use engine_backend::scene::ComponentInstance;
use gpui::{prelude::*, *};
use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use std::any::Any;
use std::sync::Arc;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use super::category_section::group_rows_by_category;
use super::ObjectTypeFieldsSection;

impl ObjectTypeFieldsSection {
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
                    // Read straight off the live `World`-resident component
                    // when one exists (Pulsar-Native#561) -- correctly
                    // handles `#[sub_props]` nesting, and it's the one real
                    // value, not a copy. Falls back to the flat-JSON channel
                    // only for the shrinking set of classes with no
                    // `ComponentRuntimeBehavior` (see
                    // `SceneDatabase::update_component_property`'s doc), then
                    // to this throwaway instance's own default value.
                    let current_any: Box<dyn Any> = self
                        .scene_db
                        .read_live_component_property(&self.object_id, class_name, prop.name)
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
                        .unwrap_or_else(|| (prop.getter)(instance.as_ref()));

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
                                // Live `World` mutation first, no JSON
                                // involved (Pulsar-Native#561) -- true for
                                // every real, migrated component. Only the
                                // handful of props-only classes with no
                                // `ComponentRuntimeBehavior` fall through to
                                // the legacy flat-JSON path below.
                                let new_val =
                                    match db.update_live_component_property(&oid, &cls, &pn, new_val)
                                    {
                                        Ok(()) => return,
                                        Err(new_val) => new_val,
                                    };
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
