use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, IconName, Root, StyledExt, TitleBar, VirtualListScrollHandle,
    button::Button,
    download_manager::{DownloadEntry, DownloadManagerDrawer},
    h_flex,
    input::{InputEvent, InputState, TextInput},
    scroll::ScrollbarState,
    v_flex,
};

use crate::components;
use crate::parser::{SketchfabMe, SketchfabModel, SketchfabModelDetail};
use crate::search_index::fetch_sketchfab_me;
use crate::utils::actions::{DownloadState, DownloadMsg, LicenseFilter, SortBy};

pub struct FabSearchWindow {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) search_query: String,
    pub(crate) search_input: Entity<InputState>,

    pub(crate) sort_by: SortBy,
    pub(crate) filter_downloadable: bool,
    pub(crate) filter_animated: bool,
    pub(crate) filter_staffpicked: bool,
    pub(crate) filter_license: LicenseFilter,
    pub(crate) show_license_menu: bool,

    pub(crate) results: Vec<SketchfabModel>,
    pub(crate) next_url: Option<String>,

    pub(crate) is_loading: bool,
    pub(crate) is_loading_more: bool,
    pub(crate) error: Option<String>,
    pub(crate) last_url: Option<String>,

    pub(crate) selected_item_uid: Option<String>,
    pub(crate) item_detail: Option<Box<SketchfabModelDetail>>,
    pub(crate) detail_loading: bool,
    pub(crate) detail_error: Option<String>,

    pub(crate) image_cache: HashMap<String, Option<Arc<gpui::RenderImage>>>,
    pub(crate) image_inflight: usize,
    pub(crate) image_queue: VecDeque<String>,

    pub(crate) entity: Option<Entity<Self>>,

    pub(crate) api_token: Option<String>,
    pub(crate) me: Option<Box<SketchfabMe>>,
    pub(crate) me_loading: bool,
    pub(crate) show_token_input: bool,
    pub(crate) token_input: Entity<InputState>,

    pub(crate) download_state: HashMap<String, DownloadState>,
    pub(crate) show_download_manager: bool,

    pub(crate) selected_gallery_idx: usize,

    pub(crate) liked_uids: HashSet<String>,
    pub(crate) like_inflight: HashSet<String>,

    pub(crate) results_scroll_handle: VirtualListScrollHandle,
    pub(crate) results_scroll_state: ScrollbarState,
    pub(crate) detail_scroll_handle: ScrollHandle,
    pub(crate) detail_scroll_state: ScrollbarState,
    pub(crate) gallery_scroll_handle: ScrollHandle,
    pub(crate) gallery_scroll_state: ScrollbarState,
}

impl FabSearchWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_input =
            cx.new(|cx| InputState::new(_window, cx).placeholder("Search Sketchfab models\u{2026}"));
        let token_input = cx
            .new(|cx| InputState::new(_window, cx).placeholder("Paste your Sketchfab API token\u{2026}"));

        cx.subscribe(
            &search_input,
            |this, _input, event: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    crate::handlers::on_begin_search(this, cx);
                }
            },
        )
        .detach();

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
            image_queue: VecDeque::new(),
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
            results_scroll_handle: VirtualListScrollHandle::new(),
            results_scroll_state: ScrollbarState::default(),
            detail_scroll_handle: ScrollHandle::new(),
            detail_scroll_state: ScrollbarState::default(),
            gallery_scroll_handle: ScrollHandle::new(),
            gallery_scroll_state: ScrollbarState::default(),
        };
        let entity = cx.entity();
        this.entity = Some(entity);
        if saved_token.is_some() {
            this.fetch_me(cx);
        }
        this.scan_downloads_folder();
        this
    }

    pub(crate) fn build_search_url(&self) -> String {
        let q = urlencoding::encode(self.search_query.trim()).into_owned();
        let mut url = format!(
            "https://api.sketchfab.com/v3/search?type=models&q={}&count=24",
            q
        );
        if let Some(s) = self.sort_by.api_value() {
            url.push_str(&format!("&sort_by={}", s));
        }
        if self.filter_downloadable {
            url.push_str("&downloadable=true");
        }
        if self.filter_animated {
            url.push_str("&animated=true");
        }
        if self.filter_staffpicked {
            url.push_str("&staffpicked=true");
        }
        if let Some(l) = self.filter_license.api_value() {
            url.push_str(&format!("&license={}", l));
        }
        url
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
            smol::block_on(tx.send(fetch_sketchfab_me(&token)));
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
                    });
                });
            }
        })
        .detach();
    }

    pub(crate) fn scan_downloads_folder(&mut self) {
        let dir = dirs::download_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Sketchfab");
        let Ok(read_dir) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let uid = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&filename)
                .to_string();
            if self.download_state.contains_key(&uid) {
                continue;
            }
            let total_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            self.download_state.insert(
                uid,
                DownloadState::Done {
                    filename,
                    path,
                    total_bytes,
                },
            );
        }
    }

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
            smol::block_on(tx.send(maybe));
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
                    });
                });
            }
        })
        .detach();
    }

    #[allow(dead_code)]
    pub(crate) fn is_liked(&self, uid: &str) -> bool {
        self.liked_uids.contains(uid)
    }
}

impl Focusable for FabSearchWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for FabSearchWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "FabSearchWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(900.0, 650.0)
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        cx.new(|cx| FabSearchWindow::new(window, cx))
    }
}

impl Render for FabSearchWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_input = self.search_input.read(cx).value().to_string();
        if current_input != self.search_query {
            self.search_query = current_input;
        }

        let bg = cx.theme().background;
        let border_col = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted_fg = cx.theme().muted_foreground;
        let accent_bg = cx.theme().secondary;
        let accent_fg = cx.theme().secondary_foreground;
        let active_bg = cx.theme().accent;
        let active_fg = cx.theme().accent_foreground;

        // ── Body ──────────────────────────────────────────────────────────
        let body: AnyElement = if self.is_loading {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(muted_fg).child("Searching Sketchfab\u{2026}"))
                .into_any_element()
        } else if let Some(ref err) = self.error.clone() {
            let has_url = self.last_url.is_some();
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_color(gpui::red()).child(err.clone()))
                .when(has_url, |el| {
                    el.child(
                        Button::new("open-browser")
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
            components::render_detail_view(self, cx)
        } else if self.results.is_empty() {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_color(muted_fg)
                        .child("Search Sketchfab to browse free 3D models"),
                )
                .into_any_element()
        } else {
            let avail_w: f32 = window.viewport_size().width.into();
            components::render_results_grid(self, avail_w, muted_fg, border_col, fg)
        };

        // ── Filter bar ────────────────────────────────────────────────────
        let filter_bar = components::render_filter_bar(
            self, border_col, muted_fg, accent_bg, accent_fg, active_bg, active_fg, cx,
        );

        // ── Auth section ──────────────────────────────────────────────────────
        let auth_section = components::render_auth_section(self, fg, muted_fg, border_col, cx);

        // ── Download manager ──────────────────────────────────────────────
        let dl_count = self.download_state.len();
        let active_dl = self
            .download_state
            .values()
            .filter(|s| matches!(s, DownloadState::InProgress { .. }))
            .count();
        let show_dl_btn = dl_count > 0;
        let show_dl_mgr = self.show_download_manager;

        let dl_entries = components::build_download_entries(self);
        let entity_for_drawer = cx.entity().clone();

        let layout = v_flex()
            .size_full()
            .bg(bg)
            .child(TitleBar::new().child("Sketchfab"))
            .child(
                div()
                    .w_full()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .border_b_1()
                    .border_color(border_col)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div().flex_1().min_w(px(200.0)).child(
                                    TextInput::new(&self.search_input).w_full().prefix(
                                        ui::Icon::new(IconName::Search)
                                            .size_4()
                                            .text_color(muted_fg),
                                    ),
                                ),
                            )
                            .child(
                                Button::new("search-btn")
                                    .label("Search")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        crate::handlers::on_begin_search(this, cx);
                                    })),
                            )
                            .child(div().w(px(1.0)).h(px(20.0)).bg(border_col))
                            .child(auth_section)
                            .when(show_dl_btn, |el| {
                                let dl_label = if active_dl > 0 {
                                    format!("\u{2193} {} active", active_dl)
                                } else {
                                    format!("\u{2193} {} done", dl_count)
                                };
                                el.child(div().w(px(1.0)).h(px(20.0)).bg(border_col)).child(
                                    Button::new("dl-mgr-btn").label(dl_label).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.show_download_manager =
                                                !this.show_download_manager;
                                            if this.show_download_manager {
                                                this.scan_downloads_folder();
                                            }
                                            cx.notify();
                                        }),
                                    ),
                                )
                            }),
                    ),
            )
            .child(filter_bar)
            .child(body);

        div()
            .size_full()
            .child(layout)
            .when(show_dl_mgr, |el| {
                el.child(
                    DownloadManagerDrawer::new(dl_entries).on_close(move |_, _, cx| {
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
