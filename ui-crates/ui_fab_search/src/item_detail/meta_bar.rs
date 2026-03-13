use std::sync::Arc;
use gpui::{prelude::*, *};
use ui::{
    avatar::Avatar,
    h_flex, v_flex,
    ActiveTheme, Sizable, Size, StyledExt,
};

use crate::parser::fmt_count;

/// Metadata bar: seller avatar, name, category, views, likes, and publish date.
#[derive(IntoElement)]
pub struct MetaBar {
    pub seller_name: SharedString,
    /// Pre-decoded avatar image; `None` while downloading or if unavailable.
    pub seller_avatar: Option<Arc<gpui::RenderImage>>,
    pub category: Option<SharedString>,
    pub view_count: i64,
    pub like_count: i64,
    pub published_at: Option<SharedString>,
}

impl MetaBar {
    pub fn new(
        seller_name: impl Into<SharedString>,
        seller_avatar: Option<Arc<gpui::RenderImage>>,
        category: Option<impl Into<SharedString>>,
        view_count: i64,
        like_count: i64,
        published_at: Option<impl Into<SharedString>>,
    ) -> Self {
        Self {
            seller_name: seller_name.into(),
            seller_avatar,
            category: category.map(|c| c.into()),
            view_count,
            like_count,
            published_at: published_at.map(|d| d.into()),
        }
    }
}

impl RenderOnce for MetaBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_3()
            .border_b_1()
            .border_color(border)
            // ── author row ───────────────────────────────────────────────
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Avatar::new()
                            .with_size(Size::Medium)
                            .name(self.seller_name.clone())
                            .map(|av: Avatar| {
                                if let Some(arc) = self.seller_avatar {
                                    av.src(gpui::ImageSource::Render(arc))
                                } else {
                                    av
                                }
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_bold()
                                    .text_color(fg)
                                    .child(self.seller_name),
                            )
                            .when_some(self.category, |el, cat| {
                                el.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(cat),
                                )
                            }),
                    ),
            )
            // ── stats row ────────────────────────────────────────────────
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        div().text_sm().text_color(muted)
                            .child(format!("👁 {} views", fmt_count(self.view_count))),
                    )
                    .child(
                        div().text_sm().text_color(muted)
                            .child(format!("♥ {} likes", fmt_count(self.like_count))),
                    )
                    .when_some(self.published_at, |el, date| {
                        el.child(
                            div().text_sm().text_color(muted)
                                .child(format!("Published {}", &date[..date.len().min(10)])),
                        )
                    }),
            )
    }
}

