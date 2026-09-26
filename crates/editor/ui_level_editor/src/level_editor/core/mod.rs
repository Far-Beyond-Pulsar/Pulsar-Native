//! Editor data layer — everything that is *about* scene content and its
//! persistence, as opposed to UI or per-frame editor state:
//!
//! | File | What |
//! |------|------|
//! | `scene_edit` | Scene operations written directly against the SceneDB world shared with the renderer |
//! | `asset_updates` | Rebuild placed class instances when a class asset is updated; forward to a running game |
//! | `commands` | `SceneCommand` — single auditable, undo-tracked mutation path into the scene |
//! | `world_settings_data` | World/settings config model (serialized + replicated) |
//! | `native_scripts` | Script-binding data model (Rust actor records on scene objects) |
//!
//! Live editor state lives under `crate::level_editor::state`; dock panels and
//! their components live under `crate::level_editor::ui` (+ `workspace`).

pub mod asset_updates;
pub mod commands;
pub mod native_scripts;
pub mod scene_edit;
pub mod world_settings_data;
