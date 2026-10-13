//! The issues panel: what is wrong with the widget, with one-click repairs.

use gpui::*;
use ui::button::{Button, ButtonVariants};
use ui::scroll::ScrollbarAxis;
use ui::tag::Tag;
use ui::{h_flex, v_flex, ActiveTheme, Disableable, IconName, Sizable};

use crate::model::validate::WidgetFix;
use crate::model::Severity;

use super::commands::Ctx;
use super::common::chrome::{all_clear, issue_row, panel_header};
use super::common::panel::dock_panel;

pub struct IssuesPanel {
    ctx: Ctx,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(IssuesPanel, "widget-issues", "Issues");

impl IssuesPanel {
    pub fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify()), cx.observe(&ctx.lib, |_, _, cx| cx.notify())];
        Self { ctx, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }
}

/// Apply a repair. Ones that need the project's widgets are handled by the caller.
fn apply(ctx: &Ctx, fix: &WidgetFix, cx: &mut App) {
    let fix = fix.clone();
    ctx.doc.update(cx, |d, cx| {
        d.edit("Fix problem", None, cx, |w| w.apply_fix(&fix));
        if matches!(fix, WidgetFix::NewWidgetId) {
            d.save(cx);
        }
    });
}

impl Render for IssuesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let library = self.ctx.library_now(cx);
        let issues = self.ctx.widget(cx).validate(&library);
        let count = |s: Severity| issues.iter().filter(|i| i.severity == s).count();
        let (errors, warnings, notes) = (count(Severity::Error), count(Severity::Warning), count(Severity::Info));
        let fixable = issues.iter().any(|i| i.fix.is_some());

        let rows: Vec<AnyElement> = issues
            .iter()
            .enumerate()
            .map(|(n, issue)| {
                let (ctx, select) = (self.ctx.clone(), self.ctx.clone());
                let node = issue.node;
                let fixes: Vec<(String, WidgetFix)> = issue.fix.iter().map(|f| (f.label(), f.clone())).collect();
                div()
                    .id(SharedString::from(format!("widget-issue-{n}")))
                    .w_full()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        if let Some(node) = node {
                            select.select(vec![node], cx);
                        }
                    })
                    .child(issue_row(SharedString::from(format!("widget-issue-fix-{n}")), issue, fixes, move |fix, _, cx| apply(&ctx, &fix, cx), cx))
                    .into_any_element()
            })
            .collect();

        let body = if rows.is_empty() {
            div().w_full().p_3().child(all_clear("No problems found", cx)).into_any_element()
        } else {
            v_flex().w_full().p_3().gap_1p5().children(rows).into_any_element()
        };

        let tags = h_flex()
            .gap_1()
            .children((errors > 0).then(|| Tag::danger().small().child(format!("{errors} errors"))))
            .children((warnings > 0).then(|| Tag::warning().small().child(format!("{warnings} warnings"))))
            .children((notes > 0).then(|| Tag::info().small().child(format!("{notes} notes"))));

        let ctx = self.ctx.clone();
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .track_focus(&self.focus_handle)
            .child(panel_header(
                IconName::TriangleAlert,
                "Issues",
                Some(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(tags)
                        .child(Button::new("widget-fix-all").label("Fix all").small().disabled(!fixable).tooltip("Apply every suggested repair").on_click(move |_, _, cx| {
                            let library = ctx.library_now(cx);
                            ctx.doc.update(cx, |d, cx| {
                                d.edit("Fix all problems", None, cx, |w| {
                                    for _ in 0..8 {
                                        let fixes: Vec<WidgetFix> = w.validate(&library).into_iter().filter_map(|i| i.fix).filter(|f| !matches!(f, WidgetFix::NewWidgetId | WidgetFix::UpdateNameHint { .. })).collect();
                                        if fixes.is_empty() {
                                            break;
                                        }
                                        for fix in fixes {
                                            w.apply_fix(&fix);
                                        }
                                    }
                                })
                            })
                        }))
                        .into_any_element(),
                ),
                cx,
            ))
            .child(v_flex().id("widget-issues-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).child(body))
    }
}
