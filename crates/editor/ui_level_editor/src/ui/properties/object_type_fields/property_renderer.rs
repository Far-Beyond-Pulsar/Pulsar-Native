//! Per-component property rendering for the virtual list.
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
//!   4. Builds the card/category/property items lazily: `rebuild_layout` describes
//!      the cards (metadata only) and `ui_common::inspector_model::flatten`
//!      applies search and collapse; `gpui::list` renders only visible items.
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
use ui::{button::ButtonVariants as _, h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable};

use ui_common::inspector_model::{CardDesc, PropDesc};
use super::{CardMeta, ObjectTypeFieldsSection, PropertyMetadataCacheEntry};
use crate::core::commands::{execute_command, SceneCommand};

/// A card's values for an unresolved instance (data this build could not
/// turn into a live value), read from the instance's own payload in the
/// world: decoded whole when the class accepts it now, otherwise field by
/// field (top level, or one `#[sub_props]` group down), with the class
/// default for a field the payload lacks or cannot supply.
fn read_unresolved_card_values(
    world: &World,
    object_id: &crate::scene_edit::ObjectId,
    class_name: &str,
    index: usize,
    properties: &[PropertyMetadata],
    default_instance: &dyn pulsar_reflection::EngineClass,
) -> Vec<Box<dyn Any>> {
    let payload = crate::scene_edit::components::unresolved_payload(world, object_id, index)
        .unwrap_or(serde_json::Value::Null);
    if let Some(Ok(decoded)) = REGISTRY.create_instance_from_json(class_name, &payload) {
        return properties
            .iter()
            .map(|prop| (prop.getter)(decoded.as_ref()))
            .collect();
    }
    let field = |name: &str| {
        let map = payload.as_object()?;
        map.get(name).or_else(|| {
            map.values()
                .filter_map(serde_json::Value::as_object)
                .find_map(|group| group.get(name))
        })
    };
    properties
        .iter()
        .map(|prop| {
            field(prop.name)
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

/// Pull a live card's full value snapshot fresh from the world: one batched
/// read of the instance's typed value (the default instance's values if it
/// vanished meanwhile). This is the only place a clean render still touches
/// `World` -- every other render serves from
/// [`ObjectTypeFieldsSection::world_value_cache`].
fn read_card_values_fresh(
    world: &World,
    object_id: &crate::scene_edit::ObjectId,
    class_name: &str,
    index: usize,
    properties: &[PropertyMetadata],
    default_instance: &dyn pulsar_reflection::EngineClass,
) -> Vec<Box<dyn Any>> {
    crate::scene_edit::components::with_world_component(
        world,
        object_id,
        class_name,
        index,
        |instance| {
            properties
                .iter()
                .map(|prop| (prop.getter)(instance))
                .collect::<Vec<_>>()
        },
    )
    .unwrap_or_else(|| {
        properties
            .iter()
            .map(|prop| (prop.getter)(default_instance))
            .collect()
    })
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

    /// Describe the cards to the shared inspector.
    ///
    /// Runs only when something that shapes the list changed (the component
    /// set, the search query, a collapse toggle): it builds metadata, never
    /// elements.  The list is told which range of items changed so the scroll
    /// position survives a collapse or a keystroke in the search box.
    pub(super) fn rebuild_layout(&mut self, attached: &[ComponentInstance]) {
        let mut cards: Vec<CardDesc> = Vec::new();
        self.card_meta.clear();
        for (idx, component) in attached.iter().enumerate() {
            let class_name = &component.class_name;
            if class_name == pulsar_class::CLASS_INSTANCE {
                if let Some(view) = &self.class_view {
                    cards.push(CardDesc {
                        key: format!("{class_name}#{idx}"),
                        title: format!("Class: {}", view.class_name),
                        props: Vec::new(),
                        custom: true,
                        note: None,
                    });
                    self.card_meta.push(CardMeta {
                        class_name: class_name.clone(),
                        idx,
                    });
                }
                continue;
            }
            if self.ensure_metadata_cached(class_name).is_none() {
                continue;
            }
            let Some(entry) = self.property_metadata_cache.get(class_name.as_str()) else {
                continue;
            };
            cards.push(CardDesc {
                key: format!("{class_name}#{idx}"),
                title: class_name.clone(),
                props: entry
                    .properties
                    .iter()
                    .map(|p| PropDesc {
                        name: p.name.to_string(),
                        display_name: p.display_name.to_string(),
                        category: p.category.map(str::to_string),
                        category_color: p.category_color.map(str::to_string),
                        category_default_collapsed: p.category_default_collapsed,
                        category_order: p.category_order,
                        extra_rows: 0,
                    })
                    .collect(),
                custom: false,
                note: pulsar_world_registry::unfinished_component(class_name).map(|u| {
                    format!("Unfinished: {}. Tracked in {}.", u.reason, u.issue)
                }),
            });
            self.card_meta.push(CardMeta {
                class_name: class_name.clone(),
                idx,
            });
        }

        self.inspector.set_cards(cards);
        self.layout_dirty = false;
    }

    /// Make sure `card_key`'s value snapshot is present and current, binding the
    /// card to its instance entity the first time it is pulled.
    fn ensure_card_values(
        &mut self,
        class_name: &str,
        idx: usize,
        properties: &[PropertyMetadata],
    ) -> Option<()> {
        let card_key = (class_name.to_string(), idx);
        let dirty = self.dirty_classes.remove(&card_key);
        if !dirty && self.world_value_cache.contains_key(&card_key) {
            return Some(());
        }
        let default_inst: &dyn pulsar_reflection::EngineClass = &*self
            .property_metadata_cache
            .get(class_name)?
            ._default_instance;
        let scene = self.scene_db.read();
        let world = &scene.world;
        let object_id = &self.object_id;
        let live =
            crate::scene_edit::components::is_live_instance(world, object_id, class_name, idx);
        // Bind the card to its instance entity, so the values the object's
        // subscription delivers land on it.
        if live
            && !self.unsubscribable_classes.contains(class_name)
            && !self.card_entities.values().any(|card| card == &card_key)
        {
            if pulsar_world_registry::component_id_for_class(class_name).is_none() {
                self.unsubscribable_classes.insert(class_name.to_string());
            } else if let Some(instance) =
                crate::scene_edit::components::instance_at(world, object_id, idx)
            {
                self.card_entities.insert(instance, card_key.clone());
            }
        }
        let values = if live {
            read_card_values_fresh(world, object_id, class_name, idx, properties, default_inst)
        } else {
            read_unresolved_card_values(
                world,
                object_id,
                class_name,
                idx,
                properties,
                default_inst,
            )
        };
        drop(scene);
        // Array / struct editors skip re-reading unchanged values; this one
        // was just pulled fresh, so they must look again.
        self.property_state
            .mark_stale(&format!("{class_name}#{idx}"));
        self.world_value_cache.insert(card_key, values);
        Some(())
    }

    /// One property row.
    ///
    /// Values come from the per-card snapshot cache (Pulsar-Native#575) and are
    /// re-pulled from `World` only when a signal fired for that card; the
    /// editor behind the row is cached per `(card, property)`, so scrolling a
    /// row out and back keeps its focus and caret state.
    pub(super) fn render_property_item(
        &mut self,
        card_ix: usize,
        prop_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let empty = || div().into_any_element();
        let Some(meta) = self.card_meta.get(card_ix) else {
            return empty();
        };
        let class_name = meta.class_name.clone();
        let idx = meta.idx;
        let Some(properties) = self
            .property_metadata_cache
            .get(class_name.as_str())
            .map(|c| Arc::clone(&c.properties))
        else {
            return empty();
        };
        let Some(prop) = properties.get(prop_ix) else {
            return empty();
        };
        if self.ensure_card_values(&class_name, idx, &properties).is_none() {
            return empty();
        }
        let card_key = (class_name.clone(), idx);
        // Components built from a class slot: each property's class default,
        // for the override markers and revert (#921).
        let slot_default_values = self.slot_default_values_for(idx, &properties);
        let object_id = self.object_id.clone();
        let editor_key = format!("{class_name}#{idx}");

        let write_back = {
            let state_arc = self.state_arc.clone();
            let oid = object_id.clone();
            let cls = class_name.clone();
            let pn = prop.name.to_string();
            Arc::new(
                move |new_val: Box<dyn Any + Send>, _window: &mut Window, _cx: &mut App| {
                    execute_command(
                        &mut state_arc.write(),
                        SceneCommand::SetComponentProperty {
                            id: oid.clone(),
                            class_name: cls.clone(),
                            component_index: idx,
                            prop_name: pn.clone(),
                            value: new_val,
                        },
                    );
                },
            )
        };

        let Some(value) = self
            .world_value_cache
            .get(&card_key)
            .and_then(|values| values.get(prop_ix))
        else {
            return empty();
        };
        let row = ui_common::render_property_row_runtime(
            &mut self.property_state,
            "level",
            // Per-INSTANCE editor identity (Pulsar-Native#519): two cards of
            // one class must never share widget state.
            &editor_key,
            &class_name,
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
            .and_then(|defaults| defaults.get(prop_ix))
            .and_then(|d| d.as_ref())
        {
            Some(default) => {
                let overridden =
                    !crate::scene_edit::classes::property_equals_default(value.as_ref(), default.as_ref());
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
                    &ui_common::PropertyOverride {
                        overridden,
                        on_revert: Some(revert),
                    },
                    cx,
                )
            }
            None => row,
        };

        row
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
        let instance = default.instance()?;
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
    pub(super) fn render_class_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        use crate::scene_edit::classes::ClassVariableView;
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
            let ClassVariableView {
                name,
                kind,
                value,
                overridden,
                ..
            } = var;
            // The reflected type and a typed value for the row's editor.
            let typed: Option<(&'static pulsar_reflection::RuntimeTypeInfo, Box<dyn Any>)> =
                match kind {
                    VariableKind::Bool => RUNTIME_TYPE_REGISTRY.get::<bool>().map(|t| {
                        (
                            t,
                            Box::new(value.as_bool().unwrap_or(false)) as Box<dyn Any>,
                        )
                    }),
                    VariableKind::Int => RUNTIME_TYPE_REGISTRY
                        .get::<i64>()
                        .map(|t| (t, Box::new(value.as_i64().unwrap_or(0)) as Box<dyn Any>))
                        .or_else(|| {
                            RUNTIME_TYPE_REGISTRY.get::<i32>().map(|t| {
                                (
                                    t,
                                    Box::new(value.as_i64().unwrap_or(0) as i32) as Box<dyn Any>,
                                )
                            })
                        }),
                    VariableKind::Float => RUNTIME_TYPE_REGISTRY
                        .get::<f64>()
                        .map(|t| (t, Box::new(value.as_f64().unwrap_or(0.0)) as Box<dyn Any>))
                        .or_else(|| {
                            RUNTIME_TYPE_REGISTRY.get::<f32>().map(|t| {
                                (
                                    t,
                                    Box::new(value.as_f64().unwrap_or(0.0) as f32) as Box<dyn Any>,
                                )
                            })
                        }),
                    VariableKind::String => RUNTIME_TYPE_REGISTRY.get::<String>().map(|t| {
                        (
                            t,
                            Box::new(value.as_str().unwrap_or_default().to_string())
                                as Box<dyn Any>,
                        )
                    }),
                    VariableKind::Other(_) => None,
                };
            let row = match typed {
                Some((type_info, current)) => {
                    let write_back = {
                        let state_arc = self.state_arc.clone();
                        let oid = object_id.clone();
                        let var_name = name.clone();
                        Arc::new(
                            move |new_val: Box<dyn Any + Send>,
                                  _window: &mut Window,
                                  _cx: &mut App| {
                                let Ok(json) =
                                    RUNTIME_TYPE_REGISTRY.serialize_json_for_any(new_val.as_ref())
                                else {
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
                            },
                        )
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
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(name.clone()),
                    )
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
                        SceneCommand::SetClassVariable {
                            id: oid.clone(),
                            name: var_name.clone(),
                            value: None,
                        },
                    );
                })
            };
            rows.push(ui_common::decorate_property_override(
                row,
                SharedString::from(format!("revert-class-var-{name}")),
                &ui_common::PropertyOverride {
                    overridden: *overridden,
                    on_revert: Some(revert),
                },
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

        // Every component slot of the class, grouped by slot, including the
        // slots the class places on generated child objects -- whose
        // overrides were otherwise only visible with that child selected
        // (#932). Overridden properties get the same marker and revert.
        if !view.slots.is_empty() {
            rows.push(
                div()
                    .pt_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child("Component overrides")
                    .into_any_element(),
            );
        }
        let any_override = view.variables.iter().any(|var| var.overridden)
            || view
                .slots
                .iter()
                .any(|slot| slot.removed || !slot.overridden.is_empty());
        let child_names: std::collections::HashMap<String, String> = {
            let scene = self.scene_db.read();
            view.slots
                .iter()
                .filter_map(|slot| slot.object_id.as_ref())
                .filter(|id| **id != object_id)
                .filter_map(|id| {
                    crate::scene_edit::objects::get_object_name(&scene.world, id)
                        .map(|name| (id.clone(), name))
                })
                .collect()
        };
        for slot in &view.slots {
            rows.push(self.render_slot_overrides(slot, &object_id, &child_names, cx));
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
                                .flex_1()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(cx.theme().foreground)
                                .child(format!("Class: {}", view.class_name)),
                        )
                        .when(any_override, |el| {
                            let state_arc = self.state_arc.clone();
                            let oid = object_id.clone();
                            el.child(
                                ui::button::Button::new("reset-class-overrides")
                                    .label("Reset all")
                                    .xsmall()
                                    .ghost()
                                    .tooltip("Put every variable and component back to the class (one undo step)")
                                    .on_click(move |_, _, _| {
                                        execute_command(
                                            &mut state_arc.write(),
                                            SceneCommand::ResetClassOverrides { id: oid.clone() },
                                        );
                                    }),
                            )
                        }),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    /// One class slot on the instance root's card: its component class,
    /// which object carries it, and each overridden property with a revert
    /// (`RevertClassSlot` by dot path). A slot the instance removed can be
    /// restored whole.
    fn render_slot_overrides(
        &self,
        slot: &crate::scene_edit::classes::ClassSlotView,
        root_id: &str,
        child_names: &std::collections::HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let place = match slot.object_id.as_deref() {
            None => "removed".to_string(),
            Some(id) if id == root_id => "on this object".to_string(),
            Some(id) => format!(
                "on {}",
                child_names.get(id).map(String::as_str).unwrap_or(id)
            ),
        };
        let header = h_flex()
            .w_full()
            .gap_1()
            .items_center()
            .child(Icon::new(IconName::Component).xsmall().text_color(muted))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(slot.class_name.clone()),
            )
            .child(div().text_xs().text_color(muted).child(place));

        let revert = |path: Option<String>| {
            let state_arc = self.state_arc.clone();
            let root = root_id.to_string();
            let slot_id = slot.slot_id.clone();
            Arc::new(move |_window: &mut Window, _cx: &mut App| {
                execute_command(
                    &mut state_arc.write(),
                    SceneCommand::RevertClassSlot {
                        id: root.clone(),
                        slot_id: slot_id.clone(),
                        path: path.clone(),
                    },
                );
            }) as Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>
        };

        let mut body: Vec<AnyElement> = Vec::new();
        if slot.removed {
            body.push(ui_common::decorate_property_override(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Removed on this instance")
                    .into_any_element(),
                SharedString::from(format!("restore-slot-{}", slot.slot_id)),
                &ui_common::PropertyOverride {
                    overridden: true,
                    on_revert: Some(revert(None)),
                },
                cx,
            ));
        } else if slot.overridden.is_empty() {
            body.push(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Matches the class")
                    .into_any_element(),
            );
        } else {
            for prop in &slot.overridden {
                let row = h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child(prop.path.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .max_w(px(160.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(compact_json(&prop.value)),
                    )
                    .into_any_element();
                body.push(ui_common::decorate_property_override(
                    row,
                    SharedString::from(format!("revert-slot-{}-{}", slot.slot_id, prop.path)),
                    &ui_common::PropertyOverride {
                        overridden: true,
                        on_revert: Some(revert(Some(prop.path.clone()))),
                    },
                    cx,
                ));
            }
        }

        v_flex()
            .w_full()
            .gap_1()
            .pl_1()
            .child(header)
            .children(body)
            .into_any_element()
    }
}

/// A JSON value as one short line for an override row.
fn compact_json(value: &serde_json::Value) -> String {
    let text = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if text.chars().count() > 48 {
        format!("{}…", text.chars().take(47).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod slot_override_tests {
    use super::compact_json;
    use serde_json::json;

    #[test]
    fn values_are_shortened_for_one_line() {
        assert_eq!(compact_json(&json!("text")), "text");
        assert_eq!(compact_json(&json!([1.0, 0.5])), "[1.0,0.5]");
        let long = compact_json(&json!("x".repeat(100)));
        assert_eq!(long.chars().count(), 48);
        assert!(long.ends_with('…'));
    }
}
