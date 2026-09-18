use gpui::*;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    popup_menu::PopupMenuExt,
    IconName, Sizable,
};

use super::super::actions::{SetBuildConfig, SetTargetPlatform};
use crate::level_editor::state::{BuildConfig, LevelEditorState, TargetPlatform};

/// Build configuration and platform dropdowns - Comprehensive build settings for all 290+ Rust targets
pub struct BuildDropdowns;

mod config;
mod platform;
mod platform_options;
mod render;
