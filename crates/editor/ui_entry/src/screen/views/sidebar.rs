use gpui::prelude::*;
use gpui::*;
use ui::{
    button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName,
};

use crate::core::events::{FabSearchRequested, SettingsRequested};
use crate::core::types::EntryScreenView;
use crate::screen::EntryScreen;

pub fn render_sidebar(screen: &mut EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let accent = theme.accent;
    let foreground = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let border = theme.border;
    let primary = theme.primary;
    let primary_fg = theme.primary_foreground;

    let accent_bg = accent.opacity(0.12);
    let hover_bg = accent.opacity(0.07);

    let is_recent = screen.state.ui.view == EntryScreenView::Recent;
    let is_templates = screen.state.ui.view == EntryScreenView::Templates;
    let is_new = screen.state.ui.view == EntryScreenView::NewProject;
    let is_clone = screen.state.ui.view == EntryScreenView::CloneGit;
    let is_cloud = screen.state.ui.view == EntryScreenView::CloudProjects;
    let is_friends = screen.state.ui.view == EntryScreenView::Friends;

    v_flex()
        .w(px(220.))
        .h_full()
        .flex_shrink_0()
        .overflow_y_hidden()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(border)
        .child(
            h_flex()
                .w_full()
                .px_4()
                .pt_5()
                .pb_4()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .flex_shrink_0()
                        .w(px(44.))
                        .h(px(44.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .when(screen.state.logo.is_none(), |this| this.bg(primary))
                        .when_some(screen.state.logo.clone(), |this, logo| {
                            this.child(
                                img(ImageSource::Render(logo))
                                    .w(px(58.))
                                    .h(px(58.))
                                    .object_fit(gpui::ObjectFit::Contain),
                            )
                        })
                        .when(screen.state.logo.is_none(), |this| {
                            this.child(
                                Icon::new(IconName::Star)
                                    .size(px(18.))
                                    .text_color(primary_fg),
                            )
                        }),
                )
                .child(
                    v_flex()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(foreground)
                                .child("Pulsar Engine"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child("Project Manager"),
                        ),
                ),
        )
        .child(div().w_full().h(px(1.0)).bg(border))
        .child(
            v_flex()
                .w_full()
                .px_3()
                .pt_4()
                .pb_2()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(muted_fg)
                        .px_2()
                        .pb_1p5()
                        .child("PROJECTS"),
                )
                .child(nav_item(
                    "nav-recent",
                    IconName::Clock,
                    "Recent Projects",
                    is_recent,
                    accent,
                    foreground,
                    muted_fg,
                    accent_bg,
                    hover_bg,
                    cx.listener(|this, _, _, cx| {
                        this.state.ui.view = EntryScreenView::Recent;
                        cx.notify();
                    }),
                ))
                .child(nav_item(
                    "nav-templates",
                    IconName::List,
                    "Templates",
                    is_templates,
                    accent,
                    foreground,
                    muted_fg,
                    accent_bg,
                    hover_bg,
                    cx.listener(|this, _, _, cx| {
                        this.state.ui.view = EntryScreenView::Templates;
                        cx.notify();
                    }),
                ))
                .child(nav_item(
                    "nav-new",
                    IconName::Plus,
                    "New Project",
                    is_new,
                    accent,
                    foreground,
                    muted_fg,
                    accent_bg,
                    hover_bg,
                    cx.listener(|this, _, _, cx| {
                        this.state.ui.view = EntryScreenView::NewProject;
                        cx.notify();
                    }),
                ))
                .child(nav_item(
                    "nav-clone",
                    IconName::Github,
                    "Clone from Git",
                    is_clone,
                    accent,
                    foreground,
                    muted_fg,
                    accent_bg,
                    hover_bg,
                    cx.listener(|this, _, _, cx| {
                        this.state.ui.view = EntryScreenView::CloneGit;
                        cx.notify();
                    }),
                )),
        )
        .child(
            v_flex()
                .w_full()
                .px_3()
                .pt_2()
                .pb_2()
                .gap_0p5()
                .child(div().w_full().h(px(1.0)).bg(border).mb_2())
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(muted_fg)
                        .px_2()
                        .pb_1p5()
                        .child("CLOUD"),
                )
                .child(nav_item(
                    "nav-cloud",
                    IconName::Cloud,
                    "Cloud Projects",
                    is_cloud,
                    accent,
                    foreground,
                    muted_fg,
                    accent_bg,
                    hover_bg,
                    cx.listener(|this, _, _, cx| {
                        this.state.ui.view = EntryScreenView::CloudProjects;
                        cx.notify();
                    }),
                )),
        )
        .child(
            v_flex()
                .w_full()
                .px_3()
                .pt_2()
                .pb_2()
                .gap_0p5()
                .child(div().w_full().h(px(1.0)).bg(border).mb_2())
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(muted_fg)
                        .px_2()
                        .pb_1p5()
                        .child("SOCIAL"),
                )
                .child(
                    h_flex()
                        .id("nav-friends")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .when(is_friends, |this| this.bg(accent_bg))
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::Group)
                                .size(px(15.))
                                .text_color(if is_friends { accent } else { muted_fg }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if is_friends {
                                    gpui::FontWeight::SEMIBOLD
                                } else {
                                    gpui::FontWeight::NORMAL
                                })
                                .text_color(if is_friends { foreground } else { muted_fg })
                                .child("Friends"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_right()
                                .text_xs()
                                .text_color(muted_fg)
                                .child("online"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.view = EntryScreenView::Friends;
                            cx.notify();
                        })),
                ),
        )
        .child(
            v_flex()
                .w_full()
                .px_3()
                .pt_2()
                .pb_2()
                .gap_0p5()
                .child(div().w_full().h(px(1.0)).bg(border).mb_2())
                .child(
                    div()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(muted_fg)
                        .px_2()
                        .pb_1p5()
                        .child("DISCOVER"),
                )
                .child(
                    h_flex()
                        .id("nav-fab")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::ShoppingBag)
                                .size(px(15.))
                                .text_color(muted_fg),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_fg)
                                .child("FAB Marketplace"),
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(FabSearchRequested);
                        })),
                ),
        )
        .child(div().flex_1())
        .child(
            v_flex()
                .w_full()
                .px_3()
                .pb_4()
                .gap_0p5()
                .child(div().w_full().h(px(1.0)).bg(border).mb_2())
                .when_some(screen.state.auth.message.clone(), |this, msg| {
                    this.child(
                        div()
                            .px_3()
                            .pb_2()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(msg),
                    )
                })
                .child(
                    h_flex()
                        .id("nav-open")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::FolderOpen)
                                .size(px(15.))
                                .text_color(muted_fg),
                        )
                        .child(div().text_sm().text_color(muted_fg).child("Open Folder"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_folder_dialog(cx);
                        })),
                )
                .child(
                    h_flex()
                        .id("nav-deps")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::Settings)
                                .size(px(15.))
                                .text_color(muted_fg),
                        )
                        .child(div().text_sm().text_color(muted_fg).child("Dependencies"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.ui.show_onboarding = true;
                            cx.notify();
                        })),
                )
                .child(
                    h_flex()
                        .id("nav-settings")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::Settings)
                                .size(px(15.))
                                .text_color(muted_fg),
                        )
                        .child(div().text_sm().text_color(muted_fg).child("Settings"))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(SettingsRequested);
                        })),
                ),
        )
        .child(div().h(px(12.)))
}

fn nav_item(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    is_active: bool,
    accent: gpui::Hsla,
    foreground: gpui::Hsla,
    muted_fg: gpui::Hsla,
    accent_bg: gpui::Hsla,
    hover_bg: gpui::Hsla,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    h_flex()
        .id(id)
        .w_full()
        .gap_2p5()
        .items_center()
        .px_3()
        .py_2()
        .rounded_lg()
        .cursor_pointer()
        .when(is_active, |this| this.bg(accent_bg))
        .hover(|this| this.bg(hover_bg))
        .child(Icon::new(icon).size(px(15.)).text_color(if is_active {
            foreground
        } else {
            muted_fg
        }))
        .child(
            div()
                .text_sm()
                .font_weight(if is_active {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if is_active { foreground } else { muted_fg })
                .child(label),
        )
        .on_click(on_click)
}
