//! The drawer's one-line header: a tab per category with its count, then
//! the severity filter, an active source filter, the search and the
//! dismissed-findings toggle.

use gpui::{prelude::*, *};
use pulsar_findings::{Category, Severity};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    menu::PopupMenuItem,
    ActiveTheme as _, Icon, IconName, Sizable as _,
};

use crate::screen::ProblemsDrawer;
use crate::utils::items::{self, TabCount};
use crate::utils::types::DiagnosticSeverity;

/// A severity's icon and color.
pub fn severity_style(severity: Severity, cx: &App) -> (IconName, Hsla) {
    match severity {
        Severity::Error => (IconName::CircleX, cx.theme().danger),
        Severity::Warning => (IconName::TriangleAlert, cx.theme().warning),
        Severity::Info => (IconName::Info, cx.theme().info),
    }
}

pub fn render_header(drawer: &mut ProblemsDrawer, cx: &mut Context<ProblemsDrawer>) -> impl IntoElement {
    let counts = items::tab_counts(&drawer.items, &drawer.dismissed);
    let dismissed = items::dismissed_count(&drawer.items, drawer.tab, &drawer.dismissed);
    let theme = cx.theme().clone();

    h_flex()
        .w_full()
        .flex_shrink_0()
        .h(px(40.0))
        .px_2()
        .gap_1()
        .items_center()
        .border_b_1()
        .border_color(theme.border)
        .bg(theme.sidebar)
        .children(
            Category::ALL
                .into_iter()
                .map(|category| render_tab(drawer, category, counts[&category], cx)),
        )
        .child(div().flex_1())
        .when_some(drawer.filtered_source.clone(), |this, source| {
            this.child(
                Button::new("source-filter")
                    .small()
                    .ghost()
                    .label(format!("Source: {source}"))
                    .icon(IconName::Close)
                    .tooltip("Show every source")
                    .on_click(cx.listener(|this, _, _, cx| this.set_source_filter(None, cx))),
            )
        })
        .child(render_severity_filter(drawer, cx))
        .child(
            div().w(px(240.0)).child(
                TextInput::new(&drawer.search_input)
                    .small()
                    .cleanable()
                    .prefix(Icon::new(IconName::Search).size_3p5().text_color(theme.muted_foreground)),
            ),
        )
        .when(dismissed > 0 || drawer.dismissed_only, |this| {
            this.child(
                Button::new("dismissed-view")
                    .small()
                    .when(drawer.dismissed_only, |b| b.primary())
                    .when(!drawer.dismissed_only, |b| b.ghost())
                    .icon(IconName::EyeOff)
                    .label(format!("Dismissed {dismissed}"))
                    .tooltip(if drawer.dismissed_only {
                        "Back to the current findings"
                    } else {
                        "Show the findings this project dismissed"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_dismissed_view(cx))),
            )
        })
}

fn render_tab(
    drawer: &ProblemsDrawer,
    category: Category,
    count: TabCount,
    cx: &mut Context<ProblemsDrawer>,
) -> impl IntoElement {
    let theme = cx.theme();
    let selected = drawer.tab == category;
    let badge_color = count
        .worst
        .map(|worst| severity_style(worst, cx).1)
        .unwrap_or(theme.muted_foreground);
    h_flex()
        .id(SharedString::from(format!("problems-tab-{}", category.label())))
        .h(px(28.0))
        .px_2p5()
        .gap_1p5()
        .items_center()
        .rounded(theme.radius)
        .text_sm()
        .cursor_pointer()
        .text_color(if selected {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .when(selected, |this| this.bg(theme.secondary))
        .when(!selected, |this| this.hover(|s| s.bg(theme.secondary.opacity(0.5))))
        .child(category.label())
        .when(count.count > 0, |this| {
            this.child(
                div()
                    .px_1p5()
                    .rounded_full()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .bg(badge_color.opacity(0.16))
                    .text_color(badge_color)
                    .child(count.count.to_string()),
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| this.show_tab(category, cx)))
}

fn render_severity_filter(drawer: &ProblemsDrawer, cx: &mut Context<ProblemsDrawer>) -> impl IntoElement {
    let current = drawer.filtered_severity.clone();
    let label = match &current {
        None => "All severities",
        Some(DiagnosticSeverity::Error) => "Errors",
        Some(DiagnosticSeverity::Warning) => "Warnings",
        Some(_) => "Info",
    };
    let view = cx.entity();
    Button::new("severity-filter")
        .small()
        .ghost()
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu_with_anchor(Corner::TopRight, move |menu, _, _| {
            [
                ("All severities", None),
                ("Errors", Some(DiagnosticSeverity::Error)),
                ("Warnings", Some(DiagnosticSeverity::Warning)),
                ("Info", Some(DiagnosticSeverity::Information)),
            ]
            .into_iter()
            .fold(menu, |menu, (label, severity)| {
                let view = view.clone();
                let checked = severity == current;
                menu.item(PopupMenuItem::new(label).checked(checked).on_click(move |_, _, cx| {
                    let severity = severity.clone();
                    view.update(cx, |this, cx| this.set_filter(severity, cx));
                }))
            })
        })
}
