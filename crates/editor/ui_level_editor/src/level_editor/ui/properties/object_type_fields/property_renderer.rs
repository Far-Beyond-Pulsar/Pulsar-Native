//! Per-component property card rendering.
//!
//! For each [`ComponentInstance`] attached to the selected object, this module:
//!   1. Looks up cached property metadata (populated lazily, reused across frames).
//!   2. Serves current values from the section's per-card snapshot cache,
//!      re-pulling from the live World only when a signal fired for that
//!      specific card since the last render (Pulsar-Native#575): its own
//!      World subscription event, a legacy JSON-path write, or a
//!      structural/epoch invalidation.
//!   3. Delegates row rendering to [`ui_common::render_property_row_runtime`],
//!      which picks the editor registered for each property's type.
//!   4. Groups rows into collapsible category sections via [`category_section`].
//!
//! Performance notes:
//! - The metadata cache eliminates the per-frame `create_instance()` +
//!   `get_properties()` allocation — previously the single biggest cost.
//! - The value snapshot cache eliminates the per-render World re-pull that
//!   used to run unconditionally for every visible property on every pass;
//!   a clean render performs zero World reads. A fresh pull holds one
//!   `store.read()` per component (`with_world_component`), reading all
//!   property values inside that lock instead of one acquisition per
//!   property.

use engine_backend::scene::ComponentInstance;
use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyMetadata, REGISTRY, RUNTIME_TYPE_REGISTRY};
use std::any::Any;
use std::sync::Arc;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use crate::level_editor::core::commands::{execute_command, SceneCommand};
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

/// Pull a card's full value snapshot fresh from the live sources: one
/// batched `World` read when the entity/component is hydrated, otherwise
/// per-property fallback reads (live miss → JSON → default instance). This
/// is the only place a clean render still touches `World` — every other
/// render serves from [`ObjectTypeFieldsSection::world_value_cache`].
fn read_card_values_fresh(
    scene_db: &crate::level_editor::scene_database::SceneDatabase,
    object_id: &crate::level_editor::scene_database::ObjectId,
    class_name: &str,
    properties: &[PropertyMetadata],
    component: &ComponentInstance,
    default_instance: &dyn pulsar_reflection::EngineClass,
) -> Vec<Box<dyn Any>> {
    let batch = scene_db.with_world_component(object_id, class_name, |instance| {
        properties
            .iter()
            .map(|prop| (prop.getter)(instance))
            .collect::<Vec<_>>()
    });
    match batch {
        Some(values) => values,
        None => properties
            .iter()
            .map(|prop| {
                read_property_from_world(scene_db, object_id, class_name, prop, component, default_instance)
            })
            .collect(),
    }
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
    /// ## Performance (Pulsar-Native#575: subscribe, don't poll)
    ///
    /// Property VALUES come from the per-card snapshot cache (the section's
    /// `world_value_cache`) and are only re-pulled from `World` when a signal
    /// actually fired for that specific card -- its own World subscription
    /// event, a legacy JSON-path write, or a structural/epoch invalidation.
    /// An unrelated scene change (a gizmo drag anywhere, any other revision
    /// bump) re-renders this panel from cached values with zero `World`
    /// traffic. A fresh pull still happens under ONE `store.read()`
    /// acquisition per card (`with_world_component`).
    pub(super) fn render_component_sections(
        &mut self,
        attached: &[ComponentInstance],
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

                // ── Cache-until-signaled value fetch (Pulsar-Native#575) ──
                //
                // Clean card + cached snapshot: lend the cached values to the
                // row builder below, zero `World` traffic. Dirty or never-
                // pulled card: arm its World subscription (once per mounted
                // card) and pull fresh values.
                let card_dirty = self.dirty_classes.remove(class_name.as_str());
                let mut values = if card_dirty {
                    None
                } else {
                    // Take out of the cache rather than borrow: the row loop
                    // below needs `&mut self` for widget state, and the vec
                    // goes straight back in afterwards.
                    self.world_value_cache.remove(class_name.as_str())
                };
                if values.is_none() {
                    // Arm once per mounted card. Classes with no World
                    // registration are remembered as permanently
                    // un-subscribable (they ride the legacy JSON fallback
                    // forever); a live-entity miss is NOT remembered -- the
                    // entity may hydrate any moment, so retry next render.
                    if !self.world_subs.contains_key(class_name.as_str())
                        && !self.unsubscribable_classes.contains(class_name.as_str())
                    {
                        if pulsar_world_registry::component_id_for_class(class_name).is_none() {
                            self.unsubscribable_classes.insert(class_name.clone());
                        } else if let Some(sub) =
                            scene_db.subscribe_component(&object_id, class_name)
                        {
                            self.world_subs.insert(class_name.clone(), sub);
                        }
                    }
                    values = Some(read_card_values_fresh(
                        &scene_db,
                        &object_id,
                        class_name,
                        &properties,
                        component,
                        default_inst,
                    ));
                }
                let values = values.expect("fresh pull or cache hit fills this");

                let mut row_data: Vec<(
                    AnyElement,
                    Option<String>,
                    Option<String>,
                    bool,
                    Option<usize>,
                )> = Vec::new();

                // One unified row loop: `values` is the card's live snapshot
                // (fresh pull or cache hit -- indistinguishable from here
                // on). Borrowed, not consumed; it goes back into the cache
                // right after this loop.
                for (prop, value) in properties.iter().zip(values.iter()) {
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

                // Hand the snapshot back -- next render lends it out again
                // unless a signal marked this card dirty.
                self.world_value_cache.insert(class_name.clone(), values);

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
