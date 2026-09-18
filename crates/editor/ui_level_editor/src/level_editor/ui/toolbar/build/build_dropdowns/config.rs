use super::*;

impl BuildDropdowns {
    pub(super) fn config_button(config_label: &'static str) -> impl IntoElement {
                Button::new("build_config_dropdown")
                    .label(config_label)
                    .icon(IconName::Settings)
                    .small()
                    .ghost()
                    .tooltip("Select build configuration")
                    .popup_menu(move |menu, _, _| {
                        menu.label("Build Configuration")
                            .separator()
                            .menu_with_icon(
                                "Debug",
                                IconName::Bug,
                                Box::new(SetBuildConfig(BuildConfig::Debug)),
                            )
                            .menu_with_icon(
                                "Release",
                                IconName::Flash,
                                Box::new(SetBuildConfig(BuildConfig::Release)),
                            )
                            .menu_with_icon(
                                "Shipping",
                                IconName::Package,
                                Box::new(SetBuildConfig(BuildConfig::Shipping)),
                            )
                    })
    }
}