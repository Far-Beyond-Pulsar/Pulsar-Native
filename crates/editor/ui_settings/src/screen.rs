use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::FluentBuilder as _, App, Context, FocusHandle, Focusable, IntoElement,
    ParentElement as _, Render, SharedString, Styled, Window,
};

use engine_state::{NS_EDITOR, NS_PROJECT};

use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex, ActiveTheme, Icon, IconName, Sizable, Theme, ThemeMode,
};

use crate::utils::config;

pub struct ModernSettingsScreen {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) group_variant: ui::group_box::GroupBoxVariant,
    pub(crate) size: ui::Size,
    pub(crate) project_path: Option<PathBuf>,
    pub(crate) has_pending_changes: bool,
}

impl ModernSettingsScreen {
    pub fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = window;
        Self {
            focus_handle: cx.focus_handle(),
            group_variant: ui::group_box::GroupBoxVariant::Outline,
            size: ui::Size::default(),
            project_path,
            has_pending_changes: false,
        }
    }

    fn setting_pages(&self, _window: &mut Window, cx: &mut Context<Self>) -> Vec<SettingPage> {
        let view = cx.entity();

        let mark_dirty: Arc<dyn Fn(&mut App) + Send + Sync> = {
            let view = view.clone();
            Arc::new(move |cx: &mut App| {
                view.update(cx, |screen: &mut ModernSettingsScreen, cx| {
                    screen.has_pending_changes = true;
                    cx.notify();
                });
            })
        };

        let ui_controls_page = {
            let view2 = view.clone();
            SettingPage::new("UI Controls")
                .default_open(true)
                .icon(Icon::new(IconName::Settings2))
                .group(SettingGroup::new().title("Settings Display").items(vec![
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
                        ),
                    )
                    .description("Switch between light and dark themes."),
                    SettingItem::new(
                        "Group Variant",
                        SettingField::dropdown(
                            vec![
                                ("normal".into(), "Normal".into()),
                                ("outline".into(), "Outline".into()),
                                ("fill".into(), "Fill".into()),
                            ],
                            {
                                let v = view.clone();
                                move |cx: &App| config::group_variant_to_value(v.read(cx).group_variant)
                            },
                            {
                                let v = view2.clone();
                                move |val: SharedString, cx: &mut App| {
                                    v.update(cx, |this, cx| {
                                        this.group_variant = config::group_variant_from_value(val.as_ref());
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
                                let v = view.clone();
                                move |cx: &App| config::size_to_value(v.read(cx).size)
                            },
                            {
                                let v = view2.clone();
                                move |val: SharedString, cx: &mut App| {
                                    v.update(cx, |this, cx| {
                                        this.size = config::size_from_value(val.as_ref());
                                        cx.notify();
                                    });
                                }
                            },
                        )
                        .default_value("medium"),
                    )
                    .description("Select the size for the setting group."),
                ]))
        };

        let editor_groups = config::groups_for_namespace(NS_EDITOR, mark_dirty.clone());
        let editor_page = SettingPage::new("Editor")
            .icon(Icon::new(IconName::Code))
            .groups(editor_groups);

        let project_groups = config::groups_for_namespace(NS_PROJECT, mark_dirty.clone());
        let project_page = SettingPage::new("Project")
            .icon(Icon::new(IconName::Folder))
            .groups(project_groups);

        vec![ui_controls_page, editor_page, project_page]
    }
}

impl Focusable for ModernSettingsScreen {
    fn focus_handle(&self, _: &gpui::App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ModernSettingsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_pending = self.has_pending_changes;

        v_flex()
            .size_full()
            .when(has_pending, |this| {
                this.child(crate::components::render_save_bar(cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .child(
                        Settings::new("app-settings")
                            .with_size(self.size)
                            .with_group_variant(self.group_variant)
                            .pages(self.setting_pages(window, cx)),
                    ),
            )
    }
}
