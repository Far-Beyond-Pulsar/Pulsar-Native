use gpui::prelude::*;
use gpui::*;
use ui::{button::Button, button::ButtonVariants as _, h_flex, v_flex, ActiveTheme as _, Icon, IconName};

use crate::screen::EntryScreen;
use crate::core::types::InstallStatus;

pub fn render_dependency_setup(screen: &mut EntryScreen, _window: &mut Window, cx: &mut Context<EntryScreen>) -> impl IntoElement {
    let theme = cx.theme();
    let progress = screen.state.install_progress.as_ref();
    let install_progress_val = progress.map(|p| p.progress).unwrap_or(0.0);
    let install_logs: Vec<String> = progress.map(|p| p.logs.clone()).unwrap_or_default();
    let is_complete = progress.map(|p| p.status == InstallStatus::Complete).unwrap_or(false);
    let is_error = progress.map(|p| matches!(p.status, InstallStatus::Error(_))).unwrap_or(false);
    let status_text = progress.map(|p| match &p.status {
        InstallStatus::Idle => "Ready",
        InstallStatus::Downloading => "Downloading...",
        InstallStatus::Installing => "Installing...",
        InstallStatus::Complete => "Complete!",
        InstallStatus::Error(_) => "Error",
    }).unwrap_or("Checking Rust toolchain");

    div()
        .absolute()
        .size_full()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.background.opacity(0.86))
        .child(
            v_flex()
                .w_full()
                .max_w(px(560.))
                .p_6()
                .gap_5()
                .rounded_xl()
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .shadow_lg()
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child("Dependency Setup"),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Setting up required build tools for Pulsar Engine"),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(8.))
                        .bg(theme.secondary.opacity(0.3))
                        .rounded_sm()
                        .child(
                            div()
                                .h_full()
                                .rounded_sm()
                                .bg(if is_error { gpui::red() } else if is_complete { theme.success_foreground } else { theme.accent })
                                .w(relative(install_progress_val.max(0.0).min(1.0))),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(match progress.as_ref().map(|p| &p.status) {
                                    Some(InstallStatus::Complete) => theme.success_foreground,
                                    Some(InstallStatus::Error(_)) => gpui::red(),
                                    _ => theme.accent,
                                }))
                                .child(div().text_sm().text_color(theme.foreground).child("Checking Rust")),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(div().w(px(6.)).h(px(6.)).rounded_full().bg(match progress.as_ref().map(|p| &p.status) {
                                    Some(InstallStatus::Complete) => theme.success_foreground,
                                    _ => theme.muted_foreground,
                                }))
                                .child(div().text_sm().text_color(if install_progress_val > 0.0 { theme.foreground } else { theme.muted_foreground }).child("Installing Rust")),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .max_h(px(240.))
                        .p_3()
                        .rounded_md()
                        .bg(gpui::black().opacity(0.3))
                        .overflow_hidden()
                        .child(
                            v_flex()
                                .gap_1()
                                .children(
                                    install_logs.iter().rev().take(30).rev().map(|log| {
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .font_family(SharedString::from("monospace"))
                                            .child(log.clone())
                                    }),
                                ),
                        ),
                )
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            div().text_sm().text_color(theme.muted_foreground).child(status_text),
                        )
                        .child(
                            Button::new("close-dependency-setup")
                                .label("Close")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.ui.show_dependency_setup = false;
                                    cx.notify();
                                })),
                        ),
                ),
        )
}
