//! The editor shell's MCP tools: the ones that need the main window's tab area.
//!
//! `editor_open_file` opens a file in its default editor tab, as double-clicking
//! it in the Content browser does (the same `create_editor_for_file` and
//! `add_panel` the chat panel's `open_file_in_default_editor` tool uses), so an
//! AI client can put an asset in front of the person.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AnyWindowHandle, App, AppContext as _, Entity, Global};
use pulsar_mcp::{Annotations, McpToolRegistration, ToolResult};
use serde_json::{Value, json};
use ui::dock::TabPanel;

/// Where an opened file's editor goes: the main window's center tab area. Set
/// once the app is built.
struct OpenTarget {
    tabs: Entity<TabPanel>,
    window: AnyWindowHandle,
}

impl Global for OpenTarget {}

/// Remember the tab area files open into.
pub(super) fn set_open_target(tabs: Entity<TabPanel>, window: AnyWindowHandle, cx: &mut App) {
    cx.set_global(OpenTarget { tabs, window });
}

fn open_file(arguments: &str) -> ToolResult {
    let arguments: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    let Some(path) = arguments.get("path").and_then(Value::as_str) else {
        return ToolResult::error("`path` is required: a file path, absolute or relative to the project");
    };
    let mut path = PathBuf::from(path);
    if path.is_relative() {
        match engine_state::get_project_path() {
            Some(root) => path = PathBuf::from(root).join(path),
            None => return ToolResult::error("no project is open to resolve a relative path against"),
        }
    }
    // A plugin's mounted class (`plugins/Gameplay/...`) is virtual: it exists to the
    // editor's file system, not necessarily to the disk.
    if !engine_fs::virtual_fs::exists(&path).unwrap_or(false) {
        return ToolResult::error(format!("{} does not exist", path.display()));
    }
    let shown = path.display().to_string();
    let project = engine_state::get_project_path().map(PathBuf::from);
    let opened = ui_common::ui_call::call(Duration::from_secs(60), move |cx| {
        let (tabs, window) = {
            let target = cx
                .try_global::<OpenTarget>()
                .ok_or_else(|| "the editor window is not ready".to_owned())?;
            (target.tabs.clone(), target.window)
        };
        cx.update_window(window, |_root, window, cx| {
            let manager = plugin_manager::global()
                .ok_or_else(|| "the plugin manager is not available".to_owned())?;
            let mut manager = manager.write();
            manager.set_project_root(project);
            super::refresh_plugin_editor_settings(&mut manager);
            let panel = manager
                .create_editor_for_file(&path, window, cx)
                .map_err(|error| error.to_string())?;
            tabs.update(cx, |tabs, cx| tabs.add_panel(panel, window, cx));
            Ok::<(), String>(())
        })
        .map_err(|error| error.to_string())?
    });
    match opened {
        Ok(Ok(())) => ToolResult::json(&json!({ "opened": shown })),
        Ok(Err(message)) | Err(message) => {
            ToolResult::error(format!("could not open {shown}: {message}"))
        }
    }
}

pulsar_plugins::inventory::submit! {
    McpToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        name: "editor_open_file",
        title: "Open a file in its editor",
        description: "Open a file in the editor that handles its type, in a tab of the main window, as double-clicking it in the Content browser does: a .class folder opens in the Blueprint editor, a .level in the level editor, a material in the shader editor. The path is absolute or relative to the project; a plugin's mounted files (plugins/Gameplay/classes/PlayerCharacter.class) are valid. Shows the file to the person.",
        input_schema: r#"{"type":"object","properties":{"path":{"type":"string","description":"The file or class folder to open, absolute or project-relative (e.g. classes/Hero.class or plugins/Gameplay/classes/PlayerCharacter.class)."}},"required":["path"]}"#,
        annotations: Annotations::WRITES_IDEMPOTENT,
        call: open_file,
    }
}
