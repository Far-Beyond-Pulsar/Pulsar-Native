use gpui::*;
use ui::{
    IconName, Sizable,
    button::{Button, ButtonVariants as _},
    popup_menu::PopupMenuExt,
};

use super::super::actions::{SetBuildConfig, SetTargetPlatform};
use crate::state::{BuildConfig, TargetPlatform};

/// Build configuration and platform dropdowns - Comprehensive build settings for all 290+ Rust targets
pub struct BuildDropdowns;

mod config;
mod platform;
mod platform_options;
mod render;
