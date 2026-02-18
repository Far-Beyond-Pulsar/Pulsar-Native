//! Log Viewer Drawer component

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, Icon, IconName, StyledExt, Sizable as _,
};
use notify::{Watcher, RecursiveMode, Event as NotifyEvent};

use crate::log_reader::{LogLine, LogReader};
use crate::virtual_table::{render_virtual_log_table, VirtualScrollState};

actions!(log_viewer, [ToggleLogViewer]);

pub struct LogViewerDrawer {
    log_reader: Option<LogReader>,
    lines_cache: Vec<LogLine>,
    scroll_state: VirtualScrollState,
    auto_scroll: bool,
    is_locked_to_bottom: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
    _watcher: Option<notify::RecommendedWatcher>,
    load_task: Option<Task<()>>,
}

const BUFFER_SIZE: usize = 5000; // Keep 5k lines above and below
const LOAD_CHUNK_SIZE: usize = 1000; // Load 1k lines at a time

impl LogViewerDrawer {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut drawer = Self {
            log_reader: None,
            lines_cache: Vec::new(),
            scroll_state: VirtualScrollState::new(),
            auto_scroll: true,
            is_locked_to_bottom: true,
            error: None,
            focus_handle: cx.focus_handle(),
            _watcher: None,
            load_task: None,
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
                        self.scroll_state.total_lines = reader.total_lines();
                        
                        // Start file watcher
                        self.start_file_watcher(path.clone(), cx);
                        
                        self.log_reader = Some(reader);
                        self.error = None;
                        
                        if self.is_locked_to_bottom {
                            self.scroll_to_bottom();
                        }
                        
                        self.reload_visible_lines(cx);
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
    
    fn start_file_watcher(&mut self, log_path: std::path::PathBuf, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel();
        
        match notify::recommended_watcher(move |res: Result<NotifyEvent, notify::Error>| {
            if let Ok(_event) = res {
                let _ = tx.send(());
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
                    while rx.recv().is_ok() {
                        let _ = cx.update(|cx| {
                            let _ = this.update(cx, |drawer, cx| {
                                if let Some(ref mut reader) = drawer.log_reader {
                                    match reader.reload() {
                                        Ok(changed) => {
                                            if changed {
                                                let old_total = drawer.scroll_state.total_lines;
                                                drawer.scroll_state.total_lines = reader.total_lines();
                                                
                                                if drawer.is_locked_to_bottom {
                                                    drawer.scroll_to_bottom();
                                                    drawer.reload_visible_lines(cx);
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
    
    fn scroll_to_bottom(&mut self) {
        self.scroll_state.scroll_to_bottom();
        self.is_locked_to_bottom = true;
    }
    
    fn reload_visible_lines(&mut self, cx: &mut Context<Self>) {
        if let Some(ref reader) = self.log_reader {
            // Calculate range to load with buffer
            let viewport_start = self.scroll_state.visible_start;
            let viewport_end = self.scroll_state.visible_end;
            
            // Load buffer above and below
            let buffer_start = viewport_start.saturating_sub(BUFFER_SIZE);
            let buffer_end = (viewport_end + BUFFER_SIZE).min(reader.total_lines());
            
            match reader.read_lines(buffer_start, buffer_end) {
                Ok(lines) => {
                    self.lines_cache = lines;
                    self.scroll_state.cache_start = buffer_start;
                    self.scroll_state.cache_end = buffer_end;
                }
                Err(e) => {
                    tracing::error!("[LOG_VIEWER] Failed to read lines: {}", e);
                }
            }
        }
    }
    
    pub fn on_scroll(&mut self, delta_y: f32, cx: &mut Context<Self>) {
        let _old_offset = self.scroll_state.scroll_offset;
        self.scroll_state.on_scroll(delta_y);
        
        // Check if user scrolled away from bottom
        let at_bottom = self.scroll_state.is_at_bottom();
        if self.is_locked_to_bottom && !at_bottom {
            self.is_locked_to_bottom = false;
            tracing::debug!("[LOG_VIEWER] Unlocked from bottom");
        }
        
        // Check if we need to load more lines
        self.check_and_load_if_needed(cx);
        
        cx.notify();
    }
    
    fn check_and_load_if_needed(&mut self, cx: &mut Context<Self>) {
        let viewport_start = self.scroll_state.visible_start;
        let viewport_end = self.scroll_state.visible_end;
        
        // Check if we're approaching the edge of our cached range
        let needs_load_above = viewport_start < self.scroll_state.cache_start + LOAD_CHUNK_SIZE;
        let needs_load_below = viewport_end > self.scroll_state.cache_end - LOAD_CHUNK_SIZE;
        
        if needs_load_above || needs_load_below {
            self.reload_visible_lines(cx);
        }
    }
    
    fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let log_path = self.log_reader.as_ref()
            .map(|r| r.file_path().display().to_string())
            .unwrap_or_else(|| "No log file loaded".to_string());
        
        let total_lines = self.scroll_state.total_lines;
        let show_jump_to_latest = !self.is_locked_to_bottom;
        
        h_flex()
            .w_full()
            .h(px(56.))
            .px_4()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .when(show_jump_to_latest, |this| {
                this.child(
                    Button::new("jump-to-latest")
                        .label("Jump to Latest")
                        .icon(IconName::ChevronDown)
                        .on_click(cx.listener(|drawer, _event, _window, cx| {
                            drawer.scroll_to_bottom();
                            drawer.reload_visible_lines(cx);
                            cx.notify();
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
                    .icon(if self.is_locked_to_bottom { IconName::Check } else { IconName::Pause })
                    .ghost()
                    .tooltip(if self.is_locked_to_bottom { "Live mode (locked to bottom)" } else { "Static mode (scroll freely)" })
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.is_locked_to_bottom = !drawer.is_locked_to_bottom;
                        if drawer.is_locked_to_bottom {
                            drawer.scroll_to_bottom();
                            drawer.reload_visible_lines(cx);
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
                        drawer.scroll_to_bottom();
                        drawer.reload_visible_lines(cx);
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
                        let drawer_lines = self.lines_cache.clone();
                        let drawer_scroll = self.scroll_state.clone();
                        this.child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .bg(cx.theme().background)
                                .on_scroll_wheel(cx.listener(|drawer, event: &ScrollWheelEvent, _window, cx| {
                                    let delta_y: f32 = event.delta.pixel_delta(px(1.0)).y.into();
                                    drawer.on_scroll(delta_y, cx);
                                }))
                                .child(render_virtual_log_table(&drawer_lines, &drawer_scroll, cx))
                        )
                    })
            )
    }
}


