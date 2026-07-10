use gpui::{prelude::*, *};
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    popup_menu::PopupMenuExt,
    v_flex, ActiveTheme as _, IconName, Sizable as _,
};

use crate::screen::ProblemsDrawer;
use crate::utils::actions::*;
use crate::utils::types::DiagnosticSeverity;

pub fn render_header(
    drawer: &mut ProblemsDrawer,
    error_count: usize,
    warning_count: usize,
    info_count: usize,
    total_count: usize,
    cx: &mut Context<ProblemsDrawer>,
) -> impl IntoElement {
    let current_filter_label = match &drawer.filtered_severity {
        None => format!("All Problems ({})", total_count),
        Some(DiagnosticSeverity::Error) => format!("Errors ({})", error_count),
        Some(DiagnosticSeverity::Warning) => format!("Warnings ({})", warning_count),
        Some(DiagnosticSeverity::Information) => format!("Info ({})", info_count),
        Some(DiagnosticSeverity::Hint) => "Hints".to_string(),
    };

    let is_all_selected = drawer.filtered_severity.is_none();
    let is_errors_selected = drawer.filtered_severity == Some(DiagnosticSeverity::Error);
    let is_warnings_selected = drawer.filtered_severity == Some(DiagnosticSeverity::Warning);
    let is_info_selected = drawer.filtered_severity == Some(DiagnosticSeverity::Information);

    v_flex()
        .w_full()
        .gap_3()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .text_base()
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(cx.theme().foreground)
                                .child(t!("Problems.Title").to_string()),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .when(error_count > 0, |this| {
                                    this.child(render_severity_badge(
                                        DiagnosticSeverity::Error,
                                        error_count,
                                        cx,
                                    ))
                                })
                                .when(warning_count > 0, |this| {
                                    this.child(render_severity_badge(
                                        DiagnosticSeverity::Warning,
                                        warning_count,
                                        cx,
                                    ))
                                })
                                .when(info_count > 0, |this| {
                                    this.child(render_severity_badge(
                                        DiagnosticSeverity::Information,
                                        info_count,
                                        cx,
                                    ))
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("toggle-grouping")
                                .ghost()
                                .small()
                                .icon(if drawer.group_by_file {
                                    IconName::List
                                } else {
                                    IconName::Folder
                                })
                                .tooltip(if drawer.group_by_file {
                                    t!("Problems.Action.ShowFlatList").to_string()
                                } else {
                                    t!("Problems.Action.GroupByFile").to_string()
                                })
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.toggle_grouping(cx)),
                                ),
                        )
                        .child(
                            Button::new("clear-all")
                                .ghost()
                                .small()
                                .icon(IconName::Close)
                                .tooltip(t!("Problems.Action.ClearAll").to_string())
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.clear_diagnostics(cx)),
                                ),
                        ),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    div().flex_1().min_w(px(200.0)).child(
                        TextInput::new(&drawer.search_input).w_full().prefix(
                            ui::Icon::new(IconName::Search)
                                .size_4()
                                .text_color(cx.theme().muted_foreground),
                        ),
                    ),
                )
                .child(
                    Button::new("filter-dropdown")
                        .ghost()
                        .small()
                        .icon(IconName::Filter)
                        .label(current_filter_label.clone())
                        .popup_menu_with_anchor(
                            Corner::BottomRight,
                            move |menu, _window, _cx| {
                                menu.menu_with_check(
                                    t!("Problems.Filter.All").to_string(),
                                    is_all_selected,
                                    Box::new(FilterAll),
                                )
                                .separator()
                                .menu_with_check(
                                    t!("Problems.Filter.Errors").to_string(),
                                    is_errors_selected,
                                    Box::new(FilterErrors),
                                )
                                .menu_with_check(
                                    t!("Problems.Filter.Warnings").to_string(),
                                    is_warnings_selected,
                                    Box::new(FilterWarnings),
                                )
                                .menu_with_check(
                                    t!("Problems.Filter.Information").to_string(),
                                    is_info_selected,
                                    Box::new(FilterInfo),
                                )
                            },
                        ),
                ),
        )
}

pub fn render_severity_badge(
    severity: DiagnosticSeverity,
    count: usize,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .px_2()
        .py_0p5()
        .rounded_md()
        .bg(severity.color(cx).opacity(0.15))
        .child(
            ui::Icon::new(severity.icon())
                .size_3()
                .text_color(severity.color(cx)),
        )
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(severity.color(cx))
                .child(count.to_string()),
        )
}

pub fn render_empty_state(drawer: &ProblemsDrawer, cx: &App) -> Div {
    div().size_full().child(
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_8()
            .child(
                v_flex()
                    .gap_4()
                    .items_center()
                    .max_w(px(400.0))
                    .px_6()
                    .py_8()
                    .rounded_xl()
                    .bg(cx.theme().secondary.opacity(0.2))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.3))
                    .child(
                        div()
                            .w(px(64.0))
                            .h(px(64.0))
                            .rounded_full()
                            .bg(cx.theme().success.opacity(0.15))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                ui::Icon::new(IconName::Check)
                                    .size(px(32.0))
                                    .text_color(cx.theme().success),
                            ),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(cx.theme().foreground)
                            .child("No problems detected"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .line_height(rems(1.5))
                            .child(if !drawer.search_query.is_empty() {
                                t!("Problems.Empty.NoMatch").to_string()
                            } else {
                                t!("Problems.Empty.AllGood").to_string()
                            }),
                    ),
            ),
    )
}
