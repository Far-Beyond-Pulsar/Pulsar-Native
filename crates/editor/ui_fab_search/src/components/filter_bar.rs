use gpui::{prelude::*, *};
use ui::{ActiveTheme, StyledExt};

use crate::FabSearchWindow;
use crate::utils::actions::{LicenseFilter, SortBy};

pub fn render_filter_bar(
    window: &FabSearchWindow,
    border_col: gpui::Hsla,
    muted_fg: gpui::Hsla,
    accent_bg: gpui::Hsla,
    accent_fg: gpui::Hsla,
    active_bg: gpui::Hsla,
    active_fg: gpui::Hsla,
    cx: &mut Context<FabSearchWindow>,
) -> AnyElement {
    let dl_active = window.filter_downloadable;
    let an_active = window.filter_animated;
    let sp_active = window.filter_staffpicked;
    let lic_active = window.filter_license != LicenseFilter::All;
    let lic_label = window.filter_license.label();
    let show_lic = window.show_license_menu;

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
            let active = window.sort_by == s;
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
                        crate::handlers::on_begin_search(this, cx);
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
                .child("\u{2193} Download")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter_downloadable = !this.filter_downloadable;
                    if !this.search_query.is_empty() {
                        crate::handlers::on_begin_search(this, cx);
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
                .child("\u{25B6} Animated")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter_animated = !this.filter_animated;
                    if !this.search_query.is_empty() {
                        crate::handlers::on_begin_search(this, cx);
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
                .child("\u{2605} Staff Pick")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter_staffpicked = !this.filter_staffpicked;
                    if !this.search_query.is_empty() {
                        crate::handlers::on_begin_search(this, cx);
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
                .child(format!("\u{00A9} {}", lic_label))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_license_menu = !this.show_license_menu;
                    cx.notify();
                })),
        )
        .when(show_lic, |el| {
            el.children(LicenseFilter::all().into_iter().map(|lic| {
                let sel = window.filter_license == lic;
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
                            crate::handlers::on_begin_search(this, cx);
                        } else {
                            cx.notify();
                        }
                    }))
            }))
        });

    filter_bar.into_any_element()
}
