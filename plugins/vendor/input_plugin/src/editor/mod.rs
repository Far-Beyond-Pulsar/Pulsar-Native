//! The two editors: one for actions, one for mappings.
//!
//! Both are docked workspaces of panels over one shared
//! [`common::document::Document`], so every edit is undoable and every panel
//! stays in step. What they share lives here: choosers for actions and keys,
//! the list editor for triggers and modifiers, and the project's library.

pub mod action;
pub mod behavior_ui;
pub mod chooser;
pub mod common;
pub mod drag;
pub mod fields;
pub mod form;
pub mod keypicker;
pub mod mapping;
pub mod state;

use std::sync::Arc;

use gpui::*;
use plugin_editor_api::OpenAsset;
use ui::dock::PanelView;

use crate::library::Library;
use crate::model::validate::ActionSource;
use chooser::ChoiceItem;

pub const ACTION_EDITOR_ID: &str = "input-action-editor";
pub const MAPPING_EDITOR_ID: &str = "input-mapping-editor";

pub(crate) fn as_panel<P: PanelView + 'static>(panel: P) -> Arc<dyn PanelView> {
    Arc::new(panel)
}

/// The project's actions as things to choose from. `except` leaves one out
/// (an action can't be chorded with itself).
pub fn action_choices(library: &Library, except: Option<&crate::model::AssetId>) -> Vec<ChoiceItem> {
    library
        .actions()
        .into_iter()
        .filter(|a| Some(&a.id) != except && library.action(&a.id).is_some())
        .map(|a| ChoiceItem::new(a.id.to_string(), a.event_name()).detail(a.relative.clone()).tag(a.asset.value_type.short()))
        .collect()
}

/// Open another input asset in its editor.
pub(crate) fn open_asset(path: std::path::PathBuf, window: &mut Window, cx: &mut App) {
    window.dispatch_action(Box::new(OpenAsset { path }), cx);
}
