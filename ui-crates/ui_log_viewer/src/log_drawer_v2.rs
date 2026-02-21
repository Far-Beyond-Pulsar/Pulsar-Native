use crate::log_reader::LogReader;
use gpui::{prelude::*, *};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme as _, IconName,
    v_virtual_list, VirtualListScrollHandle,
};

const MAX_LINES_IN_MEMORY: usize = 10000;
const UNLOAD_THRESHOLD: usize = 1000; // Drop 1k lines when we exceed max
const POLL_INTERVAL_MS: u64 = 100;
const LINE_HEIGHT: Pixels = px(20.0);
const MIN_FILTERED_LINES: usize = 500; // Trigger load if filtered view has fewer than this
const LOAD_CHUNK_SIZE: usize = 2000; // Load this many lines at a time

/// Message sent from background task to UI
enum LogUpdate {
    InitialLoad(Vec<String>, PathBuf),
    NewLines(Vec<String>),
    OlderLines(Vec<String>, usize), // Lines and the starting index
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Unknown,
}

impl LogLevel {
    fn from_line(line: &str) -> Self {
        let upper = line.to_uppercase();
        if upper.contains("ERROR") || upper.contains("ERR") {
            LogLevel::Error
        } else if upper.contains("WARN") {
            LogLevel::Warn
        } else if upper.contains("INFO") {
            LogLevel::Info
        } else if upper.contains("DEBUG") {
            LogLevel::Debug
        } else if upper.contains("TRACE") {
            LogLevel::Trace
        } else {
            LogLevel::Unknown
        }
    }

    fn color(&self, theme: &ui::Theme) -> Hsla {
        match self {
            LogLevel::Error => theme.danger,
            LogLevel::Warn => theme.warning,
            LogLevel::Info => theme.info,
            LogLevel::Debug => theme.muted_foreground,
            LogLevel::Trace => theme.muted_foreground.opacity(0.7),
            LogLevel::Unknown => theme.foreground,
        }
    }
}

pub struct LogDrawer {
    /// All log lines currently in memory (sliding window)
    lines: Vec<String>,
    /// Starting line number of the first line in memory (1-indexed)
    start_line_num: usize,
    /// Total lines in the file
    total_lines: usize,
    /// Filtered view of lines (indices into lines array)
    filtered_indices: Vec<usize>,
    /// Whether we're locked to the bottom (auto-scroll)
    locked_to_bottom: bool,
    /// Error message if any
    error_message: Option<String>,
    /// Channel to receive updates from background task
    update_receiver: Option<smol::channel::Receiver<LogUpdate>>,
    /// Background task handle
    _background_task: Option<Task<()>>,
    /// Entity reference for virtual list
    entity: Option<Entity<Self>>,
    /// Scroll handle
    scroll_handle: VirtualListScrollHandle,
    /// Search query
    search_query: String,
    /// Active log level filter
    level_filter: Option<LogLevel>,
    /// Path to the log file
    log_path: Option<PathBuf>,
    /// Whether we're currently loading older lines
    loading_older: bool,
    /// Minimum filtered lines to keep in memory
    min_filtered_lines: usize,
}

impl LogDrawer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let scroll_handle = VirtualListScrollHandle::new();
        let mut drawer = Self {
            lines: Vec::new(),
            start_line_num: 1,
            total_lines: 0,
            filtered_indices: Vec::new(),
            locked_to_bottom: true,
            error_message: None,
            update_receiver: None,
            _background_task: None,
            entity: None,
            scroll_handle,
            search_query: String::new(),
            level_filter: None,
            log_path: None,
            loading_older: false,
            min_filtered_lines: MIN_FILTERED_LINES,
        };
        
        // Store entity reference
        drawer.entity = Some(cx.entity().clone());
        
        drawer
    }
    
    /// Update filtered indices based on search and level filter
    fn update_filter(&mut self) {
        self.filtered_indices.clear();
        
        let search_lower = self.search_query.to_lowercase();
        let has_search = !search_lower.is_empty();
        
        for (idx, line) in self.lines.iter().enumerate() {
            // Check level filter
            if let Some(filter_level) = self.level_filter {
                if LogLevel::from_line(line) != filter_level {
                    continue;
                }
            }
            
            // Check search query
            if has_search && !line.to_lowercase().contains(&search_lower) {
                continue;
            }
            
            self.filtered_indices.push(idx);
        }
    }
    
    fn set_level_filter(&mut self, level: Option<LogLevel>, cx: &mut Context<Self>) {
        self.level_filter = level;
        self.update_filter();
        self.check_and_load_more(cx);
        cx.notify();
    }
    
    fn set_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.search_query = query;
        self.update_filter();
        cx.notify();
    }
    
    fn clear_logs(&mut self, cx: &mut Context<Self>) {
        self.lines.clear();
        self.filtered_indices.clear();
        self.start_line_num = 1;
        self.total_lines = 0;
        cx.notify();
    }
    
    /// Start monitoring the log file (called when drawer opens)
    pub fn start_monitoring(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = smol::channel::bounded(100);
        self.update_receiver = Some(rx.clone());
        
        // Spawn UI update task
        cx.spawn(async move |this, cx| {
            while let Ok(update) = rx.recv().await {
                let _ = cx.update(|cx| {
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |drawer, cx| {
                            drawer.handle_update(update, cx);
                        });
                    }
                });
            }
        }).detach();
        
        // Spawn background file monitoring task
        let task = cx.background_executor().spawn(async move {
            Self::background_file_monitor(tx).await;
        });
        
        self._background_task = Some(task);
    }
    
    /// Stop monitoring (called when drawer closes)
    pub fn stop_monitoring(&mut self) {
        self.update_receiver = None;
        self._background_task = None;
    }
    
    /// Background task that monitors the log file and sends updates
    async fn background_file_monitor(tx: smol::channel::Sender<LogUpdate>) {
        use std::fs::File;
        use std::io::{BufRead, BufReader, Seek, SeekFrom};
        
        // Get log file path
        let log_path = match LogReader::get_latest_log_path() {
            Ok(path) => path,
            Err(e) => {
                let _ = tx.send(LogUpdate::Error(format!("Failed to find log file: {}", e))).await;
                return;
            }
        };
        
        // Open file
        let mut file = match File::open(&log_path) {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.send(LogUpdate::Error(format!("Failed to open log file: {}", e))).await;
                return;
            }
        };
        
        // Read initial content
        let mut reader = BufReader::new(&file);
        let mut initial_lines = Vec::new();
        let mut line = String::new();
        
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    initial_lines.push(line.trim_end().to_string());
                }
                Err(e) => {
                    let _ = tx.send(LogUpdate::Error(format!("Failed to read log: {}", e))).await;
                    return;
                }
            }
        }
        
        // Send initial load with path
        if tx.send(LogUpdate::InitialLoad(initial_lines, log_path.clone())).await.is_err() {
            return;
        }
        
        let mut last_position = match file.seek(SeekFrom::Current(0)) {
            Ok(pos) => pos,
            Err(_) => return,
        };
        
        // Poll for new lines
        loop {
            smol::Timer::after(Duration::from_millis(POLL_INTERVAL_MS)).await;
            
            // Check file size
            let current_size = match file.metadata() {
                Ok(meta) => meta.len(),
                Err(_) => continue,
            };
            
            // If file was truncated, restart
            if current_size < last_position {
                if file.seek(SeekFrom::Start(0)).is_err() {
                    continue;
                }
                last_position = 0;
            }
            
            // If no new data, continue
            if current_size == last_position {
                continue;
            }
            
            // Seek to last position
            if file.seek(SeekFrom::Start(last_position)).is_err() {
                continue;
            }
            
            // Read new lines
            let mut reader = BufReader::new(&file);
            let mut new_lines = Vec::new();
            line.clear();
            
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        new_lines.push(line.trim_end().to_string());
                    }
                    Err(_) => break,
                }
            }
            
            // Update position
            if let Ok(pos) = file.seek(SeekFrom::Current(0)) {
                last_position = pos;
            }
            
            // Send updates (non-blocking)
            if !new_lines.is_empty() {
                if tx.try_send(LogUpdate::NewLines(new_lines)).is_err() {
                    break;
                }
            }
        }
    }
    
    /// Handle updates from background task
    fn handle_update(&mut self, update: LogUpdate, cx: &mut Context<Self>) {
        match update {
            LogUpdate::InitialLoad(lines, path) => {
                self.log_path = Some(path);
                self.total_lines = lines.len();
                self.start_line_num = if lines.len() > MAX_LINES_IN_MEMORY {
                    lines.len() - MAX_LINES_IN_MEMORY + 1
                } else {
                    1
                };
                
                // Only keep the last MAX_LINES_IN_MEMORY lines
                if lines.len() > MAX_LINES_IN_MEMORY {
                    self.lines = lines[lines.len() - MAX_LINES_IN_MEMORY..].to_vec();
                } else {
                    self.lines = lines;
                }
                
                self.error_message = None;
                self.update_filter();
                
                // Check if we need to load more due to filtering
                self.check_and_load_more(cx);
                
                cx.notify();
                
                // Scroll to bottom after a brief delay
                if self.locked_to_bottom {
                    cx.spawn(async move |this, cx| {
                        smol::Timer::after(Duration::from_millis(50)).await;
                        let _ = cx.update(|cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |drawer, cx| {
                                    drawer.scroll_to_bottom();
                                    cx.notify();
                                });
                            }
                        });
                    }).detach();
                }
            }
            LogUpdate::NewLines(new_lines) => {
                if new_lines.is_empty() {
                    return;
                }
                
                self.total_lines += new_lines.len();
                self.lines.extend(new_lines);
                
                // Sliding window: drop old lines if we exceed max + threshold
                if self.lines.len() > MAX_LINES_IN_MEMORY + UNLOAD_THRESHOLD {
                    let drop_count = UNLOAD_THRESHOLD;
                    self.lines.drain(0..drop_count);
                    self.start_line_num += drop_count;
                }
                
                self.update_filter();
                
                // Check if we need to load more due to filtering
                self.check_and_load_more(cx);
                
                cx.notify();
                
                // Auto-scroll if locked
                if self.locked_to_bottom {
                    self.scroll_to_bottom();
                }
            }
            LogUpdate::OlderLines(older_lines, start_idx) => {
                // Prepend older lines
                self.start_line_num = start_idx + 1;
                let mut new_lines = older_lines;
                new_lines.extend(self.lines.drain(..));
                self.lines = new_lines;
                
                // Trim from the end if too large
                if self.lines.len() > MAX_LINES_IN_MEMORY + UNLOAD_THRESHOLD {
                    let new_len = MAX_LINES_IN_MEMORY;
                    self.lines.truncate(new_len);
                }
                
                self.loading_older = false;
                self.update_filter();
                cx.notify();
                
                // Check if we need even more lines after loading
                self.check_and_load_more(cx);
            }
            LogUpdate::Error(msg) => {
                self.error_message = Some(msg);
                cx.notify();
            }
        }
    }
    
    /// Check if we need to load more lines (either from top or to fill filtered view)
    fn check_and_load_more(&mut self, cx: &mut Context<Self>) {
        // Don't load if already loading
        if self.loading_older {
            return;
        }
        
        // If filtered view is too small and we can load from the beginning
        if self.filtered_indices.len() < self.min_filtered_lines && self.start_line_num > 1 {
            self.load_older_lines(cx);
        }
    }
    
    /// Trigger loading older lines from disk
    fn load_older_lines(&mut self, cx: &mut Context<Self>) {
        if self.loading_older || self.start_line_num <= 1 {
            return;
        }
        
        let Some(log_path) = self.log_path.clone() else {
            return;
        };
        
        self.loading_older = true;
        
        let start_line = self.start_line_num;
        let update_tx = self.update_receiver.as_ref().and_then(|rx| {
            // Get the sender from the receiver (we need to store it separately or reconstruct)
            // For now, we'll create a new channel for this operation
            None as Option<smol::channel::Sender<LogUpdate>>
        });
        
        // We need to send this back somehow - let's use a separate spawn that updates directly
        cx.spawn(async move |this, cx| {
            // Load older lines from disk
            let result = smol::unblock(move || {
                let file = std::fs::File::open(&log_path).ok()?;
                let reader = BufReader::new(file);
                
                // Calculate which lines to read
                let end_line = start_line - 1;
                let chunk_start = if end_line > LOAD_CHUNK_SIZE {
                    end_line - LOAD_CHUNK_SIZE
                } else {
                    0
                };
                
                // Read the chunk
                let mut lines: Vec<String> = Vec::new();
                for (idx, line_result) in reader.lines().enumerate() {
                    if idx >= chunk_start && idx < end_line {
                        if let Ok(line_str) = line_result {
                            lines.push(line_str.trim_end().to_string());
                        }
                    }
                    if idx >= end_line {
                        break;
                    }
                }
                
                Some((lines, chunk_start))
            }).await;
            
            // Update the drawer directly
            let _ = cx.update(|cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |drawer, cx| {
                        if let Some((lines, start_idx)) = result {
                            drawer.handle_update(LogUpdate::OlderLines(lines, start_idx), cx);
                        } else {
                            drawer.loading_older = false;
                        }
                    });
                }
            });
        }).detach();
    }
    
    fn scroll_to_bottom(&mut self) {
        let count = if self.search_query.is_empty() && self.level_filter.is_none() {
            self.lines.len()
        } else {
            self.filtered_indices.len()
        };
        
        if count > 0 {
            self.scroll_handle.scroll_to_item(count - 1, ScrollStrategy::Bottom);
        }
    }
    
    fn jump_to_latest(&mut self, _event: &gpui::ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.locked_to_bottom = true;
        self.scroll_to_bottom();
        cx.notify();
    }
    
    fn handle_scroll(&mut self, event: &gpui::ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // Only detach if scrolling up (pixel delta on Windows, or check both)
        let scrolling_up = match event.delta {
            ScrollDelta::Pixels(delta) => delta.y > px(0.0),
            ScrollDelta::Lines(delta) => delta.y > 0.0,
        };
        
        if scrolling_up && self.locked_to_bottom {
            self.locked_to_bottom = false;
            cx.notify();
        }
        
        // Check if scrolled near top and need to load older lines
        if scrolling_up {
            let (current_scroll_item, _pixels) = self.scroll_handle.logical_scroll_top();
            // If scrolled to within 100 items of the top, load more
            if current_scroll_item < 100 && self.start_line_num > 1 {
                self.load_older_lines(cx);
            }
        }
        
        // TODO: Check if scrolled to actual bottom and re-lock
    }
}

impl Render for LogDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = self.entity.clone().unwrap();
        let theme = cx.theme().clone();
        
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                // Toolbar with controls
                h_flex()
                    .w_full()
                    .h(px(44.0))
                    .px_4()
                    .items_center()
                    .justify_between()
                    .bg(theme.background)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} / {} lines",
                                if self.search_query.is_empty() && self.level_filter.is_none() {
                                    self.lines.len()
                                } else {
                                    self.filtered_indices.len()
                                },
                                self.total_lines
                            ))
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new("clear-logs")
                                    .label("Clear")
                                    .icon(IconName::Trash)
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.clear_logs(cx);
                                    }))
                            )
                            .when(!self.locked_to_bottom, |this| {
                                this.child(
                                    Button::new("jump-to-latest")
                                        .label("Jump to Latest")
                                        .icon(IconName::ChevronDown)
                                        .on_click(cx.listener(Self::jump_to_latest))
                                )
                            })
                    )
            )
            .child(
                // Filter bar
                v_flex()
                    .w_full()
                    .bg(theme.background)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        // Search and filter bar
                        h_flex()
                            .h(px(44.0))
                            .px_4()
                            .gap_2()
                            .items_center()
                            .child(
                                // Search input placeholder
                                div()
                                    .flex_1()
                                    .h(px(32.0))
                                    .px_3()
                                    .border_1()
                                    .border_color(theme.border)
                                    .rounded(px(4.0))
                                    .bg(theme.background)
                                    .text_color(theme.muted_foreground)
                                    .items_center()
                                    .child("🔍 Search logs... (coming soon)")
                            )
                            .child(
                                // Filter buttons
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("filter-all")
                                            .label("All")
                                            .when(self.level_filter.is_none(), |btn| btn.primary())
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.set_level_filter(None, cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("filter-error")
                                            .label("Errors")
                                            .when(self.level_filter == Some(LogLevel::Error), |btn| btn.primary())
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.set_level_filter(Some(LogLevel::Error), cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("filter-warn")
                                            .label("Warnings")
                                            .when(self.level_filter == Some(LogLevel::Warn), |btn| btn.primary())
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.set_level_filter(Some(LogLevel::Warn), cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("filter-info")
                                            .label("Info")
                                            .when(self.level_filter == Some(LogLevel::Info), |btn| btn.primary())
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.set_level_filter(Some(LogLevel::Info), cx);
                                            }))
                                    )
                            )
                    )
            )
            .child(
                // Log content with scroll detection
                div()
                    .flex_1()
                    .w_full()
                    .on_scroll_wheel(cx.listener(Self::handle_scroll))
                    .map(|this| {
                        if let Some(ref error) = self.error_message {
                            this.child(
                                v_flex()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .text_color(theme.muted_foreground)
                                            .child(error.clone())
                                    )
                            )
                        } else if self.lines.is_empty() {
                            this.child(
                                v_flex()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .text_color(theme.muted_foreground)
                                            .child("No logs yet...")
                                    )
                            )
                        } else {
                            // Determine what to show: filtered or all lines
                            let show_filtered = !self.search_query.is_empty() || self.level_filter.is_some();
                            let line_count = if show_filtered {
                                self.filtered_indices.len()
                            } else {
                                self.lines.len()
                            };
                            
                            if line_count == 0 {
                                return this.child(
                                    v_flex()
                                        .size_full()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .text_color(theme.muted_foreground)
                                                .child("No matching logs")
                                        )
                                );
                            }
                            
                            let item_sizes = (0..line_count)
                                .map(|_| Size {
                                    width: px(1000.0),
                                    height: LINE_HEIGHT,
                                })
                                .collect::<Vec<_>>();
                            let item_sizes = std::rc::Rc::new(item_sizes);
                            
                            let search_query = self.search_query.clone();
                            let filtered_indices = self.filtered_indices.clone();
                            
                            this.child(
                                v_virtual_list(
                                    entity,
                                    "log-lines",
                                    item_sizes,
                                    move |_view, visible_range, _window, cx| {
                                        let theme = cx.theme().clone();
                                        let lines_to_show: Vec<(usize, &String)> = if show_filtered {
                                            visible_range.clone()
                                                .filter_map(|idx| {
                                                    filtered_indices.get(idx).and_then(|&line_idx| {
                                                        _view.lines.get(line_idx).map(|line| (idx, line))
                                                    })
                                                })
                                                .collect()
                                        } else {
                                            visible_range.clone()
                                                .filter_map(|idx| {
                                                    _view.lines.get(idx).map(|line| (idx, line))
                                                })
                                                .collect()
                                        };
                                        
                                        lines_to_show
                                            .into_iter()
                                            .map(|(idx, line)| {
                                                // Map memory index to absolute line number
                                                let abs_line_num = if show_filtered {
                                                    // For filtered, idx is into filtered_indices
                                                    if let Some(&mem_idx) = filtered_indices.get(idx) {
                                                        _view.start_line_num + mem_idx
                                                    } else {
                                                        idx + 1
                                                    }
                                                } else {
                                                    _view.start_line_num + idx
                                                };
                                                Self::render_log_line(abs_line_num, line, &search_query, &theme)
                                            })
                                            .collect()
                                    },
                                ).track_scroll(&self.scroll_handle)
                            )
                        }
                    })
            )
    }
}

impl LogDrawer {
    fn render_log_line(line_num: usize, content: &str, search_query: &str, theme: &ui::Theme) -> impl IntoElement {
        let level = LogLevel::from_line(content);
        let level_color = level.color(theme);
        
        h_flex()
            .w_full()
            .h(LINE_HEIGHT)
            .items_center()
            .px_2()
            .gap_2()
            .child(
                // Level indicator dot
                div()
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded(px(4.0))
                    .bg(level_color)
            )
            .child(
                // Line number
                div()
                    .w(px(50.0))
                    .text_color(theme.muted_foreground)
                    .child(format!("{}", line_num))
            )
            .child(
                // Content with optional search highlighting
                div()
                    .flex_1()
                    .text_color(level_color)
                    .when(!search_query.is_empty(), |this| {
                        // Highlight search terms
                        let lower_content = content.to_lowercase();
                        let lower_search = search_query.to_lowercase();
                        
                        if let Some(pos) = lower_content.find(&lower_search) {
                            this.child(
                                h_flex()
                                    .gap_0()
                                    .child(content[..pos].to_string())
                                    .child(
                                        div()
                                            .bg(theme.warning.opacity(0.3))
                                            .px_1()
                                            .child(content[pos..pos + search_query.len()].to_string())
                                    )
                                    .child(content[pos + search_query.len()..].to_string())
                            )
                        } else {
                            this.child(content.to_string())
                        }
                    })
                    .when(search_query.is_empty(), |this| {
                        this.child(content.to_string())
                    })
            )
    }
}
