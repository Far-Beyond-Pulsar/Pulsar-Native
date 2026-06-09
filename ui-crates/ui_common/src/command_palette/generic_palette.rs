use gpui::{
    div, prelude::*, px, Axis, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, MouseButton, Render, SharedString, StyledText, Window,
};
use ui::{
    h_flex,
    input::{Escape, InputEvent, InputState, TextInput},
    scroll::{Scrollbar, ScrollbarState},
    text::TextView,
    v_flex, ActiveTheme as _, Icon, IconName, StyledExt, VirtualListScrollHandle,
};
use ui::scroll::Scrollable;

use super::palette_trait::{PaletteDelegate, PaletteItem};

struct CategoryState {
    name: String,
    expanded: bool,
}

/// Generic palette component that works with any PaletteDelegate
/// Handles all rendering - delegates just provide data
pub struct GenericPalette<D: PaletteDelegate> {
    focus_handle: FocusHandle,
    pub search_input: Entity<InputState>,
    delegate: D,
    filtered_categories: Vec<(String, Vec<D::Item>)>,
    category_states: Vec<CategoryState>,
    selected_index: usize,
    show_docs: bool,
    /// Cached lowercased query for text highlighting
    search_query: SharedString,
    /// Scroll handle for the item list
    scroll_handle: VirtualListScrollHandle,
    /// Scrollbar state for the item list
    scrollbar_state: ScrollbarState,
    /// Scroll handle for the docs panel
    docs_scroll_handle: VirtualListScrollHandle,
    /// Scrollbar state for the docs panel
    docs_scrollbar_state: ScrollbarState,
}

impl<D: PaletteDelegate> EventEmitter<DismissEvent> for GenericPalette<D> {}

impl<D: PaletteDelegate> GenericPalette<D> {
    pub fn new(delegate: D, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Get all the data we need from delegate before moving it
        let placeholder = delegate.placeholder().to_string();
        let categories = delegate.categories();
        let collapsed = delegate.categories_collapsed_by_default();

        let search_input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder(&placeholder, window, cx);
            state
        });

        let category_states: Vec<CategoryState> = categories
            .iter()
            .map(|(name, _)| CategoryState {
                name: name.clone(),
                expanded: !collapsed,
            })
            .collect();

        let filtered_categories = categories.clone();

        // Subscribe to input changes
        cx.subscribe(&search_input, |this, _input, event: &InputEvent, cx| {
            if event == &InputEvent::Change {
                let query = this.search_input.read(cx).text().to_string();
                this.update_filter(&query);
                cx.notify();
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            delegate,
            filtered_categories,
            category_states,
            selected_index: 0,
            show_docs: false,
            search_query: SharedString::default(),
            scroll_handle: VirtualListScrollHandle::new(),
            scrollbar_state: ScrollbarState::default(),
            docs_scroll_handle: VirtualListScrollHandle::new(),
            docs_scrollbar_state: ScrollbarState::default(),
        }
    }

    pub fn delegate(&self) -> &D {
        &self.delegate
    }

    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
    }

    /// Reset filter state for a fresh palette open
    /// Called when the palette is reopened to clear search and reset selection
    pub fn reset_filter(&mut self) {
        let categories = self.delegate.categories();
        let collapsed = self.delegate.categories_collapsed_by_default();

        self.filtered_categories = categories.clone();
        self.category_states = categories
            .iter()
            .map(|(name, _)| CategoryState {
                name: name.clone(),
                expanded: !collapsed,
            })
            .collect();
        self.selected_index = 0;
        self.search_query = SharedString::default();
        self.scroll_handle = VirtualListScrollHandle::new();
        self.scrollbar_state = ScrollbarState::default();
        self.docs_scroll_handle = VirtualListScrollHandle::new();
        self.docs_scrollbar_state = ScrollbarState::default();
    }

    /// Swap the delegate and update all state
    /// This allows the same GenericPalette instance to show different content
    pub fn swap_delegate(&mut self, new_delegate: D, window: &mut Window, cx: &mut Context<Self>) {
        // Get data from new delegate
        let placeholder = new_delegate.placeholder().to_string();
        let categories = new_delegate.categories();
        let collapsed = new_delegate.categories_collapsed_by_default();

        // Update delegate
        self.delegate = new_delegate;

        // Update placeholder
        self.search_input.update(cx, |input, cx| {
            input.set_placeholder(&placeholder, window, cx);
            input.set_value("", window, cx); // Clear search
        });

        // Update categories
        self.category_states = categories
            .iter()
            .map(|(name, _)| CategoryState {
                name: name.clone(),
                expanded: !collapsed,
            })
            .collect();

        self.filtered_categories = categories;
        self.selected_index = 0;
        self.show_docs = false;
        self.search_query = SharedString::default();
        self.scroll_handle = VirtualListScrollHandle::new();
        self.scrollbar_state = ScrollbarState::default();
        self.docs_scroll_handle = VirtualListScrollHandle::new();
        self.docs_scrollbar_state = ScrollbarState::default();

        cx.notify();
    }

    fn update_filter(&mut self, query: &str) {
        let old_categories = self.filtered_categories.clone();
        self.filtered_categories = self.delegate.filter(query);
        self.search_query = SharedString::from(query.to_lowercase());

        // Update category states
        let collapsed = self.delegate.categories_collapsed_by_default();
        self.category_states = self
            .filtered_categories
            .iter()
            .map(|(name, items)| CategoryState {
                name: name.clone(),
                // Auto-expand categories with matches when searching, or respect default
                expanded: if query.is_empty() {
                    !collapsed
                } else {
                    !items.is_empty()
                },
            })
            .collect();

        // Only reset selection if the filtered results actually changed
        // This prevents arrow key navigation from resetting selection
        let categories_changed = old_categories.len() != self.filtered_categories.len()
            || old_categories
                .iter()
                .zip(self.filtered_categories.iter())
                .any(|(a, b)| a.0 != b.0 || a.1.len() != b.1.len());

        if categories_changed {
            self.selected_index = 0;
        } else {
            // Clamp selection to valid range
            let visible_items = self.get_all_visible_items();
            if !visible_items.is_empty() && self.selected_index >= visible_items.len() {
                self.selected_index = visible_items.len() - 1;
            }
        }
    }

    fn get_all_visible_items(&self) -> Vec<D::Item> {
        self.filtered_categories
            .iter()
            .enumerate()
            .filter(|(idx, _)| {
                self.category_states
                    .get(*idx)
                    .map(|s| s.expanded)
                    .unwrap_or(true)
            })
            .flat_map(|(_, (_, items))| items.iter().cloned())
            .collect()
    }

    fn select_item(&mut self, cx: &mut Context<Self>) {
        let visible_items = self.get_all_visible_items();
        if let Some(item) = visible_items.get(self.selected_index) {
            self.delegate.confirm(item);
            cx.emit(DismissEvent);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let visible_items = self.get_all_visible_items();
        if visible_items.is_empty() {
            return;
        }

        let new_index = ((self.selected_index as isize) + delta)
            .rem_euclid(visible_items.len() as isize) as usize;

        self.selected_index = new_index;
        cx.notify();
    }

    fn toggle_category(&mut self, category_index: usize, cx: &mut Context<Self>) {
        if let Some(state) = self.category_states.get_mut(category_index) {
            state.expanded = !state.expanded;
            cx.notify();
        }
    }

    /// Highlight matching portions of text for a given search query
    /// Returns bold text for the first match
    fn highlight_query(&self, text: &str) -> gpui::AnyElement {
        if self.search_query.is_empty() {
            return StyledText::new(text).into_any_element();
        }

        let text_lower = text.to_lowercase();
        let query_lower = self.search_query.as_ref();

        if let Some(match_pos) = text_lower.find(query_lower) {
            let match_range = match_pos..match_pos + query_lower.len();
            StyledText::new(text)
                .with_highlights([(match_range, gpui::FontWeight::BOLD.into())])
                .into_any_element()
        } else {
            StyledText::new(text).into_any_element()
        }
    }
}

impl<D: PaletteDelegate> Render for GenericPalette<D> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_index = self.selected_index;
        let visible_items = self.get_all_visible_items();
        let selected_item = visible_items.get(selected_index).cloned();
        let show_docs = self.show_docs && self.delegate.supports_docs();

        // Outer wrapper: full-screen darkened background overlay
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000099))
            .track_focus(&self.focus_handle)
            // Block ALL mouse events from falling through
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _event, _window, cx| {
                    cx.emit(DismissEvent);
                }),
            )
            .on_mouse_down(MouseButton::Right, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_down(MouseButton::Middle, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(MouseButton::Right, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_up(MouseButton::Middle, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_mouse_move(|_, _, cx| {
                cx.stop_propagation();
            })
            .on_scroll_wheel(|_, _, cx| {
                cx.stop_propagation();
            })
            .on_action(cx.listener(|_this, _: &Escape, _window, cx| {
                // Handle ESC key action (bubbled up from input or direct)
                cx.emit(DismissEvent);
            }))
            .on_key_down(cx.listener(|_this, event: &KeyDownEvent, _window, cx| {
                // Fallback for raw escape keystrokes
                if event.keystroke.key.as_str() == "escape" {
                    cx.emit(DismissEvent);
                    cx.stop_propagation();
                }
            }))
            .child(self.render_palette_container(selected_index, selected_item.as_ref(), window, cx))
            .when(show_docs, |this| {
                this.child(self.render_docs_panel(selected_item, window, cx))
            })
    }
}

impl<D: PaletteDelegate> GenericPalette<D> {
    /// Renders the main palette panel with search bar and item list.
    fn render_palette_container(
        &mut self,
        selected_index: usize,
        _selected_item: Option<&D::Item>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .w(px(640.))
            .max_w(px(900.))
            .min_w(px(480.))
            .h(px(480.))
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(px(12.))
            .shadow_lg()
            .overflow_hidden()
            .child(self.render_search_bar(cx))
            .child(self.render_item_list(selected_index, cx))
    }

    /// Renders the search input row.
    fn render_search_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .gap_3()
            .items_center()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        cx.emit(DismissEvent);
                        cx.stop_propagation();
                    }
                    "down" | "arrowdown" => {
                        this.move_selection(1, cx);
                        cx.stop_propagation();
                    }
                    "up" | "arrowup" => {
                        this.move_selection(-1, cx);
                        cx.stop_propagation();
                    }
                    "enter" | "return" => {
                        this.select_item(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                Icon::new(IconName::Search)
                    .size(px(18.0))
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                TextInput::new(&self.search_input)
                    .appearance(false)
                    .bordered(false)
                    .flex_1(),
            )
    }

    /// Renders the scrollable item list, or a "no items found" message.
    fn render_item_list(&mut self, selected_index: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.filtered_categories.iter().all(|(_, items)| items.is_empty()) {
            return self.render_no_results(cx);
        }

        let has_categories = self.filtered_categories.iter().any(|(name, _)| !name.is_empty());

        div()
            .relative()
            .h(px(384.))
            .child(
                v_flex()
                    .h_full()
                    .scrollable(Axis::Vertical)
                    .id("palette-list")
                    .track_scroll(&self.scroll_handle)
                    .child(
                        v_flex()
                            .gap_0p5()
                            .p_2()
                            .children(self.render_category_items(selected_index, has_categories, cx)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&self.scrollbar_state, &self.scroll_handle)),
            )
            .into_any_element()
    }

    /// Renders the "no items found" empty state.
    fn render_no_results(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .h(px(320.))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Search)
                            .size(px(32.0))
                            .text_color(cx.theme().muted_foreground.opacity(0.3)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No items found"),
                    ),
            )
            .into_any_element()
    }

    /// Renders all category headers and their items for the item list.
    fn render_category_items(
        &self,
        selected_index: usize,
        has_categories: bool,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        self.filtered_categories
            .iter()
            .enumerate()
            .flat_map(|(cat_idx, (cat_name, items))| {
                let mut elements = Vec::new();
                // Global index across all visible items
                let mut global_index = 0;

                if !cat_name.is_empty() && has_categories {
                    let expanded = self
                        .category_states
                        .get(cat_idx)
                        .map(|s| s.expanded)
                        .unwrap_or(true);

                    elements.push(self.render_category_header(cat_idx, cat_name, items.len(), expanded, cx));

                    if expanded {
                        for item in items {
                            let is_selected = global_index == selected_index;
                            elements.push(self.render_item(item, is_selected, global_index, cx));
                            global_index += 1;
                        }
                    }
                } else {
                    for item in items {
                        let is_selected = global_index == selected_index;
                        elements.push(self.render_item(item, is_selected, global_index, cx));
                        global_index += 1;
                    }
                }

                elements
            })
            .collect()
    }

    /// Renders a single category header row.
    fn render_category_header(
        &self,
        cat_idx: usize,
        cat_name: &str,
        item_count: usize,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        h_flex()
            .w_full()
            .px_2()
            .py_1p5()
            .gap_2()
            .items_center()
            .cursor_pointer()
            .rounded(px(4.))
            .hover(|s| s.bg(cx.theme().muted.opacity(0.1)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.toggle_category(cat_idx, cx);
                }),
            )
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(12.))
                .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(cat_name.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .opacity(0.6)
                    .child(format!("({})", item_count)),
            )
            .into_any_element()
    }

    /// Renders the documentation panel shown on the right when space is pressed.
    fn render_docs_panel(&mut self, selected_item: Option<D::Item>, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let doc_content = selected_item.as_ref().and_then(|item| item.documentation());
        let scroll_handle = self.docs_scroll_handle.clone();
        let scrollbar_state = self.docs_scrollbar_state.clone();

        div()
            .relative()
            .w(px(360.))
            .h(px(480.))
            .child(
                v_flex()
                    .h_full()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(12.))
                    .shadow_lg()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::SubmitDocument)
                                    .size(px(16.0))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Documentation"),
                            ),
                    )
                    .child({
                        div()
                            .relative()
                            .h_full()
                            .child(
                                v_flex()
                                    .h_full()
                                    .scrollable(Axis::Vertical)
                                    .id("palette-docs")
                                    .track_scroll(&scroll_handle)
                                    .child(
                                        v_flex()
                                            .p_4()
                                            .gap_3()
                                            .map(|el| {
                                                if let Some(doc_text) = doc_content {
                                                    el.child(
                                                        TextView::markdown("node-docs", doc_text, window, cx)
                                                            .selectable(),
                                                    )
                                                } else {
                                                    el.child(
                                                        div()
                                                            .h(px(300.))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .child(
                                                                div()
                                                                    .text_sm()
                                                                    .text_color(cx.theme().muted_foreground)
                                                                    .child("No documentation available"),
                                                            ),
                                                    )
                                                }
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .child(Scrollbar::vertical(&scrollbar_state, &scroll_handle)),
                            )
                    }),
            )
    }
}

impl<D: PaletteDelegate> Focusable for GenericPalette<D> {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl<D: PaletteDelegate> GenericPalette<D> {
    fn render_item(
        &self,
        item: &D::Item,
        is_selected: bool,
        item_index: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .rounded(px(6.))
            .gap_3()
            .items_center()
            .cursor_pointer()
            .when(is_selected, |this| {
                this.bg(cx.theme().primary.opacity(0.15))
            })
            .hover(|s| s.bg(cx.theme().muted.opacity(0.2)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.selected_index = item_index;
                    this.select_item(cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.selected_index != item_index {
                    this.selected_index = item_index;
                    cx.notify();
                }
            }))
            .child(
                Icon::new(item.icon())
                    .size(px(18.0))
                    .text_color(if is_selected {
                        cx.theme().primary
                    } else {
                        cx.theme().muted_foreground
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_0p5()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if is_selected {
                                cx.theme().foreground
                            } else {
                                cx.theme().foreground.opacity(0.9)
                            })
                            .child(self.highlight_query(item.name())),
                    )
                    .child(
                        div()
                            .text_xs()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(cx.theme().muted_foreground)
                            .opacity(0.75)
                            .child(item.description().to_string()),
                    ),
            )
            .into_any_element()
    }
}
