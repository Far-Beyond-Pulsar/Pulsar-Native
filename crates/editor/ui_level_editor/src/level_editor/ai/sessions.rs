//! The open level editors the AI tools act on.
//!
//! AI tools never touch a level file: they edit the live `LevelEditorState` of
//! an open editor -- exactly what the user edits, shown live -- and only a
//! save (the user's, or `level_editor_save_scene`) writes the file.
//!
//! Editors register themselves once when their panel is built and are dropped
//! from here with it, so the registry never depends on keeping a path key in
//! sync through open / Save As / New Scene. A tool call's file path is only a
//! way to pick one of several open editors, matched against each editor's
//! *current* scene path.

use crate::level_editor::LevelEditorState;
use std::path::Path;
use std::sync::{Arc, LazyLock, RwLock, Weak};

type EditorState = Arc<parking_lot::RwLock<LevelEditorState>>;

static OPEN_EDITORS: LazyLock<RwLock<Vec<Weak<parking_lot::RwLock<LevelEditorState>>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Make an open editor reachable by the AI tools. Idempotent.
pub fn register_editor(state: &EditorState) {
    let mut editors = OPEN_EDITORS.write().unwrap_or_else(|p| p.into_inner());
    editors.retain(|weak| weak.strong_count() > 0);
    if !editors.iter().any(|weak| weak.as_ptr() == Arc::as_ptr(state)) {
        editors.push(Arc::downgrade(state));
    }
}

/// Forget an editor (its panel is closing).
pub fn unregister_editor(state: &EditorState) {
    let mut editors = OPEN_EDITORS.write().unwrap_or_else(|p| p.into_inner());
    editors.retain(|weak| weak.strong_count() > 0 && weak.as_ptr() != Arc::as_ptr(state));
}

/// Every live level editor, oldest first.
pub fn open_editors() -> Vec<EditorState> {
    let editors = OPEN_EDITORS.read().unwrap_or_else(|p| p.into_inner());
    editors.iter().filter_map(Weak::upgrade).collect()
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Describe an editor for messages: its level path, or that it's unsaved.
fn describe(state: &EditorState) -> String {
    match &state.read().scene.current_scene {
        Some(path) => path.display().to_string(),
        None => "<unsaved new level>".to_string(),
    }
}

/// The editor a tool call is for.
///
/// `requested` picks the editor showing that level. With no usable path --
/// none given, or one that isn't an existing file, e.g. for a level that was
/// never saved -- the call goes to the only open editor. Naming a real level
/// that isn't open, or leaving the choice ambiguous, is an error that lists
/// the open levels.
pub fn find_editor(requested: Option<&Path>) -> Result<EditorState, String> {
    let editors = open_editors();
    if editors.is_empty() {
        return Err(
            "No level is open in the Level Editor. Open one with open_file_in_default_editor."
                .to_string(),
        );
    }
    if let Some(requested) = requested {
        let matching = editors.iter().find(|state| {
            state
                .read()
                .scene
                .current_scene
                .as_deref()
                .is_some_and(|open| same_file(open, requested))
        });
        if let Some(state) = matching {
            return Ok(state.clone());
        }
    }
    let names_a_real_file = requested.is_some_and(Path::is_file);
    if editors.len() == 1 && !names_a_real_file {
        return Ok(editors[0].clone());
    }
    let open: Vec<String> = editors.iter().map(describe).collect();
    Err(match requested.filter(|_| names_a_real_file) {
        Some(path) => format!(
            "{} is not open in the Level Editor. Open levels: {}. Open it with \
             open_file_in_default_editor, or pass the path of an open level.",
            path.display(),
            open.join(", ")
        ),
        None => format!(
            "Several levels are open; pass the file path of one: {}",
            open.join(", ")
        ),
    })
}
