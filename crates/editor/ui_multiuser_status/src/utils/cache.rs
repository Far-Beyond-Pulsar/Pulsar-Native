//! Avatar image cache for multiuser participants

use gpui::RenderImage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// Lock policy: recover from poisoning. This is a plain URL -> image cache; a
/// panicking holder cannot leave it inconsistent, so keep serving it.
fn lock<K, V>(m: &Mutex<HashMap<K, V>>) -> MutexGuard<'_, HashMap<K, V>> {
    m.lock().unwrap_or_else(|poisoned| {
        tracing::error!("avatar cache mutex was poisoned by a panic; recovering");
        poisoned.into_inner()
    })
}

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
        lock(&self.cache).get(url).cloned()
    }

    /// Store a fetched avatar image
    pub fn insert(&self, url: String, image: Arc<RenderImage>) {
        lock(&self.cache).insert(url, image);
    }

    /// Check if URL is currently being fetched (to avoid duplicate requests)
    pub fn is_fetching(&self, url: &str) -> bool {
        lock(&self.cache).contains_key(url)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Arc<RenderImage> {
        let frame = image::Frame::new(image::RgbaImage::new(1, 1));
        Arc::new(RenderImage::new(smallvec::smallvec![frame]))
    }

    #[test]
    fn cache_returns_inserted_image_and_none_for_unknown_url() {
        let cache = AvatarCache::new();
        assert!(cache.get("https://a.test/1.png").is_none());
        let img = image();
        cache.insert("https://a.test/1.png".into(), Arc::clone(&img));
        assert!(Arc::ptr_eq(
            &cache.get("https://a.test/1.png").unwrap(),
            &img
        ));
        assert!(cache.get("https://a.test/2.png").is_none());
    }

    #[test]
    fn is_fetching_is_true_only_for_cached_urls() {
        let cache = AvatarCache::default();
        assert!(!cache.is_fetching("https://a.test/1.png"));
        cache.insert("https://a.test/1.png".into(), image());
        assert!(cache.is_fetching("https://a.test/1.png"));
    }

    #[test]
    fn clones_share_the_same_storage() {
        let cache = AvatarCache::new();
        let clone = cache.clone();
        clone.insert("https://a.test/1.png".into(), image());
        assert!(cache.get("https://a.test/1.png").is_some());
    }

    #[test]
    fn cache_survives_a_panic_while_the_lock_is_held() {
        let cache = AvatarCache::new();
        let poisoner = Arc::clone(&cache.cache);
        let result = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the cache");
        })
        .join();
        assert!(result.is_err());
        assert!(cache.cache.is_poisoned());

        cache.insert("https://a.test/1.png".into(), image());
        assert!(cache.get("https://a.test/1.png").is_some());
        assert!(cache.is_fetching("https://a.test/1.png"));
    }
}
