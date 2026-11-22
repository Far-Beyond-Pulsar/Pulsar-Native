//! The navigation tree: the search box over a virtualized list of the
//! visible tree rows (a folder, its pages, a page's sections).
//!
//! The search box is a plain full-width input in a column, not a header slot
//! of `ui::sidebar::Sidebar`: that header lays its child out in a row whose
//! width the input could not claim, which is what kept the old search box
//! squeezed to its icon.

use gpui::{
    div, prelude::FluentBuilder as _, px, uniform_list, AnyElement, Context, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled,
};
use ui::{h_flex, input::TextInput, scroll::Scrollbar, v_flex, ActiveTheme as _, Icon, IconName, Sizable as _};

use super::{empty_note, SettingsScreen};
use crate::utils::model::{Target, TreeRow};

const ROW_HEIGHT: f32 = 28.0;
const INDENT: f32 = 16.0;

impl SettingsScreen {
    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.tree.len();
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                div().w_full().p_3().child(
                    TextInput::new(&self.search)
                        .w_full()
                        .small()
                        .prefix(
                            Icon::new(IconName::Search)
                                .size_4()
                                .text_color(theme.muted_foreground),
                        )
                        .cleanable(),
                ),
            )
            .child(if count == 0 {
                empty_note(
                    if self.query.is_empty() {
                        "No settings are registered."
                    } else {
                        "Nothing matches your search."
                    },
                    cx,
                )
                .into_any_element()
            } else {
                self.render_tree(count, cx).into_any_element()
            })
    }

    fn render_tree(&self, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.tree_scroll.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(
                uniform_list(
                    "settings-tree",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|ix| this.tree.get(ix).cloned())
                            .map(|row| this.render_tree_row(row, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .pb_2()
                .track_scroll(&handle),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::uniform_scroll(&self.tree_scrollbar, &handle)),
            )
    }

    fn render_tree_row(&self, row: TreeRow, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let selected = self.is_row_selected(&row);
        let (foreground, background) = if selected {
            (theme.sidebar_accent_foreground, Some(theme.sidebar_accent))
        } else {
            (theme.sidebar_foreground, None)
        };
        let is_section = matches!(row.target, Target::Section(..));
        let icon = match row.target {
            Target::Folder if row.expanded => Some(IconName::FolderOpen),
            Target::Folder => Some(IconName::Folder),
            Target::Page(ix) if self.pages[ix].plugin => Some(IconName::Puzzle),
            Target::Page(_) => Some(IconName::Settings2),
            Target::Section(..) => None,
        };
        let chevron_id = SharedString::from(format!("settings-tree-chevron-{}", row.id));
        let toggle_id = row.id.clone();
        let target = row.target;
        let click_id = row.id.clone();

        h_flex()
            .id(SharedString::from(format!("settings-tree-{}", row.id)))
            .h(px(ROW_HEIGHT))
            .mx_2()
            .pl(px(8.0 + row.depth as f32 * INDENT))
            .pr_2()
            .gap_1p5()
            .rounded(theme.radius)
            .text_sm()
            .text_color(if is_section && !selected {
                theme.muted_foreground
            } else {
                foreground
            })
            .when_some(background, |this, bg| this.bg(bg))
            .when(!selected, |this| this.hover(|this| this.bg(theme.sidebar_accent.opacity(0.5))))
            .cursor_pointer()
            .child(
                div()
                    .id(chevron_id)
                    .w(px(16.0))
                    .flex_shrink_0()
                    .when(row.expandable, |this| {
                        this.child(
                            Icon::new(if row.expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size_3p5()
                            .text_color(theme.muted_foreground),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            crate::handlers::toggle(this, &toggle_id);
                            cx.notify();
                        }))
                    }),
            )
            .when_some(icon, |this, icon| {
                this.child(Icon::new(icon).size_4().flex_shrink_0())
            })
            .child(div().flex_1().min_w_0().truncate().child(row.label))
            .on_click(cx.listener(move |this, _, _, cx| {
                crate::handlers::activate(this, target, &click_id);
                cx.notify();
            }))
            .into_any_element()
    }

    /// The row of the page shown is highlighted, and the row of the section
    /// the content is scrolled to (the page row then stays unhighlighted).
    fn is_row_selected(&self, row: &TreeRow) -> bool {
        let Some((page_ix, _)) = self.current_page() else {
            return false;
        };
        match row.target {
            Target::Folder => false,
            Target::Page(ix) => {
                ix == page_ix
                    && (self.selected_section.is_none() || !row.expanded || self.pages[ix].sections.len() < 2)
            }
            Target::Section(ix, section) => ix == page_ix && self.selected_section == Some(section),
        }
    }
}
