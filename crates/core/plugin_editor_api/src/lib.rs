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
//! | [`asset_events`] | `AssetUpdated`: publish / subscribe to asset changes by kind |
//! | [`ai`] | `AiToolDefinition`, `FsContext` |
//! | [`components`] | `ComponentDefinition`, `EditorPluginComponents` |
//! | [`traits`] | `ProjectTraits`: query the project trait database (roles, implementers) |
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

#![warn(clippy::undocumented_unsafe_blocks)]

// ── Modules ──────────────────────────────────────────────────────────────────

#[cfg(feature = "editor")]
pub mod actions;
#[cfg(feature = "editor")]
pub mod ai;
pub mod asset_events;
#[cfg(feature = "editor")]
pub mod asset_payload;
pub mod component_events;
#[cfg(feature = "editor")]
pub mod components;
#[cfg(feature = "editor")]
pub mod editor_element;
pub mod error;
pub mod events;
pub mod file_types;
#[cfg(feature = "editor")]
pub mod helpers;
pub mod identifiers;
pub mod import_companion;
#[cfg(feature = "editor")]
pub mod llm;
pub mod manifest;
#[cfg(feature = "editor")]
pub mod menus;
pub mod metadata;
#[cfg(feature = "editor")]
pub mod panel_restore;
#[cfg(feature = "editor")]
pub mod plugin;
#[cfg(feature = "editor")]
pub mod read_only;
#[cfg(feature = "editor")]
pub mod registrar;
pub mod runtime;
pub mod scripting;
#[cfg(feature = "editor")]
pub mod statusbar;
#[cfg(feature = "editor")]
pub mod surface_animation;
pub mod traits;
#[cfg(feature = "editor")]
pub mod undo;
pub mod version;

// ── Re-exports for plugin convenience ────────────────────────────────────────
//
// Every public type from each submodule is re-exported at the crate root so
// consumers can write `use plugin_editor_api::*` or `use plugin_editor_api::Foo`
// without knowing the internal module structure.  This also ensures that the
// `export_plugin!` macro, which references types via `$crate::Name`, resolves
// correctly.

#[cfg(feature = "editor")]
pub use actions::*;
#[cfg(feature = "editor")]
pub use ai::*;
pub use asset_events::*;
#[cfg(feature = "editor")]
pub use asset_payload::*;
pub use component_events::*;
#[cfg(feature = "editor")]
pub use components::*;
#[cfg(feature = "editor")]
pub use editor_element::*;
pub use error::*;
pub use events::*;
pub use file_types::*;
#[cfg(feature = "editor")]
pub use helpers::*;
pub use identifiers::*;
pub use import_companion::*;
pub use manifest::{PluginDependency, PluginManifest};
#[cfg(feature = "editor")]
pub use menus::*;
pub use metadata::*;
#[cfg(feature = "editor")]
pub use panel_restore::*;
#[cfg(feature = "editor")]
pub use plugin::*;
#[cfg(feature = "editor")]
pub use registrar::{Plugin, PluginAdapter, PluginBuilder, Registration};
pub use runtime::*;
pub use scripting::*;
#[cfg(feature = "editor")]
pub use statusbar::*;
pub use traits::ProjectTraits;
pub use traits::{
    TraitDescriptor, TraitImplementer, TraitMethodDescriptor, TraitParamDescriptor, TraitSnapshot,
};
#[cfg(feature = "editor")]
pub use undo::{
    bind_undo_keys, DocumentId, History, HistoryView, Redo, SharedHistory, Undo, UndoEntry,
    UndoService,
};
pub use version::*;

/// Re-export GPUI's core types for plugin use.
/// The `export_plugin!` macro references these via `$crate::Window`, `$crate::App`.
#[cfg(feature = "editor")]
pub use gpui::{App, Window};

/// Re-export UI dock types for plugin use.
/// The `export_plugin!` macro references `$crate::PanelView`.
#[cfg(feature = "editor")]
pub use ui::dock::{Panel, PanelView};

/// Re-export serde_json::Value for plugin use.
/// The `export_plugin!` macro references `$crate::JsonValue`.
pub use serde_json::Value as JsonValue;

pub use profiling;
/// The engine's event plumbing (Gamma v2 host bus, engine event hub,
/// built-in events). `export_plugin!` wires every plugin library to the
/// host's bus through it.
pub use pulsar_events;
/// Plugin settings: `export_plugin!` binds a library's handles to the host.
pub use pulsar_settings;
pub use pulsar_world_registry;
