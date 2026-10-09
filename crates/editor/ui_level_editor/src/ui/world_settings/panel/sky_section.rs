//! World Settings' Sky section (#1057): the level's one sky, its
//! `AtmosphereComponent`.
//!
//! With a sky, the section edits that component's properties in place,
//! through the same reflected property rows and `SetComponentProperty`
//! command as the properties panel (so every edit is undoable). Without
//! one, the whole section is a note that the sky renders black and a
//! button that creates it ([`crate::scene_edit::sky::create_sky`]).
//!
//! The Wind section ([`super::wind_section`]) edits the level's one global
//! wind the same way, through [`component_property_rows`].

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
use crate::scene_edit::sky::{self, LevelSky};
use crate::WorldSettingsPanel;

const ATMOSPHERE_CLASS: &str = engine_backend::scene::level_rules::ATMOSPHERE_CLASS;

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

impl WorldSettingsPanelImpl {
    pub(super) fn render_sky_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let found = sky::level_sky(&self.state.read().scene.world());
        match found {
            None => self.render_missing_sky(cx),
            Some(found) => self.render_sky(found, window, cx),
        }
    }

    fn render_missing_sky(&mut self, cx: &mut Context<WorldSettingsPanel>) -> AnyElement {
        let state = self.state.clone();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("This level has no sky, so it renders black."),
            )
            .child(
                Button::new("world-create-sky")
                    .label("Create Sky")
                    .small()
                    .primary()
                    .on_click(cx.listener(move |panel, _event, _window, cx| {
                        let result = sky::create_sky(&mut state.write());
                        panel.world_settings.sky_message =
                            (!result.changed).then(|| result.no_op_reason.to_string());
                        cx.notify();
                    })),
            )
            .children(self.sky_message.clone().map(|message| {
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(message)
            }))
            .into_any_element()
    }

    fn render_sky(
        &mut self,
        found: LevelSky,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let properties = match &self.atmosphere_properties {
            Some(properties) => Arc::clone(properties),
            None => match ClassProperties::load(ATMOSPHERE_CLASS) {
                Some(properties) => {
                    self.atmosphere_properties = Some(Arc::clone(&properties));
                    properties
                }
                None => {
                    return div()
                        .text_xs()
                        .child("AtmosphereComponent is not registered in this build.")
                        .into_any_element()
                }
            },
        };
        let Some(rows) = component_property_rows(
            &self.state,
            &mut self.sky_property_state,
            ATMOSPHERE_CLASS,
            &properties,
            &found.object_id,
            found.component_index,
            window,
            cx,
        ) else {
            return div().into_any_element();
        };

        let note = if found.enabled {
            format!("The sky is {}'s AtmosphereComponent.", found.object_name)
        } else {
            format!(
                "{}'s AtmosphereComponent is disabled, so the sky renders black.",
                found.object_name
            )
        };
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(note),
            )
            .children(rows)
            .into_any_element()
    }
}
