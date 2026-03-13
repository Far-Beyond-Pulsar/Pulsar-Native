pub mod image_loader;
pub mod item_detail;
pub mod parser;
pub mod auth;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme,
    Root,
    TitleBar,
    v_flex,
    h_flex,
    button::Button,
    input::{InputState, TextInput},
    scroll::{Scrollbar, ScrollbarState},
    skeleton::Skeleton,
    spinner::Spinner,
    Sizable,
    v_virtual_list, VirtualListScrollHandle,
    IconName,
    StyledExt,
    download_item::DownloadItemStatus,
    download_manager::{DownloadEntry, DownloadManagerDrawer},
};
use parser::{fmt_count, SketchfabModel, SketchfabModelDetail, SketchfabDownloadInfo, SketchfabMe};

// ── Download state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) enum DownloadState {
    InProgress {
        filename: String,
        bytes_received: u64,
        total_bytes: Option<u64>,
        /// Transfer rate samples in bytes/s (oldest → newest, capped at 60).
        speed_history: Vec<f64>,
        /// Most recent sample (bytes/s) — kept separately for quick access.
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

/// Progress messages streamed from the download thread back to the UI.
enum DownloadMsg {
    Progress { bytes_received: u64, total: Option<u64>, speed_bps: f64 },
    Done { path: PathBuf, total: u64 },
    Error(String),
}

// ── Sort options ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum SortBy {
    Relevance,
    MostViewed,
    MostLiked,
    Newest,
    Oldest,
}

impl SortBy {
    fn api_value(&self) -> Option<&'static str> {
        match self {
            SortBy::Relevance  => None,
            SortBy::MostViewed => Some("-viewCount"),
            SortBy::MostLiked  => Some("-likeCount"),
            SortBy::Newest     => Some("-publishedAt"),
            SortBy::Oldest     => Some("publishedAt"),
        }
    }
    fn label(&self) -> &'static str {
        match self {
            SortBy::Relevance  => "Relevance",
            SortBy::MostViewed => "Most Viewed",
            SortBy::MostLiked  => "Most Liked",
            SortBy::Newest     => "Newest",
            SortBy::Oldest     => "Oldest",
        }
    }
    fn all() -> [SortBy; 5] {
        [SortBy::Relevance, SortBy::MostViewed, SortBy::MostLiked, SortBy::Newest, SortBy::Oldest]
    }
}

// ── License filter ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum LicenseFilter {
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
    fn api_value(&self) -> Option<&'static str> {
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
    fn label(&self) -> &'static str {
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
    fn all() -> Vec<LicenseFilter> {
        use LicenseFilter::*;
        vec![All, CC0, CcBy, CcBySa, CcByNd, CcByNc, CcByNcSa, CcByNcNd, Standard, Editorial]
    }
}

// ── Main window struct ───────────────────────────────────────────────────────

pub struct FabSearchWindow {
    focus_handle: FocusHandle,
    search_query: String,
    search_input: Entity<InputState>,

    // filters
    sort_by: SortBy,
    filter_downloadable: bool,
    filter_animated: bool,
    filter_staffpicked: bool,
    filter_license: LicenseFilter,
    show_license_menu: bool,

    // results
    results: Vec<SketchfabModel>,
    next_url: Option<String>,

    is_loading: bool,
    is_loading_more: bool,
    error: Option<String>,
    last_url: Option<String>,

    // item detail
    selected_item_uid: Option<String>,
    item_detail: Option<Box<SketchfabModelDetail>>,
    detail_loading: bool,
    detail_error: Option<String>,

    // image cache: None = in-flight, Some(arc) = ready
    image_cache: HashMap<String, Option<std::sync::Arc<gpui::RenderImage>>>,
    // concurrency cap for image downloads
    image_inflight: usize,
    image_queue: std::collections::VecDeque<String>,

    // entity ref (needed for v_virtual_list)
    entity: Option<Entity<Self>>,

    // ── Auth ─────────────────────────────────────────────────────────────
    api_token: Option<String>,
    me: Option<Box<SketchfabMe>>,
    me_loading: bool,
    show_token_input: bool,
    token_input: Entity<InputState>,

    // ── Downloads ────────────────────────────────────────────────────────
    download_state: HashMap<String, DownloadState>,
    show_download_manager: bool,

    // ── Likes ────────────────────────────────────────────────────────────
    liked_uids: HashSet<String>,
    like_inflight: HashSet<String>,

    // scrollbars

    results_scroll_handle: VirtualListScrollHandle,
    results_scroll_state: ScrollbarState,
    detail_scroll_handle: ScrollHandle,
    detail_scroll_state: ScrollbarState,
    gallery_scroll_handle: ScrollHandle,
    gallery_scroll_state: ScrollbarState,
}

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

        let saved_token = auth::load_saved_token();

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
            liked_uids: HashSet::new(),
            like_inflight: HashSet::new(),

            results_scroll_handle: VirtualListScrollHandle::new(),
            results_scroll_state: ScrollbarState::default(),
            detail_scroll_handle: ScrollHandle::new(),
            detail_scroll_state: ScrollbarState::default(),
            gallery_scroll_handle: ScrollHandle::new(),
            gallery_scroll_state: ScrollbarState::default(),
        };
        // Store self-entity reference so v_virtual_list can capture it
        let entity = cx.entity();
        this.entity = Some(entity);
        // If we have a saved token, verify it immediately
        if saved_token.is_some() {
            this.fetch_me(cx);
        }
        this
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.selected_item_uid = None;
        self.item_detail = None;
        self.detail_loading = false;
        self.detail_error = None;
        cx.notify();
    }

    // ── Auth methods ─────────────────────────────────────────────────────────

    fn set_token(&mut self, token: String, cx: &mut Context<Self>) {
        let token = token.trim().to_string();
        if token.is_empty() { return; }
        auth::save_token(&token);
        self.api_token = Some(token);
        self.me = None;
        self.show_token_input = false;
        self.fetch_me(cx);
    }

    fn clear_token(&mut self, cx: &mut Context<Self>) {
        auth::delete_token();
        self.api_token = None;
        self.me = None;
        self.me_loading = false;
        self.liked_uids.clear();
        self.like_inflight.clear();
        cx.notify();
    }

    fn fetch_me(&mut self, cx: &mut Context<Self>) {
        let token = match &self.api_token {
            Some(t) => t.clone(),
            None => return,
        };
        self.me_loading = true;
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<Result<Box<SketchfabMe>, String>>(1);
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
                                // Pre-load avatar image
                                if let Some(url) = me.avatar_url(64).map(|s| s.to_string()) {
                                    view.ensure_image_loaded(url, cx);
                                }
                                view.me = Some(me);
                            }
                            Err(_) => {
                                // Token is invalid; clear it
                                view.api_token = None;
                                auth::delete_token();
                            }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    // ── Download methods ─────────────────────────────────────────────────────

    fn start_download(&mut self, uid: String, cx: &mut Context<Self>) {
        // Don't restart an in-progress download.
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
                // 1. Resolve download URL
                let info = fetch_sketchfab_download_info(&uid_thread, &token)?;
                let fmt = info
                    .gltf
                    .or(info.glb)
                    .or(info.source)
                    .ok_or_else(|| "No downloadable format available".to_string())?;

                // 2. Prepare destination
                let dest_dir = dirs::download_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("Sketchfab");
                std::fs::create_dir_all(&dest_dir).map_err(|e| format!("mkdir: {e}"))?;
                let dest = dest_dir.join(&filename_thread);

                // 3. Stream download, reporting progress every 500ms
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
                let mut buf = vec![0u8; 64 * 1024]; // 64 KiB chunks
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
                        }))
                        .ok();
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
                            })
                            .ok();
                        })
                        .ok();
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
                            })
                            .ok();
                        })
                        .ok();
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
                            })
                            .ok();
                        })
                        .ok();
                        break;
                    }
                }
            }
        })
        .detach();
    }

    // ── Like methods ─────────────────────────────────────────────────────────

    #[allow(dead_code)]
    fn is_liked(&self, uid: &str) -> bool { self.liked_uids.contains(uid) }

    fn toggle_like(&mut self, uid: String, cx: &mut Context<Self>) {
        let token = match &self.api_token {
            Some(t) => t.clone(),
            None => return,
        };
        if self.like_inflight.contains(&uid) { return; }
        let currently_liked = self.liked_uids.contains(&uid);
        // Optimistic update
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
                            // Revert optimistic update on error
                            if currently_liked { view.liked_uids.insert(uid); } else { view.liked_uids.remove(&uid); }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    fn ensure_image_loaded(&mut self, url: String, cx: &mut Context<Self>) {
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

    fn start_image_fetch(&mut self, url: String, cx: &mut Context<Self>) {
        self.image_inflight += 1;
        let (tx, rx) = smol::channel::bounded::<Option<std::sync::Arc<gpui::RenderImage>>>(1);
        let url_thread = url.clone();
        std::thread::spawn(move || {
            let maybe = image_loader::fetch_and_decode(&url_thread).ok();
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

    fn open_item_detail(&mut self, uid: String, cx: &mut Context<Self>) {
        if self.selected_item_uid.as_deref() == Some(&uid) {
            return;
        }
        self.selected_item_uid = Some(uid.clone());
        self.item_detail = None;
        self.detail_loading = true;
        self.detail_error = None;
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<Box<SketchfabModelDetail>, String>)>(1);
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
                                // preload all thumbnail sizes as gallery images
                                let urls: Vec<String> = detail.all_thumbnail_urls()
                                    .into_iter().take(8).map(|s| s.to_string()).collect();
                                for url in urls {
                                    view.ensure_image_loaded(url, cx);
                                }
                                // avatar
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

    fn build_search_url(&self) -> String {
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

    fn begin_search(&mut self, cx: &mut Context<Self>) {
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
                            Err(e) => {
                                view.error = Some(e);
                            }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
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
                            Err(e) => {
                                view.error = Some(e);
                            }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }
}

// ── Cache ────────────────────────────────────────────────────────────────────

const CACHE_CAP: usize = 30;
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

struct SearchPage {
    models: Vec<SketchfabModel>,
    next: Option<String>,
}

#[allow(dead_code)]
enum CacheValue {
    Page { models: Vec<SketchfabModel>, next: Option<String> },
    Detail(Box<SketchfabModelDetail>),
}

struct CacheEntry {
    key: String,
    value: CacheValue,
    inserted_at: Instant,
}

struct SearchCache { entries: VecDeque<CacheEntry> }

impl SearchCache {
    fn new() -> Self { Self { entries: VecDeque::with_capacity(CACHE_CAP) } }

    fn evict(&mut self) {
        self.entries.retain(|e| e.inserted_at.elapsed() < CACHE_TTL);
    }

    fn get_detail(&mut self, key: &str) -> Option<Box<SketchfabModelDetail>> {
        self.evict();
        self.entries.iter().find(|e| e.key == key).and_then(|e| {
            if let CacheValue::Detail(ref d) = e.value { Some(d.clone()) } else { None }
        })
    }

    fn insert(&mut self, key: String, value: CacheValue) {
        self.entries.retain(|e| e.key != key);
        if self.entries.len() >= CACHE_CAP { self.entries.pop_front(); }
        self.entries.push_back(CacheEntry { key, value, inserted_at: Instant::now() });
    }
}

fn global_cache() -> &'static Mutex<SearchCache> {
    static CACHE: OnceLock<Mutex<SearchCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(SearchCache::new()))
}

// ── Fetch ────────────────────────────────────────────────────────────────────

fn fetch_sketchfab_models(url: &str) -> (Vec<String>, Result<SearchPage, String>) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r, Err(e) => return (vec![], Err(format!("tokio: {}", e))),
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
        let result = serde_json::from_str::<parser::SketchfabSearchResponse>(&text)
            .map_err(|e| { logv!("parse: {}", e); format!("Parse error: {e}") })
            .map(|parsed| {
                logv!("parsed {} models", parsed.results.len());
                SearchPage { next: parsed.next, models: parsed.results }
            });
        (log, result)
    });
    (log, result)
}

fn fetch_sketchfab_model_detail(uid: &str) -> (Vec<String>, Result<Box<SketchfabModelDetail>, String>) {
    let url = format!("https://api.sketchfab.com/v3/models/{}", uid);

    if let Ok(mut cache) = global_cache().lock() {
        if let Some(cached) = cache.get_detail(&url) {
            return (vec!["cache hit".into()], Ok(cached));
        }
    }

    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r, Err(e) => return (vec![], Err(format!("tokio: {}", e))),
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

// ── Auth / download / like fetch functions ───────────────────────────────────

fn make_auth_client(token: &str) -> Result<reqwest::blocking::Client, String> {
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

fn fetch_sketchfab_me(token: &str) -> Result<Box<SketchfabMe>, String> {
    let client = make_auth_client(token)?;
    let resp = client.get("https://api.sketchfab.com/v3/me")
        .send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {} — {}", status, &text[..text.len().min(120)]));
    }
    serde_json::from_str::<SketchfabMe>(&text)
        .map(Box::new)
        .map_err(|e| format!("parse /me: {e}"))
}

fn fetch_sketchfab_download_info(uid: &str, token: &str) -> Result<SketchfabDownloadInfo, String> {
    let client = make_auth_client(token)?;
    let url = format!("https://api.sketchfab.com/v3/models/{}/download", uid);
    let resp = client.get(&url).send().map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {} — {}", status, &text[..text.len().min(200)]));
    }
    serde_json::from_str::<SketchfabDownloadInfo>(&text)
        .map_err(|e| format!("parse download info: {e}"))
}

fn sketchfab_like_model(uid: &str, token: &str) -> Result<(), String> {
    let client = make_auth_client(token)?;
    let params = [("model", uid)];
    let resp = client.post("https://api.sketchfab.com/v3/me/likes")
        .form(&params)
        .send().map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() && status.as_u16() != 204 {
        let text = resp.text().unwrap_or_default();
        return Err(format!("HTTP {} — {}", status, &text[..text.len().min(120)]));
    }
    Ok(())
}

fn sketchfab_unlike_model(uid: &str, token: &str) -> Result<(), String> {
    let client = make_auth_client(token)?;
    let url = format!("https://api.sketchfab.com/v3/me/likes/{}", uid);
    let resp = client.delete(&url).send().map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() && status.as_u16() != 204 {
        let text = resp.text().unwrap_or_default();
        return Err(format!("HTTP {} — {}", status, &text[..text.len().min(120)]));
    }
    Ok(())
}

// ── Focusable + Render ───────────────────────────────────────────────────────

impl Focusable for FabSearchWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Render for FabSearchWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_input = self.search_input.read(cx).value().to_string();
        if current_input != self.search_query { self.search_query = current_input; }

        let bg         = cx.theme().background;
        let _card_bg   = cx.theme().sidebar;
        let border_col = cx.theme().border;
        let fg         = cx.theme().foreground;
        let muted_fg   = cx.theme().muted_foreground;
        let accent_bg  = cx.theme().secondary;
        let accent_fg  = cx.theme().secondary_foreground;
        let active_bg  = cx.theme().accent;
        let active_fg  = cx.theme().accent_foreground;

        // ── Body ──────────────────────────────────────────────────────────
        let body: AnyElement = if self.is_loading {
            div().flex_1().flex().items_center().justify_center()
                .child(div().text_color(muted_fg).child("Searching Sketchfab…"))
                .into_any_element()

        } else if let Some(ref err) = self.error.clone() {
            let has_url = self.last_url.is_some();
            div().flex_1().min_h_0().flex().flex_col()
                .items_center().justify_center().gap_2()
                .child(div().text_color(gpui::red()).child(err.clone()))
                .when(has_url, |el| el.child(
                    Button::new("open-browser").label("Open in Browser")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(url) = &this.last_url { cx.open_url(url); }
                        }))
                ))
                .into_any_element()

        } else if self.selected_item_uid.is_some() {
            if self.detail_loading {
                div().flex_1().min_h_0().flex().items_center().justify_center()
                    .child(div().text_color(muted_fg).child("Loading model details…"))
                    .into_any_element()

            } else if let Some(ref err) = self.detail_error.clone() {
                div().flex_1().min_h_0().flex().flex_col()
                    .items_center().justify_center().gap_2()
                    .child(div().text_color(gpui::red()).child(err.clone()))
                    .child(Button::new("back-err").label("← Back")
                        .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))))
                    .into_any_element()

            } else if let Some(ref detail) = self.item_detail {
                let entity = cx.entity().clone();

                // collect loaded images keyed by URL
                let mut loaded: HashMap<String, std::sync::Arc<gpui::RenderImage>> =
                    detail.all_thumbnail_urls().into_iter().take(8).filter_map(|url| {
                        self.image_cache.get(url).and_then(|o| o.clone())
                            .map(|arc| (url.to_string(), arc))
                    }).collect();

                if let Some(ref user) = detail.user {
                    if let Some(url) = user.avatar_url(128) {
                        if let Some(Some(arc)) = self.image_cache.get(url) {
                            loaded.insert(url.to_string(), arc.clone());
                        }
                    }
                }

                item_detail::ItemDetailView::new(
                    detail.clone(),
                    loaded,
                    self.detail_scroll_handle.clone(),
                    self.detail_scroll_state.clone(),
                    self.gallery_scroll_handle.clone(),
                    self.gallery_scroll_state.clone(),
                    move |_window, cx| { entity.update(cx, |this, cx| this.go_back(cx)); },
                )
                .map(|view| {
                    // Attach download callback if user is logged in and model is downloadable
                    if detail.is_downloadable && self.api_token.is_some() {
                        let uid = detail.uid.clone();
                        let dl_status = self.download_state.get(&uid).cloned();
                        let entity2 = cx.entity().clone();
                        view.with_download(
                            move |_window, cx| {
                                entity2.update(cx, |this, cx| this.start_download(uid.clone(), cx));
                            },
                            dl_status,
                        )
                    } else {
                        view
                    }
                })
                .into_any_element()

            } else {
                div().flex_1().min_h_0().flex().items_center().justify_center()
                    .child(div().text_color(muted_fg).child("Loading…"))
                    .into_any_element()
            }

        } else if self.results.is_empty() {
            div().flex_1().min_h_0().flex().items_center().justify_center()
                .child(div().text_color(muted_fg)
                    .child("Search Sketchfab to browse free 3D models"))
                .into_any_element()

        } else {
            // ── Virtual results grid ──────────────────────────────────────
            // Card dimensions and spacing (must match the rendered card)
            const CARD_W: f32 = 260.0;
            const CARD_H: f32 = 260.0; // thumb(146) + body(114)
            const GAP:    f32 = 16.0;  // gap_4
            const PAD:    f32 = 16.0;  // px_4 padding on each side
            const ROW_H:  f32 = CARD_H + GAP;

            // Derive column count from available viewport width.
            // Formula: cols = floor((avail_w - PAD + GAP) / (CARD_W + GAP))
            // where avail_w = total_w - 2*PAD (removes left+right padding).
            let avail_w: f32 = window.viewport_size().width.into();
            let avail_w = avail_w - 2.0 * PAD;
            let cols: usize = (((avail_w + GAP) / (CARD_W + GAP)).floor() as usize).max(1);

            let entity = self.entity.clone().unwrap();
            let total_results = self.results.len();
            let result_rows = total_results.div_ceil(cols);
            let skel_rows: usize = if self.is_loading_more { 2 } else { 0 };
            let end_row: usize = if !self.is_loading_more && self.next_url.is_none() { 1 } else { 0 };
            let total_items = result_rows + skel_rows + end_row;

            let item_sizes = Rc::new(
                (0..total_items).map(|_| size(px(0.0), px(ROW_H))).collect::<Vec<_>>()
            );

            div().relative().flex_1().min_h_0().overflow_hidden()
                .child(
                    v_virtual_list(
                        entity,
                        "skfb-results-grid",
                        item_sizes,
                        move |view: &mut FabSearchWindow, range, _window, cx| {
                            let theme = cx.theme().clone();
                            let fg = theme.foreground;
                            let muted_fg = theme.muted_foreground;
                            let card_bg = theme.secondary;
                            let border_col = theme.border;

                            let total_res = view.results.len();
                            let res_rows = total_res.div_ceil(cols);
                            let s_rows: usize = if view.is_loading_more { 2 } else { 0 };

                            // Near-bottom detection: kick off load_more when
                            // the visible range reaches the last result row.
                            if !view.is_loading && !view.is_loading_more
                                && view.next_url.is_some()
                                && range.end >= res_rows.saturating_sub(1)
                            {
                                view.load_more(cx);
                            }

                            range.map(|row_idx| -> AnyElement {
                                if row_idx < res_rows {
                                    // ── Real card row ─────────────────────
                                    let start = row_idx * cols;
                                    let end = (start + cols).min(view.results.len());
                                    let row_cards: Vec<AnyElement> = (start..end).map(|idx| {
                                        let model = &view.results[idx];
                                        let name     = model.name.clone();
                                        let author   = model.user.as_ref().map(|u| u.display().to_string()).unwrap_or_default();
                                        let category = model.primary_category().map(|s| s.to_string()).unwrap_or_default();
                                        let subtitle = if category.is_empty() { author } else { format!("{} · {}", author, category) };
                                        let views    = fmt_count(model.view_count);
                                        let likes    = fmt_count(model.like_count);
                                        let is_dl    = model.is_downloadable;
                                        let is_anim  = model.animation_count > 0;
                                        let is_sp    = model.staffpicked_at.as_ref().map(|v| !v.is_null()).unwrap_or(false);
                                        let card_uid = model.uid.clone();
                                        let like_uid = card_uid.clone();
                                        let is_liked  = view.liked_uids.contains(&card_uid);
                                        let like_busy = view.like_inflight.contains(&card_uid);
                                        let show_like = view.api_token.is_some();
                                        let thumb_state = model.thumb_url(260)
                                            .map(|u| view.image_cache.get(u).and_then(|o| o.clone()));
                                        // None          = no URL
                                        // Some(None)    = in-flight
                                        // Some(Some(…)) = loaded
                                        let thumb_loading = matches!(thumb_state, None | Some(None));
                                        let thumb_arc = thumb_state.flatten();

                                        div()
                                            .id(SharedString::from(format!("skfb-card-{}", idx)))
                                            .cursor_pointer()
                                            .w(px(260.0)).rounded_lg().bg(card_bg)
                                            .border_1().border_color(border_col)
                                            .overflow_hidden().flex().flex_col()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.open_item_detail(card_uid.clone(), cx);
                                            }))
                                            .child(
                                                div().w_full().h(px(146.0)).overflow_hidden().relative()
                                                    .map(|el| {
                                                        if let Some(arc) = thumb_arc {
                                                            el.child(img(gpui::ImageSource::Render(arc))
                                                                .w_full().h_full().object_fit(gpui::ObjectFit::Cover))
                                                        } else if thumb_loading {
                                                            el.bg(border_col).flex().items_center().justify_center()
                                                                .child(Spinner::new().with_size(ui::Size::Medium)
                                                                    .color(muted_fg))
                                                        } else {
                                                            el.bg(border_col)
                                                        }
                                                    })
                                                    .when(is_sp, |el| el.child(
                                                        div().absolute().top(px(6.0)).left(px(6.0))
                                                            .px_2().py(px(2.0)).rounded_full()
                                                            .bg(gpui::rgb(0xFACC15))
                                                            .text_xs().font_bold().text_color(gpui::rgb(0x000000))
                                                            .child("★ Staff Pick")
                                                    ))
                                                    .when(is_dl, |el| el.child(
                                                        div().absolute().bottom(px(6.0)).right(px(6.0))
                                                            .px_2().py(px(2.0)).rounded_full()
                                                            .bg(gpui::rgb(0x22C55E))
                                                            .text_xs().font_bold().text_color(gpui::rgb(0xFFFFFF))
                                                            .child("↓")
                                                    ))
                                                    .when(is_anim, |el| el.child(
                                                        div().absolute().bottom(px(6.0)).left(px(6.0))
                                                            .px_2().py(px(2.0)).rounded_full()
                                                            .bg(gpui::rgb(0x6366F1))
                                                            .text_xs().font_bold().text_color(gpui::rgb(0xFFFFFF))
                                                            .child("▶")
                                                    ))
                                            )
                                            .child(div().p_3().child(v_flex().gap_1()
                                                .child(div().text_sm().font_bold().text_color(fg).line_clamp(2).child(name))
                                                .child(div().text_xs().text_color(muted_fg).child(subtitle))
                                                .child(h_flex().gap_3().items_center().mt_1()
                                                    .child(div().text_xs().text_color(muted_fg).child(format!("👁 {}", views)))
                                                    .child(div().text_xs().text_color(muted_fg).child(format!("♥ {}", likes)))
                                                    .when(show_like, |el| el.child(
                                                        div()
                                                            .id(SharedString::from(format!("like-{}", idx)))
                                                            .ml_auto()
                                                            .cursor_pointer()
                                                            .text_sm()
                                                            .text_color(if is_liked { gpui::Hsla::from(gpui::rgb(0xEF4444)) } else { muted_fg })
                                                            .opacity(if like_busy { 0.4 } else { 1.0 })
                                                            .child(if is_liked { "♥" } else { "♡" })
                                                            .on_click(cx.listener(move |this, _event, _, cx| {
                                                                cx.stop_propagation();
                                                                this.toggle_like(like_uid.clone(), cx);
                                                            }))
                                                    ))
                                                )
                                            ))
                                            .into_any_element()
                                    }).collect();

                                    h_flex().px_4().py(px(8.0)).gap_4().items_start()
                                        .children(row_cards)
                                        .into_any_element()

                                } else if row_idx < res_rows + s_rows {
                                    // ── Skeleton row ──────────────────────
                                    h_flex().px_4().py(px(8.0)).gap_4().items_start()
                                        .children((0..cols).map(|i| {
                                            div()
                                                .id(SharedString::from(format!("skel-{}-{}", row_idx, i)))
                                                .w(px(260.0)).rounded_lg()
                                                .border_1().border_color(border_col)
                                                .overflow_hidden().flex().flex_col()
                                                .child(Skeleton::new().w_full().h(px(146.0)))
                                                .child(div().p_3().child(v_flex().gap_2()
                                                    .child(Skeleton::new().w(px(160.0)).h_4())
                                                    .child(Skeleton::new().secondary(true).w(px(100.0)).h_3())
                                                    .child(Skeleton::new().secondary(true).w(px(120.0)).h_3())
                                                ))
                                        }))
                                        .into_any_element()

                                } else {
                                    // ── End of results row ────────────────
                                    h_flex().w_full().px_4().justify_center().py_3()
                                        .child(div().text_xs().text_color(muted_fg).child("End of results"))
                                        .into_any_element()
                                }
                            }).collect()
                        },
                    ).track_scroll(&self.results_scroll_handle)
                )
                .child(div().absolute().inset_0()
                    .child(Scrollbar::vertical(&self.results_scroll_state, &self.results_scroll_handle)))
                .into_any_element()
        };

        // ── Filter bar ────────────────────────────────────────────────────
        let dl_active  = self.filter_downloadable;
        let an_active  = self.filter_animated;
        let sp_active  = self.filter_staffpicked;
        let lic_active = self.filter_license != LicenseFilter::All;
        let lic_label  = self.filter_license.label();
        let show_lic   = self.show_license_menu;

        let filter_bar = div()
            .w_full().px_4().py_2()
            .border_b_1().border_color(border_col)
            .flex().flex_row().flex_wrap().gap_2().items_center()
            .child(div().text_xs().font_bold().text_color(muted_fg).child("Sort:"))
            .children(SortBy::all().into_iter().map(|s| {
                let active = self.sort_by == s;
                let lbl = s.label();
                div()
                    .id(SharedString::from(format!("sort-{}", lbl)))
                    .cursor_pointer().px_2().py(px(3.0)).rounded_full()
                    .text_xs().font_medium()
                    .bg(if active { active_bg } else { accent_bg })
                    .text_color(if active { active_fg } else { accent_fg })
                    .child(lbl)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.sort_by = s;
                        if !this.search_query.is_empty() { this.begin_search(cx); }
                        else { cx.notify(); }
                    }))
            }))
            .child(div().w(px(1.0)).h(px(16.0)).bg(border_col))
            .child(div().id("f-dl").cursor_pointer().px_2().py(px(3.0)).rounded_full()
                .text_xs().font_medium()
                .bg(if dl_active { active_bg } else { accent_bg })
                .text_color(if dl_active { active_fg } else { accent_fg })
                .child("↓ Download")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter_downloadable = !this.filter_downloadable;
                    if !this.search_query.is_empty() { this.begin_search(cx); }
                    else { cx.notify(); }
                }))
            )
            .child(div().id("f-an").cursor_pointer().px_2().py(px(3.0)).rounded_full()
                .text_xs().font_medium()
                .bg(if an_active { active_bg } else { accent_bg })
                .text_color(if an_active { active_fg } else { accent_fg })
                .child("▶ Animated")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter_animated = !this.filter_animated;
                    if !this.search_query.is_empty() { this.begin_search(cx); }
                    else { cx.notify(); }
                }))
            )
            .child(div().id("f-sp").cursor_pointer().px_2().py(px(3.0)).rounded_full()
                .text_xs().font_medium()
                .bg(if sp_active { active_bg } else { accent_bg })
                .text_color(if sp_active { active_fg } else { accent_fg })
                .child("★ Staff Pick")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.filter_staffpicked = !this.filter_staffpicked;
                    if !this.search_query.is_empty() { this.begin_search(cx); }
                    else { cx.notify(); }
                }))
            )
            .child(div().w(px(1.0)).h(px(16.0)).bg(border_col))
            .child(div().id("f-lic").cursor_pointer().px_2().py(px(3.0)).rounded_full()
                .text_xs().font_medium()
                .bg(if lic_active { active_bg } else { accent_bg })
                .text_color(if lic_active { active_fg } else { accent_fg })
                .child(format!("© {}", lic_label))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_license_menu = !this.show_license_menu;
                    cx.notify();
                }))
            )
            .when(show_lic, |el| el.children(LicenseFilter::all().into_iter().map(|lic| {
                let sel = self.filter_license == lic;
                let lbl = lic.label();
                div()
                    .id(SharedString::from(format!("lic-{}", lbl)))
                    .cursor_pointer().px_2().py(px(3.0)).rounded_full()
                    .text_xs().border_1().border_color(border_col)
                    .bg(if sel { active_bg } else { cx.theme().background })
                    .text_color(if sel { active_fg } else { muted_fg })
                    .child(lbl)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter_license = lic;
                        this.show_license_menu = false;
                        if !this.search_query.is_empty() { this.begin_search(cx); }
                        else { cx.notify(); }
                    }))
            })));

        // ── Auth bar ──────────────────────────────────────────────────────
        let logged_in   = self.api_token.is_some();
        let me_name     = self.me.as_ref().map(|m| m.display().to_string());
        let me_avatar   = self.me.as_ref()
            .and_then(|m| m.avatar_url(64).map(|s| s.to_string()))
            .and_then(|url| self.image_cache.get(&url).and_then(|o| o.clone()));
        let me_loading  = self.me_loading;
        let show_tok    = self.show_token_input;
        let account_tier = self.me.as_ref().and_then(|m| m.account.clone());

        let auth_section: AnyElement = if me_loading {
            div().text_xs().text_color(muted_fg).child("Verifying…").into_any_element()
        } else if logged_in {
            h_flex().gap_2().items_center()
                .map(|el| {
                    if let Some(arc) = me_avatar {
                        el.child(img(gpui::ImageSource::Render(arc))
                            .w_6().h_6().rounded_full().overflow_hidden())
                    } else {
                        el.child(div().w_6().h_6().rounded_full().bg(border_col))
                    }
                })
                .when_some(me_name, |el, name| {
                    el.child(div().text_xs().font_medium().text_color(fg).child(name))
                })
                .when_some(account_tier, |el, tier| {
                    el.child(div().text_xs().px_2().py(px(2.0)).rounded_full()
                        .bg(gpui::rgb(0x6366F1)).text_color(gpui::rgb(0xFFFFFF))
                        .child(tier))
                })
                .child(Button::new("logout-btn").label("Logout")
                    .on_click(cx.listener(|this, _, _, cx| this.clear_token(cx))))
                .into_any_element()
        } else if show_tok {
            h_flex().gap_2().items_center()
                .child(div().w(px(300.0))
                    .child(TextInput::new(&self.token_input).w_full()))
                .child(Button::new("verify-tok").label("Verify & Save")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        let tok = this.token_input.read(cx).value().to_string();
                        this.set_token(tok, cx);
                    })))
                .child(Button::new("cancel-tok").label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_token_input = false;
                        cx.notify();
                    })))
                .child(div().text_xs().text_color(muted_fg)
                    .child("Get your token at sketchfab.com/settings/password"))
                .into_any_element()
        } else {
            Button::new("login-btn").label("Login with API Token")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_token_input = true;
                    cx.notify();
                }))
                .into_any_element()
        };

        // ── Root ──────────────────────────────────────────────────────────
        let dl_count = self.download_state.len();
        let active_dl = self.download_state.values()
            .filter(|s| matches!(s, DownloadState::InProgress { .. }))
            .count();
        let show_dl_btn = dl_count > 0;
        let show_dl_mgr = self.show_download_manager;

        // Build download manager entries for the drawer
        let dl_entries: Vec<DownloadEntry> = self.download_state.iter().map(|(uid, state)| {
            match state {
                DownloadState::InProgress { filename, bytes_received, total_bytes, speed_bps, speed_history } => {
                    let progress_pct = total_bytes
                        .filter(|&t| t > 0)
                        .map(|t| (*bytes_received as f32 / t as f32 * 100.0).min(100.0))
                        .unwrap_or(0.0);
                    DownloadEntry {
                        uid: SharedString::from(uid.clone()),
                        filename: SharedString::from(filename.clone()),
                        progress_pct,
                        speed_bps: *speed_bps,
                        speed_history: speed_history.clone(),
                        status: DownloadItemStatus::InProgress,
                        bytes_received: *bytes_received,
                        total_bytes: *total_bytes,
                        path: None,
                    }
                }
                DownloadState::Done { filename, path, total_bytes } => {
                    DownloadEntry {
                        uid: SharedString::from(uid.clone()),
                        filename: SharedString::from(filename.clone()),
                        progress_pct: 100.0,
                        speed_bps: 0.0,
                        speed_history: Vec::new(),
                        status: DownloadItemStatus::Done,
                        bytes_received: *total_bytes,
                        total_bytes: Some(*total_bytes),
                        path: Some(path.clone()),
                    }
                }
                DownloadState::Error { filename, message } => {
                    DownloadEntry {
                        uid: SharedString::from(uid.clone()),
                        filename: SharedString::from(filename.clone()),
                        progress_pct: 0.0,
                        speed_bps: 0.0,
                        speed_history: Vec::new(),
                        status: DownloadItemStatus::Error(message.clone()),
                        bytes_received: 0,
                        total_bytes: None,
                        path: None,
                    }
                }
            }
        }).collect();

        let entity_for_drawer = cx.entity().clone();

        let layout = v_flex()
            .size_full().bg(bg)
            .child(TitleBar::new().child("Sketchfab"))
            .child(
                div().w_full().px_4().pt_4().pb_3()
                    .border_b_1().border_color(border_col)
                    .child(h_flex().w_full().gap_2().items_center()
                        .child(div().flex_1().min_w(px(200.0))
                            .child(TextInput::new(&self.search_input).w_full()
                                .prefix(ui::Icon::new(IconName::Search)
                                    .size_4().text_color(muted_fg))))
                        .child(Button::new("search-btn")
                            .label("Search")
                            .on_click(cx.listener(|this, _, _, cx| this.begin_search(cx))))
                        .child(div().w(px(1.0)).h(px(20.0)).bg(border_col))
                        .child(auth_section)
                        .when(show_dl_btn, |el| {
                            let dl_label = if active_dl > 0 {
                                format!("↓ {} active", active_dl)
                            } else {
                                format!("↓ {} done", dl_count)
                            };
                            el.child(div().w(px(1.0)).h(px(20.0)).bg(border_col))
                              .child(Button::new("dl-mgr-btn")
                                .label(dl_label)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_download_manager = !this.show_download_manager;
                                    cx.notify();
                                })))
                        })
                    )
            )
            .child(filter_bar)
            .child(body);

        div().size_full()
            .child(layout)
            .when(show_dl_mgr, |el| {
                el.child(
                    DownloadManagerDrawer::new(dl_entries)
                        .on_close(move |_, _, cx| {
                            entity_for_drawer.update(cx, |view, cx| {
                                view.show_download_manager = false;
                                cx.notify();
                            });
                        }),
                )
            })
            .children(Root::render_modal_layer(window, cx))
    }
}
