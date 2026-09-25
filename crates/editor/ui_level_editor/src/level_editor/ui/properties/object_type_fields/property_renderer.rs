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
use pulsar_scenedb::World;
use std::any::Any;
use std::sync::Arc;
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use super::category_section::group_rows_by_category;
use super::{ObjectTypeFieldsSection, PropertyMetadataCacheEntry};
use crate::level_editor::core::commands::{execute_command, SceneCommand};

/// Read a property value from the live World, with JSON and default-instance
/// fallbacks.  Used only when the batch read (via `with_world_component`)
/// fails — e.g. the entity doesn't exist in the World.
fn read_property_from_world(
    world: &World,
    object_id: &crate::level_editor::scene_edit::ObjectId,
    class_name: &str,
    prop: &PropertyMetadata,
    component: &ComponentInstance,
    default_instance: &dyn pulsar_reflection::EngineClass,
) -> Box<dyn Any> {
    crate::level_editor::scene_edit::components::read_live_component_property(
        world,
        object_id,
        class_name,
        prop.name,
    )
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

/// Pull a card's value snapshot from its OWN metadata JSON blob (falling
/// back to the default instance for absent/null fields) -- the source of
/// truth for every NON-live-typed instance card. A duplicate instance has no
/// `World` presence (`World` holds one typed value per `(entity, type)`), so
/// reading "the" live value here would silently show another instance's
/// fields -- exactly Pulsar-Native#519's bug, on the read side.
fn read_card_values_from_metadata(
    component: &ComponentInstance,
    properties: &[PropertyMetadata],
    default_instance: &dyn pulsar_reflection::EngineClass,
) -> Vec<Box<dyn Any>> {
    properties
        .iter()
        .map(|prop| {
            component
                .data
                .get(prop.name)
                .filter(|json| !json.is_null())
                .and_then(|json| {
                    RUNTIME_TYPE_REGISTRY
                        .deserialize_json_for_type(prop.type_info, json.clone())
                        .ok()
                })
                .unwrap_or_else(|| (prop.getter)(default_instance))
        })
        .collect()
}

/// Pull a card's full value snapshot fresh from the live sources: one
/// batched `World` read when the entity/component is hydrated, otherwise
/// per-property fallback reads (live miss → JSON → default instance). This
/// is the only place a clean render still touches `World` — every other
/// render serves from [`ObjectTypeFieldsSection::world_value_cache`].
fn read_card_values_fresh(
    world: &World,
    object_id: &crate::level_editor::scene_edit::ObjectId,
    class_name: &str,
    properties: &[PropertyMetadata],
    component: &ComponentInstance,
    default_instance: &dyn pulsar_reflection::EngineClass,
) -> Vec<Box<dyn Any>> {
    let batch = crate::level_editor::scene_edit::components::with_world_component(
        world,
        object_id,
        class_name,
        |instance| {
            properties
                .iter()
                .map(|prop| (prop.getter)(instance))
                .collect::<Vec<_>>()
        },
    );
    match batch {
        Some(values) => values,
        None => properties
            .iter()
            .map(|prop| {
                read_property_from_world(
                    world,
                    object_id,
                    class_name,
                    prop,
                    component,
                    default_instance,
                )
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
                if class_name == pulsar_class::CLASS_INSTANCE {
                    return self.render_class_card(window, cx);
                }

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

                // ── Per-instance identity (Pulsar-Native#519) ──────────────
                //
                // An object can carry several instances of the SAME class;
                // `idx` (the component-list index -- the same identity
                // remove/enable/reorder already use) is what makes each card
                // its own value store, its own editor state, and its own
                // subscription, instead of every duplicate collapsing onto
                // the first.
                let card_key = (class_name.clone(), idx);
                let editor_key = format!("{class_name}#{idx}");

                // Which instance of this class is the live-typed one? Only
                // that card reads (and subscribes to) `World`; every OTHER
                // instance reads and writes its own metadata JSON blob --
                // `World` physically holds one typed value per
                // `(entity, ComponentId)`, so duplicates cannot share it.
                let live_idx = {
                    let world = scene_db.read();
                    crate::level_editor::scene_edit::components::live_typed_component_index(
                        &world.world,
                        &object_id,
                        class_name,
                    )
                };

                // ── Cache-until-signaled value fetch (Pulsar-Native#575) ──
                //
                // Clean card + cached snapshot: lend the cached values to the
                // row builder below, zero `World` traffic. Dirty or never-
                // pulled card: arm its World subscription (live cards, once
                // per mounted card) and pull fresh values from whichever
                // source backs THIS instance.
                let card_dirty = self.dirty_classes.remove(&card_key);
                let mut values = if card_dirty {
                    None
                } else {
                    // Take out of the cache rather than borrow: the row loop
                    // below needs `&mut self` for widget state, and the vec
                    // goes straight back in afterwards.
                    self.world_value_cache.remove(&card_key)
                };
                if values.is_none() {
                    if live_idx == Some(idx)
                        && !self.world_subs.contains_key(&card_key)
                        && !self.unsubscribable_classes.contains(class_name.as_str())
                    {
                        if pulsar_world_registry::component_id_for_class(class_name).is_none() {
                            self.unsubscribable_classes.insert(class_name.clone());
                        } else if let Some(sub) = {
                            let mut world = scene_db.write();
                            crate::level_editor::scene_edit::components::subscribe_component(
                                &mut world.world,
                                &object_id,
                                class_name,
                            )
                        } {
                            self.world_subs.insert(card_key.clone(), sub);
                        }
                    }
                    values = Some(if live_idx == Some(idx) {
                        read_card_values_fresh(
                            &scene_db.read().world,
                            &object_id,
                            class_name,
                            &properties,
                            component,
                            default_inst,
                        )
                    } else {
                        read_card_values_from_metadata(component, &properties, default_inst)
                    });
                }
                let values = values.expect("fresh pull or cache hit fills this");

                // Components built from a class slot: each property's class
                // default, for the override markers and revert (#921).
                let slot_default_values = self.slot_default_values_for(idx, &properties);

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
                for (prop_index, (prop, value)) in properties.iter().zip(values.iter()).enumerate() {
                    let write_back = {
                        let state_arc = self.state_arc.clone();
                        let oid = object_id.clone();
                        let cls = class_name.clone();
                        let ci = idx;
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
                                        component_index: ci,
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
                        // Per-INSTANCE editor identity (Pulsar-Native#519):
                        // two cards of one class must never share widget
                        // state, or every keystroke would land in whichever
                        // card rendered first.
                        &editor_key,
                        class_name,
                        &prop.display_name,
                        prop.name,
                        prop.type_info,
                        value.as_ref(),
                        write_back,
                        window,
                        cx,
                    );

                    let row = match slot_default_values
                        .as_ref()
                        .and_then(|defaults| defaults.get(prop_index))
                        .and_then(|d| d.as_ref())
                    {
                        Some(default) => {
                            let overridden = !crate::level_editor::scene_edit::classes::property_equals_default(
                                value.as_ref(),
                                default.as_ref(),
                            );
                            let revert = {
                                let state_arc = self.state_arc.clone();
                                let oid = object_id.clone();
                                let cls = class_name.clone();
                                let pn = prop.name.to_string();
                                Arc::new(move |_window: &mut Window, _cx: &mut App| {
                                    execute_command(
                                        &mut state_arc.write(),
                                        SceneCommand::RevertComponentProperty {
                                            id: oid.clone(),
                                            class_name: cls.clone(),
                                            component_index: idx,
                                            prop_name: pn.clone(),
                                        },
                                    );
                                })
                            };
                            ui_common::decorate_property_override(
                                row,
                                SharedString::from(format!("revert-{editor_key}-{}", prop.name)),
                                &ui_common::PropertyOverride { overridden, on_revert: Some(revert) },
                                cx,
                            )
                        }
                        None => row,
                    };

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
                self.world_value_cache.insert(card_key, values);

                let (mut uncategorized, categorized) = group_rows_by_category(row_data);
                let category_elements = self.render_categorized_rows(class_name, categorized, cx);
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

impl ObjectTypeFieldsSection {
    /// Each property's class default for card `idx`, when that component was
    /// built from a class slot. Read once per class-cache refresh.
    fn slot_default_values_for(
        &mut self,
        idx: usize,
        properties: &[PropertyMetadata],
    ) -> Option<Arc<Vec<Option<Box<dyn Any>>>>> {
        if let Some(values) = self.slot_default_values.get(&idx) {
            return Some(Arc::clone(values));
        }
        let default = self.slot_defaults.get(&idx)?;
        let instance = default.instance.as_deref()?;
        let getters = instance.get_properties();
        let values: Vec<Option<Box<dyn Any>>> = properties
            .iter()
            .map(|prop| {
                getters
                    .iter()
                    .find(|g| g.name == prop.name)
                    .map(|g| (g.getter)(instance))
            })
            .collect();
        let values = Arc::new(values);
        self.slot_default_values.insert(idx, Arc::clone(&values));
        Some(values)
    }

    /// The card of a placed class instance's `ClassInstance`: the class it
    /// references and its script variables, edited through the same
    /// reflected property rows as components. Values that differ from the
    /// class default are marked and can be reverted; both edits and reverts
    /// are undoable `SetClassVariable` commands.
    fn render_class_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::level_editor::scene_edit::classes::ClassVariableView;
        use pulsar_class::VariableKind;

        let view = self.class_view.clone()?;
        let object_id = self.object_id.clone();
        let mut rows: Vec<AnyElement> = Vec::new();

        if !view.resolved {
            rows.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(format!(
                        "Class '{}' is missing from this project; its data is kept.",
                        view.class_name
                    ))
                    .into_any_element(),
            );
        }

        for var in &view.variables {
            let ClassVariableView { name, kind, value, overridden, .. } = var;
            // The reflected type and a typed value for the row's editor.
            let typed: Option<(&'static pulsar_reflection::RuntimeTypeInfo, Box<dyn Any>)> = match kind {
                VariableKind::Bool => RUNTIME_TYPE_REGISTRY
                    .get::<bool>()
                    .map(|t| (t, Box::new(value.as_bool().unwrap_or(false)) as Box<dyn Any>)),
                VariableKind::Int => RUNTIME_TYPE_REGISTRY
                    .get::<i64>()
                    .map(|t| (t, Box::new(value.as_i64().unwrap_or(0)) as Box<dyn Any>))
                    .or_else(|| {
                        RUNTIME_TYPE_REGISTRY
                            .get::<i32>()
                            .map(|t| (t, Box::new(value.as_i64().unwrap_or(0) as i32) as Box<dyn Any>))
                    }),
                VariableKind::Float => RUNTIME_TYPE_REGISTRY
                    .get::<f64>()
                    .map(|t| (t, Box::new(value.as_f64().unwrap_or(0.0)) as Box<dyn Any>))
                    .or_else(|| {
                        RUNTIME_TYPE_REGISTRY
                            .get::<f32>()
                            .map(|t| (t, Box::new(value.as_f64().unwrap_or(0.0) as f32) as Box<dyn Any>))
                    }),
                VariableKind::String => RUNTIME_TYPE_REGISTRY.get::<String>().map(|t| {
                    (t, Box::new(value.as_str().unwrap_or_default().to_string()) as Box<dyn Any>)
                }),
                VariableKind::Other(_) => None,
            };
            let row = match typed {
                Some((type_info, current)) => {
                    let write_back = {
                        let state_arc = self.state_arc.clone();
                        let oid = object_id.clone();
                        let var_name = name.clone();
                        Arc::new(move |new_val: Box<dyn Any + Send>, _window: &mut Window, _cx: &mut App| {
                            let Ok(json) = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(new_val.as_ref()) else {
                                return;
                            };
                            execute_command(
                                &mut state_arc.write(),
                                SceneCommand::SetClassVariable {
                                    id: oid.clone(),
                                    name: var_name.clone(),
                                    value: Some(json),
                                },
                            );
                        })
                    };
                    ui_common::render_property_row_runtime(
                        &mut self.property_state,
                        "level",
                        "ClassInstance#variables",
                        pulsar_class::CLASS_INSTANCE,
                        name,
                        name,
                        type_info,
                        current.as_ref(),
                        write_back,
                        window,
                        cx,
                    )
                }
                None => h_flex()
                    .w_full()
                    .justify_between()
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child(name.clone()))
                    .child(div().text_sm().child(value.to_string()))
                    .into_any_element(),
            };
            let revert = {
                let state_arc = self.state_arc.clone();
                let oid = object_id.clone();
                let var_name = name.clone();
                Arc::new(move |_window: &mut Window, _cx: &mut App| {
                    execute_command(
                        &mut state_arc.write(),
                        SceneCommand::SetClassVariable { id: oid.clone(), name: var_name.clone(), value: None },
                    );
                })
            };
            rows.push(ui_common::decorate_property_override(
                row,
                SharedString::from(format!("revert-class-var-{name}")),
                &ui_common::PropertyOverride { overridden: *overridden, on_revert: Some(revert) },
                cx,
            ));
        }

        if view.variables.is_empty() && view.resolved {
            rows.push(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("This class has no script variables.")
                    .into_any_element(),
            );
        }

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
                        .child(Icon::new(IconName::Code).small())
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(format!("Class: {}", view.class_name)),
                        ),
                )
                .children(rows)
                .into_any_element(),
        )
    }
}
