//! AI tool bridge for the level editor.
//!
//! Two halves, kept together because they serve one consumer (the editor's
//! AI tooling in `plugin_editor_api`):
//!
//! - [`sessions`] — registry of currently open scenes, keyed by normalized
//!   file path, so AI tools can find the `LevelEditorState` a `.level` file
//!   is being edited in.
//! - [`tools`] — the `AiToolDefinition` catalogue (`ai_tools`,
//!   `capabilities_for_file`, `execute_ai_tool`) that mutates those states
//!   through the ordinary `SceneCommand` path.

pub mod sessions;
pub mod tools;