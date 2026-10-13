//! Panel chrome shared by both physics editors: the same "Details"-style
//! header, rules and empty states the rest of the engine's editors use, and
//! the issue rows the validators feed.

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

use crate::model::{Issue, Severity};

pub fn severity_color(severity: Severity, cx: &App) -> Hsla {
    let theme = cx.theme();
    match severity {
        Severity::Info => theme.info,
        Severity::Warning => theme.warning,
        Severity::Error => theme.danger,
    }
}

pub fn severity_icon(severity: Severity) -> IconName {
    match severity {
        Severity::Info => IconName::Info,
        Severity::Warning => IconName::TriangleAlert,
        Severity::Error => IconName::XmarkCircle,
    }
}

/// Panel title bar: icon, title, and optional trailing content on the right.
pub fn panel_header(
    icon: IconName,
    title: impl Into<SharedString>,
    trailing: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .flex_shrink_0()
        .h(px(32.0))
        .px_2()
        .gap_2()
        .items_center()
        .bg(theme.secondary)
        .border_b_1()
        .border_color(theme.border)
        .child(Icon::new(icon).size(px(16.0)).text_color(theme.info))
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(theme.foreground)
                .child(title.into()),
        )
        .child(div().flex_1())
        .children(trailing)
}

/// Thin vertical rule between toolbar groups.
pub fn toolbar_divider(cx: &App) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .w(px(1.0))
        .h(px(18.0))
        .mx_1()
        .bg(cx.theme().border)
}

/// Centered icon + message shown when a panel has nothing to display.
pub fn empty_message(
    icon: IconName,
    title: impl Into<SharedString>,
    hint: impl Into<SharedString>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_4()
        .child(
            Icon::new(icon)
                .size(px(28.0))
                .text_color(theme.muted_foreground.opacity(0.55)),
        )
        .child(
            div()
                .text_sm()
                .font_medium()
                .text_color(theme.foreground.opacity(0.85))
                .child(title.into()),
        )
        .child(
            div()
                .text_xs()
                .text_center()
                .text_color(theme.muted_foreground)
                .child(hint.into()),
        )
}

/// A banner across the top of an editor, for a problem with the file itself.
pub fn banner(
    severity: Severity,
    message: impl Into<SharedString>,
    actions: Vec<Button>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let color = severity_color(severity, cx);
    h_flex()
        .w_full()
        .flex_shrink_0()
        .px_3()
        .py_1p5()
        .gap_2()
        .items_center()
        .bg(color.opacity(0.10))
        .border_b_1()
        .border_color(color.opacity(0.35))
        .child(Icon::new(severity_icon(severity)).size(px(14.0)).text_color(color))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(theme.foreground.opacity(0.92))
                .child(message.into()),
        )
        .children(actions)
}

/// One validation finding, with a button for each way to repair it.
/// `on_fix` is called with the chosen repair.
pub fn issue_row<F: Clone + 'static>(
    id: impl Into<SharedString>,
    issue: &Issue<F>,
    fixes: Vec<(String, F)>,
    on_fix: impl Fn(F, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let color = severity_color(issue.severity, cx);
    let id: SharedString = id.into();
    let on_fix = std::rc::Rc::new(on_fix);
    h_flex()
        .w_full()
        .px_2()
        .py_1p5()
        .gap_2()
        .items_start()
        .rounded(px(6.0))
        .bg(color.opacity(0.08))
        .border_1()
        .border_color(color.opacity(0.28))
        .child(
            Icon::new(severity_icon(issue.severity))
                .size(px(13.0))
                .text_color(color)
                .mt_0p5(),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(issue.subject.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.foreground.opacity(0.85))
                        .child(issue.message.clone()),
                ),
        )
        .children(fixes.into_iter().enumerate().map(|(n, (label, fix))| {
            let on_fix = on_fix.clone();
            Button::new(SharedString::from(format!("{id}-{n}")))
                .label(label)
                .ghost()
                .xsmall()
                .on_click(move |_, window, cx| on_fix(fix.clone(), window, cx))
        }))
}

/// "All good" row shown where a list of issues would be.
pub fn all_clear(message: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .px_2()
        .py_1p5()
        .gap_2()
        .items_center()
        .rounded(px(6.0))
        .bg(theme.success.opacity(0.08))
        .border_1()
        .border_color(theme.success.opacity(0.28))
        .child(Icon::new(IconName::CircleCheck).size(px(13.0)).text_color(theme.success))
        .child(
            div()
                .text_xs()
                .text_color(theme.foreground.opacity(0.85))
                .child(message.into()),
        )
}
