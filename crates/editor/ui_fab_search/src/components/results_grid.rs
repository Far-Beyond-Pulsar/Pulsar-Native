use std::rc::Rc;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Sizable, StyledExt, button::Button, download_item::DownloadItemStatus,
    download_manager::DownloadEntry, h_flex, scroll::Scrollbar, skeleton::Skeleton, spinner::Spinner,
    v_flex, v_virtual_list,
};

use crate::FabSearchWindow;
use crate::utils::actions::{DownloadState, LicenseFilter, SortBy};
use crate::parser::fmt_count;

pub fn render_results_grid(
    window: &FabSearchWindow,
    avail_w: f32,
    _muted_fg: gpui::Hsla,
    _border_col: gpui::Hsla,
    _fg: gpui::Hsla,
) -> AnyElement {
    const CARD_W: f32 = 260.0;
    const CARD_H: f32 = 260.0;
    const GAP: f32 = 16.0;
    const PAD: f32 = 16.0;
    const ROW_H: f32 = CARD_H + GAP;

    let avail_w = avail_w - 2.0 * PAD;
    let cols: usize = (((avail_w + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
    let actual_card_w: f32 = (avail_w - (cols - 1) as f32 * GAP) / cols as f32;

    let entity = window.entity.clone().unwrap();
    let total_results = window.results.len();
    let result_rows = total_results.div_ceil(cols);
    let skel_rows: usize = if window.is_loading_more { 2 } else { 0 };
    let end_row: usize = if !window.is_loading_more && window.next_url.is_none() {
        1
    } else {
        0
    };
    let total_items = result_rows + skel_rows + end_row;

    let item_sizes = Rc::new(
        (0..total_items)
            .map(|_| size(px(0.0), px(ROW_H)))
            .collect::<Vec<_>>(),
    );

    div().relative().flex_1().min_h_0().overflow_hidden()
        .child(
            v_virtual_list(
                entity,
                "skfb-results-grid",
                item_sizes,
                move |view: &mut FabSearchWindow, range, _window, cx| {
                    let theme = cx.theme().clone();
                    let fg = theme.foreground;
                    let muted_fg = theme.muted_foreground;
                    let card_bg = theme.secondary;
                    let border_col = theme.border;

                    let total_res = view.results.len();
                    let res_rows = total_res.div_ceil(cols);
                    let s_rows: usize = if view.is_loading_more { 2 } else { 0 };

                    if !view.is_loading && !view.is_loading_more
                        && view.next_url.is_some()
                        && range.end >= res_rows.saturating_sub(1)
                    {
                        crate::handlers::on_load_more(view, cx);
                    }

                    range.map(|row_idx| -> AnyElement {
                        if row_idx < res_rows {
                            let start = row_idx * cols;
                            let end = (start + cols).min(view.results.len());
                            let row_cards: Vec<AnyElement> = (start..end).map(|idx| {
                                let model = &view.results[idx];
                                let name     = model.name.clone();
                                let author   = model.user.as_ref().map(|u| u.display().to_string()).unwrap_or_default();
                                let category = model.primary_category().map(|s| s.to_string()).unwrap_or_default();
                                let subtitle = if category.is_empty() { author } else { format!("{} \u{00B7} {}", author, category) };
                                let views    = fmt_count(model.view_count);
                                let likes    = fmt_count(model.like_count);
                                let is_dl    = model.is_downloadable;
                                let is_anim  = model.animation_count > 0;
                                let is_sp    = model.staffpicked_at.as_ref().map(|v| !v.is_null()).unwrap_or(false);
                                let card_uid = model.uid.clone();
                                let like_uid = card_uid.clone();
                                let is_liked  = view.liked_uids.contains(&card_uid);
                                let like_busy = view.like_inflight.contains(&card_uid);
                                let show_like = view.api_token.is_some();
                                let thumb_state = model.thumb_url(260)
                                    .map(|u| view.image_cache.get(u).and_then(|o| o.clone()));
                                let thumb_loading = matches!(thumb_state, None | Some(None));
                                let thumb_arc = thumb_state.flatten();

                                div()
                                    .id(SharedString::from(format!("skfb-card-{}", idx)))
                                    .cursor_pointer()
                                    .w(px(actual_card_w)).rounded_lg().bg(card_bg)
                                    .border_1().border_color(border_col)
                                    .overflow_hidden().flex().flex_col()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        crate::handlers::on_open_item_detail(this, card_uid.clone(), cx);
                                    }))
                                    .child(
                                        div().w_full().h(px(146.0)).overflow_hidden().relative()
                                            .map(|el| {
                                                if let Some(arc) = thumb_arc {
                                                    el.child(img(gpui::ImageSource::Render(arc))
                                                        .w_full().h_full().object_fit(gpui::ObjectFit::Cover))
                                                } else if thumb_loading {
                                                    el.bg(border_col).flex().items_center().justify_center()
                                                        .child(Spinner::new().with_size(ui::Size::Medium)
                                                            .color(muted_fg))
                                                } else {
                                                    el.bg(border_col)
                                                }
                                            })
                                            .when(is_sp, |el| el.child(
                                                div().absolute().top(px(6.0)).left(px(6.0))
                                                    .px_2().py(px(2.0)).rounded_full()
                                                    .bg(gpui::rgb(0xFACC15))
                                                    .text_xs().font_bold().text_color(gpui::rgb(0x000000))
                                                    .child("\u{2605} Staff Pick")
                                            ))
                                            .when(is_dl, |el| el.child(
                                                div().absolute().bottom(px(6.0)).right(px(6.0))
                                                    .px_2().py(px(2.0)).rounded_full()
                                                    .bg(gpui::rgb(0x22C55E))
                                                    .text_xs().font_bold().text_color(gpui::rgb(0xFFFFFF))
                                                    .child("\u{2193}")
                                            ))
                                            .when(is_anim, |el| el.child(
                                                div().absolute().bottom(px(6.0)).left(px(6.0))
                                                    .px_2().py(px(2.0)).rounded_full()
                                                    .bg(gpui::rgb(0x6366F1))
                                                    .text_xs().font_bold().text_color(gpui::rgb(0xFFFFFF))
                                                    .child("\u{25B6}")
                                            ))
                                    )
                                    .child(div().p_3().child(v_flex().gap_1()
                                        .child(div().text_sm().font_bold().text_color(fg).line_clamp(2).child(name))
                                        .child(div().text_xs().text_color(muted_fg).child(subtitle))
                                        .child(h_flex().gap_3().items_center().mt_1()
                                            .child(div().text_xs().text_color(muted_fg).child(format!("\u{1F441} {}", views)))
                                            .child(div().text_xs().text_color(muted_fg).child(format!("\u{2665} {}", likes)))
                                            .when(show_like, |el| el.child(
                                                div()
                                                    .id(SharedString::from(format!("like-{}", idx)))
                                                    .ml_auto()
                                                    .cursor_pointer()
                                                    .text_sm()
                                                    .text_color(if is_liked { gpui::Hsla::from(gpui::rgb(0xEF4444)) } else { muted_fg })
                                                    .opacity(if like_busy { 0.4 } else { 1.0 })
                                                    .child(if is_liked { "\u{2665}" } else { "\u{2661}" })
                                                    .on_click(cx.listener(move |this, _event, _, cx| {
                                                        cx.stop_propagation();
                                                        crate::handlers::on_toggle_like(this, like_uid.clone(), cx);
                                                    }))
                                            ))
                                        )
                                    ))
                                    .into_any_element()
                            }).collect();

                            h_flex().px_4().py(px(8.0)).gap_4().items_start()
                                .children(row_cards)
                                .into_any_element()

                        } else if row_idx < res_rows + s_rows {
                            h_flex().px_4().py(px(8.0)).gap_4().items_start()
                                .children((0..cols).map(|i| {
                                    div()
                                        .id(SharedString::from(format!("skel-{}-{}", row_idx, i)))
                                        .w(px(actual_card_w)).rounded_lg()
                                        .border_1().border_color(border_col)
                                        .overflow_hidden().flex().flex_col()
                                        .child(Skeleton::new().w_full().h(px(146.0)))
                                        .child(div().p_3().child(v_flex().gap_2()
                                            .child(Skeleton::new().w(px(160.0)).h_4())
                                            .child(Skeleton::new().secondary(true).w(px(100.0)).h_3())
                                            .child(Skeleton::new().secondary(true).w(px(120.0)).h_3())
                                        ))
                                }))
                                .into_any_element()
                        } else {
                            h_flex().w_full().px_4().justify_center().py_3()
                                .child(div().text_xs().text_color(muted_fg).child("End of results"))
                                .into_any_element()
                        }
                    }).collect()
                },
            ).track_scroll(&window.results_scroll_handle)
        )
        .child(div().absolute().inset_0()
            .child(Scrollbar::vertical(&window.results_scroll_state, &window.results_scroll_handle)))
        .into_any_element()
}
