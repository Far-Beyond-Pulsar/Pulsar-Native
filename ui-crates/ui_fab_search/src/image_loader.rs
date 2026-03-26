//! Background image downloader for asset thumbnails and gallery images.
//!
//! Downloads image bytes via reqwest, decodes them into an RGBA8 `image::Frame`,
//! and wraps the result in a `gpui::RenderImage`.

use gpui::RenderImage;
use image::RgbaImage;
use std::sync::Arc;

fn decode_image_bytes(bytes: &[u8]) -> Result<RgbaImage, String> {
    image::load_from_memory(bytes)
        .map_err(|e| format!("image decode: {e}"))
        .map(|image| image.into_rgba8())
}

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

    let rgba = decode_image_bytes(&bytes)?;
    let frame = image::Frame::new(rgba);
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

#[cfg(test)]
mod tests {
    use super::decode_image_bytes;

    #[test]
    fn decode_image_bytes_preserves_rgba_channel_order() {
        let source = image::RgbaImage::from_raw(1, 1, vec![0x12, 0x34, 0x56, 0x78]).unwrap();
        let dynamic = image::DynamicImage::ImageRgba8(source);

        let mut bytes = Vec::new();
        dynamic
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();

        let decoded = decode_image_bytes(&bytes).unwrap();
        assert_eq!(decoded.as_raw(), &[0x12, 0x34, 0x56, 0x78]);
    }
}
