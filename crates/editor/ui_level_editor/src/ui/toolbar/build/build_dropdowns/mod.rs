use gpui::*;
use ui::{
    IconName,
    popup_menu::PopupMenu,
};

use super::super::actions::{SetBuildConfig, SetTargetPlatform};
use crate::state::{BuildConfig, TargetPlatform};

/// Build configuration and target-platform choices, appended to the Build button's menu.
pub struct BuildDropdowns;

mod config;
mod platform;
mod platform_options;

