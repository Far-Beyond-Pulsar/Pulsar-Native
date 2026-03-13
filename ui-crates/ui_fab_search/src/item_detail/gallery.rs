use gpui::{prelude::*, *};
use ui::{v_flex, ActiveTheme};

/// A single gallery image entry.
pub struct GalleryImage {
    pub url: SharedString,
    pub width: u32,
    pub height: u32,
}

/// Horizontally-wrapping grid of preview images for an asset.
/// Renders each image using a native `gpui::img()` element.
/// Falls back to a numbered placeholder when no URL is available.
#[derive(IntoElement)]
pub struct GalleryStrip {
    pub images: Vec<GalleryImage>,
}

impl GalleryStrip {
    pub fn new(images: Vec<GalleryImage>) -> Self {
        Self { images }
    }
}

impl RenderOnce for GalleryStrip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let card_bg = cx.theme().sidebar;

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_3()
            .border_b_1()
            .border_color(border)
            // ── section label ────────────────────────────────────────────
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(muted)
                    .uppercase()
                    .tracking_wide()
                    .child(format!("Gallery ({} images)", self.images.len())),
            )
            // ── image grid ───────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .children(self.images.into_iter().enumerate().map(|(i, img_data)| {
                        div()
                            .id(SharedString::from(format!("gallery-img-{}", i)))
                            .w(px(180.0))
                            .h(px(101.0))   // 16:9
                            .flex_shrink_0()
                            .rounded_md()
                            .overflow_hidden()
                            .bg(card_bg)
                            .border_1()
                            .border_color(border)
                            .cursor_pointer()
                            // Use a real img element when a URL is present
                            .when(!img_data.url.is_empty(), |el| {
                                el.child(
                                    img(img_data.url)
                                        .w_full()
                                        .h_full()
                                        .object_fit(gpui::ObjectFit::Cover),
                                )
                            })
                            .when(img_data.url.is_empty(), |el| {
                                el.flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(format!("#{}", i + 1)),
                                    )
                            })
                    })),
            )
    }
}
