//! The plugins' World Settings fields ([`crate::extend::WorldSettingField`]):
//! one card per section, each field edited by its kind and stored in the
//! level's `world_settings` under its key (`WorldSettingsData::values`). The
//! input plugin's player input mapping and the physics plugin's gravity are
//! such fields; the shell names neither.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::{prelude::*, *};
use serde_json::{json, Value};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::{SearchableList, SearchableListEvent},
    h_flex,
    input::{InputEvent, InputState, NumberInput},
    popover::Popover,
    v_flex, ActiveTheme, Sizable,
};

use super::{mark_world_settings_dirty, BlueprintCatalogState, WorldSettingsPanelImpl};
use crate::extend::{WorldSettingField, WorldSettingKind};
use crate::state::LevelEditorState;
use crate::world_settings_data::WorldSettingsData;
use crate::WorldSettingsPanel;

/// One field's widgets.
pub(super) enum FieldWidget {
    Toggle,
    Number(Entity<InputState>),
    Vector3([Entity<InputState>; 3]),
    AssetPath {
        picker: Entity<SearchableList<String>>,
        catalog: BlueprintCatalogState,
    },
}

/// A registered field and its widgets.
pub(super) struct FieldState {
    pub field: &'static WorldSettingField,
    pub widget: FieldWidget,
}

/// The card key of a section's fields.
pub(super) fn section_key(section: &str) -> String {
    format!("fields:{section}")
}

/// The sections that have fields, in order.
pub(super) fn sections(fields: &[FieldState]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for state in fields {
        if !out.contains(&state.field.section) {
            out.push(state.field.section);
        }
    }
    out
}

/// A number field's value: stored, else its default.
fn number(settings: &WorldSettingsData, field: &WorldSettingField, default: f64) -> f64 {
    settings
        .value(field.key)
        .and_then(Value::as_f64)
        .unwrap_or(default)
}

/// A three-number field's value: stored, else its default.
fn vector3(settings: &WorldSettingsData, field: &WorldSettingField, default: [f64; 3]) -> [f64; 3] {
    match settings.value(field.key).and_then(Value::as_array) {
        Some(items) if items.len() == 3 => {
            let mut out = default;
            for (slot, item) in out.iter_mut().zip(items) {
                if let Some(value) = item.as_f64() {
                    *slot = value;
                }
            }
            out
        }
        _ => default,
    }
}

fn toggle(settings: &WorldSettingsData, field: &WorldSettingField, default: bool) -> bool {
    settings
        .value(field.key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

/// Label for a project file path: its file name without the extension.
pub(super) fn asset_label(path: &String) -> String {
    if path == "None" {
        return "None".to_string();
    }
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(path)
        .to_string()
}

/// The project's files of these extensions, project-relative with `/`.
pub(super) fn list_assets(project_root: &Path, extensions: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = extensions
        .iter()
        .flat_map(|extension| engine_fs::virtual_fs::find_by_extension(project_root, extension))
        .map(|path| {
            // `find_by_extension` joins its results onto the root.
            let relative = path.strip_prefix(project_root).unwrap_or(&path);
            relative.to_string_lossy().replace('\\', "/")
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// What an asset field says under its picker.
pub(super) fn asset_message(
    state: &BlueprintCatalogState,
    selected: &str,
    extensions: &[&str],
    help: &str,
) -> String {
    let kinds = extensions
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(", ");
    match state {
        BlueprintCatalogState::Loading => format!("Loading the project's {kinds} files…"),
        BlueprintCatalogState::Ready(paths) if paths.is_empty() => {
            format!("No {kinds} file in the project. Create one to make it selectable.")
        }
        BlueprintCatalogState::Ready(paths)
            if selected != "None" && !paths.iter().any(|path| path == selected) =>
        {
            format!("The selected file was not found in the project: {selected}")
        }
        BlueprintCatalogState::Ready(_) => help.to_string(),
        BlueprintCatalogState::Failed(error) => error.clone(),
    }
}

fn numeric_input(
    value: f64,
    window: &mut Window,
    cx: &mut Context<WorldSettingsPanel>,
) -> Entity<InputState> {
    cx.new(|cx| {
        let mut input = InputState::new(window, cx).placeholder(format!("{value:.3}"));
        input.set_value(format!("{value:.3}"), window, cx);
        input
    })
}

/// Write one number of a field (`axis` for a three-number one) when the text
/// parses and is allowed; put the stored value back on blur otherwise.
fn watch_number(
    state: std::sync::Arc<parking_lot::RwLock<LevelEditorState>>,
    field: &'static WorldSettingField,
    axis: Option<usize>,
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<WorldSettingsPanel>,
) -> Subscription {
    let input_for_event = input.clone();
    cx.subscribe_in(
        input,
        window,
        move |_panel, _input, event: &InputEvent, window, cx| {
            if !matches!(event, InputEvent::Change | InputEvent::Blur) {
                return;
            }
            input_for_event.update(cx, |input, input_cx| {
                let parsed = input.text().to_string().trim().parse::<f64>().ok();
                let allowed = |value: f64| {
                    value.is_finite()
                        && match field.kind {
                            WorldSettingKind::Number { min: Some(min), .. } => value >= min,
                            _ => true,
                        }
                };
                let mut state = state.write();
                let settings = &mut state.scene.world_settings;
                match (parsed.filter(|value| allowed(*value)), field.kind, axis) {
                    (Some(value), WorldSettingKind::Number { .. }, _) => {
                        settings.set_value(field.key, Some(json!(value)));
                        mark_world_settings_dirty(&mut state);
                    }
                    (Some(value), WorldSettingKind::Vector3 { default, .. }, Some(axis)) => {
                        let mut current = vector3(settings, field, default);
                        current[axis] = value;
                        settings.set_value(field.key, Some(json!(current)));
                        mark_world_settings_dirty(&mut state);
                    }
                    _ if matches!(event, InputEvent::Blur) => {
                        let stored = match (field.kind, axis) {
                            (WorldSettingKind::Number { default, .. }, _) => {
                                number(settings, field, default)
                            }
                            (WorldSettingKind::Vector3 { default, .. }, Some(axis)) => {
                                vector3(settings, field, default)[axis]
                            }
                            _ => return,
                        };
                        input.set_value(format!("{stored:.3}"), window, input_cx);
                    }
                    _ => {}
                }
            });
        },
    )
}

impl WorldSettingsPanelImpl {
    /// Every enabled field's widgets, and the subscriptions that store what
    /// they edit.
    pub(super) fn build_fields(
        state: &std::sync::Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
        subscriptions: &mut Vec<Subscription>,
    ) -> Vec<FieldState> {
        let settings = state.read().scene.world_settings.clone();
        crate::extend::world_setting_fields()
            .into_iter()
            .map(|field| {
                let widget = match field.kind {
                    WorldSettingKind::Toggle { .. } => FieldWidget::Toggle,
                    WorldSettingKind::Number { default, .. } => {
                        let input = numeric_input(number(&settings, field, default), window, cx);
                        subscriptions.push(watch_number(
                            state.clone(),
                            field,
                            None,
                            &input,
                            window,
                            cx,
                        ));
                        FieldWidget::Number(input)
                    }
                    WorldSettingKind::Vector3 { default, .. } => {
                        let value = vector3(&settings, field, default);
                        let inputs = [0, 1, 2].map(|axis| {
                            let input = numeric_input(value[axis], window, cx);
                            subscriptions.push(watch_number(
                                state.clone(),
                                field,
                                Some(axis),
                                &input,
                                window,
                                cx,
                            ));
                            input
                        });
                        FieldWidget::Vector3(inputs)
                    }
                    WorldSettingKind::AssetPath { .. } => {
                        let picker = cx.new(|cx| {
                            SearchableList::<String>::new(
                                window,
                                cx,
                                vec!["None".to_string()],
                                asset_label,
                            )
                            .with_empty_text(format!("No {} files found", field.label))
                            .with_max_width(px(360.0))
                            .with_max_height(px(320.0))
                        });
                        let state = state.clone();
                        subscriptions.push(cx.subscribe(
                            &picker,
                            move |_panel, _picker, event: &SearchableListEvent<String>, cx| {
                                if let SearchableListEvent::Select(path) = event {
                                    let mut state = state.write();
                                    state.scene.world_settings.set_value(
                                        field.key,
                                        (path != "None").then(|| Value::String(path.clone())),
                                    );
                                    mark_world_settings_dirty(&mut state);
                                    cx.notify();
                                }
                            },
                        ));
                        FieldWidget::AssetPath {
                            picker,
                            catalog: BlueprintCatalogState::Loading,
                        }
                    }
                };
                FieldState { field, widget }
            })
            .collect()
    }

    /// List the project's files for every asset field, off the UI thread.
    pub(super) fn load_field_assets(&mut self, cx: &mut Context<WorldSettingsPanel>) {
        let wanted: BTreeMap<&'static str, &'static [&'static str]> = self
            .fields
            .iter()
            .filter_map(|state| match state.field.kind {
                WorldSettingKind::AssetPath { extensions, .. } => {
                    Some((state.field.id, extensions))
                }
                _ => None,
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let Some(root) = engine_state::get_project_path().map(PathBuf::from) else {
            for state in &mut self.fields {
                if let FieldWidget::AssetPath { catalog, .. } = &mut state.widget {
                    *catalog = BlueprintCatalogState::Failed(
                        "Open a project to browse its files.".to_string(),
                    );
                }
            }
            return;
        };
        cx.spawn(async move |this, cx| {
            let lists = cx
                .background_executor()
                .spawn(async move {
                    wanted
                        .into_iter()
                        .map(|(id, extensions)| (id, list_assets(&root, extensions)))
                        .collect::<BTreeMap<_, _>>()
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    for state in &mut panel.world_settings.fields {
                        let Some(paths) = lists.get(state.field.id) else {
                            continue;
                        };
                        if let FieldWidget::AssetPath { picker, catalog } = &mut state.widget {
                            picker.update(cx, |picker, cx| {
                                let mut items = vec!["None".to_string()];
                                items.extend(paths.iter().cloned());
                                picker.set_items(items, cx);
                            });
                            *catalog = BlueprintCatalogState::Ready(paths.clone());
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// List again whenever a file of an asset field's kind changes.
    pub(super) fn watch_field_assets(&self, cx: &mut Context<WorldSettingsPanel>) {
        let extensions: Vec<&'static str> = self
            .fields
            .iter()
            .filter_map(|state| match state.field.kind {
                WorldSettingKind::AssetPath { extensions, .. } => Some(extensions),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        if extensions.is_empty() {
            return;
        }
        let mut events = engine_fs::subscribe();
        cx.spawn(async move |this, cx| loop {
            let Ok(event) = events.recv().await else {
                continue;
            };
            let matches = event
                .path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| {
                    extensions
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(ext))
                });
            if !matches {
                continue;
            }
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    panel.world_settings.load_field_assets(cx);
                });
            });
        })
        .detach();
    }

    /// Bring the numeric inputs in step with the level (an undo, a load),
    /// except the one being typed in.
    pub(super) fn sync_fields(
        &self,
        settings: &WorldSettingsData,
        window: &mut Window,
        cx: &mut Context<WorldSettingsPanel>,
    ) {
        for state in &self.fields {
            match (&state.widget, state.field.kind) {
                (FieldWidget::Number(input), WorldSettingKind::Number { default, .. }) => {
                    let value = number(settings, state.field, default);
                    super::sync_numeric_input(input, format!("{value:.3}"), window, cx);
                }
                (FieldWidget::Vector3(inputs), WorldSettingKind::Vector3 { default, .. }) => {
                    let value = vector3(settings, state.field, default);
                    for (input, value) in inputs.iter().zip(value) {
                        super::sync_numeric_input(input, format!("{value:.3}"), window, cx);
                    }
                }
                _ => {}
            }
        }
    }

    /// One section's fields.
    pub(super) fn render_fields_section(
        &self,
        section: &str,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let settings = self.state.read().scene.world_settings.clone();
        let rows: Vec<AnyElement> = self
            .fields
            .iter()
            .filter(|state| state.field.section == section)
            .map(|state| self.render_field(state, &settings, cx))
            .collect();
        v_flex().gap_3().children(rows).into_any_element()
    }

    fn render_field(
        &self,
        state: &FieldState,
        settings: &WorldSettingsData,
        cx: &mut Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let field = state.field;
        let label = div()
            .flex_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(field.label);
        match (&state.widget, field.kind) {
            (FieldWidget::Toggle, WorldSettingKind::Toggle { default }) => {
                let value = toggle(settings, field, default);
                let level = self.state.clone();
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(label)
                    .child(
                        Button::new(format!("field-{}", field.id))
                            .label(if value { "On" } else { "Off" })
                            .small()
                            .when(value, |button| button.primary())
                            .on_click(cx.listener(move |_panel, _event, _window, cx| {
                                let mut state = level.write();
                                state
                                    .scene
                                    .world_settings
                                    .set_value(field.key, Some(Value::Bool(!value)));
                                mark_world_settings_dirty(&mut state);
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
            (FieldWidget::Number(input), WorldSettingKind::Number { unit, .. }) => h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(label)
                .child(NumberInput::new(input).xsmall())
                .child(unit_text(unit, cx))
                .into_any_element(),
            (FieldWidget::Vector3(inputs), WorldSettingKind::Vector3 { unit, .. }) => h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(label)
                .children(
                    ["X", "Y", "Z"]
                        .into_iter()
                        .zip(inputs)
                        .map(|(axis, input)| {
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
                        }),
                )
                .child(unit_text(unit, cx))
                .into_any_element(),
            (
                FieldWidget::AssetPath { picker, catalog },
                WorldSettingKind::AssetPath { extensions, help },
            ) => {
                let selected = settings
                    .value(field.key)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| "None".to_string());
                let picker = picker.clone();
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(label)
                            .child(
                                Popover::<SearchableList<String>>::new(format!(
                                    "field-{}",
                                    field.id
                                ))
                                .anchor(Corner::BottomRight)
                                .trigger(
                                    Button::new(format!("field-{}-trigger", field.id))
                                        .label(asset_label(&selected))
                                        .small()
                                        .ghost()
                                        .dropdown_caret(true),
                                )
                                .content(move |_window, _cx| picker.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(asset_message(catalog, &selected, extensions, help)),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }
}

fn unit_text(unit: &'static str, cx: &Context<WorldSettingsPanel>) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(unit)
}

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would pull in gpui's own `test` macro.
    use super::{asset_label, asset_message, list_assets, BlueprintCatalogState};

    #[test]
    fn only_files_of_the_fields_extensions_are_listed_with_project_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            "input/Game.inputmap",
            "Menu.INPUTMAP",
            "input/Jump.inputaction",
            "notes.txt",
        ] {
            let path = dir.path().join(file);
            engine_fs::virtual_fs::write_file(&path, b"{}").unwrap();
        }
        let listed = list_assets(dir.path(), &["inputmap"]);
        assert!(
            listed.contains(&"input/Game.inputmap".to_string()),
            "{listed:?}"
        );
        assert!(
            listed
                .iter()
                .all(|path| path.to_lowercase().ends_with(".inputmap")),
            "{listed:?}"
        );
        assert!(listed.iter().all(|path| !path.contains('\\')));
    }

    #[test]
    fn labels_and_messages_say_what_is_chosen() {
        assert_eq!(asset_label(&"input/Game.inputmap".to_string()), "Game");
        assert_eq!(asset_label(&"None".to_string()), "None");
        let empty = BlueprintCatalogState::Ready(Vec::new());
        assert!(asset_message(&empty, "None", &["inputmap"], "h").contains("No .inputmap file"));
        let ready = BlueprintCatalogState::Ready(vec!["a.inputmap".into()]);
        assert!(asset_message(&ready, "gone.inputmap", &["inputmap"], "h").contains("not found"));
        assert_eq!(
            asset_message(&ready, "a.inputmap", &["inputmap"], "help"),
            "help"
        );
    }
}
