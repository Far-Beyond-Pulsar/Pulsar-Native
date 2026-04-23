//! FabSearchWindow render helper methods.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, IconName, Sizable, StyledExt,
    button::Button,
    download_item::DownloadItemStatus,
    download_manager::{DownloadEntry, DownloadManagerDrawer},
    h_flex,
    input::TextInput,
    scroll::Scrollbar,
    skeleton::Skeleton,
    spinner::Spinner,
    v_flex, v_virtual_list,
};

use crate::FabSearchWindow;
use crate::dispatch::{DownloadState, LicenseFilter, SortBy};
use crate::parser::fmt_count;

impl FabSearchWindow {
    pub(crate) fn render_filter_bar(
        &self,
        border_col: gpui::Hsla,
        muted_fg: gpui::Hsla,
        accent_bg: gpui::Hsla,
        accent_fg: gpui::Hsla,
        active_bg: gpui::Hsla,
        active_fg: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dl_active = self.filter_downloadable;
        let an_active = self.filter_animated;
        let sp_active = self.filter_staffpicked;
        let lic_active = self.filter_license != LicenseFilter::All;
        let lic_label = self.filter_license.label();
        let show_lic = self.show_license_menu;

        let filter_bar = div()
            .w_full()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(border_col)
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(muted_fg)
                    .child("Sort:"),
            )
            .children(SortBy::all().into_iter().map(|s| {
                let active = self.sort_by == s;
                let lbl = s.label();
                div()
                    .id(SharedString::from(format!("sort-{}", lbl)))
                    .cursor_pointer()
                    .px_2()
                    .py(px(3.0))
                    .rounded_full()
                    .text_xs()
                    .font_medium()
                    .bg(if active { active_bg } else { accent_bg })
                    .text_color(if active { active_fg } else { accent_fg })
                    .child(lbl)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sort_by = s;
                        if !this.search_query.is_empty() {
                            this.begin_search(cx);
                        } else {
                            cx.notify();
                        }
                    }))
            }))
            .child(div().w(px(1.0)).h(px(16.0)).bg(border_col))
            .child(
                div()
                    .id("f-dl")
                    .cursor_pointer()
                    .px_2()
                    .py(px(3.0))
                    .rounded_full()
                    .text_xs()
                    .font_medium()
                    .bg(if dl_active { active_bg } else { accent_bg })
                    .text_color(if dl_active { active_fg } else { accent_fg })
                    .child("↓ Download")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter_downloadable = !this.filter_downloadable;
                        if !this.search_query.is_empty() {
                            this.begin_search(cx);
                        } else {
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .id("f-an")
                    .cursor_pointer()
                    .px_2()
                    .py(px(3.0))
                    .rounded_full()
                    .text_xs()
                    .font_medium()
                    .bg(if an_active { active_bg } else { accent_bg })
                    .text_color(if an_active { active_fg } else { accent_fg })
                    .child("▶ Animated")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter_animated = !this.filter_animated;
                        if !this.search_query.is_empty() {
                            this.begin_search(cx);
                        } else {
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .id("f-sp")
                    .cursor_pointer()
                    .px_2()
                    .py(px(3.0))
                    .rounded_full()
                    .text_xs()
                    .font_medium()
                    .bg(if sp_active { active_bg } else { accent_bg })
                    .text_color(if sp_active { active_fg } else { accent_fg })
                    .child("★ Staff Pick")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.filter_staffpicked = !this.filter_staffpicked;
                        if !this.search_query.is_empty() {
                            this.begin_search(cx);
                        } else {
                            cx.notify();
                        }
                    })),
            )
            .child(div().w(px(1.0)).h(px(16.0)).bg(border_col))
            .child(
                div()
                    .id("f-lic")
                    .cursor_pointer()
                    .px_2()
                    .py(px(3.0))
                    .rounded_full()
                    .text_xs()
                    .font_medium()
                    .bg(if lic_active { active_bg } else { accent_bg })
                    .text_color(if lic_active { active_fg } else { accent_fg })
                    .child(format!("© {}", lic_label))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_license_menu = !this.show_license_menu;
                        cx.notify();
                    })),
            )
            .when(show_lic, |el| {
                el.children(LicenseFilter::all().into_iter().map(|lic| {
                    let sel = self.filter_license == lic;
                    let lbl = lic.label();
                    div()
                        .id(SharedString::from(format!("lic-{}", lbl)))
                        .cursor_pointer()
                        .px_2()
                        .py(px(3.0))
                        .rounded_full()
                        .text_xs()
                        .border_1()
                        .border_color(border_col)
                        .bg(if sel {
                            active_bg
                        } else {
                            cx.theme().background
                        })
                        .text_color(if sel { active_fg } else { muted_fg })
                        .child(lbl)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.filter_license = lic;
                            this.show_license_menu = false;
                            if !this.search_query.is_empty() {
                                this.begin_search(cx);
                            } else {
                                cx.notify();
                            }
                        }))
                }))
            });

        filter_bar.into_any_element()
    }

    pub(crate) fn render_auth_section(
        &self,
        fg: gpui::Hsla,
        muted_fg: gpui::Hsla,
        border_col: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let logged_in = self.api_token.is_some();
        let me_name = self.me.as_ref().map(|m| m.display().to_string());
        let me_avatar = self
            .me
            .as_ref()
            .and_then(|m| m.avatar_url(64).map(|s| s.to_string()))
            .and_then(|url| self.image_cache.get(&url).and_then(|o| o.clone()));
        let me_loading = self.me_loading;
        let show_tok = self.show_token_input;
        let account_tier = self.me.as_ref().and_then(|m| m.account.clone());

        if me_loading {
            div()
                .text_xs()
                .text_color(muted_fg)
                .child("Verifying…")
                .into_any_element()
        } else if logged_in {
            h_flex()
                .gap_2()
                .items_center()
                .map(|el| {
                    if let Some(arc) = me_avatar {
                        el.child(
                            img(gpui::ImageSource::Render(arc))
                                .w_6()
                                .h_6()
                                .rounded_full()
                                .overflow_hidden(),
                        )
                    } else {
                        el.child(div().w_6().h_6().rounded_full().bg(border_col))
                    }
                })
                .when_some(me_name, |el, name| {
                    el.child(div().text_xs().font_medium().text_color(fg).child(name))
                })
                .when_some(account_tier, |el, tier| {
                    el.child(
                        div()
                            .text_xs()
                            .px_2()
                            .py(px(2.0))
                            .rounded_full()
                            .bg(gpui::rgb(0x6366F1))
                            .text_color(gpui::rgb(0xFFFFFF))
                            .child(tier),
                    )
                })
                .child(
                    Button::new("logout-btn")
                        .label("Logout")
                        .on_click(cx.listener(|this, _, _, cx| this.clear_token(cx))),
                )
                .into_any_element()
        } else if show_tok {
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w(px(300.0))
                        .child(TextInput::new(&self.token_input).w_full()),
                )
                .child(
                    Button::new("verify-tok")
                        .label("Verify & Save")
                        .on_click(cx.listener(|this, _, _window, cx| {
                            let tok = this.token_input.read(cx).value().to_string();
                            this.set_token(tok, cx);
                        })),
                )
                .child(
                    Button::new("cancel-tok")
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_token_input = false;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child("Get your token at sketchfab.com/settings/password"),
                )
                .into_any_element()
        } else {
            Button::new("login-btn")
                .label("Login with API Token")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_token_input = true;
                    cx.notify();
                }))
                .into_any_element()
        }
    }

    pub(crate) fn render_results_grid(
        &self,
        avail_w: f32,
        muted_fg: gpui::Hsla,
        border_col: gpui::Hsla,
        fg: gpui::Hsla,
    ) -> AnyElement {
        const CARD_W: f32 = 260.0;
        const CARD_H: f32 = 260.0;
        const GAP: f32 = 16.0;
        const PAD: f32 = 16.0;
        const ROW_H: f32 = CARD_H + GAP;

        let avail_w = avail_w - 2.0 * PAD;
        let cols: usize = (((avail_w + GAP) / (CARD_W + GAP)).floor() as usize).max(1);
        let actual_card_w: f32 = (avail_w - (cols - 1) as f32 * GAP) / cols as f32;

        let entity = self.entity.clone().unwrap();
        let total_results = self.results.len();
        let result_rows = total_results.div_ceil(cols);
        let skel_rows: usize = if self.is_loading_more { 2 } else { 0 };
        let end_row: usize = if !self.is_loading_more && self.next_url.is_none() {
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
                            view.load_more(cx);
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
                                    let subtitle = if category.is_empty() { author } else { format!("{} · {}", author, category) };
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
                                            this.open_item_detail(card_uid.clone(), cx);
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
                                                        .child("★ Staff Pick")
                                                ))
                                                .when(is_dl, |el| el.child(
                                                    div().absolute().bottom(px(6.0)).right(px(6.0))
                                                        .px_2().py(px(2.0)).rounded_full()
                                                        .bg(gpui::rgb(0x22C55E))
                                                        .text_xs().font_bold().text_color(gpui::rgb(0xFFFFFF))
                                                        .child("↓")
                                                ))
                                                .when(is_anim, |el| el.child(
                                                    div().absolute().bottom(px(6.0)).left(px(6.0))
                                                        .px_2().py(px(2.0)).rounded_full()
                                                        .bg(gpui::rgb(0x6366F1))
                                                        .text_xs().font_bold().text_color(gpui::rgb(0xFFFFFF))
                                                        .child("▶")
                                                ))
                                        )
                                        .child(div().p_3().child(v_flex().gap_1()
                                            .child(div().text_sm().font_bold().text_color(fg).line_clamp(2).child(name))
                                            .child(div().text_xs().text_color(muted_fg).child(subtitle))
                                            .child(h_flex().gap_3().items_center().mt_1()
                                                .child(div().text_xs().text_color(muted_fg).child(format!("👁 {}", views)))
                                                .child(div().text_xs().text_color(muted_fg).child(format!("♥ {}", likes)))
                                                .when(show_like, |el| el.child(
                                                    div()
                                                        .id(SharedString::from(format!("like-{}", idx)))
                                                        .ml_auto()
                                                        .cursor_pointer()
                                                        .text_sm()
                                                        .text_color(if is_liked { gpui::Hsla::from(gpui::rgb(0xEF4444)) } else { muted_fg })
                                                        .opacity(if like_busy { 0.4 } else { 1.0 })
                                                        .child(if is_liked { "♥" } else { "♡" })
                                                        .on_click(cx.listener(move |this, _event, _, cx| {
                                                            cx.stop_propagation();
                                                            this.toggle_like(like_uid.clone(), cx);
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
                ).track_scroll(&self.results_scroll_handle)
            )
            .child(div().absolute().inset_0()
                .child(Scrollbar::vertical(&self.results_scroll_state, &self.results_scroll_handle)))
            .into_any_element()
    }

    pub(crate) fn build_download_entries(&self) -> Vec<DownloadEntry> {
        self.download_state
            .iter()
            .map(|(uid, state)| match state {
                DownloadState::InProgress {
                    filename,
                    bytes_received,
                    total_bytes,
                    speed_bps,
                    speed_history,
                } => {
                    let progress_pct = total_bytes
                        .filter(|&t| t > 0)
                        .map(|t| (*bytes_received as f32 / t as f32 * 100.0).min(100.0))
                        .unwrap_or(0.0);
                    DownloadEntry {
                        uid: SharedString::from(uid.clone()),
                        filename: SharedString::from(filename.clone()),
                        progress_pct,
                        speed_bps: *speed_bps,
                        speed_history: speed_history.clone(),
                        status: DownloadItemStatus::InProgress,
                        bytes_received: *bytes_received,
                        total_bytes: *total_bytes,
                        path: None,
                    }
                }
                DownloadState::Done {
                    filename,
                    path,
                    total_bytes,
                } => DownloadEntry {
                    uid: SharedString::from(uid.clone()),
                    filename: SharedString::from(filename.clone()),
                    progress_pct: 100.0,
                    speed_bps: 0.0,
                    speed_history: Vec::new(),
                    status: DownloadItemStatus::Done,
                    bytes_received: *total_bytes,
                    total_bytes: Some(*total_bytes),
                    path: Some(path.clone()),
                },
                DownloadState::Error { filename, message } => DownloadEntry {
                    uid: SharedString::from(uid.clone()),
                    filename: SharedString::from(filename.clone()),
                    progress_pct: 0.0,
                    speed_bps: 0.0,
                    speed_history: Vec::new(),
                    status: DownloadItemStatus::Error(message.clone()),
                    bytes_received: 0,
                    total_bytes: None,
                    path: None,
                },
            })
            .collect()
    }
}
