//! A searchable list in a popover: pick an action, pick a key.
//!
//! The list is virtualized (only the rows in view are built), so a project
//! with thousands of actions, or the whole key table, costs no more to open
//! than a short list.

use std::ops::Range;

use gpui::prelude::FluentBuilder;
use gpui::*;
use ui::input::{InputEvent, InputState, TextInput};
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt};

/// One thing that can be chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceItem {
    /// What the chooser reports when it is picked.
    pub id: String,
    pub title: String,
    /// Smaller text after the title: a path, a device.
    pub detail: String,
    /// A short label on the right: a value type, a usage count.
    pub tag: String,
    /// Heading it is listed under; empty for none.
    pub group: String,
}

impl ChoiceItem {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self { id: id.into(), title: title.into(), detail: String::new(), tag: String::new(), group: String::new() }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = tag.into();
        self
    }

    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = group.into();
        self
    }

    fn matches(&self, words: &[String]) -> bool {
        let hay = format!("{} {} {} {}", self.title, self.detail, self.group, self.tag).to_lowercase();
        words.iter().all(|w| hay.contains(w))
    }
}

/// An item was picked; carries its id.
#[derive(Clone, Debug)]
pub struct ChosenEvent(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Header(String),
    /// An index into the pinned items.
    Pinned(usize),
    /// An index into the items.
    Item(usize),
}

pub const ROW_HEIGHT: f32 = 30.0;
pub const WIDTH: f32 = 380.0;
pub const HEIGHT: f32 = 360.0;

pub struct Chooser {
    items: Vec<ChoiceItem>,
    selected: String,
    /// Items shown whatever the search says ("None", "New action…").
    pinned: Vec<ChoiceItem>,
    filter: Entity<InputState>,
    rows: Vec<Row>,
    scroll: UniformListScrollHandle,
    highlight: Option<usize>,
    empty_text: SharedString,
    _subscription: Subscription,
}

impl EventEmitter<ChosenEvent> for Chooser {}
impl EventEmitter<DismissEvent> for Chooser {}

impl Focusable for Chooser {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.read(cx).focus_handle(cx)
    }
}

impl Chooser {
    pub fn new(items: Vec<ChoiceItem>, selected: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Search…"));
        let subscription = cx.subscribe_in(&filter, window, |this: &mut Self, _input, event: &InputEvent, _window, cx| match event {
            InputEvent::Change => {
                this.rebuild(cx);
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.pick_highlighted(cx),
            _ => {}
        });
        let mut chooser = Self {
            items,
            selected: selected.to_string(),
            pinned: Vec::new(),
            filter,
            rows: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            highlight: None,
            empty_text: "Nothing matches".into(),
            _subscription: subscription,
        };
        chooser.rebuild(cx);
        chooser
    }

    /// How many items show (headings not counted).
    pub fn row_count(&self) -> usize {
        self.rows.iter().filter(|r| !matches!(r, Row::Header(_))).count()
    }

    /// Search for `query`.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |f, cx| f.set_value(query.to_string(), window, cx));
        self.rebuild(cx);
        cx.notify();
    }

    /// Items always shown at the top, above the search results.
    pub fn with_pinned(mut self, pinned: Vec<ChoiceItem>, cx: &mut Context<Self>) -> Self {
        self.pinned = pinned;
        self.rebuild(cx);
        self
    }

    pub fn with_empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty_text = text.into();
        self
    }

    /// Bring the chooser up to date with the model, keeping the search.
    pub fn sync(&mut self, items: Vec<ChoiceItem>, selected: &str, cx: &mut Context<Self>) {
        if self.items != items || self.selected != selected {
            self.items = items;
            self.selected = selected.to_string();
            self.rebuild(cx);
            cx.notify();
        }
    }

    fn words(&self, cx: &App) -> Vec<String> {
        self.filter.read(cx).value().split_whitespace().map(str::to_lowercase).collect()
    }

    /// Work out which rows show: pinned items, then matches under their headings.
    fn rebuild(&mut self, cx: &App) {
        let words = self.words(cx);
        let mut rows = Vec::new();
        for i in 0..self.pinned.len() {
            rows.push(Row::Pinned(i));
        }
        let mut group: Option<&str> = None;
        for (i, item) in self.items.iter().enumerate() {
            if !item.matches(&words) {
                continue;
            }
            if !item.group.is_empty() && group != Some(item.group.as_str()) {
                rows.push(Row::Header(item.group.clone()));
            }
            group = Some(item.group.as_str());
            rows.push(Row::Item(i));
        }
        self.rows = rows;
        self.highlight = self.rows.iter().position(|r| !matches!(r, Row::Header(_)));
    }

    fn item(&self, row: &Row) -> Option<&ChoiceItem> {
        match row {
            Row::Pinned(i) => self.pinned.get(*i),
            Row::Item(i) => self.items.get(*i),
            Row::Header(_) => None,
        }
    }

    fn pick(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = id.clone();
        cx.emit(ChosenEvent(id));
        cx.emit(DismissEvent);
    }

    fn pick_highlighted(&mut self, cx: &mut Context<Self>) {
        let id = self.highlight.and_then(|at| self.rows.get(at)).and_then(|row| self.item(row)).map(|item| item.id.clone());
        if let Some(id) = id {
            self.pick(id, cx);
        }
    }

    /// Move the highlight to the next (or previous) item row.
    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(mut at) = self.highlight else { return };
        loop {
            at = if forward { at + 1 } else { at.wrapping_sub(1) };
            match self.rows.get(at) {
                Some(Row::Header(_)) => continue,
                Some(_) => break,
                None => return,
            }
        }
        self.highlight = Some(at);
        self.scroll.scroll_to_item(at, ScrollStrategy::Center);
        cx.notify();
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme().clone();
        range
            .filter_map(|at| {
                let row = self.rows.get(at)?.clone();
                Some(match row {
                    Row::Header(name) => h_flex()
                        .id(("header", at))
                        .h(px(ROW_HEIGHT))
                        .px_3()
                        .items_end()
                        .pb_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.muted_foreground)
                        .child(name)
                        .into_any_element(),
                    row => {
                        let item = self.item(&row)?.clone();
                        let selected = item.id == self.selected;
                        let highlighted = self.highlight == Some(at);
                        let id = item.id.clone();
                        h_flex()
                            .id(("item", at))
                            .h(px(ROW_HEIGHT))
                            .w_full()
                            .px_3()
                            .gap_2()
                            .items_center()
                            .cursor_pointer()
                            .when(highlighted, |r| r.bg(theme.accent.opacity(0.18)))
                            .when(selected, |r| r.bg(theme.accent.opacity(0.28)))
                            .hover(|r| r.bg(theme.accent.opacity(0.14)))
                            .child(div().flex_shrink_0().text_sm().text_color(theme.foreground).child(item.title.clone()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(item.detail.clone()),
                            )
                            .when(!item.tag.is_empty(), |r| {
                                r.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1p5()
                                        .rounded(px(4.0))
                                        .bg(theme.secondary)
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(item.tag.clone()),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| this.pick(id.clone(), cx)))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }
}

impl Render for Chooser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.rows.len();
        v_flex()
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .bg(theme.popover)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| match event.keystroke.key.as_str() {
                "down" => {
                    this.step(true, cx);
                    cx.stop_propagation();
                }
                "up" => {
                    this.step(false, cx);
                    cx.stop_propagation();
                }
                "escape" => cx.emit(DismissEvent),
                _ => {}
            }))
            .child(
                h_flex()
                    .flex_shrink_0()
                    .p_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(Icon::new(IconName::Search).size(px(14.0)).text_color(theme.muted_foreground))
                    .child(div().flex_1().child(TextInput::new(&self.filter).small().appearance(false))),
            )
            .child(div().flex_1().min_h_0().child(if count == 0 {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.empty_text.clone())
                    .into_any_element()
            } else {
                uniform_list("chooser", count, cx.processor(|this, range: Range<usize>, _window, cx| this.render_rows(range, cx)))
                    .track_scroll(&self.scroll)
                    .size_full()
                    .into_any_element()
            }))
    }
}

#[cfg(test)]
mod tests {
    // Explicit import: `super::*` would pull in gpui's own `test` macro.
    use super::ChoiceItem;

    #[test]
    fn an_item_matches_when_every_word_is_somewhere_in_it() {
        let item = ChoiceItem::new("a", "Jump").detail("input/Jump.inputaction").tag("Bool").group("Gameplay");
        let words = |s: &str| s.split_whitespace().map(str::to_lowercase).collect::<Vec<_>>();
        assert!(item.matches(&words("jump")));
        assert!(item.matches(&words("bool jump")));
        assert!(item.matches(&words("gameplay")));
        assert!(!item.matches(&words("jump 2d")));
        assert!(item.matches(&[]), "an empty search matches everything");
    }
}
