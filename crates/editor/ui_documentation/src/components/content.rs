//! The right side of the window: breadcrumb and actions on top, the page below.

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt as _,
    breadcrumb::{Breadcrumb, BreadcrumbItem},
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::TextInput,
    scroll::ScrollbarAxis,
    text::TextView,
    v_flex,
};

use crate::DocumentationWindow;
use crate::handlers;
use crate::utils::{DocCategory, ViewMode};

const MODES: [ViewMode; 3] = [ViewMode::Editor, ViewMode::Preview, ViewMode::Split];

impl DocumentationWindow {
    pub(crate) fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        v_flex()
            .size_full()
            .min_w_0()
            .bg(theme.background)
            .child(self.render_header(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(match self.current_category {
                        DocCategory::Engine => {
                            let page = self.engine_docs.current_path.clone();
                            let markdown = self.engine_docs.markdown_content.clone();
                            render_page("engine-page", page, markdown, window, cx)
                        }
                        DocCategory::Project => {
                            let page = self.project_docs.current_path.clone();
                            let markdown = self.project_docs.markdown_content.clone();
                            render_page("project-page", page, markdown, window, cx)
                        }
                        DocCategory::Manual => self.render_guide(window, cx),
                    }),
            )
            .into_any_element()
    }

    /// The parts of the open page's location, outermost first.
    fn crumbs(&self) -> Vec<String> {
        let parts = match self.current_category {
            DocCategory::Engine => self
                .engine_docs
                .current_path
                .as_deref()
                .map(|path| path.split('/').map(str::to_owned).collect::<Vec<_>>()),
            DocCategory::Project => self
                .project_docs
                .current_path
                .as_deref()
                .map(|path| path.split("::").map(str::to_owned).collect::<Vec<_>>()),
            DocCategory::Manual => self.manual_docs.selected_file.as_ref().map(|file| {
                let relative = self
                    .manual_docs
                    .docs_folder
                    .as_ref()
                    .and_then(|folder| file.strip_prefix(folder).ok())
                    .unwrap_or(file);
                relative
                    .components()
                    .map(|part| {
                        part.as_os_str()
                            .to_string_lossy()
                            .trim_end_matches(".md")
                            .to_owned()
                    })
                    .collect()
            }),
        };
        parts
            .filter(|parts| !parts.is_empty())
            .unwrap_or_else(|| vec![self.current_category.label().to_owned()])
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let category = self.current_category;
        let crumbs = self.crumbs();

        h_flex()
            .w_full()
            .h(px(44.0))
            .flex_none()
            .px_4()
            .gap_3()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(
                        Icon::new(category.icon())
                            .size_4()
                            .text_color(theme.muted_foreground),
                    )
                    .child(crumbs.into_iter().enumerate().fold(
                        Breadcrumb::new(),
                        |breadcrumb, (ix, part)| {
                            breadcrumb.item(BreadcrumbItem::new(
                                SharedString::from(format!("crumb-{ix}")),
                                part,
                            ))
                        },
                    )),
            )
            .child(match category {
                DocCategory::Manual => self.render_guide_actions(cx).into_any_element(),
                _ => Button::new("refresh-docs")
                    .icon(IconName::Refresh)
                    .ghost()
                    .small()
                    .tooltip("Reload")
                    .on_click(cx.listener(|this, _, _, cx| {
                        handlers::refresh_current_category(this);
                        cx.notify();
                    }))
                    .into_any_element(),
            })
    }

    fn render_guide_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let has_file = self.manual_docs.selected_file.is_some();
        let dirty = self.manual_docs.is_dirty();
        let mode = self.manual_docs.view_mode;

        h_flex()
            .gap_2()
            .items_center()
            .when(dirty, |this| {
                this.child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(div().size_1p5().rounded_full().bg(theme.warning))
                        .child("Unsaved"),
                )
            })
            .when(has_file, |this| {
                this.child(
                    ButtonGroup::new("guide-mode")
                        .child(
                            Button::new("mode-edit")
                                .label("Edit")
                                .icon(IconName::Code)
                                .small()
                                .selected(mode == ViewMode::Editor),
                        )
                        .child(
                            Button::new("mode-preview")
                                .label("Preview")
                                .icon(IconName::Eye)
                                .small()
                                .selected(mode == ViewMode::Preview),
                        )
                        .child(
                            Button::new("mode-split")
                                .label("Split")
                                .icon(IconName::SplitArea)
                                .small()
                                .selected(mode == ViewMode::Split),
                        )
                        .ghost()
                        .on_click(cx.listener(|this, selected: &Vec<usize>, _, cx| {
                            if let Some(mode) = selected.first().and_then(|ix| MODES.get(*ix)) {
                                handlers::set_view_mode(this, *mode);
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new("save-guide")
                        .label("Save")
                        .icon(IconName::Check)
                        .small()
                        .when(dirty, |button| button.primary())
                        .when(!dirty, |button| button.ghost())
                        .on_click(cx.listener(|this, _, window, cx| {
                            handlers::save_current_file(this, window, cx);
                            cx.notify();
                        })),
                )
            })
    }

    fn render_guide(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.manual_docs.selected_file.is_none() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .text_color(theme.muted_foreground)
                .child(Icon::new(IconName::BookOpen).size_8())
                .child(
                    div()
                        .text_sm()
                        .child("Pick a guide from the list, or write a new one."),
                )
                .child(
                    Button::new("empty-new-guide")
                        .label("New guide")
                        .icon(IconName::Plus)
                        .small()
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            handlers::open_new_file_dialog(this);
                            cx.notify();
                        })),
                )
                .into_any_element();
        }

        let markdown = self.manual_docs.markdown_preview.clone();
        let editor = self.manual_docs.editor_input_state.clone();
        let editor_pane = div().size_full().child(
            TextInput::new(&editor)
                .w_full()
                .h_full()
                .appearance(false)
                .bordered(false),
        );
        let preview_pane = div().size_full().scrollable(ScrollbarAxis::Vertical).child(
            v_flex()
                .w_full()
                .max_w(px(860.0))
                .mx_auto()
                .px_8()
                .py_6()
                .child(
                    TextView::markdown("guide-preview", markdown, window, cx)
                        .debounce_ms(30)
                        .selectable(),
                ),
        );

        match self.manual_docs.view_mode {
            ViewMode::Editor => editor_pane.into_any_element(),
            ViewMode::Preview => preview_pane.into_any_element(),
            ViewMode::Split => h_flex()
                .size_full()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_r_1()
                        .border_color(theme.border)
                        .child(editor_pane),
                )
                .child(div().flex_1().min_w_0().h_full().child(preview_pane))
                .into_any_element(),
        }
    }
}

/// A rendered page of API documentation. With no page chosen the source's own
/// message (welcome, nothing generated, load error) is shown narrower and lower.
fn render_page(
    id: &'static str,
    page: Option<String>,
    markdown: String,
    window: &mut Window,
    cx: &mut Context<DocumentationWindow>,
) -> AnyElement {
    let has_page = page.is_some();
    let key = SharedString::from(format!("{id}-{}", page.as_deref().unwrap_or("none")));
    div()
        .size_full()
        .scrollable(ScrollbarAxis::Vertical)
        .child(
            v_flex()
                .w_full()
                .max_w(px(if has_page { 860.0 } else { 620.0 }))
                .mx_auto()
                .px_8()
                .py_6()
                .when(!has_page, |this| this.pt(px(72.0)))
                .child(TextView::markdown(key, markdown, window, cx).selectable()),
        )
        .into_any_element()
}
