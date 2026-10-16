//! The level editor's own menu strip (Edit / Create / Assets / View / Go /
//! Project).
//!
//! These sit in the level editor's top bar, separate from the app-wide menus in
//! the title bar (Pulsar-Native#1012). Adding a menu here is one more
//! [`EditorMenu`] in [`LevelEditorMenus::render`]; nothing in the app shell
//! needs to know about it.

use std::collections::BTreeMap;

use gpui::*;
use plugin_editor_api::{EditorMenu, EditorMenuStrip, FileTypeDefinition};
use rust_i18n::t;
use ui_common::menu::{ImportAssets, NewAsset};

use crate::ui::actions::{
    AddObjectOfType, DeleteObject, DuplicateObject, FocusSelected, NewScene, OpenScene, SaveScene,
    SaveSceneAs, ToggleCameraModeSelector, ToggleGrid, ToggleViewportOptions,
};
use crate::LevelEditorState;

pub struct LevelEditorMenus;

impl LevelEditorMenus {
    pub fn render(state: &LevelEditorState) -> impl IntoElement {
        let selected = state.scene.selected_object().is_some();
        EditorMenuStrip::new("level-editor-menus")
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.Edit"))
                    .undo_redo(
                        t!("LevelEditor.Menu.Undo"),
                        t!("LevelEditor.Menu.Redo"),
                        Some(state.scene.history().clone() as _),
                    )
                    .separator()
                    .action_if(t!("LevelEditor.Menu.Duplicate"), DuplicateObject, selected)
                    .action_if(t!("LevelEditor.Menu.Delete"), DeleteObject, selected),
            )
            .menu(Self::create_menu())
            .menu(Self::assets_menu(&creatable_file_types()))
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.View"))
                    .action(t!("LevelEditor.Menu.ToggleGrid"), ToggleGrid)
                    .separator()
                    .action("Viewport controls", ToggleViewportOptions)
                    .action("Camera controls", ToggleCameraModeSelector),
            )
            .menu(EditorMenu::new(t!("LevelEditor.Menu.Go")).action_if(
                t!("LevelEditor.Menu.FocusSelected"),
                FocusSelected,
                selected,
            ))
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.Project"))
                    .action(t!("LevelEditor.Menu.NewScene"), NewScene)
                    .action(t!("LevelEditor.Menu.OpenScene"), OpenScene)
                    .separator()
                    .action(t!("LevelEditor.Menu.SaveScene"), SaveScene)
                    .action(t!("LevelEditor.Menu.SaveSceneAs"), SaveSceneAs),
            )
    }

    /// Assets: New, with an item for every file type that can be created,
    /// grouped by the type's categories as the file manager's New submenu
    /// groups them, and Import. The app shell hands both to the file manager,
    /// which creates or copies into the folder it lists.
    fn assets_menu(file_types: &[FileTypeDefinition]) -> EditorMenu {
        let mut new_menu = EditorMenu::new(t!("LevelEditor.Menu.New"));
        let mut categories: BTreeMap<&str, Vec<&FileTypeDefinition>> = BTreeMap::new();
        let mut subcategories: BTreeMap<(&str, &str), Vec<&FileTypeDefinition>> = BTreeMap::new();
        let mut top_level: Vec<&FileTypeDefinition> = Vec::new();
        for file_type in file_types {
            match file_type.categories.as_slice() {
                [] => top_level.push(file_type),
                [category] => categories
                    .entry(category.as_str())
                    .or_default()
                    .push(file_type),
                [category, subcategory, ..] => subcategories
                    .entry((category.as_str(), subcategory.as_str()))
                    .or_default()
                    .push(file_type),
            }
        }
        for (category, _) in subcategories.keys() {
            categories.entry(category).or_default();
        }
        let item = |menu: EditorMenu, file_type: &FileTypeDefinition| {
            menu.action(
                file_type.display_name.clone(),
                NewAsset {
                    file_type_id: file_type.id.as_str().to_string(),
                },
            )
        };
        top_level.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        for file_type in top_level {
            new_menu = item(new_menu, file_type);
        }
        for (category, mut types) in categories {
            let mut category_menu = EditorMenu::new(category.to_string());
            types.sort_by(|a, b| a.display_name.cmp(&b.display_name));
            for file_type in types {
                category_menu = item(category_menu, file_type);
            }
            for ((_, subcategory), types) in subcategories
                .iter_mut()
                .filter(|((parent, _), _)| *parent == category)
            {
                let mut subcategory_menu = EditorMenu::new(subcategory.to_string());
                types.sort_by(|a, b| a.display_name.cmp(&b.display_name));
                for file_type in types.iter().copied() {
                    subcategory_menu = item(subcategory_menu, file_type);
                }
                category_menu = category_menu.submenu(subcategory_menu);
            }
            new_menu = new_menu.submenu(category_menu);
        }
        EditorMenu::new(t!("LevelEditor.Menu.Assets"))
            .submenu(new_menu)
            .separator()
            .action(t!("LevelEditor.Menu.Import"), ImportAssets)
    }

    /// Create: what used to be the global Game Engine menu's objects and
    /// lights, each added at the origin as one undo step and selected.
    fn create_menu() -> EditorMenu {
        let mut menu = EditorMenu::new(t!("LevelEditor.Menu.Create"));
        let mut last_group = None;
        for &(kind, label) in crate::scene_edit::create::KINDS {
            // Separators between objects, lights, meshes and environment.
            let this_group = match kind.split('_').next() {
                Some("light") => "light",
                Some("mesh") => "mesh",
                Some("sky" | "wind") => "environment",
                _ => "object",
            };
            if last_group.is_some_and(|last| last != this_group) {
                menu = menu.separator();
            }
            last_group = Some(this_group);
            menu = menu.action(
                t!(label),
                AddObjectOfType {
                    object_type: kind.to_string(),
                },
            );
        }
        menu
    }
}

/// The registered file types an Assets menu can create.
fn creatable_file_types() -> Vec<FileTypeDefinition> {
    let Some(plugins) = plugin_manager::global() else {
        return Vec::new();
    };
    // `try_read`: this runs while the editor is being built, and the caller
    // that is building it (layout restore, opening a file) holds the manager's
    // write lock, so a blocking read would wait on this very thread forever.
    // The toolbar signature counts these types, so the menu is rebuilt with the
    // full list on the next frame.
    let Some(plugins) = plugins.try_read() else {
        return Vec::new();
    };
    plugins
        .file_type_registry()
        .get_all_file_types()
        .into_iter()
        .filter(|file_type| file_type.is_creatable())
        .cloned()
        .collect()
}

/// How many file types the Assets menu lists, for the toolbar's signature:
/// the menu is rebuilt when a plugin registers or drops one.
pub(crate) fn creatable_file_type_count() -> usize {
    let Some(plugins) = plugin_manager::global() else {
        return 0;
    };
    // `try_read`, see `creatable_file_types`: a blocking read deadlocks when the
    // toolbar is built under the manager's write lock. A contended count of 0
    // differs from the real one, so the signature changes and the menu is
    // rebuilt next frame.
    let Some(plugins) = plugins.try_read() else {
        return 0;
    };
    plugins
        .file_type_registry()
        .get_all_file_types()
        .into_iter()
        .filter(|file_type| file_type.is_creatable())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_editor_api::{EditorMenuItem, FileStructure, FileTypeId};

    fn file_type(id: &str, name: &str, categories: &[&str]) -> FileTypeDefinition {
        FileTypeDefinition {
            id: FileTypeId::new(id),
            extension: id.into(),
            display_name: name.into(),
            icon: ui::IconName::Cube,
            color: gpui::Hsla::default(),
            structure: FileStructure::Standalone,
            default_content: serde_json::Value::Null,
            categories: categories.iter().map(|c| c.to_string()).collect(),
            creation_directory: None,
        }
    }

    /// Labels of `menu`'s items, submenus as `label/`, and the file type
    /// each New item creates.
    fn outline(menu: &EditorMenu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| match item {
                EditorMenuItem::Action { label, action, .. } => {
                    match action.as_any().downcast_ref::<NewAsset>() {
                        Some(new) => format!("{label}={}", new.file_type_id),
                        None => label.to_string(),
                    }
                }
                EditorMenuItem::Submenu(menu) => format!("{}/", menu.label),
                EditorMenuItem::Separator => "-".into(),
                EditorMenuItem::UndoRedo { .. } => "undo/redo".into(),
            })
            .collect()
    }

    fn submenu<'a>(menu: &'a EditorMenu, label: &str) -> &'a EditorMenu {
        menu.items
            .iter()
            .find_map(|item| match item {
                EditorMenuItem::Submenu(menu) if menu.label == label => Some(menu),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label} submenu"))
    }

    #[::core::prelude::v1::test]
    fn new_lists_each_file_type_under_its_categories_as_the_file_manager_does() {
        let menu = LevelEditorMenus::assets_menu(&[
            file_type("vfx", "Particles", &["Effects", "Visual"]),
            file_type("mat", "Material", &["Rendering"]),
            file_type("txt", "Text", &[]),
            file_type("cls", "Class", &[]),
            file_type("shd", "Shader", &["Rendering"]),
            file_type("snd", "Sound Cue", &["Effects"]),
        ]);
        assert_eq!(outline(&menu)[1..], ["-".to_string(), "Import…".into()]);

        let new = submenu(&menu, &menu_label(&menu, 0));
        assert_eq!(
            outline(new),
            ["Class=cls", "Text=txt", "Effects/", "Rendering/"]
        );
        assert_eq!(
            outline(submenu(new, "Rendering")),
            ["Material=mat", "Shader=shd"]
        );
        let effects = submenu(new, "Effects");
        assert_eq!(outline(effects), ["Sound Cue=snd", "Visual/"]);
        assert_eq!(outline(submenu(effects, "Visual")), ["Particles=vfx"]);
    }

    fn menu_label(menu: &EditorMenu, index: usize) -> String {
        match &menu.items[index] {
            EditorMenuItem::Submenu(menu) => menu.label.to_string(),
            _ => panic!("item {index} is not a submenu"),
        }
    }
}
