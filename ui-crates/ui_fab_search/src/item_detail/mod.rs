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
use ui::{v_flex, ActiveTheme};

use crate::parser::{strip_html, FabItemDetail};

use changelog::{ChangelogEntry, ChangelogSection};
use description::DescriptionSection;
use format_tags::FormatTagsSection;
use gallery::{GalleryImage, GalleryStrip};
use header::DetailHeader;
use license_section::{LicenseEntry, LicenseSection};
use meta_bar::{MetaBar, RatingInfo};

/// Full item detail page — scrollable column of rich sections.
///
/// Constructs all sub-components from the raw [`FabItemDetail`] payload and
/// renders them in a polished, vertically-scrollable view.
#[derive(IntoElement)]
pub struct ItemDetailView {
    detail: Box<FabItemDetail>,
    /// Pre-decoded images ready for synchronous rendering, keyed by their original download URL.
    images: HashMap<String, Arc<gpui::RenderImage>>,
    on_back: Box<dyn Fn(&mut Window, &mut App) + 'static>,
}

impl ItemDetailView {
    pub fn new(
        detail: Box<FabItemDetail>,
        images: HashMap<String, Arc<gpui::RenderImage>>,
        on_back: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            detail,
            images,
            on_back: Box::new(on_back),
        }
    }
}

impl RenderOnce for ItemDetailView {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let d = self.detail;
        let images = self.images;

        // ── URL ─────────────────────────────────────────────────────────────
        let fab_url = SharedString::from(format!("https://www.fab.com/listings/{}", d.uid));

        // ── header ──────────────────────────────────────────────────────────
        let header = DetailHeader::new(d.title.clone(), fab_url, self.on_back);

        // ── meta bar ────────────────────────────────────────────────────────
        let rating = d.ratings.as_ref().map(|r| RatingInfo {
            average: r.average_rating,
            total: r.total,
            review_count: d.review_count,
            buckets: [
                r.rating5.unwrap_or(0),
                r.rating4.unwrap_or(0),
                r.rating3.unwrap_or(0),
                r.rating2.unwrap_or(0),
                r.rating1.unwrap_or(0),
            ],
        });

        let meta = MetaBar::new(
            d.user.seller_name.clone(),
            d.user.profile_image_url.clone(),
            d.category.as_ref().map(|c| c.name.clone()),
            rating,
            d.published_at.clone(),
        );

        // ── gallery ─────────────────────────────────────────────────────────
        let gallery_images: Vec<GalleryImage> = d
            .medias
            .iter()
            .filter(|m| m.media_type == "image" || m.media_type.is_empty())
            .take(12)
            .map(|m| {
                // Prefer the largest thumbnail image URL, fall back to media_url
                let url = m
                    .images
                    .iter()
                    .max_by_key(|i| i.width)
                    .map(|i| i.url.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or(m.media_url.as_str())
                    .to_string();
                let (w, h) = m
                    .images
                    .first()
                    .map(|i| (i.width, i.height))
                    .unwrap_or((1280, 720));
                GalleryImage {
                    url: SharedString::from(url.clone()),
                    width: w,
                    height: h,
                    image: images.get(&url).cloned(),
                }
            })
            .collect();

        let show_gallery = !gallery_images.is_empty();
        let gallery = GalleryStrip::new(gallery_images);

        // ── licenses ────────────────────────────────────────────────────────
        let is_free = d.is_free;
        let license_entries: Vec<LicenseEntry> = d
            .licenses
            .iter()
            .map(|l| {
                let price = if is_free {
                    SharedString::from("Free")
                } else if let Some(ref pt) = l.price_tier {
                    let effective = pt.discounted_price.unwrap_or(pt.price);
                    SharedString::from(format!("{} {:.2}", pt.currency_code, effective))
                } else {
                    SharedString::from("—")
                };

                let purchase_url = l.slug.as_ref().map(|slug| {
                    SharedString::from(format!("https://www.fab.com/listings/{}", slug))
                });

                LicenseEntry {
                    name: SharedString::from(l.name.clone()),
                    price,
                    purchase_url,
                }
            })
            .collect();

        let show_licenses = !license_entries.is_empty();
        let license_sec = LicenseSection::new(license_entries, is_free);

        // ── format tags ─────────────────────────────────────────────────────
        let formats: Vec<(String, String)> = d
            .asset_formats
            .iter()
            .map(|f| {
                (
                    f.asset_format_type.code.clone(),
                    f.asset_format_type.name.clone(),
                )
            })
            .collect();

        let tags: Vec<SharedString> = d
            .tags
            .iter()
            .map(|t| SharedString::from(t.name.clone()))
            .collect();

        let show_format_tags = !formats.is_empty() || !tags.is_empty();
        let format_tags = FormatTagsSection::new(formats, tags);

        // ── description ─────────────────────────────────────────────────────
        let raw_desc = d.description.as_deref().unwrap_or("");
        let desc_text = strip_html(raw_desc);
        let show_desc = !desc_text.is_empty();
        let description = DescriptionSection::new(SharedString::from(desc_text));

        // ── changelog ───────────────────────────────────────────────────────
        let changelog_entries: Vec<ChangelogEntry> = d
            .changelogs
            .iter()
            .rev()
            .map(|c| ChangelogEntry {
                date: SharedString::from(
                    c.published_at.get(..10).unwrap_or("").to_string(),
                ),
                content: SharedString::from(strip_html(&c.content)),
            })
            .collect();

        let show_changelog = !changelog_entries.is_empty();
        let changelog = ChangelogSection::new(changelog_entries);

        // ── assemble ────────────────────────────────────────────────────────
        let bg = cx.theme().background;

        div()
            .id("item-detail-root")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(bg)
            // sticky header (not inside scroll)
            .child(header)
            // scrollable body
            .child(
                div()
                    .id("item-detail-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(
                        v_flex()
                            .w_full()
                            .child(meta)
                            .when(show_gallery, |el| el.child(gallery))
                            .when(show_licenses, |el| el.child(license_sec))
                            .when(show_format_tags, |el| el.child(format_tags))
                            .when(show_desc, |el| el.child(description))
                            .when(show_changelog, |el| el.child(changelog)),
                    ),
            )
    }
}
