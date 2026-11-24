//! The selected entry, one measured list row per section: identity and
//! state, description, problems, contracts and every contribution by
//! category, all from the catalog snapshot. The window's `gpui::list` sizes
//! each row by its content, so a long description or a big contribution
//! group never overlaps the next section, and every section spans the pane.

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::{h_flex, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt};

use super::plugin_card::render_plugin_icon;
use crate::model::{Entry, EntryKind, Extensions, Group, RowState};
use crate::screen::PluginManagerWindow;

/// The widest the details column grows, so a wide window stays readable.
const MAX_WIDTH: f32 = 820.0;

/// One section of the details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailRow {
    Header,
    Description,
    Problems,
    Requires,
    Provides,
    RequiredBy,
    Group(usize),
    Nothing,
}

/// The sections `entry` shows, in order.
pub fn detail_rows(entry: &Entry) -> Vec<DetailRow> {
    let mut rows = vec![DetailRow::Header];
    if !entry.description.is_empty() {
        rows.push(DetailRow::Description);
    }
    if !entry.problems.is_empty() {
        rows.push(DetailRow::Problems);
    }
    rows.push(DetailRow::Requires);
    if !entry.provides.is_empty() {
        rows.push(DetailRow::Provides);
    }
    if !entry.required_by.is_empty() {
        rows.push(DetailRow::RequiredBy);
    }
    rows.extend((0..entry.groups.len()).map(DetailRow::Group));
    if entry.groups.is_empty() {
        rows.push(DetailRow::Nothing);
    }
    rows
}

/// Section `row` of `entry`, centered and capped at [`MAX_WIDTH`].
pub fn render_detail_row(
    entry: &Entry,
    model: &Extensions,
    row: DetailRow,
    cx: &mut Context<PluginManagerWindow>,
) -> AnyElement {
    let body = match row {
        DetailRow::Header => render_header(entry, cx).into_any_element(),
        DetailRow::Description => div()
            .w_full()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(entry.description.clone())
            .into_any_element(),
        DetailRow::Problems => {
            let color = cx.theme().danger;
            let lines: Vec<AnyElement> = entry
                .problems
                .iter()
                .map(|problem| line(problem.clone(), color, cx).into_any_element())
                .collect();
            section(
                "Problems",
                v_flex().gap_1().children(lines).into_any_element(),
                cx,
            )
            .into_any_element()
        }
        DetailRow::Requires => {
            let body = render_requirements(entry, cx);
            section("Requires", body, cx).into_any_element()
        }
        DetailRow::Provides => {
            let color = cx.theme().foreground;
            let lines: Vec<AnyElement> = entry
                .provides
                .iter()
                .map(|contract| line(contract.clone(), color, cx).into_any_element())
                .collect();
            section(
                "Provides",
                v_flex().gap_1().children(lines).into_any_element(),
                cx,
            )
            .into_any_element()
        }
        DetailRow::RequiredBy => {
            let color = cx.theme().foreground;
            let lines: Vec<AnyElement> = entry
                .required_by
                .iter()
                .map(|id| {
                    let text = model
                        .entry(id)
                        .map(|entry| format!("{} ({id})", entry.name))
                        .unwrap_or_else(|| id.to_string());
                    line(text, color, cx).into_any_element()
                })
                .collect();
            section(
                "Required by",
                v_flex().gap_1().children(lines).into_any_element(),
                cx,
            )
            .into_any_element()
        }
        DetailRow::Group(ix) => match entry.groups.get(ix) {
            Some(group) => contribution_group(group, cx),
            None => div().into_any_element(),
        },
        DetailRow::Nothing => muted("Registers nothing.", cx).into_any_element(),
    };
    h_flex()
        .w_full()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(MAX_WIDTH))
                .px_5()
                .when(row == DetailRow::Header, |this| this.pt_5())
                .pb_4()
                .child(body),
        )
        .into_any_element()
}

fn render_header(entry: &Entry, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    let state = match (entry.kind, entry.enabled) {
        (EntryKind::Engine, _) => "Always present",
        (EntryKind::Library, _) => "Loaded",
        (_, true) => "Enabled by this project",
        (_, false) => "Disabled for this project",
    };
    h_flex()
        .w_full()
        .gap_4()
        .items_center()
        .child(render_plugin_icon(entry, 64.0, cx))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_lg()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(entry.name.clone()),
                        )
                        .child(tag(entry.kind.label(), cx))
                        .child(tag(state, cx)),
                )
                .child(
                    div()
                        .text_xs()
                        .font_family("monospace")
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} · {}", entry.id, entry.source)),
                ),
        )
}

fn render_requirements(entry: &Entry, cx: &mut Context<PluginManagerWindow>) -> AnyElement {
    if entry.requirements.is_empty() {
        return muted("Requires nothing from other plugins.", cx).into_any_element();
    }
    v_flex()
        .gap_2()
        .children(entry.requirements.iter().map(|requirement| {
            let (icon, color) = match requirement.state {
                RowState::Met => (IconName::Check, cx.theme().success),
                RowState::Absent => (IconName::Info, cx.theme().muted_foreground),
                RowState::Inactive => (IconName::Info, cx.theme().muted_foreground),
                RowState::Unmet => (IconName::CircleX, cx.theme().danger),
            };
            h_flex()
                .gap_2()
                .items_start()
                .child(Icon::new(icon).size_4().text_color(color))
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family("monospace")
                                        .text_color(cx.theme().foreground)
                                        .child(requirement.contract.clone()),
                                )
                                .when(requirement.optional, |this| this.child(tag("optional", cx))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(color)
                                .child(requirement.note.clone()),
                        ),
                )
        }))
        .into_any_element()
}

fn contribution_group(group: &Group, cx: &mut Context<PluginManagerWindow>) -> AnyElement {
    let muted_color = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    let items: Vec<AnyElement> = group
        .items
        .iter()
        .map(|item| {
            let details: Vec<AnyElement> = item
                .details
                .iter()
                .map(|detail| line(detail.clone(), muted_color, cx).into_any_element())
                .collect();
            v_flex()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(foreground)
                                .child(item.label.clone()),
                        )
                        .when(item.label != item.id, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(muted_color)
                                    .child(item.id.clone()),
                            )
                        }),
                )
                .children(details)
                .into_any_element()
        })
        .collect();
    section_titled(
        format!("{} · {}", group.title, group.items.len()),
        v_flex().gap_2().children(items).into_any_element(),
        cx,
    )
    .into_any_element()
}

fn section(
    title: &'static str,
    body: AnyElement,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    section_titled(title.to_string(), body, cx)
}

fn section_titled(
    title: String,
    body: AnyElement,
    cx: &mut Context<PluginManagerWindow>,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border.opacity(0.65))
        .bg(cx.theme().sidebar.opacity(0.45))
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title),
        )
        .child(body)
}

fn tag(text: &str, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    div()
        .px_1p5()
        .py_px()
        .rounded_sm()
        .bg(cx.theme().secondary.opacity(0.65))
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

fn muted(text: &'static str, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn line(text: String, color: Hsla, cx: &mut Context<PluginManagerWindow>) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_2()
        .items_start()
        .child(
            Icon::new(IconName::ChevronRight)
                .xsmall()
                .flex_shrink_0()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(color)
                .child(text),
        )
}
