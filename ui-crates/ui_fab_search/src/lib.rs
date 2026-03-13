pub mod image_loader;
pub mod item_detail;
pub mod parser;

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{ prelude::*, * };
use ui::{
    ActiveTheme,
    Sizable,
    TitleBar,
    v_flex,
    h_flex,
    button::Button,
    input::{ InputState, TextInput },
    IconName,
    StyledExt,
};
use parser::{FabItemDetail, FabListing};

/// Simple window that lets the user search the Fab asset store
pub struct FabSearchWindow {
    focus_handle: FocusHandle,
    search_query: String,
    search_input: Entity<InputState>,
    filter_kind: Option<String>,
    results: Vec<FabListing>,
    request_log: Vec<String>,
    is_loading: bool,
    error: Option<String>,
    /// Last URL fetched — used for the "Open in Browser" fallback button.
    last_url: Option<String>,
    // ── item detail state ──────────────────────────────────────────────────
    selected_item_uid: Option<String>,
    item_detail: Option<Box<FabItemDetail>>,
    detail_loading: bool,
    detail_error: Option<String>,
    // ── image cache ────────────────────────────────────────────────────────
    /// None = download in progress; Some(path) = file sitting in disk cache.
    image_cache: HashMap<String, Option<std::path::PathBuf>>,
}

impl FabSearchWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_input = cx.new(|cx| {
            InputState::new(_window, cx).placeholder("Search fab...")
        });

        cx.subscribe(&search_input, |this, _input, event: &ui::input::InputEvent, cx| {
            if let ui::input::InputEvent::PressEnter { .. } = event {
                this.perform_search(cx);
            }
        }).detach();

        Self {
            focus_handle,
            search_query: String::new(),
            search_input,
            filter_kind: None,
            results: Vec::new(),
            request_log: Vec::new(),
            is_loading: false,
            error: None,
            last_url: None,
            selected_item_uid: None,
            item_detail: None,
            detail_loading: false,
            detail_error: None,
            image_cache: HashMap::new(),
        }
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.selected_item_uid = None;
        self.item_detail = None;
        self.detail_loading = false;
        self.detail_error = None;
        cx.notify();
    }

    /// Kick off a background download for `url` if not already cached/loading.
    /// Stores `None` immediately (= loading) then updates to `Some(arc)` once done.
    fn ensure_image_loaded(&mut self, url: String, cx: &mut Context<Self>) {
        if url.is_empty() || self.image_cache.contains_key(&url) {
            return;
        }
        self.image_cache.insert(url.clone(), None); // mark as in-flight

        let (tx, rx) =
            smol::channel::bounded::<Option<std::path::PathBuf>>(1);
        let url_for_thread = url.clone();

        std::thread::spawn(move || {
            let maybe = image_loader::download_to_cache(&url_for_thread).ok();
            smol::block_on(tx.send(maybe)).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok(maybe) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.image_cache.insert(url, maybe);
                        cx.notify();
                    })
                    .ok();
                })
                .ok();
            }
        })
        .detach();
    }

    fn open_item_detail(&mut self, uid: String, cx: &mut Context<Self>) {
        if self.selected_item_uid.as_deref() == Some(uid.as_str()) {
            return;
        }
        self.selected_item_uid = Some(uid.clone());
        self.item_detail = None;
        self.detail_loading = true;
        self.detail_error = None;
        self.request_log.push(format!("GET /i/listings/{}", uid));
        cx.notify();

        let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<Box<FabItemDetail>, String>)>(1);
        std::thread::spawn(move || {
            let result = fetch_fab_item_detail(&uid);
            smol::block_on(tx.send(result)).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok((log_lines, result)) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.detail_loading = false;
                        view.request_log.extend(log_lines);
                        match result {
                            Ok(detail) => {
                                view.request_log.push(format!("  ✓ detail: {}", detail.title));
                                // kick off gallery image downloads
                                let gallery_urls: Vec<String> = detail.medias.iter()
                                    .filter(|m| m.media_type == "image" || m.media_type.is_empty())
                                    .take(12)
                                    .map(|m| {
                                        m.images.iter()
                                            .max_by_key(|i| i.width)
                                            .map(|i| i.url.as_str())
                                            .filter(|s: &&str| !s.is_empty())
                                            .unwrap_or(m.media_url.as_str())
                                            .to_string()
                                    })
                                    .collect();
                                for url in gallery_urls {
                                    view.ensure_image_loaded(url, cx);
                                }
                                // seller avatar
                                if let Some(ref avatar_url) = detail.user.profile_image_url {
                                    if !avatar_url.is_empty() {
                                        view.ensure_image_loaded(avatar_url.clone(), cx);
                                    }
                                }
                                view.item_detail = Some(detail);
                            }
                            Err(e) => {
                                view.request_log.push(format!("  ✗ {}", e));
                                view.detail_error = Some(e);
                            }
                        }
                        cx.notify();
                    }).ok();
                }).ok();
            }
        }).detach();
    }

    fn perform_search(&mut self, cx: &mut Context<Self>) {
        if self.search_query.trim().is_empty() {
            return;
        }

        let query = self.search_query.clone();
        let ltype = self.filter_kind.clone().unwrap_or_else(|| "3d-model".to_string());
        let encoded = urlencoding::encode(&query).into_owned();
        let url = format!(
            "https://www.fab.com/i/listings/search?listing_types={}&sort_by=-relevance&q={}",
            ltype, encoded
        );

        self.is_loading = true;
        self.results.clear();
        self.error = None;
        self.last_url = Some(url.clone());
        self.request_log.push(format!("GET {}", url));
        cx.notify();

        let fetch_url = url.clone();
        let (tx, rx) = smol::channel::bounded::<(Vec<String>, Result<Vec<FabListing>, String>)>(1);
        std::thread::spawn(move || {
            let result = fetch_fab_listings(&fetch_url);
            smol::block_on(tx.send(result)).ok();
        });

        cx.spawn(async move |this, cx| {
            if let Ok((log_lines, result)) = rx.recv().await {
                cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.is_loading = false;
                        view.request_log.extend(log_lines);
                        match result {
                            Ok(listings) => {
                                view.request_log.push(
                                    format!("  ✓ {} result(s)", listings.len())
                                );
                                view.results = listings;
                                // kick off thumbnail downloads
                                let thumb_urls: Vec<String> = view.results.iter()
                                    .filter_map(|l| {
                                        l.thumbnails.first()
                                            .and_then(|t| t.best_image_url(260))
                                            .map(|s| s.to_string())
                                    })
                                    .collect();
                                for url in thumb_urls {
                                    view.ensure_image_loaded(url, cx);
                                }
                            }
                            Err(e) => {
                                view.request_log.push(format!("  ✗ {}", e));
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

// ── response cache ──────────────────────────────────────────────────────────

const CACHE_CAP: usize = 20;
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);

enum CacheValue {
    Listings(Vec<FabListing>),
    Detail(Box<FabItemDetail>),
}

struct CacheEntry {
    key: String,
    value: CacheValue,
    inserted_at: Instant,
}

struct FabCache {
    entries: VecDeque<CacheEntry>,
}

impl FabCache {
    fn new() -> Self {
        Self { entries: VecDeque::with_capacity(CACHE_CAP) }
    }

    fn get_listings(&mut self, key: &str) -> Option<Vec<FabListing>> {
        self.find(key).and_then(|e| {
            if let CacheValue::Listings(ref v) = e.value { Some(v.clone()) } else { None }
        })
    }

    fn get_detail(&mut self, key: &str) -> Option<Box<FabItemDetail>> {
        self.find(key).and_then(|e| {
            if let CacheValue::Detail(ref v) = e.value { Some(v.clone()) } else { None }
        })
    }

    fn insert(&mut self, key: String, value: CacheValue) {
        // Remove existing entry for the same key
        self.entries.retain(|e| e.key != key);
        // Evict oldest if at capacity
        if self.entries.len() >= CACHE_CAP {
            self.entries.pop_front();
        }
        self.entries.push_back(CacheEntry { key, value, inserted_at: Instant::now() });
    }

    fn find(&mut self, key: &str) -> Option<&CacheEntry> {
        // Evict expired entries first
        self.entries.retain(|e| e.inserted_at.elapsed() < CACHE_TTL);
        self.entries.iter().find(|e| e.key == key)
    }
}

fn global_cache() -> &'static Mutex<FabCache> {
    static CACHE: OnceLock<Mutex<FabCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FabCache::new()))
}

// ── fetch helpers ────────────────────────────────────────────────────────────

/// Perform a request impersonating Chrome's full TLS fingerprint (JA3/JA4 + HTTP/2 settings)
/// via rquest's BoringSSL backend.  Must be called from an OS thread, not a GPUI/smol task.
/// Returns `(log_lines, result)` so the caller can push all logs into the UI panel.
fn fetch_fab_listings(url: &str) -> (Vec<String>, Result<Vec<FabListing>, String>) {
    use rquest::Impersonate;

    let mut log: Vec<String> = Vec::new();
    macro_rules! log {
        ($($arg:tt)*) => {{
            let s = format!($($arg)*);
            println!("{}", s);
            log.push(s);
        }}
    }

    // Check cache first
    if let Ok(mut cache) = global_cache().lock() {
        if let Some(cached) = cache.get_listings(url) {
            log!("cache hit: {}", url);
            return (log, Ok(cached));
        }
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return (log, Err(format!("tokio error: {}", e))),
    };

    let result = rt.block_on(async {
        log!("→ GET {}", url);

        let client = rquest::Client::builder()
            .impersonate(Impersonate::Chrome131)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| { log!("client build: {}", e); e.to_string() })?;

        let response = client.get(url).send().await
            .map_err(|e| { log!("request error: {}", e); e.to_string() })?;

        let status = response.status();
        log!("← HTTP {}", status);
        for (k, v) in response.headers() {
            log!("  {}: {}", k, v.to_str().unwrap_or("?"));
        }

        let text = response.text().await
            .map_err(|e| { log!("body read error: {}", e); e.to_string() })?;

        log!("body ({} bytes): {}…", text.len(), &text[..text.len().min(300)]);

        if !status.is_success() {
            return Err(format!("HTTP {} — blocked. Use 'Open in Browser' as fallback.", status));
        }

        let parsed: parser::FabSearchResponse = serde_json::from_str(&text)
            .map_err(|e| { log!("parse error: {}", e); format!("Parse error: {e}") })?;

        log!("parsed {} listings", parsed.results.len());
        Ok(parsed.results)
    });

    // Store successful result in cache
    if let Ok(listings) = &result {
        if let Ok(mut cache) = global_cache().lock() {
            cache.insert(url.to_string(), CacheValue::Listings(listings.clone()));
        }
    }

    (log, result)
}

/// Fetch full item detail from GET /i/listings/{uid}.
/// Must be called from an OS thread (not a GPUI/smol task).
fn fetch_fab_item_detail(uid: &str) -> (Vec<String>, Result<Box<FabItemDetail>, String>) {
    use rquest::Impersonate;

    let mut log: Vec<String> = Vec::new();
    macro_rules! log {
        ($($arg:tt)*) => {{
            let s = format!($($arg)*);
            println!("{}", s);
            log.push(s);
        }}
    }

    let url = format!("https://www.fab.com/i/listings/{}", uid);

    // Check cache first
    if let Ok(mut cache) = global_cache().lock() {
        if let Some(cached) = cache.get_detail(&url) {
            log!("cache hit: {}", url);
            return (log, Ok(cached));
        }
    }

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return (log, Err(format!("tokio error: {}", e))),
    };

    let result = rt.block_on(async {
        log!("→ GET {}", url);

        let client = rquest::Client::builder()
            .impersonate(Impersonate::Chrome131)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| { log!("client build: {}", e); e.to_string() })?;

        let response = client.get(&url).send().await
            .map_err(|e| { log!("request error: {}", e); e.to_string() })?;

        let status = response.status();
        log!("← HTTP {}", status);

        let text = response.text().await
            .map_err(|e| { log!("body read: {}", e); e.to_string() })?;

        log!("body ({} bytes): {}…", text.len(), &text[..text.len().min(200)]);

        if !status.is_success() {
            return Err(format!("HTTP {}", status));
        }

        let parsed: FabItemDetail = serde_json::from_str(&text)
            .map_err(|e| { log!("parse error: {}", e); format!("Parse error: {e}") })?;

        log!("parsed item: {}", parsed.title);
        Ok(Box::new(parsed))
    });

    // Store successful result in cache
    if let Ok(detail) = &result {
        if let Ok(mut cache) = global_cache().lock() {
            cache.insert(url.clone(), CacheValue::Detail(detail.clone()));
        }
    }

    (log, result)
}

impl Focusable for FabSearchWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FabSearchWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_input = self.search_input.read(cx).value().to_string();
        if current_input != self.search_query {
            self.search_query = current_input;
        }

        let bg         = cx.theme().background;
        let card_bg    = cx.theme().sidebar;
        let border_col = cx.theme().border;
        let fg         = cx.theme().foreground;
        let muted_fg   = cx.theme().muted_foreground;
        let accent_bg  = cx.theme().secondary;
        let accent_fg  = cx.theme().secondary_foreground;

        // ── request log panel ──────────────────────────────────────────────
        let log_panel = div()
            .w(px(260.0))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(border_col)
            .flex()
            .flex_col()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(border_col)
                    .text_sm()
                    .font_bold()
                    .text_color(muted_fg)
                    .child("Request Log")
            )
            .child(
                div()
                    .id("log-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_2()
                    .child(
                        v_flex()
                            .gap_1()
                            .children(self.request_log.iter().rev().map(|entry| {
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .font_family("monospace")
                                    .child(entry.clone())
                            }))
                    )
            );

        // ── results body ───────────────────────────────────────────────────
        let body: AnyElement = if self.is_loading {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(muted_fg).child("Fetching results from Fab…"))
                .into_any_element()
        } else if let Some(ref err) = self.error {
            let err_text = err.clone();
            let has_url = self.last_url.is_some();

            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_color(gpui::red()).child(err_text))
                .when(has_url, |el| {
                    el.child(
                        Button::new("open-in-browser")
                            .label("Open in Browser")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(url) = &this.last_url {
                                    cx.open_url(url);
                                }
                            })),
                    )
                })
                .into_any_element()
        } else if self.selected_item_uid.is_some() {
            // ── item detail view ──────────────────────────────────────────────
            if self.detail_loading {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_color(muted_fg).child("Loading item details…"))
                    .into_any_element()
            } else if let Some(ref err) = self.detail_error {
                let err_text = err.clone();
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_color(gpui::red()).child(err_text))
                    .child(
                        Button::new("back-from-error")
                            .label("← Back")
                            .on_click(cx.listener(|this, _, _, cx| this.go_back(cx)))
                    )
                    .into_any_element()
            } else if let Some(ref detail) = self.item_detail {
                let entity = cx.entity().clone();
                // Collect only the fully-loaded images for this detail view
                let loaded_images: std::collections::HashMap<String, std::path::PathBuf> = detail
                    .medias
                    .iter()
                    .filter(|m| m.media_type == "image" || m.media_type.is_empty())
                    .take(12)
                    .filter_map(|m| {
                        let url = m
                            .images
                            .iter()
                            .max_by_key(|i| i.width)
                            .map(|i| i.url.as_str())
                            .filter(|s: &&str| !s.is_empty())
                            .unwrap_or(m.media_url.as_str())
                            .to_string();
                        self.image_cache
                            .get(&url)
                            .and_then(|opt| opt.clone())
                            .map(|path| (url, path))
                    })
                    .collect();
                item_detail::ItemDetailView::new(
                    detail.clone(),
                    loaded_images,
                    move |_window, cx| {
                        entity.update(cx, |this, cx| this.go_back(cx));
                    },
                )
                .into_any_element()
            } else {
                // selected but no detail yet (shouldn't happen, but guard)
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_color(muted_fg).child("Loading…"))
                    .into_any_element()
            }
        } else if self.results.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(muted_fg).child("Search Fab to discover assets"))
                .into_any_element()
        } else {
            let cards: Vec<AnyElement> = self.results.iter().enumerate().map(|(idx, listing)| {
                let price_text = if listing.is_free {
                    "Free".to_string()
                } else if let Some(ref sp) = listing.starting_price {
                    format!("${:.2}", sp.price)
                } else {
                    "—".to_string()
                };

                let rating_text = listing.ratings.as_ref().map(|r| {
                    format!("★ {:.1}  ({} ratings)", r.average_rating, r.total)
                }).unwrap_or_default();

                let seller   = listing.user.seller_name.clone();
                let category = listing.category.as_ref().map(|c| c.name.clone()).unwrap_or_default();
                let formats: Vec<String> = listing.asset_formats.iter()
                    .map(|f| f.asset_format_type.name.clone())
                    .take(3)
                    .collect();

                let card_uid = listing.uid.clone();
                let listing_type = listing.listing_type.clone().unwrap_or_default();
                let thumb_url: Option<String> = listing
                    .thumbnails
                    .first()
                    .and_then(|t| t.best_image_url(260))
                    .map(|s| s.to_string());
                // Look up the cached path — None-value = still downloading
                let thumb_path: Option<std::path::PathBuf> = thumb_url
                    .as_deref()
                    .and_then(|u| self.image_cache.get(u))
                    .and_then(|opt| opt.clone());

                div()
                    .id(SharedString::from(format!("fab-card-{}", idx)))
                    .cursor_pointer()
                    .w(px(260.0))
                    .rounded_lg()
                    .bg(card_bg)
                    .border_1()
                    .border_color(border_col)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_click(cx.listener(move |this, _ev, _win, cx| {
                        this.open_item_detail(card_uid.clone(), cx);
                    }))
                    // thumbnail — real image when available, muted placeholder otherwise
                    .child(
                        div()
                            .w_full()
                            .h(px(140.0))
                            .bg(card_bg)
                            .overflow_hidden()
                            .map(|el| {
                                if let Some(path) = thumb_path {
                                    el.child(
                                        img(path)
                                            .w_full()
                                            .h_full()
                                            .object_fit(gpui::ObjectFit::Cover),
                                    )
                                } else {
                                    el.bg(border_col)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted_fg)
                                                .child(listing_type),
                                        )
                                }
                            }),
                    )
                    // card body
                    .child(
                        div()
                            .p_3()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_bold()
                                            .text_color(fg)
                                            .line_clamp(2)
                                            .child(listing.title.clone())
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child(if category.is_empty() {
                                                seller.clone()
                                            } else {
                                                format!("{} · {}", seller, category)
                                            })
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_bold()
                                                    .text_color(fg)
                                                    .child(price_text)
                                            )
                                            .when(!rating_text.is_empty(), |el| {
                                                el.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(muted_fg)
                                                        .child(rating_text)
                                                )
                                            })
                                    )
                                    .when(!formats.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .flex()
                                                .flex_wrap()
                                                .gap_1()
                                                .mt_1()
                                                .children(formats.iter().map(|f| {
                                                    div()
                                                        .text_xs()
                                                        .px_2()
                                                        .py(px(2.0))
                                                        .rounded_sm()
                                                        .bg(accent_bg)
                                                        .text_color(accent_fg)
                                                        .child(f.clone())
                                                }))
                                        )
                                    })
                            )
                    )
                    .into_any_element()
            }).collect();

            div()
                .id("results-scroll")
                .flex_1()
                .overflow_y_scroll()
                .p_4()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_4()
                        .children(cards)
                )
                .into_any_element()
        };

        // ── root layout ────────────────────────────────────────────────────
        v_flex()
            .size_full()
            .bg(bg)
            .child(TitleBar::new().child("Fab Search"))
            // search row
            .child(
                div()
                    .w_full()
                    .p_4()
                    .border_b_1()
                    .border_color(border_col)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(200.0))
                                    .child(
                                        TextInput::new(&self.search_input)
                                            .w_full()
                                            .prefix(
                                                ui::Icon::new(IconName::Search)
                                                    .size_4()
                                                    .text_color(muted_fg)
                                            )
                                    )
                            )
                            .child(
                                Button::new("Search")
                                    .small()
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.perform_search(cx);
                                    }))
                            )
                    )
            )
            // log + results row
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .child(log_panel)
                    .child(body)
            )
    }
}
