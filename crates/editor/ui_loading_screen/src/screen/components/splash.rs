use gpui::*;
use std::sync::Arc;

static SPLASH_PNG: &[u8] = include_bytes!("../../../../../../assets/images/Splash.png");

pub(crate) fn decode_png(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let frame = image::Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

pub(crate) fn splash_handle() -> Option<Arc<RenderImage>> {
    decode_png(SPLASH_PNG)
}

pub(crate) fn splash_background(splash: &Option<Arc<RenderImage>>) -> Option<Div> {
    splash.clone().map(|splash| {
        div().absolute().top_0().left_0().size_full().child(
            img(ImageSource::Render(splash))
                .size_full()
                .object_fit(ObjectFit::Cover),
        )
    })
}

pub(crate) fn vignette_overlay() -> Div {
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .h(px(260.0))
        .bg(gpui::black().opacity(0.82))
}

pub(crate) fn top_tint() -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(gpui::black().opacity(0.25))
}
