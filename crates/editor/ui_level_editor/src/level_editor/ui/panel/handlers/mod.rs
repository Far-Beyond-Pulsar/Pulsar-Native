    //! Action handlers for [`LevelEditorPanel`](super::LevelEditorPanel) — the
//! `on_*` methods wired to the editor's actions in `super::Render`.
//!
//! Split out of `panel.rs`: every method here is the thin "action → state
//! mutation" shape, and none of them are called from outside the type itself.
//! Panel construction, dock reconciliation, and rendering live in `super`.
//! `pub(super)` visibility is required because `Render` in `super` names the
//! handlers (`cx.listener(Self::on_*)` and the keydown fast-paths); they stay
//! unreachable outside this module tree.

use gpui::*;

use super::pie::{begin_pie, end_pie};
use super::LevelEditorPanel;

use crate::ai_sessions;
use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};
use crate::level_editor::ui::actions::*;
use crate::level_editor::ui::toolbar;
use crate::level_editor::{request_thumbnail_capture, CameraMode, TransformTool};


mod objects;
mod playback;
mod scene;
mod tools;
mod view;
