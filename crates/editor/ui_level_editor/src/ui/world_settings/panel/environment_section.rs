//! Environment mixer sidebar, presets, and selected-component properties.

use gpui::prelude::*;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Disableable as _, Sizable,
};

use crate::scene_edit::{components, objects};
use crate::ui::world_settings::panel::sky_section::{component_property_rows, ClassProperties};
use crate::WorldSettingsPanel;

const COMPONENTS: &[(&str, &str)] = &[
    ("LightComponent", "Light"),
    ("GlobalFogComponent", "Global Fog"),
    ("LocalFogVolumeComponent", "Local Fog Volume"),
    ("PostProcessVolumeComponent", "Post Process Volume"),
    ("ReflectionCaptureComponent", "Reflection Capture"),
    ("WaterVolumeComponent", "Water Volume"),
];

const ADD_COMPONENTS: &[(&str, &str)] = &[
    ("LightComponent", "Point Light"),
    ("SpotLight", "Spot Light"),
    ("DirectionalLight", "Directional Light"),
    ("GlobalFogComponent", "Global Fog"),
    ("LocalFogVolumeComponent", "Local Fog Volume"),
    ("PostProcessVolumeComponent", "Post Process"),
    ("ReflectionCaptureComponent", "Reflection Capture"),
    ("WaterVolumeComponent", "Water Volume"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EnvironmentSelection {
    Missing(String),
    Component {
        object_id: String,
        class_name: String,
        component_index: usize,
    },
}

#[derive(Clone)]
struct SidebarItem {
    title: String,
    subtitle: String,
    selection: EnvironmentSelection,
}

impl SidebarItem {
    fn key(&self) -> String {
        match &self.selection {
            EnvironmentSelection::Missing(class) => format!("missing:{class}"),
            EnvironmentSelection::Component {
                object_id,
                class_name,
                component_index,
            } => format!("{object_id}:{class_name}:{component_index}"),
        }
    }
}

impl super::WorldSettingsPanelImpl {
    pub(super) fn render_environment_tools(
        &mut self,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let state = self.state.clone();
        let has_sun = !engine_backend::scene::level_rules::directional_lights(
            &self.state.read().scene.world(),
        )
        .is_empty();
        let presets = crate::scene_edit::environment::EnvironmentPreset::ALL
            .into_iter()
            .map(|preset| {
                let state = state.clone();
                Button::new(format!("environment-preset-{}", preset.label()))
                    .label(preset.label())
                    .small()
                    .on_click(cx.listener(move |_panel, _event, _window, cx| {
                        crate::scene_edit::environment::apply_preset(&mut state.write(), preset);
                        cx.notify();
                    }))
            });

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Starter setups add a sky and sun when missing, then tune the sun."),
            )
            .child(h_flex().gap_2().children(presets))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Add component"),
                    )
                    .children(ADD_COMPONENTS.chunks(4).map(|group| {
                        h_flex()
                            .gap_2()
                            .children(group.iter().map(|(class_name, label)| {
                                let class_name = (*class_name).to_string();
                                let state = self.state.clone();
                                Button::new(format!("environment-add-{class_name}"))
                                    .label(format!("+ {label}"))
                                    .small()
                                    .ghost()
                                    .disabled(class_name == "DirectionalLight" && has_sun)
                                    .on_click(cx.listener(move |panel, _event, _window, cx| {
                                        let result =
                                            crate::scene_edit::environment::create_component(
                                                &mut state.write(),
                                                &class_name,
                                            );
                                        if let Some(id) = result.affected_ids.first() {
                                            panel.world_settings.environment_selection =
                                                Some(EnvironmentSelection::Component {
                                                    object_id: id.clone(),
                                                    class_name: "LightComponent".to_string(),
                                                    component_index: 0,
                                                });
                                        }
                                        cx.notify();
                                    }))
                            }))
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_environment_components(
        &mut self,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let items = self.environment_sidebar_items();
        if self.environment_selection.is_none() {
            self.environment_selection = items.first().map(|item| item.selection.clone());
        }
        let selected = self.environment_selection.clone();
        let selected_key = selected.as_ref().map(selection_key).unwrap_or_default();
        let sidebar = uniform_list(
            "environment-mixer-sidebar-items",
            items.len(),
            cx.processor(move |_panel, range: std::ops::Range<usize>, _window, cx| {
                range
                    .map(|index| {
                        let item = &items[index];
                        let key = item.key();
                        let active = key == selected_key;
                        let selection = item.selection.clone();
                        h_flex()
                            .w_full()
                            .h(px(48.0))
                            .px_3()
                            .gap_2()
                            .items_center()
                            .rounded_md()
                            .when(active, |el| el.bg(cx.theme().accent.opacity(0.18)))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_sm().child(item.title.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(item.subtitle.clone()),
                                    ),
                            )
                            .id(format!("environment-item-{key}"))
                            .on_click(cx.listener(move |panel, _event, _window, cx| {
                                panel.world_settings.environment_selection =
                                    Some(selection.clone());
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.environment_sidebar_scroll)
        .w_full()
        .h_full();
        let detail = self.render_selected_environment_item(selected, window, cx);
        h_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .gap_0()
            .child(
                v_flex()
                    .w(px(250.0))
                    .h_full()
                    .min_h_0()
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child("ENVIRONMENT ITEMS"),
                    )
                    .child(div().flex_1().min_h_0().overflow_hidden().child(sidebar)),
            )
            .child(
                v_flex().flex_1().min_w_0().h_full().min_h_0().child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .h_full()
                        .child(div().w_full().px_4().py_3().child(detail)),
                ),
            )
            .into_any_element()
    }

    fn environment_sidebar_items(&self) -> Vec<SidebarItem> {
        let state = self.state.read();
        let world = state.scene.world();
        let mut items = Vec::new();
        for (class_name, title, missing_label) in [
            (
                engine_backend::scene::level_rules::ATMOSPHERE_CLASS,
                "Sky and Atmosphere",
                "Missing · creates a sky",
            ),
            (
                engine_backend::scene::level_rules::WIND_CLASS,
                "Global Wind",
                "Missing · creates global wind",
            ),
        ] {
            if let Some((object_id, object_name, component_index)) =
                find_component(&world, class_name)
            {
                items.push(SidebarItem {
                    title: title.to_string(),
                    subtitle: object_name,
                    selection: EnvironmentSelection::Component {
                        object_id,
                        class_name: class_name.to_string(),
                        component_index,
                    },
                });
            } else {
                items.push(SidebarItem {
                    title: title.to_string(),
                    subtitle: missing_label.to_string(),
                    selection: EnvironmentSelection::Missing(class_name.to_string()),
                });
            }
        }
        for (class_name, label) in COMPONENTS {
            let mut found = false;
            for object in objects::get_all_objects(&world) {
                for (component_index, component) in
                    components::get_components_metadata(&world, &object.id)
                        .into_iter()
                        .enumerate()
                {
                    if component.class_name == *class_name {
                        found = true;
                        items.push(SidebarItem {
                            title: label.to_string(),
                            subtitle: object.name.clone(),
                            selection: EnvironmentSelection::Component {
                                object_id: object.id.clone(),
                                class_name: class_name.to_string(),
                                component_index,
                            },
                        });
                    }
                }
            }
            if !found {
                items.push(SidebarItem {
                    title: label.to_string(),
                    subtitle: "Missing · add from presets and components".to_string(),
                    selection: EnvironmentSelection::Missing(class_name.to_string()),
                });
            }
        }
        items
    }

    fn render_selected_environment_item(
        &mut self,
        selection: Option<EnvironmentSelection>,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        match selection {
            Some(EnvironmentSelection::Missing(class_name)) => {
                let label = COMPONENTS
                    .iter()
                    .find(|(class, _)| *class == class_name)
                    .map(|(_, label)| *label)
                    .or_else(|| match class_name.as_str() {
                        "AtmosphereComponent" => Some("Sky and Atmosphere"),
                        "WindComponent" => Some("Global Wind"),
                        _ => None,
                    })
                    .unwrap_or("Environment Component");
                let create_class = match class_name.as_str() {
                    "AtmosphereComponent" => "AtmosphereComponent",
                    "WindComponent" => "WindComponent",
                    other => other,
                }
                .to_string();
                let state = self.state.clone();
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("This environment item is not in the level yet."),
                    )
                    .child(
                        Button::new("environment-create-selected")
                            .label(format!("Create {label}"))
                            .small()
                            .primary()
                            .on_click(cx.listener(move |panel, _event, _window, cx| {
                                let result = match create_class.as_str() {
                                    "AtmosphereComponent" => {
                                        crate::scene_edit::sky::create_sky(&mut state.write())
                                    }
                                    "WindComponent" => {
                                        crate::scene_edit::wind::create_wind(&mut state.write())
                                    }
                                    _ => crate::scene_edit::environment::create_component(
                                        &mut state.write(),
                                        &create_class,
                                    ),
                                };
                                if let Some(id) = result.affected_ids.first() {
                                    panel.world_settings.environment_selection =
                                        Some(EnvironmentSelection::Component {
                                            object_id: id.clone(),
                                            class_name: create_class.clone(),
                                            component_index: 0,
                                        });
                                }
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
            Some(EnvironmentSelection::Component {
                object_id,
                class_name,
                component_index,
            }) => {
                if class_name == engine_backend::scene::level_rules::ATMOSPHERE_CLASS {
                    return self.render_sky_section(window, cx);
                }
                if class_name == engine_backend::scene::level_rules::WIND_CLASS {
                    return self.render_wind_section(window, cx);
                }
                let object_name = objects::get_all_objects(&self.state.read().scene.world())
                    .into_iter()
                    .find(|object| object.id == object_id)
                    .map(|object| object.name)
                    .unwrap_or_else(|| "Environment Component".to_string());
                let Some((_, label)) = COMPONENTS.iter().find(|(name, _)| *name == class_name)
                else {
                    return div()
                        .text_sm()
                        .child("This environment component is unavailable.")
                        .into_any_element();
                };
                if !self.environment_properties.contains_key(&class_name) {
                    if let Some(properties) = ClassProperties::load(&class_name) {
                        self.environment_properties
                            .insert(class_name.clone(), properties);
                    }
                }
                let Some(properties) = self.environment_properties.get(&class_name).cloned() else {
                    return div()
                        .text_sm()
                        .child("Component properties are unavailable.")
                        .into_any_element();
                };
                let Some(rows) = component_property_rows(
                    &self.state,
                    &mut self.environment_property_state,
                    COMPONENTS
                        .iter()
                        .find(|(name, _)| *name == class_name)
                        .unwrap()
                        .0,
                    &properties,
                    &object_id,
                    component_index,
                    window,
                    cx,
                ) else {
                    return div()
                        .text_sm()
                        .child("This component was removed from the level.")
                        .into_any_element();
                };
                v_flex()
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{object_name} · {label}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{class_name} properties")),
                            ),
                    )
                    .children(rows)
                    .into_any_element()
            }
            None => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Select an environment item to edit its properties.")
                .into_any_element(),
        }
    }
}

fn selection_key(selection: &EnvironmentSelection) -> String {
    match selection {
        EnvironmentSelection::Missing(class) => format!("missing:{class}"),
        EnvironmentSelection::Component {
            object_id,
            class_name,
            component_index,
        } => format!("{object_id}:{class_name}:{component_index}"),
    }
}

fn find_component(
    world: &pulsar_scenedb::World,
    class_name: &str,
) -> Option<(String, String, usize)> {
    objects::get_all_objects(world)
        .into_iter()
        .find_map(|object| {
            components::get_components_metadata(world, &object.id)
                .into_iter()
                .enumerate()
                .find(|(_, component)| component.class_name == class_name)
                .map(|(index, _)| (object.id, object.name, index))
        })
}
