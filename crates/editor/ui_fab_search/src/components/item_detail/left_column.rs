//! Left column of the item detail view: media and prose.
//!
//! Owns the gallery strip and the description / tags / stats
//! stack that sit left of the info sidebar. Entry point: [`render_left_column`].

use gpui::prelude::*;
use ui::v_flex;

use super::changelog::ModelStatsSection;
use super::description::DescriptionSection;
use super::format_tags::FormatTagsSection;
use super::gallery::{GalleryHero, GalleryThumbnailRow};

/// Left column: gallery, optional description and tags, then technical stats.
pub(super) fn render_left_column(
    hero: GalleryHero,
    thumb_row: GalleryThumbnailRow,
    description: Option<DescriptionSection>,
    tag_section: Option<FormatTagsSection>,
    stats: ModelStatsSection,
) -> impl IntoElement + use<> {
    v_flex()
        .id("detail-left-col")
        .flex_1()
        .min_w_0()
        // ── hero + thumbnail strip ────────────────────────────────
        .child(render_gallery_strip(hero, thumb_row))
        .when_some(description, |el, description| el.child(description))
        .when_some(tag_section, |el, tag_section| el.child(tag_section))
        .child(stats)
}

/// Hero image with the thumbnail row underneath.
fn render_gallery_strip(
    hero: GalleryHero,
    thumb_row: GalleryThumbnailRow,
) -> impl IntoElement + use<> {
    v_flex()
        .w_full()
        .gap_3()
        .pb_2()
        .child(hero)
        .child(thumb_row)
}
