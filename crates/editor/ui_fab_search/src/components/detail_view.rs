use std::collections::HashMap;
use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{ActiveTheme, StyledExt, button::Button};

use crate::FabSearchWindow;
use crate::components::item_detail::ItemDetailView;

pub fn render_detail_view(
    window: &FabSearchWindow,
    cx: &mut Context<FabSearchWindow>,
) -> AnyElement {
    let muted_fg = cx.theme().muted_foreground;

    if window.detail_loading {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_color(muted_fg)
                    .child("Loading model details\u{2026}"),
            )
            .into_any_element()
    } else if let Some(ref err) = window.detail_error {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(div().text_color(gpui::red()).child(err.clone()))
            .child(
                Button::new("back-err")
                    .label("\u{2190} Back")
                    .on_click(cx.listener(|this, _, _, cx| {
                        crate::handlers::on_go_back(this, cx);
                    })),
            )
            .into_any_element()
    } else if let Some(ref detail) = window.item_detail {
        let entity = cx.entity().clone();

        let mut loaded: HashMap<String, Arc<gpui::RenderImage>> = detail
            .all_thumbnail_urls()
            .into_iter()
            .take(12)
            .filter_map(|url| {
                window
                    .image_cache
                    .get(url)
                    .and_then(|o| o.clone())
                    .map(|arc| (url.to_string(), arc))
            })
            .collect();

        if let Some(ref user) = detail.user {
            if let Some(url) = user.avatar_url(128) {
                if let Some(Some(arc)) = window.image_cache.get(url) {
                    loaded.insert(url.to_string(), arc.clone());
                }
            }
        }

        let entity_for_back = entity.clone();
        let view = ItemDetailView::new(
            detail.clone(),
            loaded,
            window.detail_scroll_handle.clone(),
            window.detail_scroll_state.clone(),
            window.gallery_scroll_handle.clone(),
            window.gallery_scroll_state.clone(),
            move |_window, cx| {
                entity_for_back.update(cx, |this, cx| {
                    crate::handlers::on_go_back(this, cx);
                });
            },
        );

        let view = if window.api_token.is_some() {
            let uid = detail.uid.clone();
            let dl_status = window.download_state.get(&uid).cloned();
            let entity_for_dl = entity.clone();
            view.with_download(
                move |_window, cx| {
                    entity_for_dl.update(cx, |this, cx| {
                        crate::handlers::on_start_download(this, uid.clone(), cx);
                    });
                },
                dl_status,
            )
        } else {
            view
        };

        let idx = window.selected_gallery_idx;
        let entity_for_gallery = entity.clone();
        view.with_selected_image(idx, move |new_idx, _, cx| {
            entity_for_gallery.update(cx, |this, cx| {
                this.selected_gallery_idx = new_idx;
                cx.notify();
            });
        })
        .into_any_element()
    } else {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(div().text_color(muted_fg).child("Loading\u{2026}"))
            .into_any_element()
    }
}
