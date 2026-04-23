pub mod auth;
pub mod image_loader;
pub mod item_detail;
pub mod parser;

mod dispatch;
mod results;
mod search_index;

pub(crate) use dispatch::{DownloadState, LicenseFilter, SortBy};

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use gpui::{prelude::*, *};
use parser::{SketchfabMe, SketchfabModel, SketchfabModelDetail};
use ui::{
    ActiveTheme, IconName, Root, Sizable, StyledExt, TitleBar, VirtualListScrollHandle,
    button::Button,
    download_manager::DownloadManagerDrawer,
    h_flex,
    input::{InputState, TextInput},
    scroll::ScrollbarState,
    v_flex,
};

// ── Main window struct ───────────────────────────────────────────────────────

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

    pub(crate) image_cache: HashMap<String, Option<std::sync::Arc<gpui::RenderImage>>>,
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

// ── Focusable + Render ───────────────────────────────────────────────────────

impl Focusable for FabSearchWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
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
                .child(div().text_color(muted_fg).child("Searching Sketchfab…"))
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
            if self.detail_loading {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_color(muted_fg).child("Loading model details…"))
                    .into_any_element()
            } else if let Some(ref err) = self.detail_error.clone() {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_color(gpui::red()).child(err.clone()))
                    .child(
                        Button::new("back-err")
                            .label("← Back")
                            .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
                    )
                    .into_any_element()
            } else if let Some(ref detail) = self.item_detail {
                let entity = cx.entity().clone();

                let mut loaded: HashMap<String, std::sync::Arc<gpui::RenderImage>> = detail
                    .all_thumbnail_urls()
                    .into_iter()
                    .take(12)
                    .filter_map(|url| {
                        self.image_cache
                            .get(url)
                            .and_then(|o| o.clone())
                            .map(|arc| (url.to_string(), arc))
                    })
                    .collect();

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
                    move |_window, cx| {
                        entity.update(cx, |this, cx| this.go_back(cx));
                    },
                )
                .map(|view| {
                    if self.api_token.is_some() {
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
                .map(|view| {
                    let idx = self.selected_gallery_idx;
                    let entity3 = cx.entity().clone();
                    view.with_selected_image(idx, move |new_idx, _, cx| {
                        entity3.update(cx, |this, cx| {
                            this.selected_gallery_idx = new_idx;
                            cx.notify();
                        });
                    })
                })
                .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().text_color(muted_fg).child("Loading…"))
                    .into_any_element()
            }
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
            self.render_results_grid(avail_w, muted_fg, border_col, fg)
        };

        // ── Filter bar ────────────────────────────────────────────────────
        let filter_bar = self.render_filter_bar(
            border_col, muted_fg, accent_bg, accent_fg, active_bg, active_fg, cx,
        );

        // ── Auth bar ──────────────────────────────────────────────────────
        let auth_section = self.render_auth_section(fg, muted_fg, border_col, cx);

        // ── Download manager ──────────────────────────────────────────────
        let dl_count = self.download_state.len();
        let active_dl = self
            .download_state
            .values()
            .filter(|s| matches!(s, DownloadState::InProgress { .. }))
            .count();
        let show_dl_btn = dl_count > 0;
        let show_dl_mgr = self.show_download_manager;

        let dl_entries = self.build_download_entries();
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
                                    .on_click(cx.listener(|this, _, _, cx| this.begin_search(cx))),
                            )
                            .child(div().w(px(1.0)).h(px(20.0)).bg(border_col))
                            .child(auth_section)
                            .when(show_dl_btn, |el| {
                                let dl_label = if active_dl > 0 {
                                    format!("↓ {} active", active_dl)
                                } else {
                                    format!("↓ {} done", dl_count)
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
