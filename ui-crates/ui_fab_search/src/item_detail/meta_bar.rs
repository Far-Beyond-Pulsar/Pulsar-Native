use gpui::{prelude::*, *};
use ui::{
    avatar::Avatar,
    divider::Divider,
    h_flex, v_flex,
    ActiveTheme, Icon, IconName, Sizable as _, Size,
};

/// Rating data shown in the meta bar.
pub struct RatingInfo {
    pub average: f64,
    pub total: i32,
    pub review_count: Option<i32>,
    /// Individual bucket counts (5-star down to 1-star).
    pub buckets: [i32; 5],
}

/// Metadata bar: seller avatar, name, category, star rating, and review count.
#[derive(IntoElement)]
pub struct MetaBar {
    pub seller_name: SharedString,
    pub seller_avatar_url: Option<SharedString>,
    pub category: Option<SharedString>,
    pub rating: Option<RatingInfo>,
    pub published_at: Option<SharedString>,
}

impl MetaBar {
    pub fn new(
        seller_name: impl Into<SharedString>,
        seller_avatar_url: Option<impl Into<SharedString>>,
        category: Option<impl Into<SharedString>>,
        rating: Option<RatingInfo>,
        published_at: Option<impl Into<SharedString>>,
    ) -> Self {
        Self {
            seller_name: seller_name.into(),
            seller_avatar_url: seller_avatar_url.map(|u| u.into()),
            category: category.map(|c| c.into()),
            rating,
            published_at: published_at.map(|d| d.into()),
        }
    }
}

/// Render one filled star, half-star, or empty star.
fn star_icon(filled: bool) -> impl IntoElement {
    Icon::new(if filled { IconName::Star } else { IconName::Star })
        .small()
        .map(move |i| {
            if filled {
                i.text_color(gpui::rgb(0xFACC15))   // amber-400
            } else {
                i.text_color(gpui::rgb(0x6B7280))   // gray-500
            }
        })
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
            // ── seller row ───────────────────────────────────────────────
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Avatar::new()
                            .with_size(Size::Medium)
                            .name(self.seller_name.clone())
                            .map(|av| {
                                if let Some(url) = &self.seller_avatar_url {
                                    av.src(url.clone())
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
            // ── ratings row ─────────────────────────────────────────────
            .when_some(self.rating, |el, r| {
                let avg = r.average.round() as usize;
                el.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        // five stars
                        .children((1usize..=5).map(|s| star_icon(s <= avg)))
                        // numeric average
                        .child(
                            div()
                                .text_sm()
                                .font_bold()
                                .text_color(fg)
                                .child(format!("{:.1}", r.average)),
                        )
                        // total rating count
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("({} ratings)", r.total)),
                        )
                        .when_some(r.review_count, |e, n| {
                            e.child(Divider::vertical().h(px(12.0)).color(muted))
                             .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("{} reviews", n)),
                             )
                        }),
                )
            })
            // ── published date ──────────────────────────────────────────
            .when_some(self.published_at, |el, date| {
                el.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("Published {}", &date[..date.len().min(10)])),
                )
            })
    }
}
