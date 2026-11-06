//! The user's overrides for the running project, shared by every window.
//!
//! [`apply_project_keybindings`] is the startup entry point: it reads
//! `<project>/.pulsar/keybindings.json` and binds it over the defaults. Call it
//! once the defaults (the editor's own and the plugins') are bound.

use std::path::{Path, PathBuf};

use gpui::{App, Global};

use super::file;
use super::overrides::Override;
use super::registry;

#[derive(Default)]
pub struct KeybindingStore {
    /// The project the overrides belong to.
    pub project: Option<PathBuf>,
    pub overrides: Vec<Override>,
    /// Problems in the file or in binding it, for the window to show.
    pub warnings: Vec<String>,
    /// Why edits must not be saved, if so.
    pub read_only: Option<String>,
}

impl Global for KeybindingStore {}

impl KeybindingStore {
    /// The store for the project the window is working on, loaded if another
    /// project (or none) was current.
    pub fn for_project<'a>(project: Option<&Path>, cx: &'a mut App) -> &'a mut KeybindingStore {
        let stale = cx
            .try_global::<KeybindingStore>()
            .is_none_or(|store| store.project.as_deref() != project);
        if stale {
            match project {
                Some(root) => load(root, cx),
                None => cx.set_global(KeybindingStore::default()),
            }
        }
        cx.global_mut::<KeybindingStore>()
    }

    /// Write the overrides to the project and bind them, returning why not on failure.
    pub fn save_and_apply(&mut self, cx: &mut App) -> Result<(), String> {
        if let Some(reason) = &self.read_only {
            return Err(reason.clone());
        }
        let Some(root) = self.project.clone() else {
            return Err("Open a project to keep keybindings".to_owned());
        };
        file::save(&root, &self.overrides)
            .map_err(|error| format!("could not save {}: {error}", file::FILE_NAME))?;
        self.warnings = registry::apply(&self.overrides, cx);
        Ok(())
    }
}

fn load(root: &Path, cx: &mut App) {
    let mut loaded = file::load(root);
    let mut kept = Vec::with_capacity(loaded.overrides.len());
    let mut dropped = false;
    for mut entry in loaded.overrides {
        let keystrokes = registry::normalize_keystrokes(&entry.keystrokes);
        let context = match entry.context.as_deref() {
            Some(context) => registry::normalize_context(context),
            None => Ok(None),
        };
        match (keystrokes, context) {
            (Ok(keystrokes), Ok(context)) => {
                entry.keystrokes = keystrokes;
                entry.context = context;
                kept.push(entry);
            }
            (Err(error), _) | (_, Err(error)) => {
                dropped = true;
                loaded
                    .warnings
                    .push(format!("{} was skipped: {error}", entry.keystrokes));
            }
        }
    }
    if dropped {
        file::keep_copy(&file::keybindings_path(root));
    }
    let mut store = KeybindingStore {
        project: Some(root.to_path_buf()),
        overrides: kept,
        warnings: loaded.warnings,
        read_only: loaded.read_only,
    };
    store.warnings.extend(registry::apply(&store.overrides, cx));
    cx.set_global(store);
}

/// Read the project's keybinding file and bind it over the defaults.
pub fn apply_project_keybindings(project_root: &Path, cx: &mut App) {
    load(project_root, cx);
    let store = cx.global::<KeybindingStore>();
    for warning in &store.warnings {
        tracing::warn!("keybindings: {warning}");
    }
    tracing::info!(
        overrides = store.overrides.len(),
        "keybindings: applied {}",
        file::keybindings_path(project_root).display()
    );
}
