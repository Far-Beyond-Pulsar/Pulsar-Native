use gpui::{prelude::*, *};
use ui::{
    h_flex, v_flex, Icon, IconName, ActiveTheme as _, Colorize as _,
};
use crate::entry_screen::{SettingsRequested, FabSearchRequested};
use crate::entry_screen::{EntryScreen, EntryScreenView};

pub fn render_sidebar(screen: &EntryScreen, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let accent = theme.accent;
    let foreground = theme.foreground;
    let muted_fg = theme.muted_foreground;
    let border = theme.border;
    let primary = theme.primary;
    let primary_fg = theme.primary_foreground;

    let accent_bg = accent.opacity(0.12);
    let hover_bg = accent.opacity(0.07);

    let is_recent = screen.view == EntryScreenView::Recent;
    let is_templates = screen.view == EntryScreenView::Templates;
    let is_new = screen.view == EntryScreenView::NewProject;
    let is_clone = screen.view == EntryScreenView::CloneGit;

    v_flex()
        .w(px(220.))
        .h_full()
        .flex_shrink_0()
        .overflow_y_hidden()
        .bg(theme.sidebar)
        .border_r_1()
        .border_color(border)
        // ── Branding header ──────────────────────────────────────
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
                        .w(px(34.))
                        .h(px(34.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_lg()
                        .bg(primary)
                        .child(
                            Icon::new(IconName::Star)
                                .size(px(18.))
                                .text_color(primary_fg)
                        )
                )
                .child(
                    v_flex()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(foreground)
                                .child("Pulsar Engine")
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child("Project Manager")
                        )
                )
        )
        .child(div().w_full().h(px(1.0)).bg(border))
        // ── Projects section ─────────────────────────────────────
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
                        .letter_spacing(px(0.8))
                        .text_color(muted_fg)
                        .px_2()
                        .pb_1p5()
                        .child("PROJECTS")
                )
                .child(
                    h_flex()
                        .id("nav-recent")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .when(is_recent, |this| this.bg(accent_bg))
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::FolderClosed)
                                .size(px(15.))
                                .text_color(if is_recent { accent } else { muted_fg })
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if is_recent { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL })
                                .text_color(if is_recent { foreground } else { muted_fg })
                                .child("Recent Projects")
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = EntryScreenView::Recent;
                            cx.notify();
                        }))
                )
                .child(
                    h_flex()
                        .id("nav-templates")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .when(is_templates, |this| this.bg(accent_bg))
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::Star)
                                .size(px(15.))
                                .text_color(if is_templates { accent } else { muted_fg })
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if is_templates { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL })
                                .text_color(if is_templates { foreground } else { muted_fg })
                                .child("Templates")
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = EntryScreenView::Templates;
                            cx.notify();
                        }))
                )
                .child(
                    h_flex()
                        .id("nav-new")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .when(is_new, |this| this.bg(accent_bg))
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::Plus)
                                .size(px(15.))
                                .text_color(if is_new { accent } else { muted_fg })
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if is_new { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL })
                                .text_color(if is_new { foreground } else { muted_fg })
                                .child("New Project")
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = EntryScreenView::NewProject;
                            cx.notify();
                        }))
                )
                .child(
                    h_flex()
                        .id("nav-clone")
                        .w_full()
                        .gap_2p5()
                        .items_center()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .cursor_pointer()
                        .when(is_clone, |this| this.bg(accent_bg))
                        .hover(|this| this.bg(hover_bg))
                        .child(
                            Icon::new(IconName::Github)
                                .size(px(15.))
                                .text_color(if is_clone { accent } else { muted_fg })
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if is_clone { gpui::FontWeight::SEMIBOLD } else { gpui::FontWeight::NORMAL })
                                .text_color(if is_clone { foreground } else { muted_fg })
                                .child("Clone from Git")
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = EntryScreenView::CloneGit;
                            cx.notify();
                        }))
                )
        )
        // ── Discover section ─────────────────────────────────────
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
                        .letter_spacing(px(0.8))
                        .text_color(muted_fg)
                        .px_2()
                        .pb_1p5()
                        .child("DISCOVER")
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
                                .text_color(muted_fg)
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_fg)
                                .child("FAB Marketplace")
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(FabSearchRequested);
                        }))
                )
        )
        // ── Spacer ───────────────────────────────────────────────
        .child(div().flex_1())
        // ── Utilities ────────────────────────────────────────────
        .child(
            v_flex()
                .w_full()
                .px_3()
                .pb_4()
                .gap_0p5()
                .child(div().w_full().h(px(1.0)).bg(border).mb_2())
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
                                .text_color(muted_fg)
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_fg)
                                .child("Open Folder")
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_folder_dialog(window, cx);
                        }))
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
                            Icon::new(IconName::Package)
                                .size(px(15.))
                                .text_color(muted_fg)
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_fg)
                                .child("Dependencies")
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_dependency_setup = true;
                            cx.notify();
                        }))
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
                                .text_color(muted_fg)
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_fg)
                                .child("Settings")
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(SettingsRequested);
                        }))
                )
        )
}
