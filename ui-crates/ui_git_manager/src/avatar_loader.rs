//! GitHub avatar downloader.
//!
//! Derives a GitHub username from a commit email, then fetches
//! `https://github.com/{username}.png?size=64` and decodes it into a
//! `gpui::RenderImage` ready for `ImageSource::Render`.
//!
//! All network/decode work is done on a background OS thread.

use gpui::RenderImage;
use std::sync::Arc;

/// Derive a GitHub username from a git commit email, if possible.
///
/// Handles both noreply formats:
/// - `username@users.noreply.github.com`
/// - `12345+username@users.noreply.github.com`
pub fn github_username_from_email(email: &str) -> Option<String> {
    let lower = email.to_lowercase();
    if let Some(local) = lower.strip_suffix("@users.noreply.github.com") {
        // Strip leading numeric id prefix: "12345+username" → "username"
        let username = if let Some(pos) = local.find('+') {
            &local[pos + 1..]
        } else {
            local
        };
        if !username.is_empty() {
            return Some(username.to_string());
        }
    }
    None
}

/// Build the avatar URL for a GitHub username.
pub fn avatar_url(username: &str) -> String {
    format!("https://github.com/{}.png?size=64", username)
}

/// Download and decode a GitHub avatar PNG into a `RenderImage`.
/// Intended to be called from a spawned OS thread.
pub fn fetch_avatar(url: &str) -> Result<Arc<RenderImage>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("Pulsar-Native/1.0")
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let bytes = response.bytes().map_err(|e| e.to_string())?;
    let rgba = image::load_from_memory(&bytes)
        .map_err(|e| format!("decode: {e}"))?
        .into_rgba8();
    let frame = image::Frame::new(rgba);
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}
