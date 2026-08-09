use gpui::{
    div, prelude::FluentBuilder as _, px, App, AppContext as _, Context, DismissEvent, Entity,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Render, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled as _, Window,
};
use ui::scroll::{Scrollbar, ScrollbarState};
use ui::{
    h_flex,
    input::{InputState, TextInput},
    v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
};

use crate::utils::git_integration::CommitInfo;

#[derive(Clone, Debug)]
pub struct CommitSelected(pub String);

pub struct CommitPicker {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    scroll_handle: ScrollHandle,
    scroll_state: ScrollbarState,
    commits: Vec<CommitInfo>,
}

impl CommitPicker {
    pub fn new(
        commits: Vec<CommitInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search commits…"));
        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            scroll_handle: ScrollHandle::new(),
            scroll_state: ScrollbarState::default(),
            commits,
        }
    }

    pub fn set_commits(&mut self, commits: Vec<CommitInfo>) {
        self.commits = commits;
    }
}

impl gpui::EventEmitter<DismissEvent> for CommitPicker {}
impl gpui::EventEmitter<CommitSelected> for CommitPicker {}

impl Focusable for CommitPicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CommitPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).value().to_string();
        let query_lower = query.to_lowercase();

        let filtered: Vec<&CommitInfo> = self
            .commits
            .iter()
            .filter(|c| {
                query_lower.is_empty()
                    || c.hash.to_lowercase().contains(&query_lower)
                    || c.short_hash.to_lowercase().contains(&query_lower)
                    || c.subject.to_lowercase().contains(&query_lower)
            })
            .collect();

        let is_empty = filtered.is_empty();

        let bg = cx.theme().background;
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let hover_bg = cx.theme().secondary;

        v_flex()
            .w(px(360.))
            .bg(bg)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .child(
                h_flex()
                    .px_2()
                    .py(px(6.))
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Search).size(px(14.)).text_color(muted))
                    .child(
                        div()
                            .flex_1()
                            .child(TextInput::new(&self.search_input).small()),
                    ),
            )
            .child(
                div()
                    .id("commit-picker-list")
                    .relative()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("commit-picker-scroll")
                            .max_h(px(400.))
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .py_1()
                            .when(is_empty, |el| {
                                el.child(
                                    div()
                                        .px_4()
                                        .py_4()
                                        .text_sm()
                                        .text_color(muted)
                                        .child("No commits match your search."),
                                )
                            })
                            .children(filtered.into_iter().map(|c| {
                                let hash = c.hash.clone();
                                let short = c.short_hash.clone();
                                let subject = c.subject.clone();
                                let date = c.date.clone();

                                h_flex()
                                    .id(SharedString::from(format!("commit-{}", short)))
                                    .w_full()
                                    .px_3()
                                    .py(px(6.))
                                    .gap_2()
                                    .items_center()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(hover_bg))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(CommitSelected(hash.clone()));
                                            cx.emit(DismissEvent);
                                        }),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(
                                                div()
                                                    .flex()
                                                    .gap_2()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .font_family("monospace")
                                                            .text_xs()
                                                            .text_color(muted)
                                                            .child(short),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_1()
                                                            .text_sm()
                                                            .text_color(fg)
                                                            .overflow_hidden()
                                                            .child(subject),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(date),
                                            ),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .child(Scrollbar::vertical(
                                &self.scroll_state,
                                &self.scroll_handle,
                            )),
                    ),
            )
    }
}
