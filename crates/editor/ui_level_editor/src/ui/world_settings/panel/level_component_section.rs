//! World Settings' level component sections: one per registered
//! [`crate::extend::LevelComponentSection`], a component the level holds at
//! most once (its sky, #1057; its global wind, #1123).
//!
//! With one, the section edits that component's properties in place,
//! through the same reflected property rows and `SetComponentProperty`
//! command as the properties panel (so every edit is undoable). Without
//! one, the section is the registration's note and a button that creates
//! it.

use std::any::Any;
use std::sync::Arc;

use gpui::{prelude::*, *};
use pulsar_reflection::{PropertyMetadata, REGISTRY};
use ui::{
    button::{Button, ButtonVariants as _},
    v_flex, ActiveTheme, Sizable,
};

use super::WorldSettingsPanelImpl;
use crate::commands::{execute_command, SceneCommand};
use crate::extend::{level_component, LevelComponent, LevelComponentSection};
use crate::WorldSettingsPanel;

/// A component class's reflected properties, read once.
pub(super) struct ClassProperties {
    properties: Vec<PropertyMetadata>,
    /// The default instance the metadata was read from, kept alive with it.
    _default_instance: Box<dyn pulsar_reflection::EngineClass>,
}

impl ClassProperties {
    pub(super) fn load(class_name: &str) -> Option<Arc<Self>> {
        let instance = REGISTRY.create_instance(class_name)?;
        Some(Arc::new(Self {
            properties: instance.get_properties(),
            _default_instance: instance,
        }))
    }
}

/// The property rows of `object_id`'s `class_name` component at
/// `component_index`, editing it in place through the undoable
/// `SetComponentProperty` command. `None` when the component is gone.
#[allow(clippy::too_many_arguments)]
pub(super) fn component_property_rows(
    state: &Arc<parking_lot::RwLock<crate::state::LevelEditorState>>,
    property_state: &mut ui_common::PropertyStateManager,
    class_name: &'static str,
    properties: &ClassProperties,
    object_id: &str,
    component_index: usize,
    window: &mut Window,
    cx: &mut Context<WorldSettingsPanel>,
) -> Option<Vec<AnyElement>> {
    let values: Vec<Box<dyn Any>> = {
        let state = state.read();
        let world = state.scene.world();
        crate::scene_edit::components::with_world_component(
            &world,
            object_id,
            class_name,
            component_index,
            |instance| {
                properties
                    .properties
                    .iter()
                    .map(|prop| (prop.getter)(instance))
                    .collect()
            },
        )
    }?;

    // Per instance, so a different one never inherits widget state.
    let editor_key = format!("{class_name}#{object_id}#{component_index}");
    let mut rows = Vec::with_capacity(values.len());
    for (prop, value) in properties.properties.iter().zip(values.iter()) {
        let write_back = {
            let state = state.clone();
            let object_id = object_id.to_string();
            let prop_name = prop.name.to_string();
            Arc::new(
                move |value: Box<dyn Any + Send>, _window: &mut Window, _cx: &mut App| {
                    execute_command(
                        &mut state.write(),
                        SceneCommand::SetComponentProperty {
                            id: object_id.clone(),
                            class_name: class_name.to_string(),
                            component_index,
                            prop_name: prop_name.clone(),
                            value,
                        },
                    );
                },
            )
        };
        rows.push(ui_common::render_property_row_runtime(
            property_state,
            "world",
            &editor_key,
            class_name,
            &prop.display_name,
            prop.name,
            prop.type_info,
            value.as_ref(),
            write_back,
            window,
            cx,
        ));
    }
    Some(rows)
}

/// A level component section's widget state.
#[derive(Default)]
pub(super) struct SectionState {
    property_state: ui_common::PropertyStateManager,
    properties: Option<Arc<ClassProperties>>,
    /// Why the last create did nothing, if it did nothing.
    message: Option<String>,
}

impl WorldSettingsPanelImpl {
    pub(super) fn render_level_component_section(
        &mut self,
        section: &'static LevelComponentSection,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let found = level_component(&self.state.read().scene.world(), section.find);
        match found {
            None => self.render_missing_level_component(section, cx),
            Some(found) => self.render_level_component(section, found, window, cx),
        }
    }

    fn render_missing_level_component(
        &mut self,
        section: &'static LevelComponentSection,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let state = self.state.clone();
        let message = self
            .level_components
            .get(section.class_name)
            .and_then(|section| section.message.clone());
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(section.missing_note),
            )
            .child(
                Button::new(SharedString::from(format!(
                    "world-create-{}",
                    section.class_name
                )))
                .label(section.create_label)
                .small()
                .primary()
                .on_click(cx.listener(move |panel, _event, _window, cx| {
                    let result = (section.create)(&mut state.write());
                    panel
                        .world_settings
                        .level_components
                        .entry(section.class_name)
                        .or_default()
                        .message = (!result.changed).then(|| result.no_op_reason.to_string());
                    cx.notify();
                })),
            )
            .children(message.map(|message| {
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(message)
            }))
            .into_any_element()
    }

    fn render_level_component(
        &mut self,
        section: &'static LevelComponentSection,
        found: LevelComponent,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let entry = self.level_components.entry(section.class_name).or_default();
        let properties = match &entry.properties {
            Some(properties) => Arc::clone(properties),
            None => match ClassProperties::load(section.class_name) {
                Some(properties) => {
                    entry.properties = Some(Arc::clone(&properties));
                    properties
                }
                None => {
                    return div()
                        .text_xs()
                        .child(format!(
                            "{} is not registered in this build.",
                            section.class_name
                        ))
                        .into_any_element()
                }
            },
        };
        let Some(rows) = component_property_rows(
            &self.state,
            &mut entry.property_state,
            section.class_name,
            &properties,
            &found.object_id,
            found.component_index,
            window,
            cx,
        ) else {
            return div().into_any_element();
        };
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child((section.describe)(&found.object_name, found.enabled)),
            )
            .children(rows)
            .into_any_element()
    }
}
