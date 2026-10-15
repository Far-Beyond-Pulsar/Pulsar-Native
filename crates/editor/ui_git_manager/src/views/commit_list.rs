//! Virtualized commit list: a graph variant (Tree layout) and a compact
//! avatar variant (Desktop layout).

use super::common::{FONT_MONO, avatar, lane_color, pill, placeholder, relative_time, virtual_panel};
use crate::models::*;
use crate::{GitManager, manager::GRAPH_ROW_HEIGHT};
use gpui::{prelude::FluentBuilder as _, *};
use ui::{ActiveTheme as _, IconName, h_flex, v_flex, v_virtual_list};

const LANE_WIDTH: f32 = 14.0;
const MAX_LANES: usize = 12;
/// How many rows before the end of the loaded history the next page starts loading.
const PREFETCH_ROWS: usize = 60;

pub(crate) fn render_commit_list(
    gm: &mut GitManager,
    graph: bool,
    cx: &mut Context<GitManager>,
) -> impl IntoElement + use<> {
    if gm.commits.is_empty() {
        let message = if gm.commits_loading || !gm.commits_exhausted {
            "Loading history…"
        } else if gm.repo_state.has_commits {
            "No commits to show"
        } else {
            "No commits yet. Stage your files and make the first commit."
        };
        return placeholder(IconName::GitCommit, message, cx).into_any_element();
    }

    let sizes = if graph {
        gm.commit_sizes_graph.clone()
    } else {
        gm.commit_sizes_compact.clone()
    };
    let entity = cx.entity().clone();
    let list = v_virtual_list(
        entity,
        if graph { "commit-graph-list" } else { "commit-compact-list" },
        sizes,
        move |gm: &mut GitManager, range: std::ops::Range<usize>, _window, cx| {
            if range.end + PREFETCH_ROWS >= gm.commits.len() {
                gm.load_more_commits(cx);
            }
            let rows: Vec<GraphCommit> = range
                .filter_map(|index| gm.commits.get(index).cloned())
                .collect();
            rows.into_iter()
                .map(|commit| {
                    if graph {
                        graph_row(gm, commit, cx)
                    } else {
                        compact_row(gm, commit, cx)
                    }
                })
                .collect::<Vec<AnyElement>>()
        },
    )
    .track_scroll(&gm.commit_list.scroll);

    let footer = gm.commits_loading;
    v_flex()
        .size_full()
        .child(virtual_panel(list, &gm.commit_list))
        .when(footer, |this| {
            this.child(
                div()
                    .h(px(22.))
                    .flex_shrink_0()
                    .px_3()
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child("Loading more commits…"),
            )
        })
        .into_any_element()
}

fn ref_pills(commit: &GraphCommit, cx: &App) -> Vec<Div> {
    let theme = cx.theme();
    commit
        .refs
        .iter()
        .take(4)
        .map(|label| match label.kind {
            RefKind::Head => pill(label.name.clone(), theme.primary, true),
            RefKind::Local => pill(label.name.clone(), theme.success, false),
            RefKind::Remote => pill(label.name.clone(), theme.info, false),
            RefKind::Tag => pill(label.name.clone(), theme.warning, false),
        })
        .collect()
}

fn graph_row(gm: &mut GitManager, commit: GraphCommit, cx: &mut Context<GitManager>) -> AnyElement {
    let theme = cx.theme();
    let (foreground, muted_fg, list_active, list_hover, accent, background, radius) = (
        theme.foreground,
        theme.muted_foreground,
        theme.list_active,
        theme.list_hover,
        theme.list_active_border,
        theme.background,
        theme.radius,
    );
    let selected = gm.selected_commit.as_deref() == Some(commit.hash.as_str());
    let lanes = gm.graph_lanes.clamp(1, MAX_LANES);
    let hash = commit.hash.clone();
    let pills = ref_pills(&commit, cx);
    let merge = commit.parents.len() > 1;

    h_flex()
        .id(SharedString::from(format!("commit-{}", commit.hash)))
        .w_full()
        .h(px(GRAPH_ROW_HEIGHT))
        .items_center()
        .gap_2()
        .pr_3()
        .rounded(radius)
        .cursor_pointer()
        .bg(if selected { list_active } else { transparent_black() })
        .hover(move |s| s.bg(if selected { list_active } else { list_hover }))
        .when(selected, |this| this.border_l_2().border_color(accent))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_commit(hash.clone(), cx);
            }),
        )
        .child(graph_cell(commit.graph.clone(), lanes, merge, background))
        .children(pills)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_xs()
                .text_color(foreground)
                .child(commit.summary.clone()),
        )
        .child(
            div()
                .w(px(130.))
                .flex_shrink_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_xs()
                .text_color(muted_fg)
                .child(commit.author.clone()),
        )
        .child(
            div()
                .w(px(64.))
                .flex_shrink_0()
                .text_xs()
                .text_color(muted_fg)
                .child(relative_time(commit.timestamp)),
        )
        .child(
            div()
                .w(px(56.))
                .flex_shrink_0()
                .font_family(FONT_MONO)
                .text_size(px(11.))
                .text_color(muted_fg)
                .child(commit.short_hash.clone()),
        )
        .into_any_element()
}

fn compact_row(
    gm: &mut GitManager,
    commit: GraphCommit,
    cx: &mut Context<GitManager>,
) -> AnyElement {
    gm.ensure_avatar_loaded(&commit.email, cx);
    let image = gm.avatar_cache.get(&commit.email).and_then(|v| v.clone());

    let theme = cx.theme();
    let (foreground, muted_fg, list_active, list_hover, accent, radius, primary) = (
        theme.foreground,
        theme.muted_foreground,
        theme.list_active,
        theme.list_hover,
        theme.list_active_border,
        theme.radius,
        theme.primary,
    );
    let selected = gm.selected_commit.as_deref() == Some(commit.hash.as_str());
    let hash = commit.hash.clone();
    let pills = ref_pills(&commit, cx);

    h_flex()
        .id(SharedString::from(format!("commit-{}", commit.hash)))
        .w_full()
        .h(px(crate::manager::COMPACT_ROW_HEIGHT))
        .px_2()
        .gap_2()
        .items_center()
        .rounded(radius)
        .cursor_pointer()
        .bg(if selected { list_active } else { transparent_black() })
        .hover(move |s| s.bg(if selected { list_active } else { list_hover }))
        .when(selected, |this| this.border_l_2().border_color(accent))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                this.select_commit(hash.clone(), cx);
            }),
        )
        .child(avatar(image, &commit.author, 28.0))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_px()
                .overflow_hidden()
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(foreground)
                                .child(commit.summary.clone()),
                        )
                        .children(pills),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(10.))
                        .text_color(muted_fg)
                        .child(format!(
                            "{} · {}",
                            commit.author,
                            relative_time(commit.timestamp)
                        )),
                ),
        )
        .child(
            div()
                .px(px(5.))
                .py(px(2.))
                .rounded(px(4.))
                .bg(primary.opacity(0.12))
                .flex_shrink_0()
                .font_family(FONT_MONO)
                .text_size(px(10.))
                .text_color(primary)
                .child(commit.short_hash.clone()),
        )
        .into_any_element()
}

fn stroke(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>, color: Hsla) {
    let mut builder = PathBuilder::stroke(px(1.6));
    builder.move_to(from);
    builder.line_to(to);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// The lane drawing of one row: through lines, merges in, parents out, node.
fn graph_cell(row: GraphRow, lanes: usize, merge: bool, background: Hsla) -> impl IntoElement + use<> {
    let width = lanes as f32 * LANE_WIDTH + 6.0;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let lane_x = |lane: usize| {
                bounds.origin.x + px(3.0 + LANE_WIDTH * lane.min(lanes - 1) as f32 + LANE_WIDTH / 2.0)
            };
            let top = bounds.origin.y;
            let bottom = top + bounds.size.height;
            let mid = top + bounds.size.height / 2.0;
            let node_x = lane_x(row.lane);

            for &(lane, color) in &row.through {
                if lane < lanes {
                    let x = lane_x(lane);
                    stroke(window, point(x, top), point(x, bottom), lane_color(color));
                }
            }
            for &(lane, color) in &row.incoming {
                stroke(window, point(lane_x(lane), top), point(node_x, mid), lane_color(color));
            }
            for &(lane, color) in &row.outgoing {
                stroke(window, point(node_x, mid), point(lane_x(lane), bottom), lane_color(color));
            }

            let radius = px(4.5);
            let color = lane_color(row.color);
            let node = Bounds::new(
                point(node_x - radius, mid - radius),
                size(radius * 2.0, radius * 2.0),
            );
            window.paint_quad(fill(node, color).corner_radii(Corners::all(radius)));
            if merge {
                let inner = px(2.2);
                let hole = Bounds::new(
                    point(node_x - inner, mid - inner),
                    size(inner * 2.0, inner * 2.0),
                );
                window.paint_quad(fill(hole, background).corner_radii(Corners::all(inner)));
            }
        },
    )
    .w(px(width))
    .h_full()
    .flex_shrink_0()
}
