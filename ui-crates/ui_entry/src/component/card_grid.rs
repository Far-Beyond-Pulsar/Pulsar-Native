use std::rc::Rc;

use gpui::prelude::*;
use gpui::*;
use ui::{h_flex, v_virtual_list, VirtualListScrollHandle};

use crate::screen::EntryScreen;

pub fn render_card_grid<T: 'static + Clone>(
    items: &[T],
    scroll_handle: &VirtualListScrollHandle,
    window: &mut Window,
    cx: &mut Context<EntryScreen>,
    render_card: impl Fn(&T, &mut Window, &mut Context<EntryScreen>) -> AnyElement + 'static,
) -> impl IntoElement {
    let entity = cx.entity().clone();
    let total_items = items.len();

    if total_items == 0 {
        return div().flex_1().min_h_0().into_any_element();
    }

    let available_width: f32 = window.bounds().size.width.into();
    let card_width = 320.0;
    let gap = 24.0;
    let padding = 16.0;

    let inner_width = (available_width - 2.0 * padding).max(0.0);
    let cols = (((inner_width + gap) / (card_width + gap)).floor() as usize)
        .max(1)
        .min(6);
    let _actual_card_w = (inner_width - (cols - 1) as f32 * gap) / cols as f32;
    let row_height = 120.0;
    let row_total_height = row_height + gap;
    let total_rows = total_items.div_ceil(cols);

    let item_sizes = Rc::new(
        (0..total_rows)
            .map(|_| size(px(0.0), px(row_total_height)))
            .collect::<Vec<_>>(),
    );

    let items_owned: Vec<T> = items.to_vec();
    let items_rc = Rc::new(items_owned);

    div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .child(
            v_virtual_list(
                entity,
                "card-grid",
                item_sizes,
                move |_view, range, window, cx| {
                    range
                        .map(|row_idx| {
                            let start = row_idx * cols;
                            let end = (start + cols).min(total_items);

                            let row_cards: Vec<AnyElement> = (start..end)
                                .map(|item_idx| {
                                    render_card(&items_rc[item_idx], window, cx).into_any_element()
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
