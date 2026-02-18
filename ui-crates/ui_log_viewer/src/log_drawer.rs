//! Log Viewer Drawer component

use gpui::{prelude::*, *};
use std::time::Duration;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt, Sizable as _,
};

use crate::log_reader::{LogLine, LogReader};
use crate::virtual_table::{render_virtual_log_table, VirtualScrollState};

actions!(log_viewer, [ToggleLogViewer]);

pub struct LogViewerDrawer {
    log_reader: Option<LogReader>,
    lines_cache: Vec<LogLine>,
    scroll_state: VirtualScrollState,
    auto_scroll: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
}

impl LogViewerDrawer {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut drawer = Self {
            log_reader: None,
            lines_cache: Vec::new(),
            scroll_state: VirtualScrollState::new(),
            auto_scroll: true,
            error: None,
            focus_handle: cx.focus_handle(),
        };
        
        drawer.load_latest_log(cx);
        
        drawer
    }
    
    fn load_latest_log(&mut self, cx: &mut Context<Self>) {
        match LogReader::get_latest_log_path() {
            Ok(path) => {
                match LogReader::new(&path) {
                    Ok(reader) => {
                        tracing::info!("[LOG_VIEWER] Loaded log file: {}", path.display());
                        self.scroll_state.total_lines = reader.total_lines();
                        self.log_reader = Some(reader);
                        self.error = None;
                        
                        if self.auto_scroll {
                            self.scroll_state.scroll_to_bottom();
                        }
                        
                        self.reload_visible_lines();
                        cx.notify();
                    }
                    Err(e) => {
                        self.error = Some(format!("Failed to load log file: {}", e));
                        tracing::error!("[LOG_VIEWER] {}", self.error.as_ref().unwrap());
                    }
                }
            }
            Err(e) => {
                self.error = Some(format!("Failed to find log file: {}", e));
                tracing::error!("[LOG_VIEWER] {}", self.error.as_ref().unwrap());
            }
        }
    }
    
    fn reload_visible_lines(&mut self) {
        if let Some(ref reader) = self.log_reader {
            match reader.read_lines(self.scroll_state.visible_start, self.scroll_state.visible_end) {
                Ok(lines) => {
                    self.lines_cache = lines;
                }
                Err(e) => {
                    tracing::error!("[LOG_VIEWER] Failed to read lines: {}", e);
                }
            }
        }
    }
    
    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let log_path = self.log_reader.as_ref()
            .map(|r| r.file_path().display().to_string())
            .unwrap_or_else(|| "No log file loaded".to_string());
        
        let total_lines = self.scroll_state.total_lines;
        
        h_flex()
            .w_full()
            .h(px(56.))
            .px_4()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                div()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(cx.theme().muted.opacity(0.3))
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(log_path)
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(cx.theme().accent.opacity(0.1))
                    .border_1()
                    .border_color(cx.theme().accent.opacity(0.3))
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().accent)
                    .child(format!("{} lines", total_lines))
            )
            .child(ui::divider::Divider::vertical().h(px(24.)))
            .child(
                Button::new("toggle-auto-scroll")
                    .icon(if self.auto_scroll { IconName::ArrowDown } else { IconName::Pause })
                    .ghost()
                    .tooltip(if self.auto_scroll { "Auto-scroll enabled" } else { "Auto-scroll disabled" })
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.auto_scroll = !drawer.auto_scroll;
                        if drawer.auto_scroll {
                            drawer.scroll_state.scroll_to_bottom();
                            drawer.reload_visible_lines();
                        }
                        cx.notify();
                    }))
            )
            .child(
                Button::new("scroll-to-bottom")
                    .icon(IconName::ChevronDown)
                    .ghost()
                    .tooltip("Scroll to bottom")
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.scroll_state.scroll_to_bottom();
                        drawer.reload_visible_lines();
                        cx.notify();
                    }))
            )
            .child(
                Button::new("refresh-log")
                    .icon(IconName::Refresh)
                    .ghost()
                    .tooltip("Reload log file")
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.load_latest_log(cx);
                    }))
            )
            .child(ui::divider::Divider::vertical().h(px(24.)))
            .child(
                Button::new("open-log-folder")
                    .icon(IconName::FolderOpen)
                    .ghost()
                    .tooltip("Open logs folder")
                    .on_click(cx.listener(|drawer, _event, _window, _cx| {
                        if let Some(ref reader) = drawer.log_reader {
                            if let Some(parent) = reader.file_path().parent() {
                                #[cfg(target_os = "windows")]
                                let _ = std::process::Command::new("explorer")
                                    .arg(parent)
                                    .spawn();
                                #[cfg(target_os = "macos")]
                                let _ = std::process::Command::new("open")
                                    .arg(parent)
                                    .spawn();
                                #[cfg(target_os = "linux")]
                                let _ = std::process::Command::new("xdg-open")
                                    .arg(parent)
                                    .spawn();
                            }
                        }
                    }))
            )
    }
}

impl Render for LogViewerDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when(self.error.is_some(), |this| {
                        let error = self.error.clone().unwrap();
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap_4()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(48.))
                                        .text_color(cx.theme().danger)
                                )
                                .child(
                                    div()
                                        .text_lg()
                                        .font_medium()
                                        .text_color(cx.theme().danger)
                                        .child("Failed to load logs")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(error)
                                )
                                .child(
                                    Button::new("retry-load")
                                        .label("Retry")
                                        .on_click(cx.listener(|drawer, _event, _window, cx| {
                                            drawer.load_latest_log(cx);
                                        }))
                                )
                        )
                    })
                    .when(self.error.is_none(), |this| {
                        this.child(render_virtual_log_table(&self.lines_cache, &self.scroll_state, cx))
                    })
            )
    }
}
