//! World Settings' Wind section (#1123): the level's one global wind, its
//! `WindComponent`. Every foliage component sways in it, each type by its
//! own per-band response, unless a component opts out for its own wind.
//!
//! Like the Sky section: with a wind, the section edits that component's
//! properties in place (undoable); without one, it notes that foliage uses
//! each component's own wind and offers a button that creates the level's
//! wind ([`crate::scene_edit::wind::create_wind`]).

use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    v_flex, ActiveTheme, Sizable,
};

use super::sky_section::{component_property_rows, ClassProperties};
use super::WorldSettingsPanelImpl;
use crate::scene_edit::wind::{self, LevelWind};
use crate::WorldSettingsPanel;

const WIND_CLASS: &str = engine_backend::scene::level_rules::WIND_CLASS;

impl WorldSettingsPanelImpl {
    pub(super) fn render_wind_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let found = wind::level_wind(&self.state.read().scene.world());
        match found {
            None => self.render_missing_wind(cx),
            Some(found) => self.render_wind(found, window, cx),
        }
    }

    fn render_missing_wind(&mut self, cx: &mut Context<WorldSettingsPanel>) -> AnyElement {
        let state = self.state.clone();
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("This level has no global wind: foliage sways in its own wind."),
            )
            .child(
                Button::new("world-create-wind")
                    .label("Create Wind")
                    .small()
                    .primary()
                    .on_click(cx.listener(move |panel, _event, _window, cx| {
                        let result = wind::create_wind(&mut state.write());
                        panel.world_settings.wind_message =
                            (!result.changed).then(|| result.no_op_reason.to_string());
                        cx.notify();
                    })),
            )
            .children(self.wind_message.clone().map(|message| {
                div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(message)
            }))
            .into_any_element()
    }

    fn render_wind(
        &mut self,
        found: LevelWind,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let properties = match &self.wind_properties {
            Some(properties) => Arc::clone(properties),
            None => match ClassProperties::load(WIND_CLASS) {
                Some(properties) => {
                    self.wind_properties = Some(Arc::clone(&properties));
                    properties
                }
                None => {
                    return div()
                        .text_xs()
                        .child("WindComponent is not registered in this build.")
                        .into_any_element()
                }
            },
        };
        let Some(rows) = component_property_rows(
            &self.state,
            &mut self.wind_property_state,
            WIND_CLASS,
            &properties,
            &found.object_id,
            found.component_index,
            window,
            cx,
        ) else {
            return div().into_any_element();
        };
        let note = if found.enabled {
            format!("The global wind is {}'s WindComponent.", found.object_name)
        } else {
            format!(
                "{}'s WindComponent is disabled, so foliage sways in its own wind.",
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
