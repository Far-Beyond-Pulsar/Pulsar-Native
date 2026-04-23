use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, ContextModal,
    scroll::{Scrollbar, ScrollbarState},
};

/// A single gallery image entry.
pub struct GalleryImage {
    pub url: SharedString,
    pub width: u32,
    pub height: u32,
    /// Pre-decoded image ready for synchronous rendering; `None` while downloading.
    pub image: Option<Arc<gpui::RenderImage>>,
}

// ── Hero ─────────────────────────────────────────────────────────────────────

/// Large hero display — the currently-selected gallery image.
/// Clicking the image opens a fullscreen lightbox modal.
#[derive(IntoElement)]
pub struct GalleryHero {
    pub images: Vec<GalleryImage>,
    pub selected_idx: usize,
}

impl GalleryHero {
    pub fn new(images: Vec<GalleryImage>, selected_idx: usize) -> Self {
        Self {
            images,
            selected_idx,
        }
    }
}

impl RenderOnce for GalleryHero {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let bg = cx.theme().sidebar;
        let muted = cx.theme().muted_foreground;
        let idx = self.selected_idx.min(self.images.len().saturating_sub(1));
        let image_arc = self.images.get(idx).and_then(|i| i.image.clone());

        div()
            .id("gallery-hero")
            .w_full()
            .h(px(460.0))
            .rounded_xl()
            .overflow_hidden()
            .bg(bg)
            .border_1()
            .border_color(border)
            .relative()
            .map(|el| {
                if let Some(arc) = image_arc {
                    let arc_modal = arc.clone();
                    el.child(
                        img(gpui::ImageSource::Render(arc))
                            .w_full()
                            .h_full()
                            .object_fit(gpui::ObjectFit::Cover),
                    )
                    .child(
                        div()
                            .id("hero-click-overlay")
                            .absolute()
                            .inset_0()
                            .cursor_pointer()
                            .on_click(move |_, window, cx| {
                                let arc = arc_modal.clone();
                                window.open_modal(cx, move |modal, _w, _cx| {
                                    let arc = arc.clone();
                                    modal.width(px(1080.0)).show_close(true).child(
                                        div().w(px(1040.0)).h(px(585.0)).child(
                                            img(gpui::ImageSource::Render(arc))
                                                .w_full()
                                                .h_full()
                                                .object_fit(gpui::ObjectFit::Contain),
                                        ),
                                    )
                                });
                            }),
                    )
                } else {
                    el.flex()
                        .items_center()
                        .justify_center()
                        .child(div().text_sm().text_color(muted).child("Loading image..."))
                }
            })
    }
}

// ── Thumbnail strip ───────────────────────────────────────────────────────────

#[derive(IntoElement)]
pub struct GalleryThumbnailRow {
    pub images: Vec<GalleryImage>,
    pub selected_idx: usize,
    pub scroll_handle: ScrollHandle,
    pub scroll_state: ScrollbarState,
    pub on_select: Vec<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl GalleryThumbnailRow {
    pub fn new(
        images: Vec<GalleryImage>,
        selected_idx: usize,
        scroll_handle: ScrollHandle,
        scroll_state: ScrollbarState,
        on_select: Vec<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    ) -> Self {
        Self {
            images,
            selected_idx,
            scroll_handle,
            scroll_state,
            on_select,
        }
    }
}

impl RenderOnce for GalleryThumbnailRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let accent = cx.theme().accent;
        let card_bg = cx.theme().sidebar;
        let muted = cx.theme().muted_foreground;
        let selected = self.selected_idx;

        div()
            .id("gallery-thumb-row-outer")
            .relative()
            .w_full()
            .h(px(96.0))
            .child(
                div()
                    .id("gallery-thumb-row")
                    .size_full()
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll_handle)
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .children(
                        self.images
                            .into_iter()
                            .zip(self.on_select)
                            .enumerate()
                            .map(|(i, (img_data, on_click))| {
                                let is_sel = i == selected;
                                div()
                                    .id(SharedString::from(format!("thumb-chip-{}", i)))
                                    .w(px(128.0))
                                    .h(px(72.0))
                                    .flex_shrink_0()
                                    .rounded_md()
                                    .overflow_hidden()
                                    .bg(card_bg)
                                    .when(is_sel, |el| el.border_2())
                                    .when(!is_sel, |el| el.border_1())
                                    .border_color(if is_sel { accent } else { border })
                                    .cursor_pointer()
                                    .map(|el| {
                                        if let Some(arc) = img_data.image {
                                            el.child(
                                                img(gpui::ImageSource::Render(arc))
                                                    .w_full()
                                                    .h_full()
                                                    .object_fit(gpui::ObjectFit::Cover),
                                            )
                                        } else {
                                            el.flex().items_center().justify_center().child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(format!("{}", i + 1)),
                                            )
                                        }
                                    })
                                    .on_click(on_click)
                                    .into_any_element()
                            }),
                    ),
            )
            .child(div().absolute().inset_0().child(Scrollbar::horizontal(
                &self.scroll_state,
                &self.scroll_handle,
            )))
    }
}
