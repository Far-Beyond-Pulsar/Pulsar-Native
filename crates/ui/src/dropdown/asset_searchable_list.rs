use gpui::{
    div, img, prelude::FluentBuilder, px, App, AppContext, Context, DismissEvent, Div, Entity,
    EventEmitter, FocusHandle, Focusable, ImageSource, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, SharedString, Styled, StyledImage, Window,
};
use std::rc::Rc;

use crate::{
    button::{Button, ButtonVariants as _},
    input::{InputState, TextInput},
    scroll::ScrollbarAxis,
    v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt,
};

// Row height for asset rows — twice the height of a normal searchable list row.
const ASSET_ROW_HEIGHT: f32 = 56.0;
// Square thumbnail side length.
const THUMB_SIZE: f32 = 40.0;

pub enum AssetSearchableListEvent<T: Clone + 'static> {
    Select(T),
    Action { item: T, action_id: SharedString },
}

/// Per-item action button descriptor — identical shape to [`SearchableListItemAction`] so
/// callers can share data between the two components easily.
#[derive(Clone, Debug)]
pub struct AssetListItemAction {
    pub id: SharedString,
    pub icon: Option<IconName>,
    pub label: Option<SharedString>,
    pub destructive: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetListItemState {
    Enabled,
    Disabled,
}

/// A searchable list variant designed for asset selection.
///
/// Each row is twice the height of a normal searchable-list row and shows:
/// - A square image thumbnail on the left (asset preview).
/// - On the right: a title line and a secondary description line (path, type, etc.).
/// - Optionally a set of action buttons on the far right, same pattern as [`SearchableList`].
///
/// Supply thumbnails via [`AssetSearchableList::with_image_getter`]. If no getter is provided
/// the thumbnail slot renders a neutral placeholder icon.
pub struct AssetSearchableList<T: Clone + 'static> {
    focus_handle: FocusHandle,
    search_input: Entity<InputState>,
    items: Vec<T>,
    format_title: Rc<dyn Fn(&T) -> String>,
    format_description: Rc<dyn Fn(&T) -> String>,
    image_for: Option<Rc<dyn Fn(&T) -> Option<ImageSource>>>,
    item_state: Rc<dyn Fn(&T) -> AssetListItemState>,
    item_actions: Option<Rc<dyn Fn(&T) -> Vec<AssetListItemAction>>>,
    empty_text: SharedString,
    max_width: Pixels,
    max_height: Pixels,
}

impl<T: Clone + 'static> EventEmitter<AssetSearchableListEvent<T>> for AssetSearchableList<T> {}
impl<T: Clone + 'static> EventEmitter<DismissEvent> for AssetSearchableList<T> {}

impl<T: Clone + 'static> Focusable for AssetSearchableList<T> {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl<T: Clone + 'static> AssetSearchableList<T> {
    /// Create a new asset list.
    ///
    /// - `items` — the full item set; filtering is done at render time.
    /// - `format_title` — primary label shown in bold on the first line.
    /// - `format_description` — secondary label shown in a muted colour on the second line.
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        items: Vec<T>,
        format_title: impl Fn(&T) -> String + 'static,
        format_description: impl Fn(&T) -> String + 'static,
    ) -> Self {
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search assets…"));
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

        Self {
            focus_handle: cx.focus_handle(),
            search_input,
            items,
            format_title: Rc::new(format_title),
            format_description: Rc::new(format_description),
            image_for: None,
            item_state: Rc::new(|_| AssetListItemState::Enabled),
            item_actions: None,
            empty_text: "No assets found".into(),
            max_width: px(340.0),
            max_height: px(400.0),
        }
    }

    /// Provide an image source for each item. If not set, a placeholder icon is shown.
    pub fn with_image_getter(mut self, image_for: impl Fn(&T) -> Option<ImageSource> + 'static) -> Self {
        self.image_for = Some(Rc::new(image_for));
        self
    }

    pub fn with_item_state(
        mut self,
        item_state: impl Fn(&T) -> AssetListItemState + 'static,
    ) -> Self {
        self.item_state = Rc::new(item_state);
        self
    }

    pub fn with_item_actions(
        mut self,
        item_actions: impl Fn(&T) -> Vec<AssetListItemAction> + 'static,
    ) -> Self {
        self.item_actions = Some(Rc::new(item_actions));
        self
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

    pub fn set_items(&mut self, items: Vec<T>, cx: &mut Context<Self>) {
        self.items = items;
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn row_style(&self, el: Div) -> Div {
        el.flex()
            .flex_row()
            .w_full()
            .h(px(ASSET_ROW_HEIGHT))
            .px_2()
            .gap_2()
            .items_center()
            .cursor_pointer()
            .rounded(px(4.0))
    }

    /// Thumbnail slot — image when available, neutral icon placeholder otherwise.
    fn render_thumbnail(&self, image_source: Option<ImageSource>, cx: &App) -> impl IntoElement {
        let thumb_px = px(THUMB_SIZE);

        div()
            .flex_shrink_0()
            .w(thumb_px)
            .h(thumb_px)
            .rounded(px(4.0))
            .overflow_hidden()
            .bg(cx.theme().secondary)
            .flex()
            .items_center()
            .justify_center()
            .map(move |el| match image_source {
                Some(src) => el.child(
                    img(src)
                        .w(thumb_px)
                        .h(thumb_px)
                        .object_fit(gpui::ObjectFit::Cover),
                ),
                None => el.child(
                    Icon::new(IconName::Cube)
                        .size(px(20.0))
                        .text_color(cx.theme().muted_foreground),
                ),
            })
    }
}

impl<T: Clone + 'static> Render for AssetSearchableList<T> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).value().to_lowercase();

        let filtered_items: Vec<T> = self
            .items
            .iter()
            .filter(|item| {
                if query.is_empty() {
                    return true;
                }
                let title = (self.format_title)(item).to_lowercase();
                let desc = (self.format_description)(item).to_lowercase();
                title.contains(&query) || desc.contains(&query)
            })
            .cloned()
            .collect();

        v_flex()
            .w(self.max_width)
            .h(self.max_height)
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
                div().flex_1().min_h_0().w_full().overflow_hidden().child(
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
                                let title = (self.format_title)(&item);
                                let description = (self.format_description)(&item);
                                let image_source =
                                    self.image_for.as_ref().and_then(|f| f(&item));
                                let actions = self
                                    .item_actions
                                    .as_ref()
                                    .map(|f| f(&item))
                                    .unwrap_or_default();
                                let item_state = (self.item_state)(&item);
                                let is_enabled = item_state == AssetListItemState::Enabled;
                                let theme = cx.theme().clone();
                                let selected_item = item.clone();

                                let thumbnail = self.render_thumbnail(image_source, cx);

                                self.row_style(div())
                                    .id(("asset-list-item", ix))
                                    .when(is_enabled, |el| {
                                        el.hover(move |s| s.bg(theme.accent.opacity(0.12)))
                                    })
                                    .when(!is_enabled, |el| el.cursor_default().opacity(0.7))
                                    // Thumbnail
                                    .child(thumbnail)
                                    // Text + actions fill remaining space
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .flex_1()
                                            .min_w_0()
                                            .items_center()
                                            .gap_1()
                                            // Clickable text block
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap(px(2.0))
                                                    .when(is_enabled, |el| {
                                                        el.on_mouse_down(
                                                            MouseButton::Left,
                                                            cx.listener(move |_this, _, _, cx| {
                                                                cx.emit(
                                                                    AssetSearchableListEvent::Select(
                                                                        selected_item.clone(),
                                                                    ),
                                                                );
                                                                cx.emit(DismissEvent);
                                                            }),
                                                        )
                                                    })
                                                    // Title line
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .truncate()
                                                            .text_color(if is_enabled {
                                                                cx.theme().foreground
                                                            } else {
                                                                cx.theme().muted_foreground
                                                            })
                                                            .when(!is_enabled, |el| {
                                                                el.italic().line_through()
                                                            })
                                                            .child(title),
                                                    )
                                                    // Description line
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .truncate()
                                                            .text_color(
                                                                cx.theme().muted_foreground,
                                                            )
                                                            .child(description),
                                                    ),
                                            )
                                            // Action buttons
                                            .when(!actions.is_empty(), |el| {
                                                el.child(
                                                    div()
                                                        .flex()
                                                        .flex_row()
                                                        .items_center()
                                                        .flex_shrink_0()
                                                        .gap_1()
                                                        .children(
                                                            actions.into_iter().map(|action| {
                                                                let action_item = item.clone();
                                                                let action_id =
                                                                    action.id.clone();
                                                                let button_id = format!(
                                                                    "asset-list-action-{}-{}",
                                                                    ix, action.id
                                                                );

                                                                let button =
                                                                    Button::new(button_id)
                                                                        .xsmall()
                                                                        .ghost()
                                                                        .when(
                                                                            action.destructive,
                                                                            |b| {
                                                                                b.text_color(
                                                                                    cx.theme()
                                                                                        .danger,
                                                                                )
                                                                            },
                                                                        )
                                                                        .on_click(cx.listener(
                                                                            move |_this, _, _, cx| {
                                                                                cx.emit(
                                                                                    AssetSearchableListEvent::Action {
                                                                                        item: action_item.clone(),
                                                                                        action_id: action_id.clone(),
                                                                                    },
                                                                                );
                                                                            },
                                                                        ));

                                                                match (
                                                                    action.icon,
                                                                    action.label.clone(),
                                                                ) {
                                                                    (Some(icon), Some(label)) => {
                                                                        button
                                                                            .icon(icon)
                                                                            .label(label)
                                                                            .into_any_element()
                                                                    }
                                                                    (Some(icon), None) => button
                                                                        .icon(icon)
                                                                        .into_any_element(),
                                                                    (None, Some(label)) => button
                                                                        .label(label)
                                                                        .into_any_element(),
                                                                    (None, None) => button
                                                                        .label("Action")
                                                                        .into_any_element(),
                                                                }
                                                            }),
                                                        ),
                                                )
                                            }),
                                    )
                            })),
                    ),
                ),
            )
    }
}
