use serde_json::json;
use std::sync::{LazyLock, RwLock};

#[derive(Clone, Debug)]
pub struct OpenEditorInfo {
    pub index: usize,
    pub panel_name: String,
    pub tab_name: String,
    pub is_active: bool,
}

static OPEN_EDITORS: LazyLock<RwLock<Vec<OpenEditorInfo>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

pub fn set_snapshot(snapshot: Vec<OpenEditorInfo>) {
    if let Ok(mut lock) = OPEN_EDITORS.write() {
        *lock = snapshot;
    }
}

pub fn snapshot_json() -> serde_json::Value {
    let editors = OPEN_EDITORS
        .read()
        .map(|items| items.clone())
        .unwrap_or_default();

    let active_index = editors.iter().find(|e| e.is_active).map(|e| e.index);

    json!({
        "ok": true,
        "open_count": editors.len(),
        "active_index": active_index,
        "editors": editors
            .into_iter()
            .map(|editor| {
                json!({
                    "index": editor.index,
                    "panel_name": editor.panel_name,
                    "tab_name": editor.tab_name,
                    "is_active": editor.is_active,
                })
            })
            .collect::<Vec<_>>(),
    })
}
