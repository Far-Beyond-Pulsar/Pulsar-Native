pub mod changelog;
pub mod description;
pub mod format_tags;
pub mod gallery;
pub mod header;
pub mod license_section;
pub mod meta_bar;

mod left_column;
mod sidebar;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, h_flex,
    scroll::{Scrollbar, ScrollbarState},
};

use crate::DownloadState;
use crate::parser::{SketchfabModelDetail, strip_html};

use changelog::ModelStatsSection;
use description::DescriptionSection;
use format_tags::FormatTagsSection;
use gallery::{GalleryHero, GalleryImage, GalleryThumbnailRow};
use header::DetailHeader;
use left_column::render_left_column;
use sidebar::{SidebarContent, render_sidebar};

type ImageMap = HashMap<String, Arc<gpui::RenderImage>>;
type SelectImageCallback = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;

/// Theme colors shared by the regions of the detail view.
#[derive(Clone, Copy)]
struct DetailColors {
    border: Hsla,
    fg: Hsla,
    muted: Hsla,
    card_bg: Hsla,
    success: Hsla,
}

/// Seller, category and counters shown in both the left column and the sidebar.
#[derive(Clone)]
struct AuthorInfo {
    name: SharedString,
    avatar: Option<Arc<gpui::RenderImage>>,
    category: Option<SharedString>,
    published_at: Option<String>,
    view_count: i64,
    like_count: i64,
    download_count: i64,
}

impl AuthorInfo {
    /// Collects the author data from the model detail and the loaded image cache.
    fn from_detail(d: &SketchfabModelDetail, images: &ImageMap) -> Self {
        Self {
            name: d
                .user
                .as_ref()
                .map(|u| SharedString::from(u.display().to_string()))
                .unwrap_or_default(),
            avatar: d
                .user
                .as_ref()
                .and_then(|u| u.avatar_url(128))
                .and_then(|url| images.get(url))
                .cloned(),
            category: d
                .categories
                .first()
                .map(|c| SharedString::from(c.name.clone())),
            published_at: d.published_at.clone(),
            view_count: d.view_count,
            like_count: d.like_count,
            download_count: d.download_count,
        }
    }
}

/// Date prefix (`YYYY-MM-DD`) of an ISO timestamp.
fn short_date(date: &str) -> &str {
    &date[..date.len().min(10)]
}

/// Display label for an archive format, with its size in parentheses when known.
fn format_label(label: &str, size: &str) -> String {
    if size.is_empty() {
        label.to_string()
    } else {
        format!("{} ({})", label, size)
    }
}

#[derive(IntoElement)]
pub struct ItemDetailView {
    detail: Box<SketchfabModelDetail>,
    images: ImageMap,
    scroll_handle: ScrollHandle,
    scroll_state: ScrollbarState,
    gallery_scroll_handle: ScrollHandle,
    gallery_scroll_state: ScrollbarState,
    on_back: Box<dyn Fn(&mut Window, &mut App) + 'static>,
    on_download: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
    download_status: Option<DownloadState>,
    selected_image_idx: usize,
    on_select_image: Option<SelectImageCallback>,
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
        let ItemDetailView {
            detail: d,
            images,
            scroll_handle,
            scroll_state,
            gallery_scroll_handle,
            gallery_scroll_state,
            on_back,
            on_download,
            download_status,
            selected_image_idx: selected_idx,
            on_select_image,
        } = self;

        let theme = cx.theme().clone();
        let bg = theme.background;
        let colors = DetailColors {
            border: theme.border,
            fg: theme.foreground,
            muted: theme.muted_foreground,
            card_bg: theme.popover,
            success: gpui::rgb(0x22C55E).into(),
        };

        let viewer_url = SharedString::from(d.viewer_url.clone());

        // ── header (sticky) ─────────────────────────────────────────────────
        let header = DetailHeader::new(d.name.clone(), viewer_url.clone(), on_back);

        let (hero, thumb_row) = build_gallery(
            &d,
            &images,
            selected_idx,
            on_select_image,
            gallery_scroll_handle,
            gallery_scroll_state,
        );
        let author = AuthorInfo::from_detail(&d, &images);

        // ════════════════════════════════════════════════════════════════
        //  LEFT COLUMN — media + prose
        // ════════════════════════════════════════════════════════════════
        let left_col = render_left_column(
            hero,
            thumb_row,
            &author,
            build_description(&d),
            build_tags(&d),
            build_stats(&d),
            &colors,
        );

        // ════════════════════════════════════════════════════════════════
        //  RIGHT COLUMN — floating info card
        // ════════════════════════════════════════════════════════════════
        let sidebar = render_sidebar(
            SidebarContent {
                author,
                license_label: d.license_label().map(SharedString::from),
                is_downloadable: d.is_downloadable,
                on_download,
                download_status,
                formats: sidebar_formats(&d),
                viewer_url,
            },
            &colors,
        );

        // ════════════════════════════════════════════════════════════════
        //  ASSEMBLY — single scroll, everything flows together
        // ════════════════════════════════════════════════════════════════
        render_detail_root(bg, header, left_col, sidebar, scroll_handle, scroll_state)
    }
}

/// Hero image and thumbnail row for up to 12 model thumbnails; thumbnail clicks call `on_select_image`.
fn build_gallery(
    d: &SketchfabModelDetail,
    images: &ImageMap,
    selected_idx: usize,
    on_select_image: Option<SelectImageCallback>,
    gallery_scroll_handle: ScrollHandle,
    gallery_scroll_state: ScrollbarState,
) -> (GalleryHero, GalleryThumbnailRow) {
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

    let thumb_callbacks: Vec<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>> = (0..n)
        .map(|i| {
            let cb = on_select_image.clone();
            let f: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)> = Box::new(move |_, w, cx| {
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
        gallery_scroll_handle,
        gallery_scroll_state,
        thumb_callbacks,
    );
    (hero, thumb_row)
}

/// Description section with HTML stripped, or `None` when the model has no description.
fn build_description(d: &SketchfabModelDetail) -> Option<DescriptionSection> {
    let raw_desc = d.description.as_deref().unwrap_or("");
    let desc_text = strip_html(raw_desc);
    if desc_text.is_empty() {
        return None;
    }
    Some(DescriptionSection::new(SharedString::from(desc_text)))
}

/// Tag section listing the model's tag slugs, or `None` when it has no tags.
fn build_tags(d: &SketchfabModelDetail) -> Option<FormatTagsSection> {
    let tags: Vec<SharedString> = d
        .tags
        .iter()
        .map(|t| SharedString::from(t.slug.clone()))
        .collect();
    if tags.is_empty() {
        return None;
    }
    Some(FormatTagsSection::new(vec![], tags))
}

/// Model statistics section (counts, geometry and asset totals).
fn build_stats(d: &SketchfabModelDetail) -> ModelStatsSection {
    ModelStatsSection::new(
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
    )
}

/// Labels of the available download formats, each with its size when known.
fn sidebar_formats(d: &SketchfabModelDetail) -> Vec<String> {
    d.archives
        .as_ref()
        .map(|a| {
            a.available()
                .into_iter()
                .map(|(lbl, arc)| format_label(lbl, &arc.size_label().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default()
}

/// Root layout: sticky header above a single scroll region holding both columns and a scrollbar.
fn render_detail_root(
    bg: Hsla,
    header: DetailHeader,
    left_col: impl IntoElement,
    sidebar: impl IntoElement,
    scroll_handle: ScrollHandle,
    scroll_state: ScrollbarState,
) -> impl IntoElement {
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
                        .track_scroll(&scroll_handle)
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
                        .child(Scrollbar::vertical(&scroll_state, &scroll_handle)),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::{format_label, short_date};

    #[test]
    fn short_date_keeps_the_date_prefix() {
        assert_eq!(short_date("2024-05-17T10:20:30Z"), "2024-05-17");
        assert_eq!(short_date("2024"), "2024");
        assert_eq!(short_date(""), "");
    }

    #[test]
    fn format_label_includes_size_only_when_known() {
        assert_eq!(format_label("GLB", "12.3 MB"), "GLB (12.3 MB)");
        assert_eq!(format_label("glTF", ""), "glTF");
    }
}
