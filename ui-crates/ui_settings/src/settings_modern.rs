use gpui::{
    App, AppContext, Axis, Context, Entity, FocusHandle, Focusable, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled, Window,
};

use ui::{
    ActiveTheme, Icon, IconName, Sizable, Size, Theme, ThemeMode,
    button::Button,
    group_box::GroupBoxVariant,
    h_flex,
    label::Label,
    setting::{
        NumberFieldOptions, RenderOptions, SettingField, SettingFieldElement, SettingGroup,
        SettingItem, SettingPage, Settings,
    },
    text::{Text, TextView},
    v_flex,
};

struct AppSettings {
    auto_switch_theme: bool,
    cli_path: SharedString,
    font_family: SharedString,
    font_size: f64,
    line_height: f64,
    notifications_enabled: bool,
    auto_update: bool,
    resettable: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_switch_theme: false,
            cli_path: "/usr/local/bin/bash".into(),
            font_family: "Arial".into(),
            font_size: 14.0,
            line_height: 12.0,
            notifications_enabled: true,
            auto_update: true,
            resettable: true,
        }
    }
}

impl Global for AppSettings {}

impl AppSettings {
    fn global(cx: &App) -> &AppSettings {
        cx.global::<AppSettings>()
    }

    fn global_mut(cx: &mut App) -> &mut AppSettings {
        cx.global_mut::<AppSettings>()
    }
}

pub struct ModernSettingsScreen {
    focus_handle: FocusHandle,
    group_variant: GroupBoxVariant,
    size: Size,
}

struct OpenURLSettingField {
    label: SharedString,
    url: SharedString,
}

impl OpenURLSettingField {
    fn new(label: impl Into<SharedString>, url: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            url: url.into(),
        }
    }
}

impl SettingFieldElement for OpenURLSettingField {
    type Element = Button;

    fn render_field(&self, options: &RenderOptions, _: &mut Window, _: &mut App) -> Self::Element {
        let url = self.url.clone();
        Button::new("open-url")
            .outline()
            .label(self.label.clone())
            .with_size(options.size)
            .on_click(move |_, _window, cx| {
                cx.open_url(url.as_str());
            })
    }
}

impl ModernSettingsScreen {
    fn group_variant_to_value(variant: GroupBoxVariant) -> SharedString {
        match variant {
            GroupBoxVariant::Normal => "normal".into(),
            GroupBoxVariant::Outline => "outline".into(),
            GroupBoxVariant::Fill => "fill".into(),
        }
    }

    fn group_variant_from_value(value: &str) -> GroupBoxVariant {
        match value {
            "normal" => GroupBoxVariant::Normal,
            "fill" => GroupBoxVariant::Fill,
            _ => GroupBoxVariant::Outline,
        }
    }

    fn size_to_value(size: Size) -> SharedString {
        match size {
            Size::XSmall => "xsmall".into(),
            Size::Small => "small".into(),
            Size::Large => "large".into(),
            _ => "medium".into(),
        }
    }

    fn size_from_value(value: &str) -> Size {
        match value {
            "xsmall" => Size::XSmall,
            "small" => Size::Small,
            "large" => Size::Large,
            _ => Size::Medium,
        }
    }

    pub fn new(
        _project_path: Option<std::path::PathBuf>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.set_global::<AppSettings>(AppSettings::default());

        Self {
            focus_handle: cx.focus_handle(),
            group_variant: GroupBoxVariant::Outline,
            size: Size::default(),
        }
    }

    fn setting_pages(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<SettingPage> {
        let view = cx.entity();
        let default_settings = AppSettings::default();
        let resettable = AppSettings::global(cx).resettable;

        vec![
            SettingPage::new("General")
                .resettable(resettable)
                .default_open(true)
                .icon(Icon::new(IconName::Settings2))
                .groups(vec![
                    SettingGroup::new().title("Appearance").items(vec![
                        SettingItem::new(
                            "Dark Mode",
                            SettingField::switch(
                                |cx: &App| cx.theme().mode.is_dark(),
                                |val: bool, cx: &mut App| {
                                    let mode = if val {
                                        ThemeMode::Dark
                                    } else {
                                        ThemeMode::Light
                                    };
                                    Theme::global_mut(cx).mode = mode;
                                    Theme::change(mode, None, cx);
                                },
                            )
                            .default_value(false),
                        )
                        .description("Switch between light and dark themes."),
                        SettingItem::new(
                            "Auto Switch Theme",
                            SettingField::checkbox(
                                |cx: &App| AppSettings::global(cx).auto_switch_theme,
                                |val: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).auto_switch_theme = val;
                                },
                            )
                            .default_value(default_settings.auto_switch_theme),
                        )
                        .description("Automatically switch theme based on system settings."),
                        SettingItem::new(
                            "resettable",
                            SettingField::switch(
                                |cx: &App| AppSettings::global(cx).resettable,
                                |checked: bool, cx: &mut App| {
                                    AppSettings::global_mut(cx).resettable = checked
                                },
                            ),
                        )
                        .description("Enable/Disable reset button for settings."),
                        SettingItem::new(
                            "Group Variant",
                            SettingField::dropdown(
                                vec![
                                    ("normal".into(), "Normal".into()),
                                    ("outline".into(), "Outline".into()),
                                    ("fill".into(), "Fill".into()),
                                ],
                                {
                                    let view = view.clone();
                                    move |cx: &App| Self::group_variant_to_value(view.read(cx).group_variant)
                                },
                                {
                                    let view = view.clone();
                                    move |val: SharedString, cx: &mut App| {
                                        view.update(cx, |view, cx| {
                                            view.group_variant = Self::group_variant_from_value(val.as_ref());
                                            cx.notify();
                                        });
                                    }
                                },
                            )
                            .default_value("outline"),
                        )
                        .description("Select the variant for setting groups."),
                        SettingItem::new(
                            "Group Size",
                            SettingField::dropdown(
                                vec![
                                    ("medium".into(), "Medium".into()),
                                    ("small".into(), "Small".into()),
                                    ("xsmall".into(), "XSmall".into()),
                                ],
                                {
                                    let view = view.clone();
                                    move |cx: &App| Self::size_to_value(view.read(cx).size)
                                },
                                {
                                    let view = view.clone();
                                    move |val: SharedString, cx: &mut App| {
                                        view.update(cx, |view, cx| {
                                            view.size = Self::size_from_value(val.as_ref());
                                            cx.notify();
                                        });
                                    }
                                },
                            )
                            .default_value("medium"),
                        )
                        .description("Select the size for the setting group."),
                    ]),
                    SettingGroup::new()
                        .title("Font")
                        .item(
                            SettingItem::new(
                                "Font Family",
                                SettingField::dropdown(
                                    vec![
                                        ("Arial".into(), "Arial".into()),
                                        ("Helvetica".into(), "Helvetica".into()),
                                        ("Times New Roman".into(), "Times New Roman".into()),
                                        ("Courier New".into(), "Courier New".into()),
                                    ],
                                    |cx: &App| AppSettings::global(cx).font_family.clone(),
                                    |val: SharedString, cx: &mut App| {
                                        AppSettings::global_mut(cx).font_family = val;
                                    },
                                )
                                .default_value(default_settings.font_family),
                            )
                            .description("Select the font family for the story."),
                        )
                        .item(
                            SettingItem::new(
                                "Font Size",
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 8.0,
                                        max: 72.0,
                                        ..Default::default()
                                    },
                                    |cx: &App| AppSettings::global(cx).font_size,
                                    |val: f64, cx: &mut App| {
                                        AppSettings::global_mut(cx).font_size = val;
                                    },
                                )
                                .default_value(default_settings.font_size),
                            )
                            .description(
                                "Adjust the font size for better readability between 8 and 72.",
                            ),
                        )
                        .item(
                            SettingItem::new(
                                "Line Height",
                                SettingField::number_input(
                                    NumberFieldOptions {
                                        min: 8.0,
                                        max: 32.0,
                                        ..Default::default()
                                    },
                                    |cx: &App| AppSettings::global(cx).line_height,
                                    |val: f64, cx: &mut App| {
                                        AppSettings::global_mut(cx).line_height = val;
                                    },
                                )
                                .default_value(default_settings.line_height),
                            )
                            .description(
                                "Adjust the line height for better readability between 8 and 32.",
                            ),
                        ),
                    SettingGroup::new().title("Other").items(vec![
                        SettingItem::render(|options, _, _| {
                            h_flex()
                                .w_full()
                                .justify_between()
                                .flex_wrap()
                                .gap_3()
                                .child("This is a custom element item by use SettingItem::element.")
                                .child(
                                    Button::new("action")
                                        .icon(IconName::Globe)
                                        .label("Repository...")
                                        .outline()
                                        .with_size(options.size)
                                        .on_click(|_, _, cx| {
                                            cx.open_url("https://github.com/longbridge/gpui-component");
                                        }),
                                )
                                .into_any_element()
                        }),
                        SettingItem::new(
                            "CLI Path",
                            SettingField::input(
                                |cx: &App| AppSettings::global(cx).cli_path.clone(),
                                |val: SharedString, cx: &mut App| {
                                    AppSettings::global_mut(cx).cli_path = val;
                                },
                            )
                            .default_value(default_settings.cli_path),
                        )
                        .layout(Axis::Vertical)
                        .description(
                            "Path to the CLI executable. \n\
                        This item uses Vertical layout. The title,\
                        description, and field are all aligned vertically with width 100%.",
                        ),
                    ]),
                ]),
            SettingPage::new("Software Update")
                .resettable(resettable)
                .icon(Icon::new(IconName::Cpu))
                .groups(vec![SettingGroup::new().title("Updates").items(vec![
                    SettingItem::new(
                        "Enable Notifications",
                        SettingField::switch(
                            |cx: &App| AppSettings::global(cx).notifications_enabled,
                            |val: bool, cx: &mut App| {
                                AppSettings::global_mut(cx).notifications_enabled = val;
                            },
                        )
                        .default_value(default_settings.notifications_enabled),
                    )
                    .description("Receive notifications about updates and news."),
                    SettingItem::new(
                        "Auto Update",
                        SettingField::switch(
                            |cx: &App| AppSettings::global(cx).auto_update,
                            |val: bool, cx: &mut App| {
                                AppSettings::global_mut(cx).auto_update = val;
                            },
                        )
                        .default_value(default_settings.auto_update),
                    )
                    .description("Automatically download and install updates."),
                ])]),
            SettingPage::new("About")
                .resettable(resettable)
                .icon(Icon::new(IconName::Info))
                .group(
                    SettingGroup::new().item(SettingItem::render(|_options, _, cx| {
                        v_flex()
                            .gap_3()
                            .w_full()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::GalleryVerticalEnd).size_16())
                            .child("GPUI Component")
                            .child(
                                Label::new(
                                    "Rust GUI components for building fantastic cross-platform \
                                    desktop application by using GPUI.",
                                )
                                .text_sm()
                                .text_color(cx.theme().muted_foreground),
                            )
                            .into_any_element()
                    })),
                )
                .group(SettingGroup::new().title("Links").items(vec![
                    SettingItem::new(
                        "GitHub Repository",
                        SettingField::element(OpenURLSettingField::new(
                            "Repository...",
                            "https://github.com/longbridge/gpui-component",
                        )),
                    )
                    .description("Open the GitHub repository in your default browser."),
                    SettingItem::new(
                        "Documentation",
                        SettingField::element(OpenURLSettingField::new(
                            "Rust Docs...",
                            "https://docs.rs/gpui-component",
                        )),
                    )
                    .description(Text::from(TextView::markdown(
                        "settings-story-docs-md",
                        "Rust doc for the `gpui-component` crate.",
                        window,
                        cx,
                    ))),
                    SettingItem::new(
                        "Website",
                        SettingField::render(|options, _window, _cx| {
                            Button::new("open-url")
                                .outline()
                                .label("Website...")
                                .with_size(options.size)
                                .on_click(|_, _window, cx| {
                                    cx.open_url("https://longbridge.github.io/gpui-component/");
                                })
                        }),
                    )
                    .description("Official website and documentation for the GPUI Component."),
                ])),
        ]
    }
}

impl Focusable for ModernSettingsScreen {
    fn focus_handle(&self, _: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ModernSettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        Settings::new("app-settings")
            .with_size(self.size)
            .with_group_variant(self.group_variant)
            .pages(self.setting_pages(window, cx))
    }
}
