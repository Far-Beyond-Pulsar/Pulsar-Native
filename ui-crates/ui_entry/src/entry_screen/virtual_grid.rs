use std::rc::Rc;

use gpui::{prelude::*, *};
use ui::{h_flex, v_virtual_list, VirtualListScrollHandle};

/// Renders a responsive, virtualized grid of cards.
///
/// This is used by the entry screen to display large collections of cards (templates,
/// recent projects, etc.) without rendering everything at once.
///
/// - `available_width` is the full width that the grid may use (typically the viewport width minus sidebars).
/// - `card_width` is the *desired minimum* width of a card. The grid will automatically stretch cards to fill the available width.
/// - `card_height` is the height of the card (used to compute the row height for virtualization).
/// - `gap` is the spacing between cards.
/// - `padding` is the horizontal padding applied around the grid.
///
/// The `render_card` closure is invoked for each visible item index and is responsible for returning a card element.
/// It is passed the computed `card_width` so the caller can size the card consistently.
pub fn render_card_grid<V, R, F>(
    view_entity: Entity<V>,
    id: impl Into<ElementId>,
    available_width: f32,
    total_items: usize,
    card_width: f32,
    card_height: f32,
    gap: f32,
    padding: f32,
    scroll_handle: &VirtualListScrollHandle,
    render_card: F,
) -> AnyElement
where
    V: Render,
    R: IntoElement,
    F: 'static + Fn(&mut V, usize, f32, &mut Window, &mut Context<V>) -> R,
{
    if total_items == 0 {
        return div().flex_1().min_h_0().into_any_element();
    }

    let inner_width = (available_width - 2.0 * padding).max(0.0);
    let cols = (((inner_width + gap) / (card_width + gap)).floor() as usize).max(1);
    let actual_card_w = (inner_width - (cols - 1) as f32 * gap) / cols as f32;
    let row_height = card_height + gap;
    let total_rows = total_items.div_ceil(cols);

    let item_sizes = Rc::new(
        (0..total_rows)
            .map(|_| size(px(0.0), px(row_height)))
            .collect::<Vec<_>>(),
    );

    div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(
            v_virtual_list(
                view_entity,
                id,
                item_sizes,
                move |view, range, window, cx| {
                    range
                        .map(|row_idx| {
                            let start = row_idx * cols;
                            let end = (start + cols).min(total_items);

                            let row_cards: Vec<AnyElement> = (start..end)
                                .map(|item_idx| {
                                    render_card(view, item_idx, actual_card_w, window, cx)
                                        .into_any_element()
                                })
                                .collect();

                            h_flex()
                                .px(px(padding))
                                .py(px(padding))
                                .gap(px(gap))
                                .items_start()
                                .children(row_cards)
                                .into_any_element()
                        })
                        .collect()
                },
            )
            .track_scroll(scroll_handle),
        )
        .into_any_element()
}
