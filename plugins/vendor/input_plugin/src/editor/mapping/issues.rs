//! What is wrong with the mapping, with one-click repairs.

use gpui::*;
use ui::button::Button;
use ui::scroll::ScrollbarAxis;
use ui::tag::Tag;
use ui::{h_flex, v_flex, ActiveTheme, Disableable, IconName, Sizable, StyledExt};

use crate::model::validate::MappingFix;
use crate::model::Severity;

use super::super::common::chrome::{all_clear, issue_row, panel_header};
use super::super::common::panel::dock_panel;
use super::MapCtx;

pub struct IssuesPanel {
    ctx: MapCtx,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

dock_panel!(IssuesPanel, "input-mapping-issues", "Issues");

impl IssuesPanel {
    pub fn new(ctx: MapCtx, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&ctx.doc, |_, _, cx| cx.notify()), cx.observe(&ctx.lib, |_, _, cx| cx.notify())];
        Self { ctx, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }
}

fn label(fix: &MappingFix) -> String {
    match fix {
        MappingFix::RefreshHint { .. } => "Update note".to_string(),
        MappingFix::RemoveEntry { .. } => "Remove".to_string(),
        MappingFix::ClearAction { .. } => "Clear action".to_string(),
        MappingFix::RenameCategory { name, .. } => format!("Rename to `{name}`"),
    }
}

impl Render for IssuesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let library = self.ctx.library(cx);
        let issues = self.ctx.asset(cx).validate(library.as_ref());
        let count = |s: Severity| issues.iter().filter(|i| i.severity == s).count();
        let (errors, warnings, notes) = (count(Severity::Error), count(Severity::Warning), count(Severity::Info));
        let fixable = issues.iter().any(|i| i.fix.is_some());

        let rows: Vec<AnyElement> = issues
            .iter()
            .enumerate()
            .map(|(n, issue)| {
                let (ctx, select) = (self.ctx.clone(), self.ctx.clone());
                let (entry, category) = (issue.entry, issue.category);
                let fixes: Vec<(String, MappingFix)> = issue.fix.iter().map(|f| (label(f), f.clone())).collect();
                div()
                    .id(SharedString::from(format!("mapping-issue-{n}")))
                    .w_full()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| match (entry, category) {
                        (Some(entry), _) => select.select_entry(entry, cx),
                        (None, Some(category)) => select.select_category(category, cx),
                        _ => {}
                    })
                    .child(issue_row(
                        SharedString::from(format!("mapping-issue-fix-{n}")),
                        issue,
                        fixes,
                        move |fix, _, cx| ctx.edit("Fix problem", None, cx, |m| m.apply_fix(&fix)),
                        cx,
                    ))
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
                        .child(Button::new("mapping-fix-all").label("Fix all").small().disabled(!fixable).tooltip("Apply every suggested repair").on_click(move |_, _, cx| {
                            let library = ctx.library(cx);
                            ctx.edit("Fix all problems", None, cx, |m| {
                                for _ in 0..4 {
                                    let fixes: Vec<MappingFix> = m.validate(library.as_ref()).into_iter().filter_map(|i| i.fix).collect();
                                    if fixes.is_empty() {
                                        break;
                                    }
                                    for fix in fixes {
                                        m.apply_fix(&fix);
                                    }
                                }
                            });
                        }))
                        .into_any_element(),
                ),
                cx,
            ))
            .child(v_flex().id("input-mapping-issues-scroll").flex_1().min_h_0().scrollable(ScrollbarAxis::Vertical).child(body))
    }
}
