//! The left side of the window: the source switcher, search, and the tree.

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::TextInput,
    scroll::Scrollbar,
    v_flex,
};

use crate::DocumentationWindow;
use crate::handlers;
use crate::utils::{DocCategory, DocRow, RowKind, rows};

const ROW_HEIGHT: f32 = 28.0;
const INDENT: f32 = 14.0;

impl DocumentationWindow {
    pub(crate) fn row_count(&self) -> usize {
        match self.current_category {
            DocCategory::Engine => rows::engine_row_count(&self.engine_docs),
            DocCategory::Project => rows::project_row_count(&self.project_docs),
            DocCategory::Manual => rows::manual_row_count(&self.manual_docs),
        }
    }

    fn row_at(&self, ix: usize) -> Option<DocRow> {
        match self.current_category {
            DocCategory::Engine => rows::engine_row(&self.engine_docs, ix),
            DocCategory::Project => rows::project_row(&self.project_docs, ix),
            DocCategory::Manual => rows::manual_row(&self.manual_docs, ix),
        }
    }

    pub(crate) fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let category = self.current_category;
        let count = self.row_count();

        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .child(self.render_switcher(cx))
                    .child(self.render_search_row(cx)),
            )
            .child(
                h_flex()
                    .px_4()
                    .pb_1()
                    .justify_between()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(match category {
                        DocCategory::Engine => "API REFERENCE",
                        DocCategory::Project => "YOUR PROJECT",
                        DocCategory::Manual => "GUIDES",
                    })
                    .when(count > 0, |this| this.child(count.to_string())),
            )
            .child(if count == 0 {
                self.render_empty_tree(cx).into_any_element()
            } else {
                self.render_tree(count, cx).into_any_element()
            })
            .into_any_element()
    }

    fn render_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.current_category;
        ButtonGroup::new("docs-category")
            .children(DocCategory::ALL.map(|category| {
                Button::new(category.label())
                    .label(category.label())
                    .icon(category.icon())
                    .small()
                    .selected(current == category)
            }))
            .ghost()
            .on_click(cx.listener(|this, selected: &Vec<usize>, _, cx| {
                if let Some(category) = selected.first().and_then(|ix| DocCategory::ALL.get(*ix)) {
                    handlers::set_category(this, *category);
                    cx.notify();
                }
            }))
    }

    fn render_search_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        match self.current_category {
            DocCategory::Manual => Button::new("new-guide")
                .label("New guide")
                .icon(IconName::Plus)
                .small()
                .w_full()
                .on_click(cx.listener(|this, _, _, cx| {
                    handlers::open_new_file_dialog(this);
                    cx.notify();
                }))
                .into_any_element(),
            category => {
                let input = match category {
                    DocCategory::Engine => &self.engine_docs.search_input_state,
                    _ => &self.project_docs.search_input_state,
                };
                TextInput::new(input)
                    .w_full()
                    .prefix(
                        Icon::new(IconName::Search)
                            .size_4()
                            .text_color(theme.muted_foreground),
                    )
                    .appearance(true)
                    .bordered(true)
                    .into_any_element()
            }
        }
    }

    fn render_tree(&mut self, count: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let handle = self.tree_scroll.clone();
        let scrollbar_state = self.tree_scrollbar.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                uniform_list(
                    "docs-tree",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|ix| this.row_at(ix))
                            .map(|row| render_row(row, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .py_1()
                .track_scroll(&handle),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::uniform_scroll(&scrollbar_state, &handle)),
            )
    }

    fn render_empty_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let searching = match self.current_category {
            DocCategory::Engine => !self.engine_docs.search_query.is_empty(),
            DocCategory::Project => !self.project_docs.search_query.is_empty(),
            DocCategory::Manual => false,
        };
        let (icon, text) = if searching {
            (IconName::Search, "Nothing matches your search.")
        } else {
            match self.current_category {
                DocCategory::Engine => (
                    IconName::Code,
                    "Engine docs are generated by a release build.",
                ),
                DocCategory::Project => (IconName::Package, "Open a project to see its API."),
                DocCategory::Manual => (IconName::BookOpen, "No guides yet. Create the first one."),
            }
        };
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .text_center()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(Icon::new(icon).size_6())
            .child(text)
    }
}

fn render_row(row: DocRow, cx: &mut Context<DocumentationWindow>) -> AnyElement {
    let theme = cx.theme();
    let selected = row.selected;
    let is_leaf = row.kind == RowKind::Leaf;
    let action = row.action.clone();
    let foreground = if selected {
        theme.sidebar_accent_foreground
    } else {
        theme.sidebar_foreground
    };
    let icon_color = if selected {
        theme.primary
    } else {
        theme.muted_foreground
    };

    h_flex()
        .id(row.id.clone())
        .h(px(ROW_HEIGHT))
        .mx_2()
        .pl(px(6.0 + row.depth as f32 * INDENT))
        .pr_2()
        .gap_1p5()
        .items_center()
        .rounded(theme.radius)
        .cursor_pointer()
        .text_sm()
        .text_color(foreground)
        .when(selected, |el| el.bg(theme.sidebar_accent))
        .when(!selected, |el| el.hover(|el| el.bg(theme.list_hover)))
        .child(if is_leaf {
            div().w(px(12.0)).flex_none().into_any_element()
        } else {
            Icon::new(if row.expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .size_3()
            .text_color(theme.muted_foreground)
            .into_any_element()
        })
        .child(Icon::new(row.icon).size_3p5().text_color(icon_color))
        .child(div().flex_1().min_w_0().truncate().child(row.label))
        .when_some(row.count, |el, count| {
            el.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(count.to_string()),
            )
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            handlers::activate_row(this, action.clone(), window, cx);
            cx.notify();
        }))
        .into_any_element()
}
