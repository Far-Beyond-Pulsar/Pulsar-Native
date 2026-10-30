//! The input mapping the player controller possesses its pawn with: an
//! `.inputmap` of the project, chosen beside the gameplay roles in World
//! Settings. Possession puts it on the character (`.agents/TRAITS.md`,
//! "Input and possession").

use std::path::Path;

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    dropdown::SearchableList,
    h_flex,
    popover::Popover,
    v_flex, ActiveTheme, Sizable,
};

use super::{mark_world_settings_dirty, BlueprintCatalogState, WorldSettingsPanelImpl};
use crate::WorldSettingsPanel;

/// File extension of an input mapping asset.
const MAPPING_EXTENSION: &str = "inputmap";

/// Label for a mapping path: its file name without the extension.
pub(super) fn mapping_label(path: &String) -> String {
    if path == "None" {
        return "None".to_string();
    }
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(path)
        .to_string()
}

/// The project's `.inputmap` files, project-relative with `/` separators.
pub(super) fn load_input_mappings(project_root: &Path) -> Vec<String> {
    let mut mappings: Vec<String> =
        engine_fs::virtual_fs::find_by_extension(project_root, MAPPING_EXTENSION)
            .into_iter()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .collect();
    mappings.sort();
    mappings
}

/// What the row says under the picker.
pub(super) fn mapping_message(state: &BlueprintCatalogState, selected: &str) -> String {
    match state {
        BlueprintCatalogState::Loading => "Loading input mappings…".to_string(),
        BlueprintCatalogState::Ready(paths) if paths.is_empty() => {
            "No input mapping in the project. Create an .inputmap to make one selectable."
                .to_string()
        }
        BlueprintCatalogState::Ready(paths)
            if selected != "None" && !paths.iter().any(|path| path == selected) =>
        {
            format!("Selected input mapping was not found in the project: {selected}")
        }
        BlueprintCatalogState::Ready(_) => {
            "The keys, mouse and pad the Player Character hears once the Player Controller possesses it."
                .to_string()
        }
        BlueprintCatalogState::Failed(error) => error.clone(),
    }
}

impl WorldSettingsPanelImpl {
    /// Read the project's mappings off the UI thread and give them to the
    /// picker.
    pub(super) fn load_input_mappings(&mut self, cx: &mut Context<WorldSettingsPanel>) {
        let Some(root) = engine_state::get_project_path().map(std::path::PathBuf::from) else {
            self.mapping_catalog = BlueprintCatalogState::Failed(
                "Open a project to browse its input mappings.".to_string(),
            );
            return;
        };
        let picker = self.mapping_picker.clone();
        cx.spawn(async move |this, cx| {
            let paths = cx
                .background_executor()
                .spawn(async move { load_input_mappings(&root) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    picker.update(cx, |picker, cx| {
                        let mut items = vec!["None".to_string()];
                        items.extend(paths.iter().cloned());
                        picker.set_items(items, cx);
                    });
                    panel.world_settings.mapping_catalog = BlueprintCatalogState::Ready(paths);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    /// Reload the list whenever an `.inputmap` is created, moved or removed.
    pub(super) fn watch_input_mappings(&self, cx: &mut Context<WorldSettingsPanel>) {
        let mut events = engine_fs::subscribe();
        cx.spawn(async move |this, cx| loop {
            let Ok(event) = events.recv().await else {
                continue;
            };
            if event.path.extension().and_then(|e| e.to_str()) != Some(MAPPING_EXTENSION) {
                continue;
            }
            cx.update(|cx| {
                let _ = this.update(cx, |panel, cx| {
                    panel.world_settings.load_input_mappings(cx);
                });
            });
        })
        .detach();
    }

    /// Stores the picked mapping in the level; "None" clears it.
    pub(super) fn select_input_mapping(
        &mut self,
        path: &str,
        cx: &mut Context<WorldSettingsPanel>,
    ) {
        let mut state = self.state.write();
        state
            .scene
            .world_settings
            .set_player_input_mapping((path != "None").then(|| path.to_string()));
        mark_world_settings_dirty(&mut state);
        cx.notify();
    }

    /// The row: its name, the mapping chosen and, below, whether that choice
    /// is available in the project.
    pub(super) fn render_input_mapping_row(
        &self,
        selected: Option<&str>,
        cx: &Context<WorldSettingsPanel>,
    ) -> AnyElement {
        let selected = selected.map(str::to_owned).unwrap_or_else(|| "None".to_string());
        let display = mapping_label(&selected);
        let picker = self.mapping_picker.clone();
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
                            .child("Player Input Mapping"),
                    )
                    .child(
                        Popover::<SearchableList<String>>::new("player-input-mapping")
                            .anchor(Corner::BottomRight)
                            .trigger(
                                Button::new("player-input-mapping-trigger")
                                    .label(display)
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
                    .child(mapping_message(&self.mapping_catalog, selected.as_str())),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_inputmap_files_are_listed_with_project_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["input/Game.inputmap", "Menu.INPUTMAP", "input/Jump.inputaction", "notes.txt"] {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "{}").unwrap();
        }
        let listed = load_input_mappings(dir.path());
        assert!(listed.contains(&"input/Game.inputmap".to_string()), "{listed:?}");
        assert!(listed.iter().all(|path| path.to_lowercase().ends_with(".inputmap")), "{listed:?}");
        assert!(listed.iter().all(|path| !path.contains('\\')));
    }

    #[test]
    fn labels_and_messages_say_what_is_chosen() {
        assert_eq!(mapping_label(&"input/Game.inputmap".to_string()), "Game");
        assert_eq!(mapping_label(&"None".to_string()), "None");
        let empty = BlueprintCatalogState::Ready(Vec::new());
        assert!(mapping_message(&empty, "None").contains("No input mapping"));
        let ready = BlueprintCatalogState::Ready(vec!["a.inputmap".into()]);
        assert!(mapping_message(&ready, "gone.inputmap").contains("not found"));
        assert!(mapping_message(&ready, "a.inputmap").contains("Player Character"));
    }
}
