use gpui::{
    div, prelude::FluentBuilder, px, App, AppContext, Context, Div, Entity, EventEmitter,
    FocusHandle,
    Focusable, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render,
    SharedString, Styled, Window,
};
use std::rc::Rc;

use crate::{
    input::{InputState, TextInput},
    scroll::ScrollbarAxis,
    v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt,
};

pub enum SearchableListEvent<T: Clone + 'static> {
    Select(T),
}

pub struct SearchableList<T: Clone + 'static> {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    items: Vec<T>,
    format_item: Rc<dyn Fn(&T) -> String>,
    icon_for: Option<Rc<dyn Fn(&T) -> IconName>>,
    empty_text: SharedString,
    max_width: Pixels,
    max_height: Pixels,
}

impl<T: Clone + 'static> EventEmitter<SearchableListEvent<T>> for SearchableList<T> {}

impl<T: Clone + 'static> Focusable for SearchableList<T> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl<T: Clone + 'static> SearchableList<T> {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        items: Vec<T>,
        format_item: impl Fn(&T) -> String + 'static,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search..."));
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            items,
            format_item: Rc::new(format_item),
            icon_for: None,
            empty_text: "No results".into(),
            max_width: px(240.0),
            max_height: px(320.0),
        }
    }

    pub fn with_empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty_text = text.into();
        self
    }

    pub fn with_max_width(mut self, width: Pixels) -> Self {
        self.max_width = width;
        self
    }

    pub fn with_max_height(mut self, height: Pixels) -> Self {
        self.max_height = height;
        self
    }

    pub fn with_icon_getter(mut self, icon_for: impl Fn(&T) -> IconName + 'static) -> Self {
        self.icon_for = Some(Rc::new(icon_for));
        self
    }

    pub fn set_items(&mut self, items: Vec<T>, cx: &mut Context<Self>) {
        self.items = items;
        cx.notify();
    }
}

impl<T: Clone + 'static> Render for SearchableList<T> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).value().to_lowercase();

        let filtered_items: Vec<T> = self
            .items
            .iter()
            .filter(|item| {
                query.is_empty() || (self.format_item)(item).to_lowercase().contains(&query)
            })
            .cloned()
            .collect();

        let row_style = |el: Div| {
            el.flex()
                .flex_row()
                .w_full()
                .h(px(28.0))
                .px_2()
                .gap_2()
                .items_center()
                .cursor_pointer()
                .rounded(px(4.0))
        };

        v_flex()
            .w(self.max_width)
            .max_h(self.max_height)
            .p_1()
            .gap_1()
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .px_1()
                    .pb_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(TextInput::new(&self.search_input).w_full().xsmall()),
            )
            .child(
                div().flex_1().w_full().overflow_hidden().child(
                    div().size_full().scrollable(ScrollbarAxis::Vertical).child(
                        v_flex()
                            .w_full()
                            .when(filtered_items.is_empty(), |el| {
                                el.child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(self.empty_text.clone()),
                                )
                            })
                            .children(filtered_items.into_iter().enumerate().map(|(ix, item)| {
                                let label = (self.format_item)(&item);
                                let icon = self.icon_for.as_ref().map(|f| f(&item));
                                let theme = cx.theme().clone();
                                let selected_item = item.clone();

                                row_style(div())
                                    .id(("searchable-list-item", ix))
                                    .hover(move |s| s.bg(theme.accent.opacity(0.12)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |_this, _, _, cx| {
                                            cx.emit(SearchableListEvent::Select(selected_item.clone()));
                                        }),
                                    )
                                    .when_some(icon, |el, icon| {
                                        el.child(
                                            Icon::new(icon)
                                                .size(px(13.0))
                                                .text_color(cx.theme().muted_foreground),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().foreground)
                                            .child(label),
                                    )
                            })),
                    ),
                ),
            )
    }
}
