//! Background image downloader for asset thumbnails and gallery images.
//!
//! Downloads image bytes via reqwest, decodes them into a BGRA8 `image::Frame`,
//! and wraps the result in a `gpui::RenderImage`.

use std::sync::Arc;
use gpui::RenderImage;

/// Download `url` and decode it into a `RenderImage` ready for
/// `ImageSource::Render`.  Always called from an OS thread.
pub fn fetch_and_decode(url: &str) -> Result<Arc<RenderImage>, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let url = url.to_string();
    let bytes: Vec<u8> = rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Pulsar-Native/1.0")
            .build()
            .map_err(|e: reqwest::Error| e.to_string())?;

        let response = client.get(&url).send().await
            .map_err(|e: reqwest::Error| e.to_string())?;

        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e: reqwest::Error| e.to_string())
    })?;

    // Decode to RGBA8
    let mut rgba = image::load_from_memory(&bytes)
        .map_err(|e| format!("image decode: {e}"))?
        .into_rgba8();

    // GPUI expects BGRA8, so swap R and B channels in-place.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let frame = image::Frame::new(rgba);
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}
