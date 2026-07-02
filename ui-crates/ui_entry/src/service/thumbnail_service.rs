use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::RenderImage;

use crate::core::types::Template;

/// Pure functions for thumbnail loading paths and downloading
pub struct ThumbnailService;

impl ThumbnailService {
    pub fn project_thumbnail_path(project_path: &str) -> PathBuf {
        Path::new(project_path).join(".pulsar").join("thumbnail.png")
    }

    pub fn template_cache_dir() -> PathBuf {
        directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|d| d.cache_dir().join("template_thumbnails"))
            .unwrap_or_else(|| PathBuf::from("template_thumbnails"))
    }

    pub fn template_cache_path(template: &Template) -> PathBuf {
        Self::template_cache_dir().join(format!("{}.png", Self::sanitize_repo_name(&template.repo_url)))
    }

    fn sanitize_repo_name(repo_url: &str) -> String {
        let trimmed = repo_url.trim_end_matches(".git").trim_end_matches('/');
        let parts: Vec<&str> = trimmed.rsplit('/').take(2).collect();
        let joined: String = parts.into_iter().rev().collect::<Vec<_>>().join("_");
        if joined.is_empty() { "template".to_string() } else { joined }
    }

    fn template_thumbnail_urls(template: &Template) -> Vec<String> {
        let trimmed = template.repo_url.trim_end_matches(".git").trim_end_matches('/');
        if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
            let mut parts = rest.split('/');
            if let (Some(owner), Some(repo)) = (parts.next(), parts.next()) {
                return vec![
                    format!("https://raw.githubusercontent.com/{owner}/{repo}/main/.pulsar/thumbnail.png"),
                    format!("https://raw.githubusercontent.com/{owner}/{repo}/master/.pulsar/thumbnail.png"),
                ];
            }
        }
        vec![]
    }

    pub fn decode_png_file(path: &Path) -> Option<Arc<RenderImage>> {
        let bytes = std::fs::read(path).ok()?;
        Self::decode_png_bytes(&bytes)
    }

    pub fn decode_png_bytes(bytes: &[u8]) -> Option<Arc<RenderImage>> {
        let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
        let frame = image::Frame::new(rgba);
        Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
    }

    fn download_bytes(url: &str) -> Option<Vec<u8>> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Pulsar-Native/1.0")
            .build().ok()?;
        let response = client.get(url).send().ok()?;
        if !response.status().is_success() { return None; }
        response.bytes().ok().map(|b| b.to_vec())
    }

    fn fetch_and_cache_template_thumbnail(template: &Template, cache_path: &Path) -> Option<Arc<RenderImage>> {
        for url in Self::template_thumbnail_urls(template) {
            if let Some(bytes) = Self::download_bytes(&url) {
                if let Ok(decoded) = image::load_from_memory(&bytes) {
                    let rgba = decoded.into_rgba8();
                    if let Some(parent) = cache_path.parent() { let _ = std::fs::create_dir_all(parent); }
                    let _ = rgba.save(cache_path);
                    let frame = image::Frame::new(rgba);
                    return Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])));
                }
            }
        }
        None
    }

    pub fn load_project_thumbnail(project_path: &str) -> Option<Arc<RenderImage>> {
        let thumb_path = Self::project_thumbnail_path(project_path);
        if thumb_path.exists() { Self::decode_png_file(&thumb_path) } else { None }
    }

    pub fn load_template_thumbnail(template: &Template) -> Option<Arc<RenderImage>> {
        let cache_path = Self::template_cache_path(template);
        if cache_path.exists() {
            Self::decode_png_file(&cache_path).or_else(|| Self::fetch_and_cache_template_thumbnail(template, &cache_path))
        } else {
            Self::fetch_and_cache_template_thumbnail(template, &cache_path)
        }
    }
}
