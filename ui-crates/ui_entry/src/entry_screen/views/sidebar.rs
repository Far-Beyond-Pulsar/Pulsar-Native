use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    v_flex, IconName, ActiveTheme as _,
};
use crate::OpenSettings;
use crate::entry_screen::{EntryScreen, EntryScreenView};

pub fn render_sidebar(screen: &EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();

    v_flex()
        .w(px(80.))
        .h_full()
        .flex_shrink_0()
        .overflow_y_hidden()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(theme.border)
        .gap_3()
        .items_center()
        .pt_8()
        .pb_6()
        .child(
            Button::new("recent-projects")
                .icon(IconName::FolderClosed)
                .label("")
                .tooltip("Recent Projects")
                .with_variant(if screen.view == EntryScreenView::Recent {
                    ui::button::ButtonVariant::Primary
                } else {
                    ui::button::ButtonVariant::Ghost
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = EntryScreenView::Recent;
                    cx.notify();
                }))
        )
        .child(
            Button::new("templates")
                .icon(IconName::Star)
                .label("")
                .tooltip("Project Templates")
                .with_variant(if screen.view == EntryScreenView::Templates {
                    ui::button::ButtonVariant::Primary
                } else {
                    ui::button::ButtonVariant::Ghost
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = EntryScreenView::Templates;
                    cx.notify();
                }))
        )
        .child(
            Button::new("new-project")
                .icon(IconName::Plus)
                .label("")
                .tooltip("Create New Project")
                .with_variant(if screen.view == EntryScreenView::NewProject {
                    ui::button::ButtonVariant::Primary
                } else {
                    ui::button::ButtonVariant::Ghost
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = EntryScreenView::NewProject;
                    cx.notify();
                }))
        )
        .child(
            Button::new("clone-git")
                .icon(IconName::GitHub)
                .label("")
                .tooltip("Clone from Git")
                .with_variant(if screen.view == EntryScreenView::CloneGit {
                    ui::button::ButtonVariant::Primary
                } else {
                    ui::button::ButtonVariant::Ghost
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.view = EntryScreenView::CloneGit;
                    cx.notify();
                }))
        )
        .child(div().flex_1())
        .child(
            Button::new("open-existing")
                .icon(IconName::FolderOpen)
                .label("")
                .tooltip("Open Existing Project")
                .with_variant(ui::button::ButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_folder_dialog(window, cx);
                }))
        )
        .child(
            Button::new("dependencies")
                .icon(IconName::Package)
                .label("")
                .tooltip("Setup Dependencies")
                .with_variant(ui::button::ButtonVariant::Ghost)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_dependency_setup = true;
                    cx.notify();
                }))
        )
        .child(
            Button::new("settings")
                .icon(IconName::Settings)
                .label("")
                .tooltip("Settings")
                .with_variant(ui::button::ButtonVariant::Ghost)
                .on_click(cx.listener(|_, _, window, cx| {
                    window.dispatch_action(Box::new(OpenSettings), cx);
                }))
        )
}
