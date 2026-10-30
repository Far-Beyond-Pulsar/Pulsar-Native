//! Level-wide settings: the level's sky (its one `AtmosphereComponent`,
//! [`sky_section`]), its global wind (its one `WindComponent`,
//! [`wind_section`]) and the persisted gameplay and simulation settings.

use std::collections::HashMap;
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

use engine_fs::world_roles::{EngineRole, ROLES};

use crate::state::LevelEditorState;
use crate::WorldSettingsPanel;

mod environment_section;
mod input_section;
mod pipeline_section;
mod sky_section;
mod wind_section;

#[derive(Clone, Debug)]
enum BlueprintCatalogState {
    Loading,
    Ready(Vec<String>),
    Failed(String),
}

/// One engine role's Blueprint picker: the Blueprints that implement the
/// role's trait, from the project index.
struct RolePicker {
    role: EngineRole,
    picker: Entity<SearchableList<String>>,
    catalog: BlueprintCatalogState,
}

/// Body for the World Settings dock panel.
pub struct WorldSettingsPanelImpl {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gravity_x_input: Entity<InputState>,
    gravity_y_input: Entity<InputState>,
    gravity_z_input: Entity<InputState>,
    time_scale_input: Entity<InputState>,
    fixed_timestep_input: Entity<InputState>,
    /// One picker per row of `engine_fs::world_roles::ROLES`.
    role_pickers: Vec<RolePicker>,
    /// The render pipeline picker and the ids it currently lists.
    pipeline_picker: Entity<SearchableList<String>>,
    pipeline_ids: Vec<String>,
    /// The player input mapping picker and the project's mappings.
    mapping_picker: Entity<SearchableList<String>>,
    mapping_catalog: BlueprintCatalogState,
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
    environment_property_state: ui_common::PropertyStateManager,
    environment_properties: HashMap<String, Arc<sky_section::ClassProperties>>,
    environment_selection: Option<environment_section::EnvironmentSelection>,
    environment_sidebar_scroll: gpui::UniformListScrollHandle,
    environment_detail_scroll: gpui::ScrollHandle,
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
        let role_pickers: Vec<RolePicker> = ROLES
            .iter()
            .map(|spec| RolePicker {
                role: spec.role,
                picker: cx.new(|cx| {
                    SearchableList::<String>::new(window, cx, Vec::new(), blueprint_label)
                        .with_empty_text(format!("No {} Blueprints found", spec.label))
                        .with_max_width(px(360.0))
                        .with_max_height(px(320.0))
                }),
                catalog: BlueprintCatalogState::Loading,
            })
            .collect();

        let pipeline_ids = Self::pipeline_ids();
        let pipeline_picker = cx.new(|cx| {
            SearchableList::<String>::new(
                window,
                cx,
                pipeline_ids.clone(),
                pipeline_section::pipeline_label,
            )
            .with_empty_text("No render pipelines")
            .with_max_width(px(360.0))
            .with_max_height(px(320.0))
        });

        let mapping_picker = cx.new(|cx| {
            SearchableList::<String>::new(window, cx, vec!["None".to_string()], input_section::mapping_label)
                .with_empty_text("No input mappings found")
                .with_max_width(px(360.0))
                .with_max_height(px(320.0))
        });

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(
            &mapping_picker,
            |panel, _picker, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(path) = event {
                    panel.world_settings.select_input_mapping(path, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe(
            &pipeline_picker,
            |panel, _picker, event: &SearchableListEvent<String>, cx| {
                if let SearchableListEvent::Select(id) = event {
                    panel.world_settings.select_pipeline(id, cx);
                }
            },
        ));
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

        for role_picker in &role_pickers {
            let state_for_selection = state.clone();
            let role = role_picker.role;
            subscriptions.push(cx.subscribe(
                &role_picker.picker,
                move |_panel, _picker, event: &SearchableListEvent<String>, cx| {
                    if let SearchableListEvent::Select(path) = event {
                        let mut state = state_for_selection.write();
                        state
                            .scene
                            .world_settings
                            .set_role_blueprint(role, (path != "None").then(|| path.clone()));
                        mark_world_settings_dirty(&mut state);
                        cx.notify();
                    }
                },
            ));
        }

        let mut panel = Self {
            state: state.clone(),
            gravity_x_input,
            gravity_y_input,
            gravity_z_input,
            time_scale_input,
            fixed_timestep_input,
            role_pickers,
            pipeline_picker,
            pipeline_ids,
            mapping_picker,
            mapping_catalog: BlueprintCatalogState::Loading,
            sky_property_state: ui_common::PropertyStateManager::new(),
            atmosphere_properties: None,
            sky_message: None,
            wind_property_state: ui_common::PropertyStateManager::new(),
            wind_properties: None,
            wind_message: None,
            environment_property_state: ui_common::PropertyStateManager::default(),
            environment_properties: HashMap::new(),
            environment_selection: None,
            environment_sidebar_scroll: gpui::UniformListScrollHandle::new(),
            environment_detail_scroll: gpui::ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        panel.load_blueprint_catalog(cx);
        panel.load_input_mappings(cx);
        panel.watch_input_mappings(cx);
        panel.watch_blueprint_index(cx);
        panel.watch_render_graphs(cx);
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
            for role_picker in &mut self.role_pickers {
                role_picker.catalog = BlueprintCatalogState::Failed(format!(
                    "Open a project to browse its {} Blueprints.",
                    role_picker.role.spec().label
                ));
            }
            return;
        };
        let pickers: Vec<(EngineRole, Entity<SearchableList<String>>)> = self
            .role_pickers
            .iter()
            .map(|role_picker| (role_picker.role, role_picker.picker.clone()))
            .collect();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load_role_blueprints(&root, rebuild) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    for (role, picker) in &pickers {
                        let paths = result
                            .as_ref()
                            .ok()
                            .and_then(|by_role| by_role.get(role))
                            .cloned()
                            .unwrap_or_default();
                        picker.update(cx, |picker, cx| {
                            let mut items = vec!["None".to_string()];
                            items.extend(paths.iter().cloned());
                            picker.set_items(items, cx);
                        });
                        if let Some(role_picker) = panel
                            .world_settings
                            .role_pickers
                            .iter_mut()
                            .find(|role_picker| role_picker.role == *role)
                        {
                            role_picker.catalog = match &result {
                                Ok(_) => BlueprintCatalogState::Ready(paths),
                                Err(error) => BlueprintCatalogState::Failed(error.clone()),
                            };
                        }
                    }
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
                    for role_picker in &mut panel.world_settings.role_pickers {
                        role_picker.catalog = BlueprintCatalogState::Loading;
                    }
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
        let pipeline_section = self.render_pipeline_section(cx);
        let sky_section = self.render_sky_section(window, cx);
        let wind_section = self.render_wind_section(window, cx);
        let environment_tools = self.render_environment_tools(cx);
        let state = self.state.read();
        let settings = &state.scene.world_settings;
        let input_row =
            self.render_input_mapping_row(settings.player_input_mapping.as_deref(), cx);
        let role_rows: Vec<AnyElement> = self
            .role_pickers
            .iter()
            .map(|role_picker| {
                self.render_role_row(role_picker, settings.role_blueprint(role_picker.role), cx)
            })
            .collect();
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(self.render_header(cx))
            .child(
                div().flex_1().min_h_0().overflow_y_scroll().child(
                    v_flex()
                        .w_full()
                        .p_4()
                        .gap_4()
                        .child(self.render_section_title("Render Pipeline"))
                        .child(pipeline_section)
                        .child(self.render_section_title("Sky"))
                        .child(sky_section)
                        .child(self.render_section_title("Wind"))
                        .child(wind_section)
                        .child(self.render_section_title("Environment Mixer"))
                        .child(environment_tools)
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
                        .child(self.render_section_title("Gameplay Roles"))
                        .child(
                            v_flex()
                                .gap_3()
                                .children(role_rows)
                                .child(input_row),
                        ),
                ),
            )
    }

    pub(crate) fn render_environment_config(
        &mut self,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let tools = self.render_environment_tools(cx);
        let components = self.render_environment_components(window, cx);
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .size_full()
                    .min_h_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .w_full()
                            .p_3()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(tools),
                    )
                    .child(div().flex_1().min_h_0().w_full().child(components)),
            )
            .into_any_element()
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

    /// One role: its name, the Blueprint chosen for it and, below, whether
    /// that choice is available in the project.
    fn render_role_row(
        &self,
        role_picker: &RolePicker,
        selected: Option<&str>,
        cx: &Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let spec = role_picker.role.spec();
        let selected = selected.map(str::to_owned).unwrap_or_else(|| "None".to_string());
        let display = blueprint_label(&selected);
        let picker = role_picker.picker.clone();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(spec.label),
                    )
                    .child(
                        Popover::<SearchableList<String>>::new(format!(
                            "{}-blueprint",
                            spec.setting_key
                        ))
                        .anchor(Corner::BottomRight)
                        .trigger(
                            Button::new(format!("{}-blueprint-trigger", spec.setting_key))
                                .label(display)
                                .small()
                                .ghost()
                                .dropdown_caret(true),
                        )
                        .content(move |_window, _cx| picker.clone()),
                    ),
            )
            .child(catalog_status(
                spec.label,
                spec.trait_path,
                &role_picker.catalog,
                selected.as_str(),
                cx,
            ))
            .into_any_element()
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

/// Adapter boundary between the panel and the project blueprint index: the
/// Blueprints that can fill each engine role, from one read of the index.
fn load_role_blueprints(
    project_root: &Path,
    rebuild: bool,
) -> Result<std::collections::BTreeMap<EngineRole, Vec<String>>, String> {
    let index = if rebuild {
        engine_fs::BlueprintTraitIndex::load_or_rebuild(project_root)
    } else {
        engine_fs::BlueprintTraitIndex::load(project_root)
    };
    index
        .map(|index| {
            ROLES
                .iter()
                .map(|spec| {
                    let blueprints = index
                        .blueprints_for_trait(spec.trait_id)
                        .into_iter()
                        .map(|entry| entry.blueprint_path.clone())
                        .collect();
                    (spec.role, blueprints)
                })
                .collect()
        })
        .map_err(|error| format!("Could not load the Blueprint trait index: {error:#}"))
}

fn is_blueprint_index_path(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .ends_with(engine_fs::BlueprintTraitIndex::RELATIVE_PATH)
}

fn blueprint_label(path: &String) -> String {
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

fn catalog_message(
    role_label: &str,
    trait_path: &str,
    state: &BlueprintCatalogState,
    selected: &str,
) -> String {
    match state {
        BlueprintCatalogState::Loading => format!("Loading {role_label} Blueprints…"),
        BlueprintCatalogState::Ready(paths) if paths.is_empty() => format!(
            "No Blueprint implements `{trait_path}`. Add that trait to a Blueprint to make it selectable."
        ),
        BlueprintCatalogState::Ready(paths)
            if selected != "None" && !paths.iter().any(|path| path == selected) =>
        {
            format!("Selected Blueprint is not in the current index: {selected}")
        }
        BlueprintCatalogState::Ready(_) => {
            format!("Select a Blueprint that implements the {role_label} trait.")
        }
        BlueprintCatalogState::Failed(error) => error.clone(),
    }
}

fn catalog_status(
    role_label: &str,
    trait_path: &str,
    state: &BlueprintCatalogState,
    selected: &str,
    cx: &Context<WorldSettingsPanel>,
) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(catalog_message(role_label, trait_path, state, selected))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{blueprint_label, catalog_message, load_role_blueprints, BlueprintCatalogState, EngineRole};

    #[test]
    fn each_role_lists_only_blueprints_implementing_its_trait() {
        let dir = tempfile::tempdir().unwrap();
        for (class, traits) in [
            ("Mode", vec![engine_fs::builtin_traits::GAME_MODE_TRAIT_PATH]),
            ("Hero", vec![engine_fs::builtin_traits::PLAYER_CHARACTER_TRAIT_PATH]),
            ("Both", vec![
                engine_fs::builtin_traits::GAME_MODE_TRAIT_PATH,
                engine_fs::builtin_traits::GAME_STATE_TRAIT_PATH,
            ]),
        ] {
            let path = dir.path().join(format!("classes/{class}/graph_save.json"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                serde_json::json!({
                    "format_version": 1, "main_graph": {}, "variables": [],
                    "blueprint_metadata": {"implemented_traits": traits}
                })
                .to_string(),
            )
            .unwrap();
        }
        let roles = load_role_blueprints(dir.path(), true).unwrap();
        assert_eq!(
            roles[&EngineRole::GameMode],
            ["classes/Both/graph_save.json", "classes/Mode/graph_save.json"]
        );
        assert_eq!(roles[&EngineRole::PlayerCharacter], ["classes/Hero/graph_save.json"]);
        assert_eq!(roles[&EngineRole::GameState], ["classes/Both/graph_save.json"]);
        assert!(roles[&EngineRole::PlayerController].is_empty());
        // The non-rebuilding path (index file watcher) reads the same.
        assert_eq!(load_role_blueprints(dir.path(), false).unwrap(), roles);
    }

    #[test]
    fn labels_and_status_messages_name_the_role() {
        assert_eq!(blueprint_label(&"classes/Mode/graph_save.json".to_string()), "Mode");
        assert_eq!(blueprint_label(&"a/b.blueprint.json".to_string()), "a/b");
        let empty = BlueprintCatalogState::Ready(Vec::new());
        let message = catalog_message("Player Controller", "types/traits/player_controller.trait.json", &empty, "None");
        assert!(message.contains("player_controller.trait.json"));
        let ready = BlueprintCatalogState::Ready(vec!["a".into()]);
        assert!(catalog_message("Game State", "t", &ready, "gone").contains("not in the current index"));
    }
}
