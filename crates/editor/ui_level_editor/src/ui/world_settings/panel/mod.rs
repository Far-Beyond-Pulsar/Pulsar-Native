//! Level-wide settings: the level's sky (its one `AtmosphereComponent`,
//! [`sky_section`]), its global wind (its one `WindComponent`,
//! [`wind_section`]) and the persisted gameplay and simulation settings.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::{SearchableList, SearchableListEvent},
    h_flex,
    input::{InputEvent, InputState, NumberInput},
    popover::Popover,
    v_flex, ActiveTheme, Sizable,
};

use crate::state::LevelEditorState;
use crate::WorldSettingsPanel;

mod sky_section;
mod wind_section;

/// Canonical game-mode trait asset path in project-relative form.
pub const GAME_MODE_TRAIT_PATH: &str = "types/traits/game_mode.trait.json";

#[derive(Clone, Debug)]
enum BlueprintCatalogState {
    Loading,
    Ready(Vec<String>),
    Failed(String),
}

/// Body for the World Settings dock panel.
pub struct WorldSettingsPanelImpl {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gravity_x_input: Entity<InputState>,
    gravity_y_input: Entity<InputState>,
    gravity_z_input: Entity<InputState>,
    time_scale_input: Entity<InputState>,
    fixed_timestep_input: Entity<InputState>,
    game_mode_picker: Entity<SearchableList<String>>,
    catalog_state: BlueprintCatalogState,
    /// The Sky section's property rows (#1057).
    sky_property_state: ui_common::PropertyStateManager,
    atmosphere_properties: Option<Arc<sky_section::ClassProperties>>,
    /// Why the last "Create Sky" did nothing, if it did nothing.
    sky_message: Option<String>,
    /// The Wind section's property rows (#1123).
    wind_property_state: ui_common::PropertyStateManager,
    wind_properties: Option<Arc<sky_section::ClassProperties>>,
    /// Why the last "Create Wind" did nothing, if it did nothing.
    wind_message: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl WorldSettingsPanelImpl {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> Self {
        let gravity = state.read().scene.world_settings.gravity;
        let gravity_x_input = make_numeric_input(gravity[0], "0.0", window, cx);
        let gravity_y_input = make_numeric_input(gravity[1], "-9.81", window, cx);
        let gravity_z_input = make_numeric_input(gravity[2], "0.0", window, cx);
        let time_scale_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("1.0");
            input.set_value(
                format!("{:.3}", state.read().scene.world_settings.time_scale),
                window,
                cx,
            );
            input
        });
        let fixed_timestep_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("0.02");
            input.set_value(
                format!("{:.3}", state.read().scene.world_settings.fixed_timestep),
                window,
                cx,
            );
            input
        });
        let game_mode_picker = cx.new(|cx| {
            SearchableList::<String>::new(window, cx, Vec::new(), game_mode_blueprint_label)
                .with_empty_text("No game mode Blueprints found")
                .with_max_width(px(360.0))
                .with_max_height(px(320.0))
        });

        let mut subscriptions = Vec::new();
        for (input, field) in [
            (gravity_x_input.clone(), NumericSetting::GravityX),
            (gravity_y_input.clone(), NumericSetting::GravityY),
            (gravity_z_input.clone(), NumericSetting::GravityZ),
            (time_scale_input.clone(), NumericSetting::TimeScale),
            (fixed_timestep_input.clone(), NumericSetting::FixedTimestep),
        ] {
            let state = state.clone();
            let input_for_event = input.clone();
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |_panel, _input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change | InputEvent::Blur) {
                        input_for_event.update(cx, |input, input_cx| {
                            let parsed = input.text().to_string().trim().parse::<f32>().ok();
                            if let Some(value) = parsed.filter(|value| field.is_valid(*value)) {
                                let mut state = state.write();
                                field.set(&mut state.scene.world_settings, value);
                                mark_world_settings_dirty(&mut state);
                            } else if matches!(event, InputEvent::Blur) {
                                let state = state.read();
                                let value = field.get(&state.scene.world_settings);
                                input.set_value(format!("{value:.3}"), window, input_cx);
                            }
                        });
                    }
                },
            ));
        }

        let state_for_selection = state.clone();
        subscriptions.push(cx.subscribe(
            &game_mode_picker,
            move |_panel, _picker, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(path) = event {
                    let mut state = state_for_selection.write();
                    state.scene.world_settings.game_mode_blueprint =
                        (path != "None").then(|| path.clone());
                    mark_world_settings_dirty(&mut state);
                    cx.notify();
                }
            },
        ));

        let mut panel = Self {
            state: state.clone(),
            gravity_x_input,
            gravity_y_input,
            gravity_z_input,
            time_scale_input,
            fixed_timestep_input,
            game_mode_picker,
            catalog_state: BlueprintCatalogState::Loading,
            sky_property_state: ui_common::PropertyStateManager::new(),
            atmosphere_properties: None,
            sky_message: None,
            wind_property_state: ui_common::PropertyStateManager::new(),
            wind_properties: None,
            wind_message: None,
            _subscriptions: subscriptions,
        };
        panel.load_blueprint_catalog(cx);
        panel.watch_blueprint_index(cx);
        panel
    }

    fn load_blueprint_catalog(&mut self, cx: &mut Context<WorldSettingsPanel>) {
        self.load_blueprint_catalog_with_rebuild(cx, true);
    }

    fn load_blueprint_catalog_with_rebuild(
        &mut self,
        cx: &mut Context<WorldSettingsPanel>,
        rebuild: bool,
    ) {
        let Some(root) = engine_state::get_project_path().map(PathBuf::from) else {
            self.catalog_state = BlueprintCatalogState::Failed(
                "Open a project to browse its game mode Blueprints.".to_string(),
            );
            return;
        };
        let picker = self.game_mode_picker.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load_game_mode_blueprints(&root, rebuild) })
                .await;
            let paths = result.as_ref().cloned().unwrap_or_default();
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    picker.update(cx, |picker, cx| {
                        let mut items = vec!["None".to_string()];
                        items.extend(paths.iter().cloned());
                        picker.set_items(items, cx);
                    });
                    panel.world_settings.catalog_state = match result {
                        Ok(paths) => BlueprintCatalogState::Ready(paths),
                        Err(error) => BlueprintCatalogState::Failed(error),
                    };
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn watch_blueprint_index(&self, cx: &mut Context<WorldSettingsPanel>) {
        let mut events = engine_fs::subscribe();
        cx.spawn(async move |this, cx| loop {
            let Ok(event) = events.recv().await else {
                continue;
            };
            if !is_blueprint_index_path(&event.path) {
                continue;
            }

            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    panel.world_settings.catalog_state = BlueprintCatalogState::Loading;
                    panel
                        .world_settings
                        .load_blueprint_catalog_with_rebuild(cx, false);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        let settings_snapshot = self.state.read().scene.world_settings.clone();
        sync_numeric_input(
            &self.gravity_x_input,
            format!("{:.3}", settings_snapshot.gravity[0]),
            window,
            cx,
        );
        sync_numeric_input(
            &self.gravity_y_input,
            format!("{:.3}", settings_snapshot.gravity[1]),
            window,
            cx,
        );
        sync_numeric_input(
            &self.gravity_z_input,
            format!("{:.3}", settings_snapshot.gravity[2]),
            window,
            cx,
        );
        sync_numeric_input(
            &self.time_scale_input,
            format!("{:.3}", settings_snapshot.time_scale),
            window,
            cx,
        );
        sync_numeric_input(
            &self.fixed_timestep_input,
            format!("{:.3}", settings_snapshot.fixed_timestep),
            window,
            cx,
        );
        let sky_section = self.render_sky_section(window, cx);
        let wind_section = self.render_wind_section(window, cx);
        let state = self.state.read();
        let settings = &state.scene.world_settings;
        let selected = settings
            .game_mode_blueprint
            .clone()
            .unwrap_or_else(|| "None".to_string());
        let catalog_state = self.catalog_state.clone();
        let picker = self.game_mode_picker.clone();
        let picker_for_popover = picker.clone();
        let display = if selected == "None" {
            "None".to_string()
        } else {
            game_mode_blueprint_label(&selected)
        };

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_header(cx))
            .child(
                div().flex_1().overflow_y_scroll().child(
                    v_flex()
                        .w_full()
                        .p_4()
                        .gap_4()
                        .child(self.render_section_title("Sky"))
                        .child(sky_section)
                        .child(self.render_section_title("Wind"))
                        .child(wind_section)
                        .child(self.render_section_title("General"))
                        .child(self.render_numeric_row(
                            "Time Scale",
                            &self.time_scale_input,
                            "x",
                            cx,
                        ))
                        .child(self.render_numeric_row(
                            "Fixed Timestep",
                            &self.fixed_timestep_input,
                            "s",
                            cx,
                        ))
                        .child(self.render_gravity_row(cx))
                        .child(self.render_toggle_row(
                            "Physics Enabled",
                            settings.physics_enabled,
                            ToggleSetting::Physics,
                            cx,
                        ))
                        .child(self.render_toggle_row(
                            "Auto Simulation",
                            settings.auto_simulation,
                            ToggleSetting::AutoSimulation,
                            cx,
                        ))
                        .child(self.render_section_title("Game Mode"))
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    Popover::<SearchableList<String>>::new("game-mode-blueprint")
                                        .anchor(Corner::BottomRight)
                                        .trigger(
                                            Button::new("game-mode-blueprint-trigger")
                                                .label(display)
                                                .small()
                                                .ghost()
                                                .dropdown_caret(true),
                                        )
                                        .content(move |_window, _cx| picker_for_popover.clone()),
                                )
                                .child(catalog_status(&catalog_state, selected.as_str(), cx)),
                        ),
                ),
            )
    }

    fn render_header(&self, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
        h_flex()
            .w_full()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_base()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child("World Settings"),
            )
    }

    fn render_section_title(&self, title: &'static str) -> impl IntoElement {
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .child(title)
    }

    fn render_numeric_row(
        &self,
        label: &'static str,
        input: &Entity<InputState>,
        unit: &'static str,
        cx: &Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(NumberInput::new(input).xsmall())
            .child(
                div()
                    .w(px(16.0))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(unit),
            )
    }

    fn render_gravity_row(&self, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Gravity"),
            )
            .child(self.render_axis_input("X", &self.gravity_x_input, cx))
            .child(self.render_axis_input("Y", &self.gravity_y_input, cx))
            .child(self.render_axis_input("Z", &self.gravity_z_input, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("m/s²"),
            )
    }

    fn render_axis_input(
        &self,
        axis: &'static str,
        input: &Entity<InputState>,
        cx: &Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(axis),
            )
            .child(NumberInput::new(input).xsmall())
    }

    fn render_toggle_row(
        &self,
        label: &'static str,
        value: bool,
        field: ToggleSetting,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> impl IntoElement {
        let state = self.state.clone();
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(
                Button::new(format!("toggle-{label}"))
                    .label(if value { "On" } else { "Off" })
                    .small()
                    .when(value, |button| button.primary())
                    .on_click(cx.listener(move |_panel, _event, _window, cx| {
                        let mut state = state.write();
                        let settings = &mut state.scene.world_settings;
                        match field {
                            ToggleSetting::Physics => settings.physics_enabled = !value,
                            ToggleSetting::AutoSimulation => settings.auto_simulation = !value,
                        }
                        mark_world_settings_dirty(&mut state);
                        cx.notify();
                    })),
            )
    }
}

fn sync_numeric_input(
    input: &Entity<InputState>,
    expected: String,
    window: &mut Window,
    cx: &mut Context<WorldSettingsPanel>,
) {
    input.update(cx, |input, cx| {
        if !input.focus_handle(cx).is_focused(window) && input.text().to_string() != expected {
            input.set_value(expected, window, cx);
        }
    });
}

fn make_numeric_input(
    value: f32,
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<WorldSettingsPanel>,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder(placeholder);
        input.set_value(format!("{value:.3}"), window, cx);
        input
    })
}

#[derive(Clone, Copy)]
enum NumericSetting {
    GravityX,
    GravityY,
    GravityZ,
    TimeScale,
    FixedTimestep,
}

impl NumericSetting {
    fn is_valid(self, value: f32) -> bool {
        value.is_finite()
            && match self {
                Self::TimeScale => value >= 0.0,
                Self::FixedTimestep => value > 0.0,
                Self::GravityX | Self::GravityY | Self::GravityZ => true,
            }
    }

    fn get(self, settings: &crate::world_settings_data::WorldSettingsData) -> f32 {
        match self {
            Self::GravityX => settings.gravity[0],
            Self::GravityY => settings.gravity[1],
            Self::GravityZ => settings.gravity[2],
            Self::TimeScale => settings.time_scale,
            Self::FixedTimestep => settings.fixed_timestep,
        }
    }

    fn set(self, settings: &mut crate::world_settings_data::WorldSettingsData, value: f32) {
        match self {
            Self::GravityX => settings.gravity[0] = value,
            Self::GravityY => settings.gravity[1] = value,
            Self::GravityZ => settings.gravity[2] = value,
            Self::TimeScale => settings.time_scale = value,
            Self::FixedTimestep => settings.fixed_timestep = value,
        }
    }
}

#[derive(Clone, Copy)]
enum ToggleSetting {
    Physics,
    AutoSimulation,
}

fn mark_world_settings_dirty(state: &mut LevelEditorState) {
    state.scene.has_unsaved_changes = true;
    state.scene.revision = state.scene.revision.wrapping_add(1);
}

/// Adapter boundary between the panel and the project blueprint index.
fn load_game_mode_blueprints(project_root: &Path, rebuild: bool) -> Result<Vec<String>, String> {
    let index = if rebuild {
        engine_fs::BlueprintTraitIndex::load_or_rebuild(project_root)
    } else {
        engine_fs::BlueprintTraitIndex::load(project_root)
    };
    index
        .map(|index| {
            index
                .blueprints_for_trait(GAME_MODE_TRAIT_PATH)
                .into_iter()
                .map(|entry| entry.blueprint_path.clone())
                .collect()
        })
        .map_err(|error| format!("Could not load the Blueprint trait index: {error:#}"))
}

fn is_blueprint_index_path(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .ends_with(engine_fs::BlueprintTraitIndex::RELATIVE_PATH)
}

fn game_mode_blueprint_label(path: &String) -> String {
    if path == "None" {
        return "None".to_string();
    }
    if Path::new(path).file_name().and_then(|name| name.to_str()) == Some("graph_save.json") {
        return Path::new(path)
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or(path)
            .to_string();
    }
    path.strip_suffix(".blueprint.json")
        .or_else(|| Path::new(path).file_stem().and_then(|stem| stem.to_str()))
        .unwrap_or(path)
        .to_string()
}

fn catalog_status(
    state: &BlueprintCatalogState,
    selected: &str,
    cx: &Context<WorldSettingsPanel>,
) -> AnyElement {
    let message = match state {
        BlueprintCatalogState::Loading => "Loading game mode Blueprints…".to_string(),
        BlueprintCatalogState::Ready(paths) if paths.is_empty() => {
            "No Blueprint implements `types/traits/game_mode.trait.json`. Add that trait to a Blueprint to make it selectable.".to_string()
        }
        BlueprintCatalogState::Ready(paths)
            if selected != "None" && !paths.iter().any(|path| path == selected) =>
        {
            format!("Selected Blueprint is not in the current index: {selected}")
        }
        BlueprintCatalogState::Ready(_) => "Select a Blueprint that implements the game mode trait.".to_string(),
        BlueprintCatalogState::Failed(error) => error.clone(),
    };
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(message)
        .into_any_element()
}
