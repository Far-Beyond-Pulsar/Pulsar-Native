//! World Settings' Sky section (#1057): the level's one sky, its
//! `AtmosphereComponent`.
//!
//! With a sky, the section edits that component's properties in place,
//! through the same reflected property rows and `SetComponentProperty`
//! command as the properties panel (so every edit is undoable). Without
//! one, the whole section is a note that the sky renders black and a
//! button that creates it ([`crate::scene_edit::sky::create_sky`]).

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

/// The atmosphere class's reflected properties, read once.
pub(super) struct AtmosphereProperties {
    properties: Vec<PropertyMetadata>,
    /// The default instance the metadata was read from, kept alive with it.
    _default_instance: Box<dyn pulsar_reflection::EngineClass>,
}

impl AtmosphereProperties {
    pub(super) fn load() -> Option<Arc<Self>> {
        let instance = REGISTRY.create_instance(ATMOSPHERE_CLASS)?;
        Some(Arc::new(Self {
            properties: instance.get_properties(),
            _default_instance: instance,
        }))
    }
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
            None => match AtmosphereProperties::load() {
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
        let values: Option<Vec<Box<dyn Any>>> = {
            let state = self.state.read();
            let world = state.scene.world();
            crate::scene_edit::components::with_world_component(
                &world,
                &found.object_id,
                ATMOSPHERE_CLASS,
                found.component_index,
                |instance| {
                    properties
                        .properties
                        .iter()
                        .map(|prop| (prop.getter)(instance))
                        .collect()
                },
            )
        };
        let Some(values) = values else {
            return div().into_any_element();
        };

        // Per sky instance, so a different sky never inherits widget state.
        let editor_key = format!(
            "{ATMOSPHERE_CLASS}#{}#{}",
            found.object_id, found.component_index
        );
        let mut rows = Vec::with_capacity(values.len());
        for (prop, value) in properties.properties.iter().zip(values.iter()) {
            let write_back = {
                let state = self.state.clone();
                let object_id = found.object_id.clone();
                let component_index = found.component_index;
                let prop_name = prop.name.to_string();
                Arc::new(
                    move |value: Box<dyn Any + Send>, _window: &mut Window, _cx: &mut App| {
                        execute_command(
                            &mut state.write(),
                            SceneCommand::SetComponentProperty {
                                id: object_id.clone(),
                                class_name: ATMOSPHERE_CLASS.to_string(),
                                component_index,
                                prop_name: prop_name.clone(),
                                value,
                            },
                        );
                    },
                )
            };
            rows.push(ui_common::render_property_row_runtime(
                &mut self.sky_property_state,
                "world",
                &editor_key,
                ATMOSPHERE_CLASS,
                &prop.display_name,
                prop.name,
                prop.type_info,
                value.as_ref(),
                write_back,
                window,
                cx,
            ));
        }

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
