//! The current tab's list: a measured `gpui::list` of group headings and
//! one-line items. Only the open item shows more (its explanation, where it
//! is, its actions and, for code, the file and its quick fixes), so a long
//! list stays scannable and one row's details never push the rest around
//! more than that row needs.

use gpui::{prelude::*, *};
use pulsar_findings::{Category, Location};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    indicator::Indicator,
    scroll::Scrollbar,
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
};

use super::content::{render_file_preview, render_hint_diff};
use super::header::severity_style;
use crate::screen::ProblemsDrawer;
use crate::utils::items::Row;

const ROW_HEIGHT: f32 = 28.0;

pub fn render_list(drawer: &mut ProblemsDrawer, cx: &mut Context<ProblemsDrawer>) -> AnyElement {
    if drawer.rows.is_empty() {
        return render_empty(drawer, cx).into_any_element();
    }
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
            list(
                drawer.list.clone(),
                cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
            )
            .size_full(),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(px(12.0))
                .child(Scrollbar::vertical(&drawer.scrollbar, &drawer.list)),
        )
        .into_any_element()
}

fn render_empty(drawer: &ProblemsDrawer, cx: &mut Context<ProblemsDrawer>) -> impl IntoElement {
    let theme = cx.theme();
    let (icon, title, body) = if !drawer.search_query.trim().is_empty()
        || drawer.filtered_severity.is_some()
        || drawer.filtered_source.is_some()
    {
        (IconName::Search, "Nothing matches", "Try another search or filter.")
    } else if drawer.dismissed_only {
        (IconName::EyeOff, "Nothing dismissed", "Findings this project dismisses are listed here.")
    } else {
        match drawer.tab {
            Category::Code => (IconName::CircleCheck, "No code problems", "Language servers and script checks report here."),
            Category::Performance => (IconName::CircleCheck, "Nothing to tune", "Budgets, frame time and the renderer's warnings report here when something needs attention."),
            Category::Project => (IconName::CircleCheck, "The project is healthy", "Plugins, imports, levels, pipelines and shaders report here."),
            Category::Editor => (IconName::CircleCheck, "The editor is responsive", "Stalled tasks and UI freezes report here."),
        }
    };
    v_flex()
        .flex_1()
        .size_full()
        .items_center()
        .justify_center()
        .gap_1()
        .text_color(theme.muted_foreground)
        .child(Icon::new(icon).size_6().mb_1())
        .child(div().text_sm().text_color(theme.foreground).child(title))
        .child(div().text_xs().child(body))
}

impl ProblemsDrawer {
    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.rows.get(ix).cloned() {
            Some(Row::Group {
                key,
                title,
                count,
                worst,
                collapsed,
            }) => self.render_group(key, title, count, worst, collapsed, cx),
            Some(Row::Item(item_ix)) => self.render_item(item_ix, window, cx),
            None => div().into_any_element(),
        }
    }

    fn render_group(
        &self,
        key: String,
        title: String,
        count: usize,
        worst: pulsar_findings::Severity,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (_, color) = severity_style(worst, cx);
        h_flex()
            .id(SharedString::from(format!("group-{key}")))
            .w_full()
            .h(px(ROW_HEIGHT))
            .px_2()
            .gap_1p5()
            .items_center()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.foreground)
            .cursor_pointer()
            .hover(|s| s.bg(theme.secondary.opacity(0.4)))
            .child(
                Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown })
                    .size_3p5()
                    .text_color(theme.muted_foreground),
            )
            .child(div().size(px(6.0)).rounded_full().bg(color))
            .child(div().min_w_0().truncate().child(title))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .font_weight(FontWeight::NORMAL)
                    .child(count.to_string()),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_group(&key, cx)))
            .into_any_element()
    }

    fn render_item(&mut self, item_ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.items.get(item_ix).cloned() else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let open = self.expanded.as_deref() == Some(item.id.as_str());
        let (icon, color) = severity_style(item.severity, cx);
        let toggle_id = item.id.clone();

        let line = h_flex()
            .id(SharedString::from(format!("item-{}", item.id)))
            .w_full()
            .h(px(ROW_HEIGHT))
            .pl(px(28.0))
            .pr_3()
            .gap_2()
            .items_center()
            .text_sm()
            .cursor_pointer()
            .child(Icon::new(icon).size_3p5().flex_shrink_0().text_color(color))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.foreground)
                    .child(item.title.clone()),
            )
            .when_some(item.metric.clone(), |this, metric| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .font_family("monospace")
                        .text_color(theme.muted_foreground)
                        .child(metric),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(160.0))
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(item.source.clone()),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_expanded(&toggle_id, cx)));

        let mut row = v_flex()
            .w_full()
            .when(open, |this| {
                this.bg(theme.secondary.opacity(0.35))
                    .border_l_2()
                    .border_color(color)
            })
            .when(!open, |this| this.hover(|s| s.bg(theme.secondary.opacity(0.25))))
            .child(line);
        if open {
            row = row.child(self.render_details(item_ix, window, cx));
        }
        row.into_any_element()
    }

    /// The open item's details.
    fn render_details(&mut self, item_ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let item = self.items[item_ix].clone();
        let theme = cx.theme().clone();
        let mut body = v_flex().w_full().pl(px(50.0)).pr_4().pb_3().gap_2();

        if !item.detail.is_empty() {
            body = body.child(
                div()
                    .w_full()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(item.detail.clone()),
            );
        }

        let place = match &item.location {
            Location::None => None,
            Location::File { path, line, .. } if *line > 0 => {
                Some(format!("{}:{line}", crate::utils::filter::display_path(self.project_root.as_deref(), &path.display().to_string())))
            }
            Location::File { path, .. } | Location::Asset(path) => Some(
                crate::utils::filter::display_path(self.project_root.as_deref(), &path.display().to_string()),
            ),
            Location::Object { id } => Some(format!("Object {id}")),
            Location::Setting { namespace, owner, key, .. } => Some(format!("Setting {namespace}.{owner}.{key}")),
        };
        if let Some(place) = place {
            body = body.child(
                div()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(theme.muted_foreground)
                    .child(place),
            );
        }

        // Code: the file around the problem, and its quick fixes.
        if let Some(diagnostic) = self.code_diagnostic(&item).cloned() {
            body = body.child(render_file_preview(self, &diagnostic, window, cx));
            if diagnostic.loading_actions {
                body = body.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(Indicator::new().with_size(ui::Size::Small))
                        .child("Looking for quick fixes..."),
                );
            }
            let index = item.code.as_ref().map_or(0, |(_, ix)| *ix);
            for (hint_ix, hint) in diagnostic.hints.iter().enumerate() {
                body = body.child(render_hint_diff(self, index, hint_ix, hint, window, cx));
            }
        }

        // Buttons: open the file (code), the finding's own actions, dismiss.
        let mut buttons = h_flex().gap_1().flex_wrap();
        if let Location::File { path, line, column } = &item.location {
            if item.category == Category::Code {
                let action = pulsar_findings::Action::OpenFile {
                    path: path.clone(),
                    line: *line,
                    column: *column,
                };
                buttons = buttons.child(
                    Button::new(SharedString::from(format!("open-{}", item.id)))
                        .small()
                        .outline()
                        .icon(IconName::ExternalLink)
                        .label("Open file")
                        .on_click(cx.listener(move |this, _, _, cx| this.run_action(&action, cx))),
                );
            }
        }
        for (ix, finding_action) in item.actions.iter().enumerate() {
            let action = finding_action.action.clone();
            buttons = buttons.child(
                Button::new(SharedString::from(format!("action-{}-{ix}", item.id)))
                    .small()
                    .outline()
                    .label(finding_action.label.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.run_action(&action, cx))),
            );
        }
        if item.dismissible() {
            let id = item.id.clone();
            let dismissed = self.dismissed.contains(&item.id);
            buttons = buttons.child(
                Button::new(SharedString::from(format!("dismiss-{}", item.id)))
                    .small()
                    .ghost()
                    .icon(if dismissed { IconName::Eye } else { IconName::EyeOff })
                    .label(if dismissed { "Restore" } else { "Dismiss" })
                    .tooltip(if dismissed {
                        "List this finding again"
                    } else {
                        "Accept this for the project: it stops being listed (Dismissed brings it back)"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_dismissed(&id, cx))),
            );
        }
        body.child(buttons).into_any_element()
    }
}
