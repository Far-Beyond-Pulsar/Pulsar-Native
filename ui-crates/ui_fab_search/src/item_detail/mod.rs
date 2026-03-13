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
use ui::{v_flex, ActiveTheme, scroll::{Scrollbar, ScrollbarState}};

use crate::parser::{strip_html, SketchfabModelDetail};

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
        }
    }
}

impl RenderOnce for ItemDetailView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let d = self.detail;
        let images = self.images;

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

