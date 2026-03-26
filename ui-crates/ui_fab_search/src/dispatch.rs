//! Download/filter type definitions and FabSearchWindow action methods.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::input::InputState;

use crate::search_index::{
    fetch_sketchfab_model_detail, fetch_sketchfab_models, fetch_sketchfab_me,
    fetch_sketchfab_download_info, sketchfab_like_model, sketchfab_unlike_model,
    SearchPage,
};
use crate::FabSearchWindow;

// ── Download state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum DownloadState {
    InProgress {
        filename: String,
        bytes_received: u64,
        total_bytes: Option<u64>,
        speed_history: Vec<f64>,
        speed_bps: f64,
    },
    Done {
        filename: String,
        path: PathBuf,
        total_bytes: u64,
    },
    Error {
        filename: String,
        message: String,
    },
}

impl DownloadState {
    pub(crate) fn filename(&self) -> &str {
        match self {
            DownloadState::InProgress { filename, .. } => filename,
            DownloadState::Done { filename, .. } => filename,
            DownloadState::Error { filename, .. } => filename,
        }
    }
}

pub(crate) enum DownloadMsg {
    Progress { bytes_received: u64, total: Option<u64>, speed_bps: f64 },
    Done { path: PathBuf, total: u64 },
    Error(String),
}

// ── Sort options ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SortBy {
    Relevance,
    MostViewed,
    MostLiked,
    Newest,
    Oldest,
}

impl SortBy {
    pub(crate) fn api_value(&self) -> Option<&'static str> {
        match self {
            SortBy::Relevance  => None,
            SortBy::MostViewed => Some("-viewCount"),
            SortBy::MostLiked  => Some("-likeCount"),
            SortBy::Newest     => Some("-publishedAt"),
            SortBy::Oldest     => Some("publishedAt"),
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            SortBy::Relevance  => "Relevance",
            SortBy::MostViewed => "Most Viewed",
            SortBy::MostLiked  => "Most Liked",
            SortBy::Newest     => "Newest",
            SortBy::Oldest     => "Oldest",
        }
    }
    pub(crate) fn all() -> [SortBy; 5] {
        [SortBy::Relevance, SortBy::MostViewed, SortBy::MostLiked, SortBy::Newest, SortBy::Oldest]
    }
}

// ── License filter ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LicenseFilter {
    All,
    CC0,
    CcBy,
    CcBySa,
    CcByNd,
    CcByNc,
    CcByNcSa,
    CcByNcNd,
    Standard,
    Editorial,
}

impl LicenseFilter {
    pub(crate) fn api_value(&self) -> Option<&'static str> {
        match self {
            LicenseFilter::All      => None,
            LicenseFilter::CC0      => Some("cc0"),
            LicenseFilter::CcBy     => Some("by"),
            LicenseFilter::CcBySa   => Some("by-sa"),
            LicenseFilter::CcByNd   => Some("by-nd"),
            LicenseFilter::CcByNc   => Some("by-nc"),
            LicenseFilter::CcByNcSa => Some("by-nc-sa"),
            LicenseFilter::CcByNcNd => Some("by-nc-nd"),
            LicenseFilter::Standard => Some("st"),
            LicenseFilter::Editorial=> Some("ed"),
        }
    }
    pub(crate) fn label(&self) -> &'static str {
        match self {
            LicenseFilter::All      => "All Licenses",
            LicenseFilter::CC0      => "CC0",
            LicenseFilter::CcBy     => "CC BY",
            LicenseFilter::CcBySa   => "CC BY-SA",
            LicenseFilter::CcByNd   => "CC BY-ND",
            LicenseFilter::CcByNc   => "CC BY-NC",
            LicenseFilter::CcByNcSa => "CC BY-NC-SA",
            LicenseFilter::CcByNcNd => "CC BY-NC-ND",
            LicenseFilter::Standard => "Standard",
            LicenseFilter::Editorial=> "Editorial",
        }
    }
    pub(crate) fn all() -> Vec<LicenseFilter> {
        use LicenseFilter::*;
        vec![All, CC0, CcBy, CcBySa, CcByNd, CcByNc, CcByNcSa, CcByNcNd, Standard, Editorial]
    }
}

// ── FabSearchWindow action methods ───────────────────────────────────────────

impl FabSearchWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_input = cx.new(|cx| {
            InputState::new(_window, cx).placeholder("Search Sketchfab models…")
        });
        let token_input = cx.new(|cx| {
            InputState::new(_window, cx).placeholder("Paste your Sketchfab API token…")
        });

        cx.subscribe(&search_input, |this, _input, event: &ui::input::InputEvent, cx| {
            if let ui::input::InputEvent::PressEnter { .. } = event {
                this.begin_search(cx);
            }
        }).detach();

        let saved_token = crate::auth::load_saved_token();

        let mut this = Self {
            focus_handle,
            search_query: String::new(),
            search_input,
            sort_by: SortBy::Relevance,
            filter_downloadable: false,
            filter_animated: false,
            filter_staffpicked: false,
            filter_license: LicenseFilter::All,
            show_license_menu: false,
            results: Vec::new(),
            next_url: None,
            is_loading: false,
            is_loading_more: false,
            error: None,
            last_url: None,
            selected_item_uid: None,
            item_detail: None,
            detail_loading: false,
            detail_error: None,
            image_cache: HashMap::new(),
            image_inflight: 0,
            image_queue: std::collections::VecDeque::new(),
            entity: None,
            api_token: saved_token.clone(),
            me: None,
            me_loading: false,
            show_token_input: false,
            token_input,
            download_state: HashMap::new(),
            show_download_manager: false,
            selected_gallery_idx: 0,
            liked_uids: HashSet::new(),
            like_inflight: HashSet::new(),
            results_scroll_handle: ui::VirtualListScrollHandle::new(),
            results_scroll_state: ui::scroll::ScrollbarState::default(),
            detail_scroll_handle: ScrollHandle::new(),
            detail_scroll_state: ui::scroll::ScrollbarState::default(),
            gallery_scroll_handle: ScrollHandle::new(),
            gallery_scroll_state: ui::scroll::ScrollbarState::default(),
        };
        let entity = cx.entity();
        this.entity = Some(entity);
        if saved_token.is_some() {
            this.fetch_me(cx);
        }
        this.scan_downloads_folder();
        this
    }

    pub(crate) fn go_back(&mut self, cx: &mut Context<Self>) {
        self.selected_item_uid = None;
        self.item_detail = None;
        self.detail_loading = false;
        self.detail_error = None;
        cx.notify();
    }

    // ── Auth methods ─────────────────────────────────────────────────────────

    pub(crate) fn set_token(&mut self, token: String, cx: &mut Context<Self>) {
        let token = token.trim().to_string();
        if token.is_empty() { return; }
        crate::auth::save_token(&token);
        self.api_token = Some(token);
        self.me = None;
        self.show_token_input = false;
        self.fetch_me(cx);
    }

    pub(crate) fn clear_token(&mut self, cx: &mut Context<Self>) {
        crate::auth::delete_token();
        self.api_token = None;
        self.me = None;
        self.me_loading = false;
        self.liked_uids.clear();
        self.like_inflight.clear();
        cx.notify();
    }

    pub(crate) fn fetch_me(&mut self, cx: &mut Context<Self>) {
        let token = match &self.api_token {
            Some(t) => t.clone(),
            None => return,
        };
        self.me_loading = true;
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<Result<Box<crate::parser::SketchfabMe>, String>>(1);
        std::thread::spawn(move || {
            smol::block_on(tx.send(fetch_sketchfab_me(&token))).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.me_loading = false;
                        match result {
                            Ok(me) => {
                                if let Some(url) = me.avatar_url(64).map(|s| s.to_string()) {
                                    view.ensure_image_loaded(url, cx);
                                }
                                view.me = Some(me);
                            }
                            Err(_) => {
                                view.api_token = None;
                                crate::auth::delete_token();
                            }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    // ── Download methods ─────────────────────────────────────────────────────

    pub(crate) fn scan_downloads_folder(&mut self) {
        let dir = dirs::download_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Sketchfab");
        let Ok(read_dir) = std::fs::read_dir(&dir) else { return; };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() { continue; }
            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let uid = path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&filename)
                .to_string();
            if self.download_state.contains_key(&uid) { continue; }
            let total_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            self.download_state.insert(uid, DownloadState::Done {
                filename,
                path,
                total_bytes,
            });
        }
    }

    pub(crate) fn start_download(&mut self, uid: String, cx: &mut Context<Self>) {
        if matches!(self.download_state.get(&uid), Some(DownloadState::InProgress { .. })) {
            return;
        }
        let token = match &self.api_token {
            Some(t) => t.clone(),
            None => return,
        };

        let filename = format!("{}.zip", uid);
        self.download_state.insert(
            uid.clone(),
            DownloadState::InProgress {
                filename: filename.clone(),
                bytes_received: 0,
                total_bytes: None,
                speed_history: Vec::new(),
                speed_bps: 0.0,
            },
        );
        cx.notify();

        let (tx, rx) = smol::channel::unbounded::<DownloadMsg>();
        let uid_thread = uid.clone();
        let filename_thread = filename.clone();

        std::thread::spawn(move || {
            let run = || -> Result<(), String> {
                let info = fetch_sketchfab_download_info(&uid_thread, &token)?;
                let fmt = info
                    .gltf
                    .or(info.glb)
                    .or(info.source)
                    .ok_or_else(|| "No downloadable format available".to_string())?;

                let dest_dir = dirs::download_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Sketchfab");
                std::fs::create_dir_all(&dest_dir).map_err(|e| format!("mkdir: {e}"))?;
                let dest = dest_dir.join(&filename_thread);

                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(600))
                    .user_agent("Pulsar-Native/1.0")
                    .build()
                    .map_err(|e| e.to_string())?;

                let resp = client.get(&fmt.url).send().map_err(|e| e.to_string())?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(format!("HTTP {} downloading file", status));
                }
                let total = resp.content_length();

                let mut file = std::fs::File::create(&dest).map_err(|e| format!("create: {e}"))?;
                let mut bytes_total: u64 = 0;
                let mut last_sample = std::time::Instant::now();
                let mut bytes_since_sample: u64 = 0;
                let mut buf = vec![0u8; 64 * 1024];
                let mut reader = std::io::BufReader::new(resp);
                use std::io::{Read, Write};

                loop {
                    let n = reader.read(&mut buf).map_err(|e| format!("read: {e}"))?;
                    if n == 0 { break; }
                    file.write_all(&buf[..n]).map_err(|e| format!("write: {e}"))?;
                    bytes_total += n as u64;
                    bytes_since_sample += n as u64;

                    let elapsed = last_sample.elapsed();
                    if elapsed >= std::time::Duration::from_millis(500) {
                        let speed = bytes_since_sample as f64 / elapsed.as_secs_f64();
                        smol::block_on(tx.send(DownloadMsg::Progress {
                            bytes_received: bytes_total,
                            total,
                            speed_bps: speed,
                        })).ok();
                        last_sample = std::time::Instant::now();
                        bytes_since_sample = 0;
                    }
                }

                file.flush().map_err(|e| format!("flush: {e}"))?;
                smol::block_on(tx.send(DownloadMsg::Done { path: dest, total: bytes_total })).ok();
                Ok(())
            };

            if let Err(e) = run() {
                smol::block_on(tx.send(DownloadMsg::Error(e))).ok();
            }
        });

        cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                match msg {
                    DownloadMsg::Progress { bytes_received, total, speed_bps } => {
                        cx.update(|cx| {
                            this.update(cx, |view, cx| {
                                if let Some(DownloadState::InProgress {
                                    bytes_received: br,
                                    total_bytes: tb,
                                    speed_bps: sb,
                                    speed_history: sh,
                                    ..
                                }) = view.download_state.get_mut(&uid)
                                {
                                    *br = bytes_received;
                                    *tb = total;
                                    *sb = speed_bps;
                                    sh.push(speed_bps);
                                    if sh.len() > 60 { sh.remove(0); }
                                }
                                cx.notify();
                            }).ok();
                        }).ok();
                    }
                    DownloadMsg::Done { path, total } => {
                        cx.update(|cx| {
                            this.update(cx, |view, cx| {
                                let filename = view
                                    .download_state
                                    .get(&uid)
                                    .map(|s| s.filename().to_string())
                                    .unwrap_or_default();
                                view.download_state.insert(
                                    uid.clone(),
                                    DownloadState::Done { filename, path, total_bytes: total },
                                );
                                cx.notify();
                            }).ok();
                        }).ok();
                        break;
                    }
                    DownloadMsg::Error(msg) => {
                        cx.update(|cx| {
                            this.update(cx, |view, cx| {
                                let filename = view
                                    .download_state
                                    .get(&uid)
                                    .map(|s| s.filename().to_string())
                                    .unwrap_or_default();
                                view.download_state.insert(
                                    uid.clone(),
                                    DownloadState::Error { filename, message: msg },
                                );
                                cx.notify();
                            }).ok();
                        }).ok();
                        break;
                    }
                }
            }
        }).detach();
    }

    // ── Like methods ─────────────────────────────────────────────────────────

    #[allow(dead_code)]
    pub(crate) fn is_liked(&self, uid: &str) -> bool { self.liked_uids.contains(uid) }

    pub(crate) fn toggle_like(&mut self, uid: String, cx: &mut Context<Self>) {
        let token = match &self.api_token {
            Some(t) => t.clone(),
            None => return,
        };
        if self.like_inflight.contains(&uid) { return; }
        let currently_liked = self.liked_uids.contains(&uid);
        if currently_liked { self.liked_uids.remove(&uid); } else { self.liked_uids.insert(uid.clone()); }
        self.like_inflight.insert(uid.clone());
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<Result<(), String>>(1);
        let uid_thread = uid.clone();
        std::thread::spawn(move || {
            let result = if currently_liked {
                sketchfab_unlike_model(&uid_thread, &token)
            } else {
                sketchfab_like_model(&uid_thread, &token)
            };
            smol::block_on(tx.send(result)).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok(result) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.like_inflight.remove(&uid);
                        if let Err(_) = result {
                            if currently_liked { view.liked_uids.insert(uid); } else { view.liked_uids.remove(&uid); }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    // ── Image loading ─────────────────────────────────────────────────────────

    pub(crate) fn ensure_image_loaded(&mut self, url: String, cx: &mut Context<Self>) {
        if url.is_empty() || self.image_cache.contains_key(&url) {
            return;
        }
        self.image_cache.insert(url.clone(), None);
        if self.image_inflight < 20 {
            self.start_image_fetch(url, cx);
        } else {
            self.image_queue.push_back(url);
        }
    }

    pub(crate) fn start_image_fetch(&mut self, url: String, cx: &mut Context<Self>) {
        self.image_inflight += 1;
        let (tx, rx) = smol::channel::bounded::<Option<Arc<gpui::RenderImage>>>(1);
        let url_thread = url.clone();
        std::thread::spawn(move || {
            let maybe = crate::image_loader::fetch_and_decode(&url_thread).ok();
            smol::block_on(tx.send(maybe)).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok(maybe) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.image_cache.insert(url, maybe);
                        view.image_inflight -= 1;
                        if let Some(next_url) = view.image_queue.pop_front() {
                            view.start_image_fetch(next_url, cx);
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    // ── Detail view ──────────────────────────────────────────────────────────

    pub(crate) fn open_item_detail(&mut self, uid: String, cx: &mut Context<Self>) {
        if self.selected_item_uid.as_deref() == Some(&uid) {
            return;
        }
        self.selected_item_uid = Some(uid.clone());
        self.item_detail = None;
        self.detail_loading = true;
        self.detail_error = None;
        self.selected_gallery_idx = 0;
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<Box<crate::parser::SketchfabModelDetail>, String>)>(1);
        std::thread::spawn(move || {
            smol::block_on(tx.send(fetch_sketchfab_model_detail(&uid))).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok((_, result)) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.detail_loading = false;
                        match result {
                            Ok(detail) => {
                                let urls: Vec<String> = detail.all_thumbnail_urls()
                                    .into_iter().take(12).map(|s| s.to_string()).collect();
                                for url in urls {
                                    view.ensure_image_loaded(url, cx);
                                }
                                if let Some(ref user) = detail.user {
                                    if let Some(url) = user.avatar_url(128) {
                                        view.ensure_image_loaded(url.to_string(), cx);
                                    }
                                }
                                view.item_detail = Some(detail);
                            }
                            Err(e) => {
                                view.detail_error = Some(e);
                            }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    // ── Search ───────────────────────────────────────────────────────────────

    pub(crate) fn build_search_url(&self) -> String {
        let q = urlencoding::encode(self.search_query.trim()).into_owned();
        let mut url = format!(
            "https://api.sketchfab.com/v3/search?type=models&q={}&count=24",
            q
        );
        if let Some(s) = self.sort_by.api_value()       { url.push_str(&format!("&sort_by={}", s)); }
        if self.filter_downloadable                      { url.push_str("&downloadable=true"); }
        if self.filter_animated                         { url.push_str("&animated=true"); }
        if self.filter_staffpicked                      { url.push_str("&staffpicked=true"); }
        if let Some(l) = self.filter_license.api_value() { url.push_str(&format!("&license={}", l)); }
        url
    }

    pub(crate) fn begin_search(&mut self, cx: &mut Context<Self>) {
        if self.search_query.trim().is_empty() { return; }

        let url = self.build_search_url();
        self.is_loading = true;
        self.is_loading_more = false;
        self.results.clear();
        self.next_url = None;
        self.error = None;
        self.last_url = Some(url.clone());
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<SearchPage, String>)>(1);
        std::thread::spawn(move || {
            smol::block_on(tx.send(fetch_sketchfab_models(&url))).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok((_, result)) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.is_loading = false;
                        match result {
                            Ok(page) => {
                                view.next_url = page.next;
                                view.results = page.models;
                                let thumb_urls: Vec<String> = view.results.iter()
                                    .filter_map(|m| m.thumb_url(260).map(|s| s.to_string()))
                                    .collect();
                                for url in thumb_urls {
                                    view.ensure_image_loaded(url, cx);
                                }
                            }
                            Err(e) => { view.error = Some(e); }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    pub(crate) fn load_more(&mut self, cx: &mut Context<Self>) {
        let url = match self.next_url.clone() {
            Some(u) => u,
            None => return,
        };
        self.is_loading_more = true;
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<SearchPage, String>)>(1);
        std::thread::spawn(move || {
            smol::block_on(tx.send(fetch_sketchfab_models(&url))).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok((_, result)) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.is_loading_more = false;
                        match result {
                            Ok(page) => {
                                view.next_url = page.next;
                                let thumb_urls: Vec<String> = page.models.iter()
                                    .filter_map(|m| m.thumb_url(260).map(|s| s.to_string()))
                                    .collect();
                                view.results.extend(page.models);
                                for url in thumb_urls {
                                    view.ensure_image_loaded(url, cx);
                                }
                            }
                            Err(e) => { view.error = Some(e); }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }
}
