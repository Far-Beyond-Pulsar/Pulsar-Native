use super::*;

impl BuildDropdowns {
    /// Append the build-configuration choices to `menu`.
    pub(crate) fn config_menu_items(menu: PopupMenu, current: BuildConfig) -> PopupMenu {
        menu
            .menu_with_check(
                "Debug",
                current == BuildConfig::Debug,
                Box::new(SetBuildConfig(BuildConfig::Debug)),
            )
            .menu_with_check(
                "Release",
                current == BuildConfig::Release,
                Box::new(SetBuildConfig(BuildConfig::Release)),
            )
            .menu_with_check(
                "Shipping",
                current == BuildConfig::Shipping,
                Box::new(SetBuildConfig(BuildConfig::Shipping)),
            )
    }

    /// Short label for a configuration, for tooltips.
    pub(crate) fn config_label(config: BuildConfig) -> &'static str {
        match config {
            BuildConfig::Debug => "Debug",
            BuildConfig::Release => "Release",
            BuildConfig::Shipping => "Shipping",
        }
    }
}
