//! Right-hand info card of the item detail view.
//!
//! Owns the floating sidebar: seller, quick stats, license, the download
//! call-to-action (idle / in progress / done / error), available formats and the
//! "View on Sketchfab" link. Entry point: [`render_sidebar`]; the download label
//! formatting is the pure [`download_progress_text`].

use gpui::{prelude::*, *};
use ui::{
    Icon, IconName, Sizable, Size, StyledExt,
    avatar::Avatar,
    button::{Button, ButtonVariants as _},
    fmt_speed, h_flex, reveal_in_file_manager,
    tag::Tag,
    v_flex,
};

use super::{AuthorInfo, DetailColors, short_date};
use crate::DownloadState;
use crate::parser::fmt_count;

/// Everything the sidebar shows besides the shared author data and colors.
pub(super) struct SidebarContent {
    pub author: AuthorInfo,
    pub license_label: Option<SharedString>,
    pub is_downloadable: bool,
    pub on_download: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
    pub download_status: Option<DownloadState>,
    pub formats: Vec<String>,
    pub viewer_url: SharedString,
}

/// Floating info card with its sections stacked top to bottom.
pub(super) fn render_sidebar(
    content: SidebarContent,
    colors: &DetailColors,
) -> impl IntoElement + use<> {
    let SidebarContent {
        author,
        license_label,
        is_downloadable,
        on_download,
        download_status,
        formats,
        viewer_url,
    } = content;

    v_flex()
        .id("detail-sidebar-card")
        .w(px(300.0))
        .flex_shrink_0()
        .bg(colors.card_bg)
        .rounded_xl()
        .border_1()
        .border_color(colors.border)
        .shadow_lg()
        .overflow_hidden()
        // ── author ────────────────────────────────────────────────
        .child(render_sidebar_author(&author, colors))
        // ── stats ─────────────────────────────────────────────────
        .child(render_sidebar_stats(&author, colors))
        // ── license ───────────────────────────────────────────────
        .child(render_license_block(license_label, is_downloadable, colors))
        // ── download CTA ──────────────────────────────────────────
        .child(render_download_cta(on_download, download_status, colors))
        // ── available formats ─────────────────────────────────────
        .when(!formats.is_empty(), |card| {
            card.child(render_formats_block(formats, colors))
        })
        // ── view on sketchfab ─────────────────────────────────────
        .child(render_view_on_sketchfab(viewer_url, colors))
}

/// Seller avatar, name and category at the top of the card.
fn render_sidebar_author(author: &AuthorInfo, colors: &DetailColors) -> impl IntoElement + use<> {
    let DetailColors {
        border, fg, muted, ..
    } = *colors;

    h_flex()
        .px_5()
        .pt_5()
        .pb_4()
        .gap_3()
        .items_center()
        .border_b_1()
        .border_color(border)
        .child(
            Avatar::new()
                .with_size(Size::Medium)
                .name(author.name.clone())
                .map(|av: Avatar| {
                    if let Some(arc) = author.avatar.clone() {
                        av.src(gpui::ImageSource::Render(arc))
                    } else {
                        av
                    }
                }),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_bold()
                        .text_color(fg)
                        .truncate()
                        .child(author.name.clone()),
                )
                .when_some(author.category.clone(), |el, cat| {
                    el.child(div().text_xs().text_color(muted).child(cat))
                }),
        )
}

/// Row of compact view / like / download / date chips.
fn render_sidebar_stats(author: &AuthorInfo, colors: &DetailColors) -> impl IntoElement + use<> {
    let DetailColors {
        border, fg, muted, ..
    } = *colors;

    h_flex()
        .px_5()
        .py_3()
        .gap_5()
        .border_b_1()
        .border_color(border)
        .child(mini_stat(
            "\u{1F441}",
            fmt_count(author.view_count),
            fg,
            muted,
        ))
        .child(mini_stat(
            "\u{2665}",
            fmt_count(author.like_count),
            fg,
            muted,
        ))
        .when(author.download_count > 0, |el| {
            el.child(mini_stat(
                "\u{2193}",
                fmt_count(author.download_count),
                fg,
                muted,
            ))
        })
        .when_some(author.published_at.as_deref(), |el, date| {
            el.child(mini_stat("Cal", short_date(date).to_string(), fg, muted))
        })
}

/// Compact vertical stat chip: icon label on top, value below.
fn mini_stat(
    icon: &str,
    value: String,
    fg: gpui::Hsla,
    muted: gpui::Hsla,
) -> impl IntoElement + use<> {
    v_flex()
        .gap(px(1.0))
        .child(div().text_xs().text_color(muted).child(icon.to_string()))
        .child(div().text_sm().font_medium().text_color(fg).child(value))
}

/// License name with a "Free" badge or a "Purchase required" note.
fn render_license_block(
    license_label: Option<SharedString>,
    is_downloadable: bool,
    colors: &DetailColors,
) -> impl IntoElement + use<> {
    let DetailColors {
        border,
        fg,
        muted,
        success,
        ..
    } = *colors;

    v_flex()
        .px_5()
        .pt_4()
        .pb_4()
        .gap_2()
        .border_b_1()
        .border_color(border)
        .child(
            div()
                .text_xs()
                .font_bold()
                .text_color(muted)
                .child("LICENSE"),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .flex_wrap()
                .when_some(license_label, |el, label| {
                    el.child(div().text_sm().font_semibold().text_color(fg).child(label))
                })
                .when(is_downloadable, |el| {
                    el.child(
                        div()
                            .px_2()
                            .py(px(2.0))
                            .rounded_full()
                            .bg(gpui::rgb(0x14532D))
                            .text_xs()
                            .font_bold()
                            .text_color(success)
                            .child("\u{2713} Free"),
                    )
                })
                .when(!is_downloadable, |el| {
                    el.child(div().text_xs().text_color(muted).child("Purchase required"))
                }),
        )
}

/// Progress label for an in-flight download: percentage (or KB received) plus speed.
pub(super) fn download_progress_text(
    bytes_received: u64,
    total_bytes: Option<u64>,
    speed_bps: f64,
) -> String {
    let pct = total_bytes
        .filter(|&t| t > 0)
        .map(|t| format!("{:.0}%", bytes_received as f64 / t as f64 * 100.0))
        .unwrap_or_else(|| format!("{} KB", bytes_received / 1024));
    let speed_str = if speed_bps > 0.0 {
        format!(" \u{00B7} {}", fmt_speed(speed_bps))
    } else {
        String::new()
    };
    format!("{}{}", pct, speed_str)
}

/// Download section: a Download button, live progress, an Open Folder link or an error with Retry.
fn render_download_cta(
    on_download: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
    download_status: Option<DownloadState>,
    colors: &DetailColors,
) -> impl IntoElement + use<> {
    let DetailColors {
        fg, muted, success, ..
    } = *colors;

    v_flex().px_5().pt_4().pb_5().gap_3().map(move |section| {
        if let Some(dl_fn) = on_download {
            match download_status.as_ref() {
                Some(DownloadState::InProgress {
                    bytes_received,
                    total_bytes,
                    speed_bps,
                    ..
                }) => section
                    .child(
                        div()
                            .text_xs()
                            .font_bold()
                            .text_color(muted)
                            .child("DOWNLOADING"),
                    )
                    .child(div().text_sm().font_medium().text_color(fg).child(
                        download_progress_text(*bytes_received, *total_bytes, *speed_bps),
                    )),
                Some(DownloadState::Done { path, .. }) => {
                    let path = path.clone();
                    section
                        .child(
                            div()
                                .text_sm()
                                .font_bold()
                                .text_color(success)
                                .child("Downloaded \u{2713}"),
                        )
                        .child(
                            Button::new("sidebar-open-folder")
                                .ghost()
                                .small()
                                .label("\u{1F4C2} Open Folder")
                                .on_click(move |_, _, _| {
                                    reveal_in_file_manager(&path);
                                }),
                        )
                }
                Some(DownloadState::Error { message, .. }) => {
                    let msg = message.clone();
                    section
                        .child(
                            div()
                                .text_xs()
                                .text_color(gpui::red())
                                .child(format!("Error: {}", msg)),
                        )
                        .child(
                            Button::new("sidebar-retry")
                                .ghost()
                                .small()
                                .label("Retry")
                                .on_click(move |_, w, cx| {
                                    (dl_fn)(w, cx);
                                }),
                        )
                }
                None => section.child(
                    Button::new("sidebar-download")
                        .label("\u{2193} Download glTF")
                        .primary()
                        .on_click(move |_, w, cx| {
                            (dl_fn)(w, cx);
                        }),
                ),
            }
        } else {
            section.child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Sign in to download"),
            )
        }
    })
}

/// "AVAILABLE FORMATS" heading with one tag per downloadable archive format.
fn render_formats_block(formats: Vec<String>, colors: &DetailColors) -> impl IntoElement + use<> {
    v_flex()
        .px_5()
        .pt_4()
        .pb_4()
        .gap_2()
        .border_t_1()
        .border_color(colors.border)
        .child(
            div()
                .text_xs()
                .font_bold()
                .text_color(colors.muted)
                .child("AVAILABLE FORMATS"),
        )
        .child(
            div().flex().flex_row().flex_wrap().gap_2().children(
                formats
                    .into_iter()
                    .map(|fmt| Tag::primary().small().rounded_full().child(fmt)),
            ),
        )
}

/// Footer button that opens the model on Sketchfab.
fn render_view_on_sketchfab(
    viewer_url: SharedString,
    colors: &DetailColors,
) -> impl IntoElement + use<> {
    div()
        .px_5()
        .pt_3()
        .pb_5()
        .border_t_1()
        .border_color(colors.border)
        .child(
            Button::new("sidebar-view-sketchfab")
                .ghost()
                .small()
                .icon(Icon::new(IconName::ExternalLink).small())
                .label("View on Sketchfab")
                .on_click(move |_, _, cx| {
                    cx.open_url(viewer_url.as_ref());
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::download_progress_text;

    #[test]
    fn progress_shows_percentage_when_total_is_known() {
        assert_eq!(download_progress_text(50, Some(200), 0.0), "25%");
        assert_eq!(download_progress_text(200, Some(200), 0.0), "100%");
    }

    #[test]
    fn progress_falls_back_to_kb_without_a_usable_total() {
        assert_eq!(download_progress_text(4096, None, 0.0), "4 KB");
        assert_eq!(download_progress_text(2048, Some(0), 0.0), "2 KB");
    }

    #[test]
    fn progress_appends_speed_only_when_positive() {
        let with_speed = download_progress_text(50, Some(100), 1_000_000.0);
        assert!(with_speed.starts_with("50% \u{00B7} "), "{}", with_speed);
        assert_eq!(download_progress_text(50, Some(100), 0.0), "50%");
    }
}
