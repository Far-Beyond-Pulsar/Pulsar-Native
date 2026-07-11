use gpui::*;
use std::sync::Arc;

pub fn fetch_avatar_image(url: &str) -> Result<Arc<RenderImage>, anyhow::Error> {
    let resp = reqwest::blocking::get(url)?;
    let bytes = resp.bytes()?;
    let img = image::load_from_memory(&bytes)?.into_rgba8();
    let frame = image::Frame::new(img);
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}
