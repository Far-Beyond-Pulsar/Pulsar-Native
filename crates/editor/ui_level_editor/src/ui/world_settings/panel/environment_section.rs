//! Environment mixer sidebar, presets, and selected-component properties.
//!
//! Everything it offers is registered by plugins ([`crate::extend`]): the
//! starter setups ([`crate::extend::EnvironmentPreset`]), the add buttons
//! (create entries with an environment label), the sidebar's level
//! components (their World Settings sections) and listed classes
//! ([`crate::extend::EnvironmentListing`]).

use gpui::prelude::*;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Disableable as _, Sizable,
};

use crate::extend::{create_entries, environment_listings, level_component_sections};
use crate::scene_edit::{components, objects};
use crate::ui::world_settings::panel::level_component_section::{
    component_property_rows, ClassProperties,
};
use crate::WorldSettingsPanel;

/// The first component of the object `id`, as the mixer selects it.
fn first_component(
    state: &crate::state::LevelEditorState,
    id: &str,
) -> Option<EnvironmentSelection> {
    let world = state.scene.world();
    let class_name = components::get_component_class_names(&world, id)
        .into_iter()
        .next()?;
    Some(EnvironmentSelection::Component {
        object_id: id.to_string(),
        class_name,
        component_index: 0,
    })
}

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
        // What the plugins offer here (a disabled plugin's are absent).
        let addable: Vec<_> = {
            let state = self.state.read();
            let world = state.scene.world();
            create_entries()
                .into_iter()
                .filter_map(|entry| {
                    let label = entry.environment_label?;
                    let offered = entry.offered.is_none_or(|offered| offered(&world));
                    Some((entry, label, offered))
                })
                .collect()
        };
        let presets: Vec<_> = crate::extend::environment_presets();
        let presets_available = !presets.is_empty();
        let presets = presets.into_iter().map(|preset| {
            let state = state.clone();
            Button::new(format!("environment-preset-{}", preset.id))
                .label(preset.label)
                .small()
                .on_click(cx.listener(move |_panel, _event, _window, cx| {
                    (preset.apply)(&mut state.write());
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
            .when(presets_available, |this| {
                this.child(h_flex().gap_2().children(presets))
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Add component"),
                    )
                    .children(addable.chunks(4).map(|group| {
                        h_flex()
                            .gap_2()
                            .children(group.iter().map(|(entry, label, offered)| {
                                let entry = *entry;
                                let state = self.state.clone();
                                Button::new(format!("environment-add-{}", entry.id))
                                    .label(format!("+ {label}"))
                                    .small()
                                    .ghost()
                                    .disabled(!offered)
                                    .on_click(cx.listener(move |panel, _event, _window, cx| {
                                        let mut state = state.write();
                                        let result = (entry.create)(&mut state);
                                        if let Some(selected) = result
                                            .affected_ids
                                            .first()
                                            .and_then(|id| first_component(&state, id))
                                        {
                                            panel.world_settings.environment_selection =
                                                Some(selected);
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
                        .track_scroll(&self.environment_detail_scroll)
                        .h_full()
                        .child(div().w_full().flex_shrink_0().px_4().py_3().child(detail)),
                ),
            )
            .into_any_element()
    }

    fn environment_sidebar_items(&self) -> Vec<SidebarItem> {
        let state = self.state.read();
        let world = state.scene.world();
        let mut items = Vec::new();
        for section in level_component_sections() {
            let class_name = section.class_name;
            if let Some(found) = crate::extend::level_component(&world, section.find) {
                items.push(SidebarItem {
                    title: section.sidebar_title.to_string(),
                    subtitle: found.object_name,
                    selection: EnvironmentSelection::Component {
                        object_id: found.object_id,
                        class_name: class_name.to_string(),
                        component_index: found.component_index,
                    },
                });
            } else {
                items.push(SidebarItem {
                    title: section.sidebar_title.to_string(),
                    subtitle: section.sidebar_missing.to_string(),
                    selection: EnvironmentSelection::Missing(class_name.to_string()),
                });
            }
        }
        for listing in environment_listings() {
            let (class_name, label) = (&listing.class_name, listing.label);
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
                let section = level_component_sections()
                    .into_iter()
                    .find(|section| section.class_name == class_name);
                let listing = environment_listings()
                    .into_iter()
                    .find(|listing| listing.class_name == class_name);
                let label = section
                    .map(|section| section.sidebar_title)
                    .or(listing.map(|listing| listing.label))
                    .unwrap_or("Environment Component");
                let create = section.map(|section| section.create).or_else(|| {
                    let entry = listing.and_then(|listing| listing.create_entry)?;
                    crate::extend::create_entry(entry).map(|entry| entry.create)
                });
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
                    .when_some(create, |this, create| {
                        this.child(
                            Button::new("environment-create-selected")
                                .label(format!("Create {label}"))
                                .small()
                                .primary()
                                .on_click(cx.listener(move |panel, _event, _window, cx| {
                                    let mut state = state.write();
                                    create(&mut state);
                                    // Select what now holds the class.
                                    let selected =
                                        find_component(&state.scene.world(), &class_name).map(
                                            |(object_id, _, component_index)| {
                                                EnvironmentSelection::Component {
                                                    object_id,
                                                    class_name: class_name.clone(),
                                                    component_index,
                                                }
                                            },
                                        );
                                    drop(state);
                                    if let Some(selected) = selected {
                                        panel.world_settings.environment_selection = Some(selected);
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .into_any_element()
            }
            Some(EnvironmentSelection::Component {
                object_id,
                class_name,
                component_index,
            }) => {
                if let Some(section) = level_component_sections()
                    .into_iter()
                    .find(|section| section.class_name == class_name)
                {
                    return self.render_level_component_section(section, window, cx);
                }
                let object_name = objects::get_all_objects(&self.state.read().scene.world())
                    .into_iter()
                    .find(|object| object.id == object_id)
                    .map(|object| object.name)
                    .unwrap_or_else(|| "Environment Component".to_string());
                let Some(listing) = environment_listings()
                    .into_iter()
                    .find(|listing| listing.class_name == class_name)
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
                    listing.class_name,
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
                                    .child(format!("{object_name} · {}", listing.label)),
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
