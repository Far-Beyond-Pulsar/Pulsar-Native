//! Background image downloader for Fab asset thumbnails and gallery images.
//!
//! Downloads image bytes via rquest + Chrome TLS impersonation, decodes them
//! into a BGRA8 `image::Frame`, and wraps the result in a `gpui::RenderImage`.
//!
//! Using `ImageSource::Render(Arc<RenderImage>)` is fully synchronous on the
//! GPUI render path — it bypasses the async asset loader entirely and returns
//! the decoded pixel data on every render call, guaranteeing the image is
//! always visible once stored in the cache.

use std::sync::Arc;
use gpui::RenderImage;

/// Download `url` and decode it into a `RenderImage` ready for
/// `ImageSource::Render`.  Always called from an OS thread.
pub fn fetch_and_decode(url: &str) -> Result<Arc<RenderImage>, String> {
    use rquest::Impersonate;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    let bytes = rt.block_on(async {
        let client = rquest::Client::builder()
            .impersonate(Impersonate::Chrome131)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;

        let response = client.get(url).send().await.map_err(|e| e.to_string())?;

        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| e.to_string())
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
