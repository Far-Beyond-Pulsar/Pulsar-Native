//! Level-wide settings: the components a level holds once (the plugins'
//! [`crate::extend::LevelComponentSection`]s: its sky, its global wind;
//! [`level_component_section`]), the environment mixer, the simulation
//! clock, the plugins' fields ([`crate::extend::WorldSettingField`],
//! [`fields_section`]) and one picker per registered role
//! (`pulsar_traits::RoleRegistration`), listing the implementers of its
//! trait.

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
use ui_common::inspector::{Inspector, InspectorRow};
use ui_common::inspector_model::CardDesc;

use pulsar_traits::roles::RoleRegistration;

use crate::state::LevelEditorState;
use crate::WorldSettingsPanel;

mod environment_section;
mod fields_section;
mod level_component_section;
mod pipeline_section;

#[derive(Clone, Debug)]
enum BlueprintCatalogState {
    Loading,
    Ready(Vec<String>),
    Failed(String),
}

/// One role's picker: the implementers of the role's trait that run as a
/// script class (what a role spawns), from the project's trait index.
struct RolePicker {
    role: &'static RoleRegistration,
    picker: Entity<SearchableList<String>>,
    catalog: BlueprintCatalogState,
}

/// Body for the World Settings dock panel.
pub struct WorldSettingsPanelImpl {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    time_scale_input: Entity<InputState>,
    fixed_timestep_input: Entity<InputState>,
    /// One picker per enabled role.
    role_pickers: Vec<RolePicker>,
    /// The plugins' fields and their widgets.
    fields: Vec<fields_section::FieldState>,
    /// The render pipeline asset picker, made when first shown.
    pipeline_picker: Option<pipeline_section::PipelinePicker>,
    /// Counts the times the project's render graphs were compiled again, so
    /// the picker lists what is there now.
    render_graph_epoch: u64,
    /// The level component sections' widget state, by class.
    level_components: HashMap<&'static str, level_component_section::SectionState>,
    environment_property_state: ui_common::PropertyStateManager,
    environment_properties: HashMap<String, Arc<level_component_section::ClassProperties>>,
    environment_selection: Option<environment_section::EnvironmentSelection>,
    environment_sidebar_scroll: gpui::UniformListScrollHandle,
    environment_detail_scroll: gpui::ScrollHandle,
    inspector: Inspector<WorldSettingsPanel>,
    inspector_initialized: bool,
    _subscriptions: Vec<Subscription>,
}

impl WorldSettingsPanelImpl {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> Self {
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
        let role_pickers: Vec<RolePicker> = pulsar_traits::roles::roles()
            .into_iter()
            .map(|role| RolePicker {
                role,
                picker: cx.new(|cx| {
                    SearchableList::<String>::new(window, cx, Vec::new(), blueprint_label)
                        .with_empty_text(format!("No {} implementers found", role.label))
                        .with_max_width(px(360.0))
                        .with_max_height(px(320.0))
                }),
                catalog: BlueprintCatalogState::Loading,
            })
            .collect();

        let mut subscriptions = Vec::new();
        let fields = Self::build_fields(&state, window, cx, &mut subscriptions);
        for (input, field) in [
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
            let key = role_picker.role.setting_key;
            subscriptions.push(cx.subscribe(
                &role_picker.picker,
                move |_panel, _picker, event: &SearchableListEvent<String>, cx| {
                    if let SearchableListEvent::Select(path) = event {
                        let mut state = state_for_selection.write();
                        state
                            .scene
                            .world_settings
                            .set_role_asset(key, (path != "None").then(|| path.clone()));
                        mark_world_settings_dirty(&mut state);
                        cx.notify();
                    }
                },
            ));
        }

        let inspector = Inspector::new(
            "world-settings-inspector",
            "Search world settings…",
            0,
            |panel: &mut WorldSettingsPanel| &mut panel.world_settings.inspector,
            window,
            cx,
        );

        let mut panel = Self {
            state: state.clone(),
            time_scale_input,
            fixed_timestep_input,
            role_pickers,
            fields,
            pipeline_picker: None,
            render_graph_epoch: 0,
            level_components: HashMap::new(),
            environment_property_state: ui_common::PropertyStateManager::default(),
            environment_properties: HashMap::new(),
            environment_selection: None,
            environment_sidebar_scroll: gpui::UniformListScrollHandle::new(),
            environment_detail_scroll: gpui::ScrollHandle::new(),
            inspector,
            inspector_initialized: false,
            _subscriptions: subscriptions,
        };
        panel.load_blueprint_catalog(cx);
        panel.load_field_assets(cx);
        panel.watch_field_assets(cx);
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
                    "Open a project to browse its {} implementers.",
                    role_picker.role.label
                ));
            }
            return;
        };
        let pickers: Vec<(&'static str, Entity<SearchableList<String>>)> = self
            .role_pickers
            .iter()
            .map(|role_picker| (role_picker.role.id, role_picker.picker.clone()))
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
                            .find(|role_picker| role_picker.role.id == *role)
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
        self.sync_fields(&settings_snapshot, window, cx);
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
        let mut cards = vec![world_settings_card("pipeline", "Render Pipeline")];
        cards.extend(
            crate::extend::level_component_sections()
                .into_iter()
                .map(|section| {
                    world_settings_card(&format!("component:{}", section.class_name), section.title)
                }),
        );
        cards.extend([
            world_settings_card("environment", "Environment Mixer"),
            world_settings_card("simulation", "Simulation"),
        ]);
        cards.extend(
            fields_section::sections(&self.fields)
                .into_iter()
                .map(|section| world_settings_card(&fields_section::section_key(section), section)),
        );
        cards.push(world_settings_card("gameplay", "Gameplay Roles"));
        self.inspector.set_cards(cards);
        if !self.inspector_initialized {
            self.inspector.collapse_all();
            self.inspector_initialized = true;
        }
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(self.render_header(cx))
            .child(self.inspector.render(render_world_settings_row, cx))
    }

    fn render_inspector_card(
        &mut self,
        card: usize,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let Some(key) = self.inspector.card(card).map(|card| card.key.clone()) else {
            return div().into_any_element();
        };
        let body = match key.as_str() {
            "pipeline" => self.render_pipeline_section(window, cx),
            "environment" => self.render_environment_tools(cx),
            "simulation" => {
                let auto_simulation = self.state.read().scene.world_settings.auto_simulation;
                v_flex()
                    .gap_3()
                    .child(self.render_numeric_row("Time Scale", &self.time_scale_input, "x", cx))
                    .child(self.render_numeric_row(
                        "Fixed Timestep",
                        &self.fixed_timestep_input,
                        "s",
                        cx,
                    ))
                    .child(self.render_toggle_row(
                        "Auto Simulation",
                        auto_simulation,
                        ToggleSetting::AutoSimulation,
                        cx,
                    ))
                    .into_any_element()
            }
            "gameplay" => {
                let state = self.state.read();
                let settings = &state.scene.world_settings;
                let rows = self.role_pickers.iter().map(|role_picker| {
                    self.render_role_row(
                        role_picker,
                        settings.role_asset(role_picker.role.setting_key),
                        cx,
                    )
                });
                v_flex().gap_3().children(rows).into_any_element()
            }
            _ if key.starts_with("fields:") => {
                let section = key["fields:".len()..].to_owned();
                self.render_fields_section(&section, cx)
            }
            _ if key.starts_with("component:") => {
                let class_name = &key["component:".len()..];
                let Some(section) = crate::extend::level_component_sections()
                    .into_iter()
                    .find(|section| section.class_name == class_name)
                else {
                    return div().into_any_element();
                };
                self.render_level_component_section(section, window, cx)
            }
            _ => div().into_any_element(),
        };
        v_flex().w_full().p_3().child(body).into_any_element()
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

    /// One role: its name, the implementer chosen for it and, below,
    /// whether that choice is available in the project.
    fn render_role_row(
        &self,
        role_picker: &RolePicker,
        selected: Option<&str>,
        cx: &Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let spec = role_picker.role;
        let selected = selected
            .map(str::to_owned)
            .unwrap_or_else(|| "None".to_string());
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
                spec.trait_id,
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

fn world_settings_card(key: &str, title: &str) -> CardDesc {
    let search_terms = match key {
        "pipeline" => "render pipeline graph",
        "environment" => "environment mixer sky atmosphere wind fog lighting",
        "simulation" => "time scale fixed timestep simulation",
        "gameplay" => "roles game mode player controller player character game state trait",
        _ => key,
    };
    CardDesc {
        key: key.to_string(),
        title: title.to_string(),
        props: vec![ui_common::inspector_model::PropDesc {
            name: format!("{key} {search_terms}"),
            display_name: title.to_string(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            extra_rows: 0,
        }],
        custom: false,
        note: None,
    }
}

fn render_world_settings_row(
    panel: &mut WorldSettingsPanel,
    row: InspectorRow,
    window: &mut Window,
    cx: &mut Context<WorldSettingsPanel>,
) -> AnyElement {
    match row {
        InspectorRow::Property { card, .. } => {
            panel.world_settings.render_inspector_card(card, window, cx)
        }
        _ => div().into_any_element(),
    }
}

#[derive(Clone, Copy)]
enum NumericSetting {
    TimeScale,
    FixedTimestep,
}

impl NumericSetting {
    fn is_valid(self, value: f32) -> bool {
        value.is_finite()
            && match self {
                Self::TimeScale => value >= 0.0,
                Self::FixedTimestep => value > 0.0,
            }
    }

    fn get(self, settings: &crate::world_settings_data::WorldSettingsData) -> f32 {
        match self {
            Self::TimeScale => settings.time_scale,
            Self::FixedTimestep => settings.fixed_timestep,
        }
    }

    fn set(self, settings: &mut crate::world_settings_data::WorldSettingsData, value: f32) {
        match self {
            Self::TimeScale => settings.time_scale = value,
            Self::FixedTimestep => settings.fixed_timestep = value,
        }
    }
}

#[derive(Clone, Copy)]
enum ToggleSetting {
    AutoSimulation,
}

fn mark_world_settings_dirty(state: &mut LevelEditorState) {
    state.scene.has_unsaved_changes = true;
    state.scene.revision = state.scene.revision.wrapping_add(1);
}

/// Adapter boundary between the panel and the project's trait index: the
/// implementers that can fill each enabled role (those running as a script
/// class, which a role spawns), by role id, from one read of the index.
fn load_role_blueprints(
    project_root: &Path,
    rebuild: bool,
) -> Result<std::collections::BTreeMap<&'static str, Vec<String>>, String> {
    let index = if rebuild {
        pulsar_traits::TraitIndex::load_or_rebuild(project_root)
    } else {
        pulsar_traits::TraitIndex::load(project_root)
    };
    index
        .map(|index| {
            pulsar_traits::roles::roles()
                .into_iter()
                .map(|role| {
                    let implementers = index
                        .implementers_of(role.trait_id)
                        .into_iter()
                        .filter(|entry| {
                            matches!(
                                entry.binding,
                                Some(pulsar_traits::Binding::ScriptClass { .. })
                            )
                        })
                        .map(|entry| entry.id.clone())
                        .collect();
                    (role.id, implementers)
                })
                .collect()
        })
        .map_err(|error| format!("Could not load the trait index: {error:#}"))
}

fn is_blueprint_index_path(path: &Path) -> bool {
    path.to_string_lossy()
        .replace('\\', "/")
        .ends_with(pulsar_traits::TraitIndex::RELATIVE_PATH)
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
    trait_id: &str,
    state: &BlueprintCatalogState,
    selected: &str,
) -> String {
    match state {
        BlueprintCatalogState::Loading => format!("Loading {role_label} implementers…"),
        BlueprintCatalogState::Ready(paths) if paths.is_empty() => format!(
            "Nothing implements `{trait_id}`. Add that trait to a Blueprint to make it selectable."
        ),
        BlueprintCatalogState::Ready(paths)
            if selected != "None" && !paths.iter().any(|path| path == selected) =>
        {
            format!("The selected implementer is not in the current index: {selected}")
        }
        BlueprintCatalogState::Ready(_) => {
            format!("Select a Blueprint that implements the {role_label} trait.")
        }
        BlueprintCatalogState::Failed(error) => error.clone(),
    }
}

fn catalog_status(
    role_label: &str,
    trait_id: &str,
    state: &BlueprintCatalogState,
    selected: &str,
    cx: &Context<WorldSettingsPanel>,
) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(catalog_message(role_label, trait_id, state, selected))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{blueprint_label, catalog_message, load_role_blueprints, BlueprintCatalogState};
    use pulsar_traits::builtin::{
        GAME_MODE_ROLE, GAME_STATE_ROLE, PLAYER_CHARACTER_ROLE, PLAYER_CONTROLLER_ROLE,
    };

    /// The roles list what the project's index says implements their traits;
    /// the shell has no implementer kind of its own, so this fixture is an
    /// index stored as the editor's Blueprint source would have stored it.
    #[test]
    fn each_role_lists_only_script_classes_implementing_its_trait() {
        let dir = tempfile::tempdir().unwrap();
        let entry = |class: &str, ids: &[&str]| {
            serde_json::json!({
                "id": format!("classes/{class}/graph_save.json"),
                "kind": "pulsar.blueprints.blueprint",
                "implemented_trait_ids": ids,
                "binding": { "kind": "script_class", "dir": format!("classes/{class}") },
            })
        };
        let stored = serde_json::json!({
            "schema_version": 3,
            "traits": [],
            "implementers": [
                entry("Both", &["pulsar.GameMode", "pulsar.GameState"]),
                entry("Hero", &["pulsar.PlayerCharacter"]),
                entry("Mode", &["pulsar.GameMode"]),
            ],
        });
        engine_fs::virtual_fs::write_file(
            &dir.path().join(pulsar_traits::TraitIndex::RELATIVE_PATH),
            stored.to_string().as_bytes(),
        )
        .unwrap();
        let roles = load_role_blueprints(dir.path(), false).unwrap();
        assert_eq!(
            roles[GAME_MODE_ROLE],
            [
                "classes/Both/graph_save.json",
                "classes/Mode/graph_save.json"
            ]
        );
        assert_eq!(
            roles[PLAYER_CHARACTER_ROLE],
            ["classes/Hero/graph_save.json"]
        );
        assert_eq!(roles[GAME_STATE_ROLE], ["classes/Both/graph_save.json"]);
        assert!(roles[PLAYER_CONTROLLER_ROLE].is_empty());
    }

    #[test]
    fn labels_and_status_messages_name_the_role() {
        assert_eq!(
            blueprint_label(&"classes/Mode/graph_save.json".to_string()),
            "Mode"
        );
        assert_eq!(blueprint_label(&"a/b.blueprint.json".to_string()), "a/b");
        let empty = BlueprintCatalogState::Ready(Vec::new());
        let message = catalog_message(
            "Player Controller",
            "pulsar.PlayerController",
            &empty,
            "None",
        );
        assert!(message.contains("pulsar.PlayerController"));
        let ready = BlueprintCatalogState::Ready(vec!["a".into()]);
        assert!(
            catalog_message("Game State", "t", &ready, "gone").contains("not in the current index")
        );
    }
}
