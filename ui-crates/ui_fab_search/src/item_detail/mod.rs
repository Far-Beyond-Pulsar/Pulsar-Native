pub mod changelog;
pub mod description;
pub mod format_tags;
pub mod gallery;
pub mod header;
pub mod license_section;
pub mod meta_bar;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{v_flex, h_flex, ActiveTheme, scroll::{Scrollbar, ScrollbarState}, button::Button};

use crate::parser::{strip_html, SketchfabModelDetail};
use crate::DownloadState;

use changelog::ModelStatsSection;
use description::DescriptionSection;
use format_tags::FormatTagsSection;
use gallery::{GalleryImage, GalleryStrip};
use header::DetailHeader;
use license_section::LicenseSection;
use meta_bar::MetaBar;

/// Full item detail page — scrollable column of rich sections.
#[derive(IntoElement)]
pub struct ItemDetailView {
    detail: Box<SketchfabModelDetail>,
    /// Pre-decoded images ready for synchronous rendering, keyed by their original download URL.
    images: HashMap<String, Arc<gpui::RenderImage>>,
    scroll_handle: ScrollHandle,
    scroll_state: ScrollbarState,
    gallery_scroll_handle: ScrollHandle,
    gallery_scroll_state: ScrollbarState,
    on_back: Box<dyn Fn(&mut Window, &mut App) + 'static>,
    /// Present when user is authenticated and the model is downloadable.
    on_download: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
    /// Current download state, if any.
    download_status: Option<DownloadState>,
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
}

impl RenderOnce for ItemDetailView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let d = self.detail;
        let images = self.images;
        let on_download = self.on_download;
        let download_status = self.download_status;

        // ── URL ─────────────────────────────────────────────────────────────
        let viewer_url = SharedString::from(d.viewer_url.clone());

        // ── header ──────────────────────────────────────────────────────────
        let header = DetailHeader::new(d.name.clone(), viewer_url.clone(), self.on_back);

        // ── meta bar ────────────────────────────────────────────────────────
        let seller_name: SharedString = d.user.as_ref()
            .map(|u| SharedString::from(u.display().to_string()))
            .unwrap_or_default();

        let seller_avatar: Option<Arc<gpui::RenderImage>> = d.user.as_ref()
            .and_then(|u| u.avatar_url(128))
            .and_then(|url| images.get(url))
            .cloned();

        let category = d.categories.first()
            .map(|c| SharedString::from(c.name.clone()));

        let meta = MetaBar::new(
            seller_name,
            seller_avatar,
            category,
            d.view_count,
            d.like_count,
            d.published_at.clone(),
        );

        // ── gallery ─────────────────────────────────────────────────────────
        let gallery_images: Vec<GalleryImage> = d
            .all_thumbnail_urls()
            .into_iter()
            .take(8)
            .map(|url| GalleryImage {
                url: SharedString::from(url.to_string()),
                width: 1920,
                height: 1080,
                image: images.get(url).cloned(),
            })
            .collect();

        let show_gallery = !gallery_images.is_empty();
        let gallery = GalleryStrip::new(
            gallery_images,
            self.gallery_scroll_handle.clone(),
            self.gallery_scroll_state.clone(),
        );

        // ── license ─────────────────────────────────────────────────────────
        let license_label: Option<SharedString> = d.license_label().map(SharedString::from);
        let show_license = license_label.is_some();
        let license_sec = LicenseSection::new(license_label, d.is_downloadable, viewer_url);

        // ── format tags ─────────────────────────────────────────────────────
        let formats: Vec<(String, String)> = d.archives.as_ref()
            .map(|a| {
                a.available()
                    .into_iter()
                    .map(|(lbl, arc)| {
                        let size_str = arc.size_label().unwrap_or_default();
                        let display = if size_str.is_empty() {
                            lbl.to_string()
                        } else {
                            format!("{} ({})", lbl, size_str)
                        };
                        (lbl.to_string(), display)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let tags: Vec<SharedString> = d.tags.iter()
            .map(|t| SharedString::from(t.slug.clone()))
            .collect();

        let show_format_tags = !formats.is_empty() || !tags.is_empty();
        let format_tags = FormatTagsSection::new(formats, tags);

        // ── description ─────────────────────────────────────────────────────
        let raw_desc = d.description.as_deref().unwrap_or("");
        let desc_text = strip_html(raw_desc);
        let show_desc = !desc_text.is_empty();
        let description = DescriptionSection::new(SharedString::from(desc_text));

        // ── model stats ──────────────────────────────────────────────────────
        let stats = ModelStatsSection::new(
            d.view_count,
            d.like_count,
            d.download_count,
            d.face_count,
            d.vertex_count,
            d.material_count,
            d.texture_count,
            d.animation_count,
            d.sound_count,
            d.pbr_type.clone(),
        );

        // ── assemble ────────────────────────────────────────────────────────
        let bg = cx.theme().background;

        div()
            .id("item-detail-root")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(bg)
            // sticky header (not inside scroll)
            .child(header)
            // scrollable body
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("item-detail-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll_handle)
                            .child(
                                v_flex()
                                    .w_full()
                                    .child(meta)
                                    .map(|el| {
                                        let muted_fg = cx.theme().muted_foreground;
                                        let green: gpui::Hsla = gpui::rgb(0x22C55E).into();
                                        if let Some(dl_fn) = on_download {
                                            el.child(
                                                h_flex().px_4().py_3().gap_3().items_center()
                                                    .border_b_1().border_color(cx.theme().border)
                                                    .map(|row| match &download_status {
                                                        Some(crate::DownloadState::InProgress {
                                                            bytes_received,
                                                            total_bytes,
                                                            speed_bps,
                                                            ..
                                                        }) => {
                                                            let prog = total_bytes
                                                                .filter(|&t| t > 0)
                                                                .map(|t| format!("{:.0}%", *bytes_received as f64 / t as f64 * 100.0))
                                                                .unwrap_or_else(|| format!("{} KB", bytes_received / 1024));
                                                            let speed_label = if *speed_bps > 0.0 {
                                                                format!(" · {}", ui::fmt_speed(*speed_bps))
                                                            } else { String::new() };
                                                            row.child(div().text_sm().text_color(muted_fg)
                                                                .child(format!("Downloading… {}{}", prog, speed_label)))
                                                        }
                                                        Some(crate::DownloadState::Done { path, .. }) => {
                                                            let file_path = path.clone();
                                                            row
                                                                .child(div().text_sm().text_color(green)
                                                                    .child("Downloaded ✓"))
                                                                .child(Button::new("open-folder")
                                                                    .label("📂 Open Folder")
                                                                    .on_click(move |_, _, _cx| {
                                                                        ui::reveal_in_file_manager(&file_path);
                                                                    }))
                                                        }
                                                        Some(crate::DownloadState::Error { message, .. }) => {
                                                            let msg = message.clone();
                                                            row.child(div().text_sm().text_color(gpui::red())
                                                                .child(format!("Error: {}", msg)))
                                                                .child(Button::new("retry-dl")
                                                                    .label("Retry")
                                                                    .on_click(move |_, window, cx| { (dl_fn)(window, cx); }))
                                                        }
                                                        None => {
                                                            row.child(Button::new("download-gltf")
                                                                .label("↓ Download glTF")
                                                                .on_click(move |_, window, cx| { (dl_fn)(window, cx); }))
                                                        }
                                                    })
                                            )
                                        } else {
                                            el
                                        }
                                    })
                                    .when(show_gallery, |el| el.child(gallery))
                                    .when(show_license, |el| el.child(license_sec))
                                    .when(show_format_tags, |el| el.child(format_tags))
                                    .when(show_desc, |el| el.child(description))
                                    .child(stats),
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

