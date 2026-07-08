//! # Pulsar Editor Plugin API
//!
//! This crate defines the core API for creating editor plugins that can be dynamically
//! loaded by the Pulsar engine. Plugins are compiled as dynamic libraries (.dll/.so/.dylib)
//! and loaded from the `plugins/editor/` directory at runtime.
//!
//! ## Architecture
//!
//! This crate follows **GPUI's element lifecycle pattern**, separating concerns into:
//!
//! - **One-time initialization** (editor creation / `EditorHandle::init`): Set up state,
//!   spawn child entities, register subscriptions. Runs once when the panel is created.
//!
//! - **Per-frame rendering** (`EditorHandle::render_frame`): Build the ephemeral element
//!   tree each frame. The tree flows through GPUI's `request_layout` → `prepaint` →
//!   `paint` lifecycle each frame, then is dropped. Persistent state lives in the
//!   editor handle across frames.
//!
//! This prevents the entire editor from being reconstructed every frame — the GPUI
//! view/entity persists, only the element tree is rebuilt per frame.
//!
//! ## Safety Model
//!
//! This plugin system eliminates undefined behavior through **permanent library loading**:
//!
//! - **Plugins are loaded once and NEVER unloaded**
//! - Function pointers, vtables, and drop glue remain valid for process lifetime
//! - Safe to share `Arc<T>`, trait objects, and function pointers across boundary
//! - No complex weak reference workarounds needed
//!
//! ## Module Organization
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`identifiers`] | `PluginId`, `FileTypeId`, `EditorId` |
//! | [`version`] | `VersionInfo`, compile-time version hashing |
//! | [`metadata`] | `PluginMetadata`, `EditorMetadata` |
//! | [`file_types`] | `FileTypeDefinition`, `FileStructure`, `PathTemplate` |
//! | [`error`] | `PluginError` type |
//! | [`statusbar`] | Statusbar button definitions |
//! | [`actions`] | `OpenAsset` action |
//! | [`ai`] | `AiToolDefinition`, `FsContext` |
//! | [`components`] | `ComponentDefinition`, `EditorPluginComponents` |
//! | [`subsystems`] | `EditorPluginSubsystems`, `Subsystem` re-exports |
//! | [`plugin`] | `EditorPlugin` trait, `export_plugin!` macro |
//! | [`editor_element`] | `EditorHandle`, `EditorElement` — init vs render lifecycle |
//! | [`helpers`] | `standalone_file_type()`, `folder_file_type()` |
//!
//! ## Creating a Plugin
//!
//! 1. Create a new crate with `crate-type = ["cdylib"]`
//! 2. Add dependency on `plugin_editor_api` (same version as engine!)
//! 3. Implement the `EditorPlugin` trait
//! 4. Use the `export_plugin!` macro to export your plugin
//!
//! ### With Init/Render Lifecycle (GPUI Pattern)
//!
//! ```rust,ignore
//! use plugin_editor_api::*;
//! use plugin_editor_api::editor_element::*;
//!
//! struct MyEditor { /* persistent state */ }
//!
//! impl EditorHandle for MyEditor {
//!     fn init(&mut self, editor_id: &EditorId, file_path: &Path,
//!             window: &mut Window, cx: &mut App) -> Result<(), PluginError> {
//!         // One-time setup (runs once)
//!         Ok(())
//!     }
//!
//!     fn render_frame(&self, ctx: &mut EditorFrameCtx) -> AnyElement {
//!         // Per-frame element tree (runs every frame)
//!         div().child("Hello").into_any_element()
//!     }
//! }
//! ```

// ── Modules ──────────────────────────────────────────────────────────────────

pub mod actions;
pub mod ai;
pub mod asset_payload;
pub mod components;
pub mod editor_element;
pub mod error;
pub mod file_types;
pub mod helpers;
pub mod identifiers;
pub mod metadata;
pub mod plugin;
pub mod statusbar;
pub mod subsystems;
pub mod version;

// ── Re-exports for plugin convenience ────────────────────────────────────────
//
// Every public type from each submodule is re-exported at the crate root so
// consumers can write `use plugin_editor_api::*` or `use plugin_editor_api::Foo`
// without knowing the internal module structure.  This also ensures that the
// `export_plugin!` macro, which references types via `$crate::Name`, resolves
// correctly.

pub use actions::*;
pub use ai::*;
pub use asset_payload::*;
pub use components::*;
pub use editor_element::*;
pub use error::*;
pub use file_types::*;
pub use helpers::*;
pub use identifiers::*;
pub use metadata::*;
pub use plugin::*;
pub use statusbar::*;
pub use subsystems::*;
pub use version::*;

/// Re-export GPUI's core types for plugin use.
/// The `export_plugin!` macro references these via `$crate::Window`, `$crate::App`.
pub use gpui::{App, Window};

/// Re-export UI dock types for plugin use.
/// The `export_plugin!` macro references `$crate::PanelView`.
pub use ui::dock::{Panel, PanelView};

/// Re-export serde_json::Value for plugin use.
/// The `export_plugin!` macro references `$crate::JsonValue`.
pub use serde_json::Value as JsonValue;
