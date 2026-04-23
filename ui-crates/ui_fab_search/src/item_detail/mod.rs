pub mod changelog;
pub mod description;
pub mod format_tags;
pub mod gallery;
pub mod header;
pub mod license_section;
pub mod meta_bar;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Icon, IconName, Sizable, Size, StyledExt,
    avatar::Avatar,
    button::{Button, ButtonVariants as _},
    fmt_speed, h_flex, reveal_in_file_manager,
    scroll::{Scrollbar, ScrollbarState},
    tag::Tag,
    v_flex,
};

use crate::DownloadState;
use crate::parser::{SketchfabModelDetail, fmt_count, strip_html};

use changelog::ModelStatsSection;
use description::DescriptionSection;
use format_tags::FormatTagsSection;
use gallery::{GalleryHero, GalleryImage, GalleryThumbnailRow};
use header::DetailHeader;

#[derive(IntoElement)]
pub struct ItemDetailView {
    detail: Box<SketchfabModelDetail>,
    images: HashMap<String, Arc<gpui::RenderImage>>,
    scroll_handle: ScrollHandle,
    scroll_state: ScrollbarState,
    gallery_scroll_handle: ScrollHandle,
    gallery_scroll_state: ScrollbarState,
    on_back: Box<dyn Fn(&mut Window, &mut App) + 'static>,
    on_download: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
    download_status: Option<DownloadState>,
    selected_image_idx: usize,
    on_select_image: Option<Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>>,
}

impl ItemDetailView {
    pub fn new(
        detail: Box<SketchfabModelDetail>,
        images: HashMap<String, Arc<gpui::RenderImage>>,
        scroll_handle: ScrollHandle,
        scroll_state: ScrollbarState,
        gallery_scroll_handle: ScrollHandle,
        gallery_scroll_state: ScrollbarState,
        on_back: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            detail,
            images,
            scroll_handle,
            scroll_state,
            gallery_scroll_handle,
            gallery_scroll_state,
            on_back: Box::new(on_back),
            on_download: None,
            download_status: None,
            selected_image_idx: 0,
            on_select_image: None,
        }
    }

    pub(crate) fn with_download(
        mut self,
        on_download: impl Fn(&mut Window, &mut App) + 'static,
        status: Option<DownloadState>,
    ) -> Self {
        self.on_download = Some(Box::new(on_download));
        self.download_status = status;
        self
    }

    pub(crate) fn with_selected_image(
        mut self,
        idx: usize,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.selected_image_idx = idx;
        self.on_select_image = Some(Rc::new(on_select));
        self
    }
}

impl RenderOnce for ItemDetailView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let d = self.detail;
        let images = self.images;
        let on_download = self.on_download;
        let download_status = self.download_status;
        let selected_idx = self.selected_image_idx;
        let on_select_image = self.on_select_image;

        let theme = cx.theme().clone();
        let bg = theme.background;
        let border = theme.border;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let card_bg = theme.popover;
        let success: gpui::Hsla = gpui::rgb(0x22C55E).into();

        let viewer_url = SharedString::from(d.viewer_url.clone());
        let viewer_url2 = viewer_url.clone();

        // ── header (sticky) ─────────────────────────────────────────────────
        let header = DetailHeader::new(d.name.clone(), viewer_url.clone(), self.on_back);

        // ── gallery images ───────────────────────────────────────────────────
        let raw_urls: Vec<String> = d
            .all_thumbnail_urls()
            .into_iter()
            .take(12)
            .map(|s| s.to_string())
            .collect();

        let make_images = |urls: &Vec<String>| -> Vec<GalleryImage> {
            urls.iter()
                .map(|url| GalleryImage {
                    url: SharedString::from(url.clone()),
                    width: 1920,
                    height: 1080,
                    image: images.get(url.as_str()).cloned(),
                })
                .collect()
        };

        let n = raw_urls.len();
        let hero_images = make_images(&raw_urls);
        let thumb_images = make_images(&raw_urls);

        let thumb_callbacks: Vec<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>> = (0
            ..n)
            .map(|i| {
                let cb = on_select_image.clone();
                let f: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)> =
                    Box::new(move |_, w, cx| {
                        if let Some(f) = &cb {
                            f(i, w, cx);
                        }
                    });
                f
            })
            .collect();

        let hero = GalleryHero::new(hero_images, selected_idx);
        let thumb_row = GalleryThumbnailRow::new(
            thumb_images,
            selected_idx,
            self.gallery_scroll_handle.clone(),
            self.gallery_scroll_state.clone(),
            thumb_callbacks,
        );

        // ── author / meta ────────────────────────────────────────────────────
        let seller_name: SharedString = d
            .user
            .as_ref()
            .map(|u| SharedString::from(u.display().to_string()))
            .unwrap_or_default();
        let seller_avatar: Option<Arc<gpui::RenderImage>> = d
            .user
            .as_ref()
            .and_then(|u| u.avatar_url(128))
            .and_then(|url| images.get(url))
            .cloned();
        let category: Option<SharedString> = d
            .categories
            .first()
            .map(|c| SharedString::from(c.name.clone()));
        let published_at: Option<String> = d.published_at.clone();
        let view_count = d.view_count;
        let like_count = d.like_count;
        let download_count = d.download_count;

        // ── content sections ─────────────────────────────────────────────────
        let raw_desc = d.description.as_deref().unwrap_or("");
        let desc_text = strip_html(raw_desc);
        let show_desc = !desc_text.is_empty();
        let description = DescriptionSection::new(SharedString::from(desc_text));

        let tags: Vec<SharedString> = d
            .tags
            .iter()
            .map(|t| SharedString::from(t.slug.clone()))
            .collect();
        let show_tags = !tags.is_empty();
        let tag_section = FormatTagsSection::new(vec![], tags);

        let stats = ModelStatsSection::new(
            view_count,
            like_count,
            download_count,
            d.face_count,
            d.vertex_count,
            d.material_count,
            d.texture_count,
            d.animation_count,
            d.sound_count,
            d.pbr_type.clone(),
        );

        // ── sidebar data ─────────────────────────────────────────────────────
        let sidebar_formats: Vec<String> = d
            .archives
            .as_ref()
            .map(|a| {
                a.available()
                    .into_iter()
                    .map(|(lbl, arc)| {
                        let size = arc.size_label().unwrap_or_default();
                        if size.is_empty() {
                            lbl.to_string()
                        } else {
                            format!("{} ({})", lbl, size)
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let license_label: Option<SharedString> = d.license_label().map(SharedString::from);
        let is_downloadable = d.is_downloadable;

        // ════════════════════════════════════════════════════════════════
        //  LEFT COLUMN — media + prose
        // ════════════════════════════════════════════════════════════════
        let left_col =
            v_flex()
                .id("detail-left-col")
                .flex_1()
                .min_w_0()
                // ── hero + thumbnail strip ────────────────────────────────
                .child(
                    v_flex()
                        .w_full()
                        .gap_3()
                        .pb_2()
                        .child(hero)
                        .child(thumb_row),
                )
                // ── author row ────────────────────────────────────────────
                .child(
                    h_flex()
                        .w_full()
                        .px_5()
                        .py_4()
                        .gap_3()
                        .items_center()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            Avatar::new()
                                .with_size(Size::Medium)
                                .name(seller_name.clone())
                                .map(|av: Avatar| {
                                    if let Some(arc) = seller_avatar.clone() {
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
                                        .child(seller_name.clone()),
                                )
                                .when_some(category.clone(), |el, cat| {
                                    el.child(div().text_xs().text_color(muted).child(cat))
                                }),
                        )
                        .child(div().flex_1())
                        .child(
                            h_flex()
                                .gap_4()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("👁 {}", fmt_count(view_count))),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("♥ {}", fmt_count(like_count))),
                                )
                                .when_some(published_at.as_deref(), |el, date| {
                                    el.child(div().text_xs().text_color(muted).child(format!(
                                        "Published {}",
                                        &date[..date.len().min(10)]
                                    )))
                                }),
                        ),
                )
                .when(show_desc, |el| el.child(description))
                .when(show_tags, |el| el.child(tag_section))
                .child(stats);

        // ════════════════════════════════════════════════════════════════
        //  RIGHT COLUMN — floating info card
        // ════════════════════════════════════════════════════════════════
        let sidebar = v_flex()
            .id("detail-sidebar-card")
            .w(px(300.0))
            .flex_shrink_0()
            .bg(card_bg)
            .rounded_xl()
            .border_1()
            .border_color(border)
            .shadow_lg()
            .overflow_hidden()
            // ── author ────────────────────────────────────────────────
            .child(
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
                            .name(seller_name.clone())
                            .map(|av: Avatar| {
                                if let Some(arc) = seller_avatar {
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
                                    .child(seller_name),
                            )
                            .when_some(category, |el, cat| {
                                el.child(div().text_xs().text_color(muted).child(cat))
                            }),
                    ),
            )
            // ── stats ─────────────────────────────────────────────────
            .child(
                h_flex()
                    .px_5()
                    .py_3()
                    .gap_5()
                    .border_b_1()
                    .border_color(border)
                    .child(mini_stat("\u{1F441}", fmt_count(view_count), fg, muted))
                    .child(mini_stat("\u{2665}", fmt_count(like_count), fg, muted))
                    .when(download_count > 0, |el| {
                        el.child(mini_stat("\u{2193}", fmt_count(download_count), fg, muted))
                    })
                    .when_some(published_at.as_deref(), |el, date| {
                        el.child(mini_stat(
                            "Cal",
                            date[..date.len().min(10)].to_string(),
                            fg,
                            muted,
                        ))
                    }),
            )
            // ── license ───────────────────────────────────────────────
            .child(
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
                                el.child(
                                    div().text_sm().font_semibold().text_color(fg).child(label),
                                )
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
                                el.child(
                                    div().text_xs().text_color(muted).child("Purchase required"),
                                )
                            }),
                    ),
            )
            // ── download CTA ──────────────────────────────────────────
            .child(v_flex().px_5().pt_4().pb_5().gap_3().map(|section| {
                if let Some(dl_fn) = on_download {
                    match download_status.as_ref() {
                        Some(DownloadState::InProgress {
                            bytes_received,
                            total_bytes,
                            speed_bps,
                            ..
                        }) => {
                            let pct = total_bytes
                                .filter(|&t| t > 0)
                                .map(|t| {
                                    format!("{:.0}%", *bytes_received as f64 / t as f64 * 100.0)
                                })
                                .unwrap_or_else(|| format!("{} KB", bytes_received / 1024));
                            let speed_str = if *speed_bps > 0.0 {
                                format!(" \u{00B7} {}", fmt_speed(*speed_bps))
                            } else {
                                String::new()
                            };
                            section
                                .child(
                                    div()
                                        .text_xs()
                                        .font_bold()
                                        .text_color(muted)
                                        .child("DOWNLOADING"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_medium()
                                        .text_color(fg)
                                        .child(format!("{}{}", pct, speed_str)),
                                )
                        }
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
            }))
            // ── available formats ─────────────────────────────────────
            .when(!sidebar_formats.is_empty(), |card| {
                card.child(
                    v_flex()
                        .px_5()
                        .pt_4()
                        .pb_4()
                        .gap_2()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            div()
                                .text_xs()
                                .font_bold()
                                .text_color(muted)
                                .child("AVAILABLE FORMATS"),
                        )
                        .child(
                            div().flex().flex_row().flex_wrap().gap_2().children(
                                sidebar_formats
                                    .into_iter()
                                    .map(|fmt| Tag::primary().small().rounded_full().child(fmt)),
                            ),
                        ),
                )
            })
            // ── view on sketchfab ─────────────────────────────────────
            .child(
                div()
                    .px_5()
                    .pt_3()
                    .pb_5()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        Button::new("sidebar-view-sketchfab")
                            .ghost()
                            .small()
                            .icon(Icon::new(IconName::ExternalLink).small())
                            .label("View on Sketchfab")
                            .on_click(move |_, _, cx| {
                                cx.open_url(viewer_url2.as_ref());
                            }),
                    ),
            );

        // ════════════════════════════════════════════════════════════════
        //  ASSEMBLY — single scroll, everything flows together
        // ════════════════════════════════════════════════════════════════
        div()
            .id("item-detail-root")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(bg)
            // sticky header strip
            .child(header)
            // body — one scroll region
            .child(
                div()
                    .id("detail-body")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("detail-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .child(
                                div().w_full().px_6().pt_6().pb_16().child(
                                    h_flex()
                                        .w_full()
                                        .gap_6()
                                        .items_start()
                                        .child(left_col)
                                        .child(sidebar),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::vertical(&self.scroll_state, &self.scroll_handle)),
                    ),
            )
    }
}

/// Compact vertical stat chip: icon label on top, value below.
fn mini_stat(icon: &str, value: String, fg: gpui::Hsla, muted: gpui::Hsla) -> impl IntoElement {
    v_flex()
        .gap(px(1.0))
        .child(div().text_xs().text_color(muted).child(icon.to_string()))
        .child(div().text_sm().font_medium().text_color(fg).child(value))
}
