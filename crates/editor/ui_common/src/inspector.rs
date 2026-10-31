//! Virtualized, searchable, collapsible inspector shared by every properties
//! panel (level editor, blueprint editor, ...).
//!
//! A host view `V` owns an [`Inspector<V>`] and tells it what to show as
//! [`CardDesc`]s (metadata only).  The inspector owns everything generic:
//! the search box, the card/category collapse state, the flattened item list
//! ([`crate::inspector_model::flatten`]), the `gpui::list` that renders only
//! the items in view, the scrollbar, and the chrome of card and category
//! headers.  The host keeps everything specific: where values come from and
//! how a property row is built, via the `render_row` callback.
//!
//! The list is `leading` host-owned rows (an object header, a transform, ...)
//! followed by the filtered cards.  Cards are never built as elements unless
//! they are on screen.

use crate::inspector_model::{
    category_color, category_default_collapsed, flatten, CardDesc, CollapseState, InspectorItem,
    Query,
};
use gpui::{prelude::*, *};
use ui::button::{Button, ButtonVariants as _};
use ui::input::{InputEvent, InputState, TextInput};
use ui::scroll::{Scrollbar, ScrollbarState};
use ui::{h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable as _};

/// What the host is asked to render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectorRow {
    /// A host-owned row before the cards; `0..leading`.
    Leading(usize),
    /// One property row of `card`.
    Property { card: usize, prop: usize },
    /// Row `ix` of the rows property `prop` offers below its own row.
    Child { card: usize, prop: usize, ix: usize },
    /// The body of a `custom` card.
    Body { card: usize },
}

/// Search text and collapse state to carry to the next inspector instance
/// (for example when the selection changes).
pub struct InspectorCarry {
    collapse: CollapseState,
    search: String,
}

pub struct Inspector<V: 'static> {
    search: Entity<InputState>,
    query: Query,
    collapse: CollapseState,
    cards: Vec<CardDesc>,
    items: Vec<InspectorItem>,
    list_state: ListState,
    scroll_state: ScrollbarState,
    leading: usize,
    id: SharedString,
    accessor: fn(&mut V) -> &mut Inspector<V>,
    _search_subscription: Subscription,
}

/// Parses a CSS-style hex colour (`#RRGGBB` or `#RRGGBBAA`).
fn parse_hex_color(hex: &str) -> Option<Hsla> {
    let raw = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
    match raw.len() {
        6 => u32::from_str_radix(raw, 16).ok().map(rgb).map(Into::into),
        8 => u32::from_str_radix(raw, 16).ok().map(rgba).map(Into::into),
        _ => None,
    }
}

impl<V: Render + 'static> Inspector<V> {
    /// `accessor` gives the inspector its own place inside the host, which is
    /// how its list and header callbacks reach it again.  `leading` is the
    /// number of host-owned rows before the cards.
    pub fn new(
        id: impl Into<SharedString>,
        placeholder: &'static str,
        leading: usize,
        accessor: fn(&mut V) -> &mut Inspector<V>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscription = cx.subscribe_in(
            &search,
            window,
            move |this, input, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    let query = Query::new(&input.read(cx).text().to_string());
                    let inspector = accessor(this);
                    if query != inspector.query {
                        inspector.query = query;
                        inspector.relayout();
                        cx.notify();
                    }
                }
            },
        );
        Self {
            search,
            query: Query::default(),
            collapse: CollapseState::default(),
            cards: Vec::new(),
            items: Vec::new(),
            // Overdraw keeps rows just outside the viewport measured, so a
            // scroll does not pop rows in.
            list_state: ListState::new(leading, ListAlignment::Top, px(400.0)),
            scroll_state: ScrollbarState::default(),
            leading,
            id: id.into(),
            accessor,
            _search_subscription: subscription,
        }
    }

    // ── State ────────────────────────────────────────────────────────────────

    /// Replace what is shown.  Call when the set of cards or their properties
    /// changed; search and collapse changes relayout by themselves.
    pub fn set_cards(&mut self, cards: Vec<CardDesc>) {
        self.cards = cards;
        self.relayout();
    }

    /// Set how many rows property `prop_name` of card `card_key` offers below
    /// its own (`pulsar_reflection::VirtualRows`).  Relays out when the count
    /// changed; returns whether it did.
    pub fn set_extra_rows(&mut self, card_key: &str, prop_name: &str, count: usize) -> bool {
        let Some(prop) = self
            .cards
            .iter_mut()
            .find(|c| c.key == card_key)
            .and_then(|c| c.props.iter_mut().find(|p| p.name == prop_name))
        else {
            return false;
        };
        if prop.extra_rows == count {
            return false;
        }
        prop.extra_rows = count;
        self.relayout();
        true
    }

    pub fn card(&self, ix: usize) -> Option<&CardDesc> {
        self.cards.get(ix)
    }

    pub fn is_searching(&self) -> bool {
        !self.query.is_empty()
    }

    /// Search text and collapse state, for [`Self::restore`] on a new instance.
    pub fn carry_over(&self, cx: &App) -> InspectorCarry {
        InspectorCarry {
            collapse: self.collapse.clone(),
            search: self.search.read(cx).text().to_string(),
        }
    }

    pub fn restore(&mut self, carry: InspectorCarry, window: &mut Window, cx: &mut Context<V>) {
        self.collapse = carry.collapse;
        self.query = Query::new(&carry.search);
        self.search
            .update(cx, |input, cx| input.set_value(&carry.search, window, cx));
        self.relayout();
    }

    /// Recompute the item list and tell the list which range changed, so the
    /// scroll position survives a collapse or a keystroke in the search box.
    fn relayout(&mut self) {
        let items = flatten(&self.cards, &self.query, &self.collapse);
        let old = &self.items;
        let prefix = old.iter().zip(&items).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(items[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        if prefix != old.len() || prefix != items.len() {
            self.list_state.splice(
                self.leading + prefix..self.leading + old.len() - suffix,
                items.len() - prefix - suffix,
            );
        }
        self.items = items;
    }

    // ── Chrome ───────────────────────────────────────────────────────────────

    /// A property row inside its card: the card's side borders, and the
    /// category accent bar when the property has a category.
    pub fn wrap_row(&self, card: usize, prop: usize, row: AnyElement, cx: &App) -> AnyElement {
        let accent = self.cards.get(card).and_then(|c| {
            let category = c.props.get(prop)?.category.as_deref()?;
            (!category.trim().is_empty()).then(|| {
                match category_color(c, category).and_then(parse_hex_color) {
                    Some(color) => color.opacity(0.85),
                    None => cx.theme().muted_foreground.opacity(0.35),
                }
            })
        });
        h_flex()
            .w_full()
            .px_3()
            .py(px(2.))
            .gap_1()
            .items_stretch()
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_r_1()
            .border_color(cx.theme().border)
            .when_some(accent, |d, color| {
                d.child(div().w(px(3.)).rounded_full().flex_shrink_0().bg(color))
            })
            .child(div().flex_1().min_w_0().child(row))
            .into_any_element()
    }

    fn card_header(&self, card_ix: usize, cx: &mut Context<V>) -> AnyElement {
        let accessor = self.accessor;
        let card = &self.cards[card_ix];
        let key = card.key.clone();
        let collapsed = !self.is_searching() && self.collapse.is_card_collapsed(&key);

        v_flex()
            .w_full()
            .pt_2()
            .child(
                v_flex()
                    .w_full()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .bg(cx.theme().sidebar)
                    .border_1()
                    .border_color(cx.theme().border)
                    .when(collapsed, |d| d.rounded(px(8.0)))
                    .when(!collapsed, |d| d.rounded_t(px(8.0)))
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("{}-card-{key}", self.id)))
                            .w_full()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    let inspector = accessor(this);
                                    inspector.collapse.toggle_card(&key);
                                    inspector.relayout();
                                    cx.notify();
                                }),
                            )
                            .child(
                                Icon::new(if collapsed {
                                    IconName::ChevronRight
                                } else {
                                    IconName::ChevronDown
                                })
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                            )
                            .child(Icon::new(IconName::Component).small())
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(cx.theme().foreground)
                                    .child(card.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(card.props.len().to_string()),
                            ),
                    )
                    .children(card.note.clone().map(|note| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(note)
                    })),
            )
            .into_any_element()
    }

    fn category_header(&self, card_ix: usize, name: &str, cx: &mut Context<V>) -> AnyElement {
        let accessor = self.accessor;
        let card = &self.cards[card_ix];
        let card_key = card.key.clone();
        let default_collapsed = category_default_collapsed(card, name);
        let collapsed = !self.is_searching()
            && self
                .collapse
                .is_category_collapsed(&card_key, name, default_collapsed);
        let color = category_color(card, name)
            .and_then(parse_hex_color)
            .unwrap_or(cx.theme().muted_foreground);
        let toggle_name = name.to_string();

        h_flex()
            .w_full()
            .px_3()
            .pt(px(4.))
            .items_center()
            .justify_between()
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_r_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    let inspector = accessor(this);
                    inspector
                        .collapse
                        .toggle_category(&card_key, &toggle_name, default_collapsed);
                    inspector.relayout();
                    cx.notify();
                }),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(name.to_string()),
            )
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .xsmall()
                .text_color(color),
            )
            .into_any_element()
    }

    // ── Render ───────────────────────────────────────────────────────────────

    /// The search bar above the virtual list.  `render_row` builds the rows
    /// the host owns (leading rows, properties, custom bodies); card and
    /// category headers are drawn here.
    pub fn render(
        &self,
        render_row: fn(&mut V, InspectorRow, &mut Window, &mut Context<V>) -> AnyElement,
        cx: &mut Context<V>,
    ) -> AnyElement {
        let accessor = self.accessor;
        let leading = self.leading;
        debug_assert_eq!(
            self.list_state.item_count(),
            leading + self.items.len(),
            "list state out of step with the item list"
        );

        let list = list(
            self.list_state.clone(),
            cx.processor(move |this: &mut V, ix: usize, window, cx| {
                if ix < leading {
                    return render_row(this, InspectorRow::Leading(ix), window, cx);
                }
                let inspector = accessor(this);
                let Some(item) = inspector.items.get(ix - leading).cloned() else {
                    return div().into_any_element();
                };
                match item {
                    InspectorItem::Card { card } => inspector.card_header(card, cx),
                    InspectorItem::Category { card, name } => {
                        inspector.category_header(card, &name, cx)
                    }
                    InspectorItem::Property { card, prop } => {
                        render_row(this, InspectorRow::Property { card, prop }, window, cx)
                    }
                    InspectorItem::Child { card, prop, ix } => {
                        render_row(this, InspectorRow::Child { card, prop, ix }, window, cx)
                    }
                    InspectorItem::Body { card } => {
                        render_row(this, InspectorRow::Body { card }, window, cx)
                    }
                }
            }),
        )
        .size_full()
        .px_3()
        .pb_3();

        let no_matches = self.is_searching() && self.items.is_empty();
        let keys: Vec<String> = self.cards.iter().map(|c| c.key.clone()).collect();
        let all_collapsed =
            !keys.is_empty() && keys.iter().all(|k| self.collapse.is_card_collapsed(k));

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().child(TextInput::new(&self.search).small()))
                    .child(
                        Button::new(SharedString::from(format!("{}-toggle-all", self.id)))
                            .icon(if all_collapsed {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .xsmall()
                            .ghost()
                            .tooltip(if all_collapsed {
                                "Expand all"
                            } else {
                                "Collapse all"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let inspector = accessor(this);
                                inspector.collapse.set_all_cards(keys.clone(), !all_collapsed);
                                inspector.relayout();
                                cx.notify();
                            })),
                    ),
            )
            .when(no_matches, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("No matching properties"),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("{}-list", self.id)))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(list)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .bottom_0()
                            .w(px(12.))
                            .child(Scrollbar::vertical(&self.scroll_state, &self.list_state)),
                    ),
            )
            .into_any_element()
    }
}

impl<V: Render + 'static> Inspector<V> {
    /// Scroll the list by `distance` (positive scrolls down).
    pub fn scroll_by(&self, distance: Pixels) {
        self.list_state.scroll_by(distance);
    }

    /// Total number of list entries, leading rows included.
    pub fn item_count(&self) -> usize {
        self.list_state.item_count()
    }
}
