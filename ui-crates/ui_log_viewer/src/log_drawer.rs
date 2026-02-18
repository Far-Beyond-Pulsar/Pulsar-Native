//! Log Viewer Drawer component

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt, Sizable as _,
};
use notify::{Watcher, RecursiveMode, Event as NotifyEvent};

use crate::log_reader::LogReader;
use crate::virtual_table::{render_virtual_log_table, LogLine, LogTableState, VirtualScrollState};

actions!(log_viewer, [ToggleLogViewer]);

pub struct LogViewerDrawer {
    log_reader: Option<LogReader>,
    table_state: LogTableState,
    scroll_state: VirtualScrollState,
    error: Option<String>,
    focus_handle: FocusHandle,
    _watcher: Option<notify::RecommendedWatcher>,
    entity: Option<Entity<Self>>,
}

const CHUNK_SIZE: usize = 1000; // Load 1k lines at a time when scrolling
const MAX_LINES_IN_MEMORY: usize = 10000; // Keep max 10k lines in memory

impl LogViewerDrawer {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let entity = cx.entity().clone();
        let mut drawer = Self {
            log_reader: None,
            table_state: LogTableState::new(),
            scroll_state: VirtualScrollState::new(),
            error: None,
            focus_handle: cx.focus_handle(),
            _watcher: None,
            entity: Some(entity),
        };
        
        drawer.load_latest_log(cx);
        
        drawer
    }
    
    fn load_latest_log(&mut self, cx: &mut Context<Self>) {
        match LogReader::get_latest_log_path() {
            Ok(path) => {
                match LogReader::new(&path) {
                    Ok(reader) => {
                        tracing::info!("[LOG_VIEWER] Loaded log file: {} ({} lines)", 
                            path.display(), reader.total_lines());
                        
                        let total_lines = reader.total_lines();
                        self.scroll_state.total_lines = total_lines;
                        
                        // Start file watcher
                        self.start_file_watcher(path.clone(), cx);
                        
                        self.log_reader = Some(reader);
                        self.error = None;
                        
                        // Load initial lines
                        self.load_visible_lines(cx);
                        
                        // Scroll to bottom if locked
                        if self.scroll_state.is_locked_to_bottom {
                            self.scroll_to_bottom(cx);
                        }
                        
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
    
    fn load_visible_lines(&mut self, _cx: &mut Context<Self>) {
        if let Some(ref reader) = self.log_reader {
            let total = reader.total_lines();
            if total == 0 {
                return;
            }
            
            // Load the last N lines (or all if less than max)
            let start = total.saturating_sub(MAX_LINES_IN_MEMORY);
            let end = total;
            
            match reader.read_lines(start, end) {
                Ok(lines) => {
                    let log_lines: Vec<LogLine> = lines.into_iter().map(|line| LogLine {
                        line_number: line.line_number,
                        content: line.content,
                    }).collect();
                    
                    self.table_state.update_lines(log_lines);
                    tracing::debug!("[LOG_VIEWER] Loaded {} lines ({}..{})", end - start, start, end);
                }
                Err(e) => {
                    tracing::error!("[LOG_VIEWER] Failed to read lines: {}", e);
                }
            }
        }
    }
    
    fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.scroll_state.scroll_to_bottom();
        self.table_state.scroll_to_bottom();
        cx.notify();
    }
    
    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.scroll_state.is_locked_to_bottom = true;
        self.scroll_to_bottom(cx);
    }
    
    fn start_file_watcher(&mut self, log_path: std::path::PathBuf, cx: &mut Context<Self>) {
        let (tx, rx) = smol::channel::unbounded();
        
        match notify::recommended_watcher(move |res: Result<NotifyEvent, notify::Error>| {
            if let Ok(_event) = res {
                let _ = tx.send_blocking(());
            }
        }) {
            Ok(mut watcher) => {
                if let Err(e) = watcher.watch(&log_path, RecursiveMode::NonRecursive) {
                    tracing::error!("[LOG_VIEWER] Failed to watch log file: {}", e);
                    return;
                }
                
                self._watcher = Some(watcher);
                
                // Spawn task to handle file change notifications
                cx.spawn(async move |this, mut cx| {
                    while rx.recv().await.is_ok() {
                        let _ = cx.update(|cx| {
                            let _ = this.update(cx, |drawer, cx| {
                                if let Some(ref mut reader) = drawer.log_reader {
                                    match reader.reload() {
                                        Ok(changed) => {
                                            if changed {
                                                let old_total = drawer.scroll_state.total_lines;
                                                drawer.scroll_state.total_lines = reader.total_lines();
                                                
                                                // Reload lines from disk
                                                drawer.load_visible_lines(cx);
                                                
                                                if drawer.scroll_state.is_locked_to_bottom {
                                                    drawer.scroll_to_bottom(cx);
                                                }
                                                cx.notify();
                                                
                                                tracing::debug!("[LOG_VIEWER] New logs: {} -> {} lines", 
                                                    old_total, drawer.scroll_state.total_lines);
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!("[LOG_VIEWER] Failed to reload: {}", e);
                                        }
                                    }
                                }
                            });
                        });
                    }
                }).detach();
            }
            Err(e) => {
                tracing::error!("[LOG_VIEWER] Failed to create file watcher: {}", e);
            }
        }
    }
    
    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let log_path = self.log_reader.as_ref()
            .map(|r| r.file_path().display().to_string())
            .unwrap_or_else(|| "No log file loaded".to_string());
        
        let total_lines = self.scroll_state.total_lines;
        let is_locked_to_bottom = self.scroll_state.is_locked_to_bottom;
        
        h_flex()
            .w_full()
            .h(px(56.))
            .px_4()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .when(!is_locked_to_bottom, |this| {
                this.child(
                    Button::new("jump-to-latest")
                        .label("Jump to Latest")
                        .icon(IconName::ChevronDown)
                        .on_click(cx.listener(|drawer, _event, _window, cx| {
                            drawer.jump_to_latest(cx);
                        }))
                )
            })
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
                    .overflow_hidden()
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
                    .icon(if is_locked_to_bottom { IconName::Check } else { IconName::Pause })
                    .ghost()
                    .tooltip(if is_locked_to_bottom { "Live mode (locked to bottom)" } else { "Static mode (scroll freely)" })
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.scroll_state.is_locked_to_bottom = !drawer.scroll_state.is_locked_to_bottom;
                        if drawer.scroll_state.is_locked_to_bottom {
                            drawer.jump_to_latest(cx);
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
                        drawer.scroll_to_bottom(cx);
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                        if let Some(entity) = self.entity.as_ref() {
                            this.child(render_virtual_log_table(entity.clone(), &self.table_state, cx))
                        } else {
                            this
                        }
                    })
            )
    }
}


