//! Thumbnail loading for the entry screen's project/template cards.
//!
//! Project thumbnails are read from `<project>/.pulsar/thumbnail.png` (written by
//! the level editor on save). Template thumbnails are fetched from the template's
//! GitHub repository and cached on disk under Pulsar's AppData cache directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Context, RenderImage};

use super::types::Template;
use super::EntryScreen;

/// Maximum number of concurrent thumbnail loads (per kind).
const MAX_INFLIGHT: usize = 4;

/// Path to a project's cached thumbnail, written by the level editor on save.
pub fn project_thumbnail_path(project_path: &str) -> PathBuf {
    Path::new(project_path).join(".pulsar").join("thumbnail.png")
}

/// Directory under Pulsar's AppData cache dir where template thumbnails are cached.
pub fn template_cache_dir() -> PathBuf {
    directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|d| d.cache_dir().join("template_thumbnails"))
        .unwrap_or_else(|| PathBuf::from("template_thumbnails"))
}

/// On-disk cache path for a given template's thumbnail.
pub fn template_cache_path(template: &Template) -> PathBuf {
    template_cache_dir().join(format!("{}.png", sanitize_repo_name(&template.repo_url)))
}

fn sanitize_repo_name(repo_url: &str) -> String {
    let trimmed = repo_url.trim_end_matches(".git").trim_end_matches('/');
    let parts: Vec<&str> = trimmed.rsplit('/').take(2).collect();
    let joined: String = parts.into_iter().rev().collect::<Vec<_>>().join("_");
    if joined.is_empty() {
        "template".to_string()
    } else {
        joined
    }
}

/// Candidate raw-GitHub URLs for a template's `.pulsar/thumbnail.png`, tried in order.
fn template_thumbnail_urls(template: &Template) -> Vec<String> {
    let trimmed = template.repo_url.trim_end_matches(".git").trim_end_matches('/');
    let Some(rest) = trimmed.strip_prefix("https://github.com/") else {
        return Vec::new();
    };
    let mut parts = rest.split('/');
    let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
        return Vec::new();
    };
    vec![
        format!("https://raw.githubusercontent.com/{owner}/{repo}/main/.pulsar/thumbnail.png"),
        format!("https://raw.githubusercontent.com/{owner}/{repo}/master/.pulsar/thumbnail.png"),
    ]
}

fn decode_png_file(path: &Path) -> Option<Arc<RenderImage>> {
    let bytes = std::fs::read(path).ok()?;
    decode_png_bytes(&bytes)
}

fn decode_png_bytes(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let frame = image::Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

/// Downloads `url` (blocking) and returns the raw bytes on success.
fn download_bytes(url: &str) -> Option<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;

    let url = url.to_string();
    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("Pulsar-Native/1.0")
            .build()
            .ok()?;

        let response = client.get(&url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response.bytes().await.ok().map(|b| b.to_vec())
    })
}

/// Fetches a template's thumbnail from GitHub and writes it to the on-disk cache.
fn fetch_and_cache_template_thumbnail(template: &Template, cache_path: &Path) -> Option<Arc<RenderImage>> {
    for url in template_thumbnail_urls(template) {
        if let Some(bytes) = download_bytes(&url) {
            if let Ok(decoded) = image::load_from_memory(&bytes) {
                let rgba = decoded.into_rgba8();
                if let Some(parent) = cache_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = rgba.save(cache_path);
                let frame = image::Frame::new(rgba);
                return Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])));
            }
        }
    }
    None
}

impl EntryScreen {
    /// Ensures a project's `.pulsar/thumbnail.png` is loaded (or queued to load).
    /// Safe to call every frame — it is a no-op once the load has been started.
    pub(crate) fn ensure_project_thumbnail_loaded(&mut self, project_path: &str, cx: &mut Context<Self>) {
        if project_path.is_empty() || self.project_thumbnails.contains_key(project_path) {
            return;
        }
        self.project_thumbnails.insert(project_path.to_string(), None);
        if self.project_thumbnail_inflight < MAX_INFLIGHT {
            self.start_project_thumbnail_fetch(project_path.to_string(), cx);
        } else {
            self.project_thumbnail_queue.push_back(project_path.to_string());
        }
    }

    fn start_project_thumbnail_fetch(&mut self, project_path: String, cx: &mut Context<Self>) {
        self.project_thumbnail_inflight += 1;
        let thumb_path = project_thumbnail_path(&project_path);
        let (tx, rx) = smol::channel::bounded::<Option<Arc<RenderImage>>>(1);
        std::thread::spawn(move || {
            let result = if thumb_path.exists() {
                decode_png_file(&thumb_path)
            } else {
                None
            };
            let _ = smol::block_on(tx.send(result));
        });

        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.project_thumbnails.insert(project_path, result);
                        view.project_thumbnail_inflight -= 1;
                        if let Some(next) = view.project_thumbnail_queue.pop_front() {
                            view.start_project_thumbnail_fetch(next, cx);
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    /// Forces a project's thumbnail to be reloaded (e.g. after the level editor wrote a new one).
    #[allow(dead_code)]
    pub(crate) fn invalidate_project_thumbnail(&mut self, project_path: &str, cx: &mut Context<Self>) {
        self.project_thumbnails.remove(project_path);
        self.ensure_project_thumbnail_loaded(project_path, cx);
    }

    /// Ensures a template's thumbnail is loaded from disk cache or fetched from GitHub.
    pub(crate) fn ensure_template_thumbnail_loaded(&mut self, template: &Template, cx: &mut Context<Self>) {
        let key = template.repo_url.clone();
        if key.is_empty() || self.template_thumbnails.contains_key(&key) {
            return;
        }
        self.template_thumbnails.insert(key.clone(), None);
        if self.template_thumbnail_inflight < MAX_INFLIGHT {
            self.start_template_thumbnail_fetch(template.clone(), cx);
        } else {
            self.template_thumbnail_queue.push_back(template.clone());
        }
    }

    fn start_template_thumbnail_fetch(&mut self, template: Template, cx: &mut Context<Self>) {
        self.template_thumbnail_inflight += 1;
        let key = template.repo_url.clone();
        let cache_path = template_cache_path(&template);
        let (tx, rx) = smol::channel::bounded::<Option<Arc<RenderImage>>>(1);
        std::thread::spawn(move || {
            let result = if cache_path.exists() {
                decode_png_file(&cache_path).or_else(|| fetch_and_cache_template_thumbnail(&template, &cache_path))
            } else {
                fetch_and_cache_template_thumbnail(&template, &cache_path)
            };
            let _ = smol::block_on(tx.send(result));
        });

        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.template_thumbnails.insert(key, result);
                        view.template_thumbnail_inflight -= 1;
                        if let Some(next) = view.template_thumbnail_queue.pop_front() {
                            view.start_template_thumbnail_fetch(next, cx);
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }
}
