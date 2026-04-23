//! DownloadManagerDrawer — a side-panel drawer listing all in-progress and
//! completed downloads with per-item speed sparklines and progress bars.
//!
//! Usage inside a GPUI `Render` impl — conditionally include as a sibling of
//! the main layout so the `anchored()` overlay covers the full window:
//!
//! ```rust
//! .when(self.show_download_manager, |el| {
//!     el.child(
//!         DownloadManagerDrawer::new(entries)
//!             .on_close(|_, window, cx| { window.close_drawer(cx); })
//!     )
//! })
//! ```

use std::rc::Rc;

use gpui::{
    div, prelude::FluentBuilder as _, px, App, ClickEvent, IntoElement, ParentElement, RenderOnce,
    SharedString, Styled, Window,
};

use crate::{
    download_item::{DownloadItem, DownloadItemStatus},
    drawer::Drawer,
    h_flex,
    root::ContextModal as _,
    speed_graph::{fmt_bytes, fmt_speed},
    v_flex, ActiveTheme, StyledExt as _,
};

// ── DownloadEntry (data record) ───────────────────────────────────────────────

/// All the data needed to render one row in the download manager.
#[derive(Clone)]
pub struct DownloadEntry {
    pub uid: SharedString,
    pub filename: SharedString,
    /// 0.0 – 100.0
    pub progress_pct: f32,
    /// Current transfer rate, bytes/s.
    pub speed_bps: f64,
    /// Rolling history of speed samples (bytes/s), oldest → newest.
    pub speed_history: Vec<f64>,
    pub status: DownloadItemStatus,
    pub bytes_received: u64,
    pub total_bytes: Option<u64>,
    /// Local path to the completed file (used to show Open Folder button).
    pub path: Option<std::path::PathBuf>,
}

// ── DownloadManagerDrawer ─────────────────────────────────────────────────────

/// A `Drawer` panel that lists all downloads with speed graphs and progress bars.
///
/// The drawer is right-anchored over the window.  Typically rendered
/// conditionally; the on_close callback should set the parent's visibility flag.
#[derive(IntoElement)]
pub struct DownloadManagerDrawer {
    downloads: Vec<DownloadEntry>,
    on_close: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
}

impl DownloadManagerDrawer {
    pub fn new(downloads: Vec<DownloadEntry>) -> Self {
        Self {
            downloads,
            on_close: Rc::new(|_, _, _| {}),
        }
    }

    /// Callback invoked when the user closes the drawer (overlay click or Esc).
    pub fn on_close(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_close = Rc::new(f);
        self
    }
}

impl RenderOnce for DownloadManagerDrawer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on_close = self.on_close.clone();
        let muted_fg = cx.theme().muted_foreground;

        // ── Summary header ────────────────────────────────────────────────
        let active_count = self
            .downloads
            .iter()
            .filter(|e| e.status == DownloadItemStatus::InProgress)
            .count();

        let title_text = if active_count > 0 {
            format!("Downloads — {} active", active_count)
        } else {
            "Downloads".to_string()
        };

        // ── Build total-speeds footer ─────────────────────────────────────
        let total_speed: f64 = self
            .downloads
            .iter()
            .filter(|e| e.status == DownloadItemStatus::InProgress)
            .map(|e| e.speed_bps)
            .sum();

        let total_bytes: u64 = self.downloads.iter().map(|e| e.bytes_received).sum();

        let footer_text = if active_count > 0 {
            format!(
                "Total {} · {}",
                fmt_bytes(total_bytes),
                fmt_speed(total_speed)
            )
        } else {
            fmt_bytes(total_bytes)
        };

        // ── Drawer items ──────────────────────────────────────────────────
        let items: Vec<DownloadItem> = self
            .downloads
            .into_iter()
            .map(|e| DownloadItem {
                id: SharedString::from(format!("dl-{}", e.uid)),
                filename: e.filename,
                progress_pct: e.progress_pct,
                speed_bps: e.speed_bps,
                speed_history: e.speed_history,
                status: e.status,
                bytes_received: e.bytes_received,
                total_bytes: e.total_bytes,
                path: e.path,
            })
            .collect();

        // ── Assemble drawer ───────────────────────────────────────────────
        let mut drawer = Drawer::new(window, cx)
            .title(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(title_text),
            )
            .footer(
                h_flex()
                    .px_4()
                    .py_2()
                    .w_full()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(div().text_xs().text_color(muted_fg).child(footer_text)),
            )
            .size(px(420.))
            .resizable(true)
            .overlay(true)
            .overlay_closable(true)
            .on_close(move |ev, window, cx| {
                window.close_drawer(cx);
                on_close(ev, window, cx);
            });

        // Empty state or list of items
        if items.is_empty() {
            drawer = drawer.child(
                v_flex()
                    .w_full()
                    .py_8()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted_fg)
                            .child("No downloads yet"),
                    ),
            );
        } else {
            for item in items {
                drawer = drawer.child(item);
            }
        }

        drawer
    }
}
