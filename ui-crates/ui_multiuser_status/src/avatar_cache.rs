//! Avatar image cache for multiuser participants

use gpui::RenderImage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Global avatar cache - stores fetched and decoded profile pictures
pub struct AvatarCache {
    cache: Arc<Mutex<HashMap<String, Arc<RenderImage>>>>,
}

impl AvatarCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get a cached avatar image by URL
    pub fn get(&self, url: &str) -> Option<Arc<RenderImage>> {
        self.cache.lock().unwrap().get(url).cloned()
    }

    /// Store a fetched avatar image
    pub fn insert(&self, url: String, image: Arc<RenderImage>) {
        self.cache.lock().unwrap().insert(url, image);
    }

    /// Check if URL is currently being fetched (to avoid duplicate requests)
    pub fn is_fetching(&self, url: &str) -> bool {
        self.cache.lock().unwrap().contains_key(url)
    }
}

impl Default for AvatarCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for AvatarCache {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
        }
    }
}

/// Fetch avatar from URL and decode into RenderImage
pub fn fetch_avatar_image(url: &str) -> Result<Arc<RenderImage>, String> {
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
