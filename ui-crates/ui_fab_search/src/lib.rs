pub mod image_loader;
pub mod item_detail;
pub mod parser;

use std::collections::{HashMap, VecDeque};
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
    button::{Button, ButtonVariants as _},
    input::{InputState, TextInput},
    scroll::{Scrollbar, ScrollbarState},
    skeleton::Skeleton,
    v_virtual_list, VirtualListScrollHandle,
    IconName,
    StyledExt,
};
use parser::{fmt_count, SketchfabModel, SketchfabModelDetail};

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

    // entity ref (needed for v_virtual_list)
    entity: Option<Entity<Self>>,

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

        cx.subscribe(&search_input, |this, _input, event: &ui::input::InputEvent, cx| {
            if let ui::input::InputEvent::PressEnter { .. } = event {
                this.begin_search(cx);
            }
        }).detach();

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
            entity: None,

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
        this
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.selected_item_uid = None;
        self.item_detail = None;
        self.detail_loading = false;
        self.detail_error = None;
        cx.notify();
    }

    fn ensure_image_loaded(&mut self, url: String, cx: &mut Context<Self>) {
        if url.is_empty() || self.image_cache.contains_key(&url) {
            return;
        }
        self.image_cache.insert(url.clone(), None);

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

// ── Focusable + Render ───────────────────────────────────────────────────────

impl Focusable for FabSearchWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Render for FabSearchWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_input = self.search_input.read(cx).value().to_string();
        if current_input != self.search_query { self.search_query = current_input; }

        let bg         = cx.theme().background;
        let card_bg    = cx.theme().sidebar;
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
                ).into_any_element()

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
            const COLS: usize = 3;
            const CARD_H: f32 = 260.0; // thumb(146) + body(114)
            const ROW_H: f32 = CARD_H + 16.0; // + row gap

            let entity = self.entity.clone().unwrap();
            let total_results = self.results.len();
            let result_rows = total_results.div_ceil(COLS);
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
                            let res_rows = total_res.div_ceil(COLS);
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
                                    let start = row_idx * COLS;
                                    let end = (start + COLS).min(view.results.len());
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
                                        let thumb_arc = model.thumb_url(260)
                                            .and_then(|u| view.image_cache.get(u))
                                            .and_then(|o| o.clone());

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
                                                        } else {
                                                            el.bg(border_col).flex().items_center().justify_center()
                                                                .child(div().text_xs().text_color(muted_fg).child(""))
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
                                        .children((0..COLS).map(|i| {
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

        // ── Root ──────────────────────────────────────────────────────────
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
                    )
            )
            .child(filter_bar)
            .child(body);

        div().size_full()
            .child(layout)
            .children(Root::render_modal_layer(window, cx))
    }
}
