//! Left column of the item detail view: media and prose.
//!
//! Owns the gallery strip, the author row and the description / tags / stats
//! stack that sit left of the info sidebar. Entry point: [`render_left_column`].

use gpui::{prelude::*, *};
use ui::{Sizable, Size, StyledExt, avatar::Avatar, h_flex, v_flex};

use super::changelog::ModelStatsSection;
use super::description::DescriptionSection;
use super::format_tags::FormatTagsSection;
use super::gallery::{GalleryHero, GalleryThumbnailRow};
use super::{AuthorInfo, DetailColors, short_date};
use crate::parser::fmt_count;

/// Left column: gallery, author row, then optional description and tags, then stats.
pub(super) fn render_left_column(
    hero: GalleryHero,
    thumb_row: GalleryThumbnailRow,
    author: &AuthorInfo,
    description: Option<DescriptionSection>,
    tag_section: Option<FormatTagsSection>,
    stats: ModelStatsSection,
    colors: &DetailColors,
) -> impl IntoElement + use<> {
    v_flex()
        .id("detail-left-col")
        .flex_1()
        .min_w_0()
        // ── hero + thumbnail strip ────────────────────────────────
        .child(render_gallery_strip(hero, thumb_row))
        // ── author row ────────────────────────────────────────────
        .child(render_author_row(author, colors))
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

/// Seller avatar, name and category on the left; view/like counts and publish date on the right.
fn render_author_row(author: &AuthorInfo, colors: &DetailColors) -> impl IntoElement + use<> {
    let DetailColors {
        border, fg, muted, ..
    } = *colors;

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
        .child(div().flex_1())
        .child(
            h_flex()
                .gap_4()
                .items_center()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("👁 {}", fmt_count(author.view_count))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("♥ {}", fmt_count(author.like_count))),
                )
                .when_some(author.published_at.as_deref(), |el, date| {
                    el.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("Published {}", short_date(date))),
                    )
                }),
        )
}
