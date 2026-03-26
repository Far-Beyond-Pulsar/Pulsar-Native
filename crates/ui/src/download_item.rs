//! DownloadItem — a single row in the download manager list.
//!
//! Shows: filename + status badge, speed sparkline, progress bar, and stats row.
//! Used by `DownloadManagerDrawer` but can also be embedded standalone.

use std::path::PathBuf;

use gpui::{
    div, prelude::FluentBuilder as _, px, App, Hsla, IntoElement, InteractiveElement as _,
    ParentElement, RenderOnce, SharedString, Styled, Window,
};

use crate::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Sizable, StyledExt as _,
    progress::Progress,
    speed_graph::{fmt_bytes, fmt_speed, SpeedGraph},
};

// ── Status ────────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
pub enum DownloadItemStatus {
    InProgress,
    Done,
    Error(String),
}

// ── DownloadItem ──────────────────────────────────────────────────────────────

/// A single download entry card — filename, speed graph, progress bar, stats.
#[derive(IntoElement)]
pub struct DownloadItem {
    pub id: SharedString,
    pub filename: SharedString,
    /// 0.0 – 100.0
    pub progress_pct: f32,
    /// Current transfer rate in bytes/s.
    pub speed_bps: f64,
    /// Historical speed samples in bytes/s, ordered oldest → newest (max ~60).
    pub speed_history: Vec<f64>,
    pub status: DownloadItemStatus,
    pub bytes_received: u64,
    pub total_bytes: Option<u64>,
    /// Local path to the completed file (present when status is Done).
    pub path: Option<PathBuf>,
}

impl RenderOnce for DownloadItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fg = cx.theme().foreground;
        let muted_fg = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let card_bg = cx.theme().secondary;

        let green: Hsla = gpui::rgb(0x22C55E).into();
        let green_dim = green.opacity(0.12);

        let is_active = self.status == DownloadItemStatus::InProgress;
        let is_done = self.status == DownloadItemStatus::Done;
        let is_error = matches!(self.status, DownloadItemStatus::Error(_));

        let progress = if is_done {
            100.0
        } else {
            match self.total_bytes {
                Some(t) if t > 0 => (self.bytes_received as f32 / t as f32 * 100.0).min(100.0),
                _ => self.progress_pct,
            }
        };

        div()
            .id(self.id.clone())
            .p_3()
            .mb_2()
            .rounded_lg()
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .child(
                v_flex()
                    .gap_2()
                    // ── Header: filename + badge ─────────────────────────
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(fg)
                                    .overflow_hidden()
                                    .child(self.filename.clone()),
                            )
                            .child(
                                // Status badge
                                if is_active {
                                    div()
                                        .px_2()
                                        .py(px(2.))
                                        .rounded_full()
                                        .text_xs()
                                        .bg(cx.theme().accent.opacity(0.15))
                                        .text_color(cx.theme().accent)
                                        .child("Downloading")
                                } else if is_done {
                                    div()
                                        .px_2()
                                        .py(px(2.))
                                        .rounded_full()
                                        .text_xs()
                                        .bg(green_dim)
                                        .text_color(green)
                                        .child("Done ✓")
                                } else {
                                    div()
                                        .px_2()
                                        .py(px(2.))
                                        .rounded_full()
                                        .text_xs()
                                        .bg(gpui::red().opacity(0.12))
                                        .text_color(gpui::red())
                                        .child("Error")
                                },
                            ),
                    )
                    // ── Speed sparkline (active or done with history) ─────
                    .when(!self.speed_history.is_empty(), |el| {
                        el.child(
                            // SpeedGraph defaults to w_full() — fills card width
                            SpeedGraph::new(&self.speed_history)
                                .height(px(44.)),
                        )
                    })
                    // ── Progress bar ─────────────────────────────────────
                    .when(is_active || is_done, |el| {
                        el.child(Progress::new().value(progress))
                    })
                    // ── Stats row ────────────────────────────────────────
                    .child(
                        h_flex()
                            .gap_3()
                            .text_xs()
                            .text_color(muted_fg)
                            // Speed (active only)
                            .when(is_active && self.speed_bps > 0.0, |el| {
                                el.child(div().child(fmt_speed(self.speed_bps)))
                            })
                            // Bytes received / total
                            .child(div().child(match self.total_bytes {
                                Some(total) => format!(
                                    "{} / {}",
                                    fmt_bytes(self.bytes_received),
                                    fmt_bytes(total)
                                ),
                                None => fmt_bytes(self.bytes_received),
                            }))
                            // ETA (active, known size, positive speed)
                            .when_some(
                                eta_str(is_active, self.bytes_received, self.total_bytes, self.speed_bps),
                                |el, eta| el.child(div().child(format!("ETA {}", eta))),
                            )
                            // Done / error messages
                            .when(is_done, |el| {
                                el.child(div().text_color(green).child("Complete"))
                            })
                            .when(is_error, |el| {
                                let msg = match &self.status {
                                    DownloadItemStatus::Error(m) => m.as_str(),
                                    _ => "",
                                };
                                el.child(div().text_color(gpui::red()).child(msg.to_string()))
                            }),
                    )
                    // ── Open Folder button (Done items with known path) ──
                    .when_some(
                        if is_done { self.path.clone() } else { None },
                        |el, path| {
                            el.child(
                                h_flex().justify_end()
                                    .child(
                                        Button::new(SharedString::from(format!("open-folder-{}", self.id)))
                                            .label("📂 Open Folder")
                                            .with_size(crate::Size::Small)
                                            .on_click(move |_, _, _cx| {
                                                reveal_in_file_manager(&path);
                                            }),
                                    ),
                            )
                        },
                    ),
            )
    }
}

/// Open the parent folder of `path` in the system file manager.
pub fn reveal_in_file_manager(path: &std::path::Path) {
    let folder = path.parent().unwrap_or(path);
    #[cfg(target_os = "windows")]
    {
        // Use /select to highlight the file; fall back to opening the folder.
        let _ = std::process::Command::new("explorer")
            .args(["/select,", &path.to_string_lossy()])
            .spawn()
            .or_else(|_| std::process::Command::new("explorer").arg(folder).spawn());
    }
    #[cfg(target_os = "macos")]
    {
        // `open -R` reveals (selects) the file in Finder.
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .or_else(|_| std::process::Command::new("open").arg(folder).spawn());
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(folder).spawn();
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn eta_str(
    is_active: bool,
    bytes_received: u64,
    total_bytes: Option<u64>,
    speed_bps: f64,
) -> Option<String> {
    if !is_active || speed_bps <= 0.0 {
        return None;
    }
    let total = total_bytes?;
    if total <= bytes_received {
        return None;
    }
    let remaining = (total - bytes_received) as f64;
    let secs = remaining / speed_bps;
    Some(if secs < 60.0 {
        format!("{:.0}s", secs)
    } else if secs < 3600.0 {
        format!("{:.0}m {:.0}s", secs / 60.0, secs % 60.0)
    } else {
        format!("{:.1}h", secs / 3600.0)
    })
}
