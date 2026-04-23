//! Cache and network fetch layer for Sketchfab search.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::parser::{SketchfabDownloadInfo, SketchfabMe, SketchfabModel, SketchfabModelDetail};

// ── Cache ────────────────────────────────────────────────────────────────────

pub(crate) const CACHE_CAP: usize = 30;
pub(crate) const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

pub(crate) struct SearchPage {
    pub models: Vec<SketchfabModel>,
    pub next: Option<String>,
}

#[allow(dead_code)]
pub(crate) enum CacheValue {
    Page {
        models: Vec<SketchfabModel>,
        next: Option<String>,
    },
    Detail(Box<SketchfabModelDetail>),
}

pub(crate) struct CacheEntry {
    pub key: String,
    pub value: CacheValue,
    pub inserted_at: Instant,
}

pub(crate) struct SearchCache {
    pub entries: VecDeque<CacheEntry>,
}

impl SearchCache {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(CACHE_CAP),
        }
    }

    pub fn evict(&mut self) {
        self.entries.retain(|e| e.inserted_at.elapsed() < CACHE_TTL);
    }

    pub fn get_detail(&mut self, key: &str) -> Option<Box<SketchfabModelDetail>> {
        self.evict();
        self.entries.iter().find(|e| e.key == key).and_then(|e| {
            if let CacheValue::Detail(ref d) = e.value {
                Some(d.clone())
            } else {
                None
            }
        })
    }

    pub fn insert(&mut self, key: String, value: CacheValue) {
        self.entries.retain(|e| e.key != key);
        if self.entries.len() >= CACHE_CAP {
            self.entries.pop_front();
        }
        self.entries.push_back(CacheEntry {
            key,
            value,
            inserted_at: Instant::now(),
        });
    }
}

pub(crate) fn global_cache() -> &'static Mutex<SearchCache> {
    static CACHE: OnceLock<Mutex<SearchCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SearchCache::new()))
}

// ── HTTP helpers ─────────────────────────────────────────────────────────────

pub(crate) fn make_auth_client(token: &str) -> Result<reqwest::blocking::Client, String> {
    let mut headers = reqwest::header::HeaderMap::new();
    let auth_value = reqwest::header::HeaderValue::from_str(&format!("Token {}", token))
        .map_err(|e| format!("invalid token: {e}"))?;
    headers.insert(reqwest::header::AUTHORIZATION, auth_value);
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Pulsar-Native/1.0")
        .default_headers(headers)
        .build()
        .map_err(|e| e.to_string())
}

// ── Fetch functions ──────────────────────────────────────────────────────────

pub(crate) fn fetch_sketchfab_models(url: &str) -> (Vec<String>, Result<SearchPage, String>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return (vec![], Err(format!("tokio: {}", e))),
    };
    let url = url.to_string();
    let (log, result) = rt.block_on(async move {
        let mut log: Vec<String> = Vec::new();
        macro_rules! logv { ($($t:tt)*) => {{ let s = format!($($t)*); println!("{}", s); log.push(s); }} }

        logv!("→ GET {}", url);
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Pulsar-Native/1.0")
            .build()
        {
            Ok(c) => c,
            Err(e) => { logv!("build: {}", e); return (log, Err(e.to_string())); }
        };

        let resp = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => { logv!("request: {}", e); return (log, Err(e.to_string())); }
        };
        let status = resp.status();
        logv!("← HTTP {}", status);
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return (log, Err(e.to_string())),
        };
        logv!("body {} bytes", text.len());
        if !status.is_success() {
            return (log, Err(format!("HTTP {} — {}", status, &text[..text.len().min(120)])));
        }
        let result = serde_json::from_str::<crate::parser::SketchfabSearchResponse>(&text)
            .map_err(|e| { logv!("parse: {}", e); format!("Parse error: {e}") })
            .map(|parsed| {
                logv!("parsed {} models", parsed.results.len());
                SearchPage { next: parsed.next, models: parsed.results }
            });
        (log, result)
    });
    (log, result)
}

pub(crate) fn fetch_sketchfab_model_detail(
    uid: &str,
) -> (Vec<String>, Result<Box<SketchfabModelDetail>, String>) {
    let url = format!("https://api.sketchfab.com/v3/models/{}", uid);

    if let Ok(mut cache) = global_cache().lock() {
        if let Some(cached) = cache.get_detail(&url) {
            return (vec!["cache hit".into()], Ok(cached));
        }
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return (vec![], Err(format!("tokio: {}", e))),
    };
    let url2 = url.clone();
    let (log, result) = rt.block_on(async move {
        let mut log: Vec<String> = Vec::new();
        macro_rules! logv { ($($t:tt)*) => {{ let s = format!($($t)*); println!("{}", s); log.push(s); }} }

        logv!("→ GET {}", url2);
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Pulsar-Native/1.0")
            .build()
        {
            Ok(c) => c,
            Err(e) => return (log, Err(e.to_string())),
        };
        let resp = match client.get(&url2).send().await {
            Ok(r) => r,
            Err(e) => return (log, Err(e.to_string())),
        };
        let status = resp.status();
        logv!("← HTTP {}", status);
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => return (log, Err(e.to_string())),
        };
        if !status.is_success() { return (log, Err(format!("HTTP {}", status))); }
        let result = serde_json::from_str::<SketchfabModelDetail>(&text)
            .map_err(|e| { logv!("parse: {}", e); format!("Parse error: {e}") })
            .map(|parsed| { logv!("parsed: {}", parsed.name); Box::new(parsed) });
        (log, result)
    });
    if let Ok(ref d) = result {
        if let Ok(mut cache) = global_cache().lock() {
            cache.insert(url, CacheValue::Detail(d.clone()));
        }
    }
    (log, result)
}

pub(crate) fn fetch_sketchfab_me(token: &str) -> Result<Box<SketchfabMe>, String> {
    let client = make_auth_client(token)?;
    let resp = client
        .get("https://api.sketchfab.com/v3/me")
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {} — {}",
            status,
            &text[..text.len().min(120)]
        ));
    }
    serde_json::from_str::<SketchfabMe>(&text)
        .map(Box::new)
        .map_err(|e| format!("parse /me: {e}"))
}

pub(crate) fn fetch_sketchfab_download_info(
    uid: &str,
    token: &str,
) -> Result<SketchfabDownloadInfo, String> {
    let client = make_auth_client(token)?;
    let url = format!("https://api.sketchfab.com/v3/models/{}/download", uid);
    let resp = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "HTTP {} — {}",
            status,
            &text[..text.len().min(200)]
        ));
    }
    serde_json::from_str::<SketchfabDownloadInfo>(&text)
        .map_err(|e| format!("parse download info: {e}"))
}

pub(crate) fn sketchfab_like_model(uid: &str, token: &str) -> Result<(), String> {
    let client = make_auth_client(token)?;
    let params = [("model", uid)];
    let resp = client
        .post("https://api.sketchfab.com/v3/me/likes")
        .form(&params)
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() && status.as_u16() != 204 {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {} — {}",
            status,
            &text[..text.len().min(120)]
        ));
    }
    Ok(())
}

pub(crate) fn sketchfab_unlike_model(uid: &str, token: &str) -> Result<(), String> {
    let client = make_auth_client(token)?;
    let url = format!("https://api.sketchfab.com/v3/me/likes/{}", uid);
    let resp = client.delete(&url).send().map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() && status.as_u16() != 204 {
        let text = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {} — {}",
            status,
            &text[..text.len().min(120)]
        ));
    }
    Ok(())
}
