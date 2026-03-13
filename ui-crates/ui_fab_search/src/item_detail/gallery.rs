use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{v_flex, ActiveTheme, ContextModal, StyledExt, scroll::{Scrollbar, ScrollbarState}};

/// A single gallery image entry.
pub struct GalleryImage {
    pub url: SharedString,
    pub width: u32,
    pub height: u32,
    /// Pre-decoded image ready for synchronous rendering; `None` while downloading.
    pub image: Option<Arc<gpui::RenderImage>>,
}

/// Horizontally-scrolling strip of preview images for an asset.
/// Click any thumbnail to open a fullscreen modal lightbox.
#[derive(IntoElement)]
pub struct GalleryStrip {
    pub images: Vec<GalleryImage>,
    pub scroll_handle: ScrollHandle,
    pub scroll_state: ScrollbarState,
}

impl GalleryStrip {
    pub fn new(images: Vec<GalleryImage>, scroll_handle: ScrollHandle, scroll_state: ScrollbarState) -> Self {
        Self { images, scroll_handle, scroll_state }
    }
}

impl RenderOnce for GalleryStrip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let card_bg = cx.theme().sidebar;
        let total = self.images.len();
        let scroll_handle = self.scroll_handle;
        let scroll_state = self.scroll_state;

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
                    .child(format!("Gallery ({} images)", total)),
            )
            // ── horizontal scroll strip with overlay scrollbar ────────────
            // Outer has a fixed height so size_full() on the inner gives it
            // a concrete pixel size — the same trick that makes vertical scroll work.
            .child(
                div()
                    .id("gallery-strip-outer")
                    .relative()
                    .w_full()
                    .h(px(160.0)) // 135px thumbs + 25px scrollbar room
                    .child(
                        div()
                            .id("gallery-strip")
                            .size_full()
                            .overflow_x_scroll()
                            .track_scroll(&scroll_handle)
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap_2()
                            .children(self.images.into_iter().enumerate().map(|(i, img_data)| {
                                let img_arc = img_data.image;

                                if let Some(arc) = img_arc {
                                    let arc_render = arc.clone();
                                    div()
                                        .id(SharedString::from(format!("gallery-thumb-{}", i)))
                                        .relative()
                                        .w(px(240.0))
                                        .h(px(135.0)) // 16:9
                                        .flex_shrink_0()
                                        .rounded_md()
                                        .overflow_hidden()
                                        .bg(card_bg)
                                        .border_1()
                                        .border_color(border)
                                        .child(
                                            img(gpui::ImageSource::Render(arc_render))
                                                .w_full()
                                                .h_full()
                                                .object_fit(gpui::ObjectFit::Cover),
                                        )
                                        // transparent overlay on top captures the click
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("gallery-click-{}", i)))
                                                .absolute()
                                                .inset_0()
                                                .cursor_pointer()
                                                .on_click(move |_, window, cx| {
                                                    let arc = arc.clone();
                                                    window.open_modal(cx, move |modal, _w, _cx| {
                                                        let arc = arc.clone();
                                                        modal
                                                            .width(px(960.0))
                                                            .show_close(true)
                                                            .child(
                                                                div()
                                                                    .w(px(920.0))
                                                                    .h(px(517.0))
                                                                    .child(
                                                                        img(gpui::ImageSource::Render(arc))
                                                                            .w_full()
                                                                            .h_full()
                                                                            .object_fit(gpui::ObjectFit::Contain),
                                                                    ),
                                                            )
                                                    });
                                                })
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .id(SharedString::from(format!("gallery-thumb-{}", i)))
                                        .w(px(240.0))
                                        .h(px(135.0))
                                        .flex_shrink_0()
                                        .rounded_md()
                                        .overflow_hidden()
                                        .bg(card_bg)
                                        .border_1()
                                        .border_color(border)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(format!("#{}", i + 1)),
                                        )
                                        .into_any_element()
                                }
                            })),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .child(Scrollbar::horizontal(&scroll_state, &scroll_handle)),
                    ),
            )
    }
}
