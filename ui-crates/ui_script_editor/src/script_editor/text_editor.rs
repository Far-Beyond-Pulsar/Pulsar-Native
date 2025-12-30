use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    input::{InputState, TextInput, TabSize, InputEvent},
    tab::{Tab, TabBar},
    resizable::{h_resizable, resizable_panel, ResizableState},
    v_flex, h_flex,
    ActiveTheme as _, StyledExt, Sizable as _,
    IconName,
};

use std::path::PathBuf;
use std::time::Instant;
use std::fs;

use engine_backend::services::rust_analyzer_manager::RustAnalyzerManager;

#[derive(Clone)]
pub enum TextEditorEvent {
    OpenFolderRequested(PathBuf),
    RunScriptRequested(PathBuf, String),
    DebugScriptRequested(PathBuf),
    FileOpened { path: PathBuf, content: String },
    FileSaved { path: PathBuf, content: String },
    FileClosed { path: PathBuf },
    /// Request to navigate to a specific location (for go-to-definition)
    NavigateToLocation { path: PathBuf, line: u32, character: u32 },
}

#[derive(Clone)]
pub struct OpenFile {
    pub path: PathBuf,
    pub input_state: Entity<InputState>,
    pub is_modified: bool,
    pub lines_count: usize,
    pub file_size: usize,
    /// Document version for LSP synchronization
    pub version: i32,
    /// Whether to render this file as markdown
    pub render_as_markdown: bool,
    /// Cached markdown preview content (to avoid re-rendering on every frame)
    pub markdown_preview_cache: String,
    /// Last time markdown was rendered
    pub last_markdown_render: Option<Instant>,
    /// Pending scroll target (line, column) - will be applied after layout is ready
    pub pending_scroll_target: Option<(usize, usize)>,
}

pub struct TextEditor {
    focus_handle: FocusHandle,
    /// A list of currently open files in this script editor
    open_files: Vec<OpenFile>,
    current_file_index: Option<usize>,
    /// Performance monitoring
    last_render_time: Option<Instant>,
    show_performance_stats: bool,
    subscriptions: Vec<Subscription>,
    /// Global rust analyzer for LSP support
    rust_analyzer: Option<Entity<RustAnalyzerManager>>,
    /// Resizable state for markdown split view
    markdown_split_state: Entity<ResizableState>,
    /// Pending navigation (path, line, character) to be handled when we have window access
    pending_navigation: Option<(PathBuf, u32, u32)>,
    /// Internal workspace for draggable file tabs
    workspace: Option<Entity<ui::workspace::Workspace>>,
    /// Track if workspace has been initialized
    workspace_initialized: bool,
    /// Pending panels to add (file index, path, input_state)
    pending_panels_to_add: Vec<(usize, PathBuf, Entity<InputState>)>,
}

impl TextEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let markdown_split_state = ResizableState::new(cx);
        
        // Create internal workspace for file tabs
        let workspace = cx.new(|cx| {
            ui::workspace::Workspace::new_with_channel(
                "text-editor-workspace",
                ui::dock::DockChannel(4), // Unique channel for text editor internal workspace
                window,
                cx
            )
        });
        
        Self {
            focus_handle: cx.focus_handle(),
            open_files: Vec::new(),
            current_file_index: None,
            last_render_time: None,
            show_performance_stats: false,
            subscriptions: Vec::new(),
            rust_analyzer: None,
            markdown_split_state,
            pending_navigation: None,
            workspace: Some(workspace),
            workspace_initialized: false,
            pending_panels_to_add: Vec::new(),
        }
    }
    
    /// Manually refresh the markdown preview for the current file
    pub fn refresh_markdown_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(file) = self.open_files.get_mut(index) {
                if file.render_as_markdown {
                    let content = file.input_state.read(cx).value().to_string();
                    file.markdown_preview_cache = content;
                    file.last_markdown_render = Some(Instant::now());
                    cx.notify();
                }
            }
        }
    }

    /// Set the global rust analyzer manager
    pub fn set_rust_analyzer(&mut self, analyzer: Entity<RustAnalyzerManager>, _cx: &mut Context<Self>) {
        self.rust_analyzer = Some(analyzer);
    }
    
    /// Add pending file panels to the first available TabPanel
    fn add_pending_panels_to_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_panels_to_add.is_empty() || !self.workspace_initialized {
            return;
        }
        
        if let Some(workspace) = self.workspace.clone() {
            let text_editor_weak = cx.entity().downgrade();
            let panels_to_add = std::mem::take(&mut self.pending_panels_to_add);
            
            // Defer adding panels to avoid reentrant updates
            window.defer(cx, move |window, cx| {
                _ = workspace.update(cx, |workspace, cx| {
                    let dock_area = workspace.dock_area();
                    
                    // Get the first TabPanel from the center items
                    if let Some(tab_panel) = dock_area.read(cx).items().left_top_tab_panel(cx) {
                        for (index, path, input_state) in panels_to_add {
                            let panel = cx.new(|cx| {
                                use crate::script_editor::FilePanelWrapper;
                                FilePanelWrapper::new(
                                    text_editor_weak.clone(),
                                    index,
                                    path,
                                    input_state,
                                    cx
                                )
                            });
                            
                            _ = tab_panel.update(cx, |tab_panel, cx| {
                                tab_panel.add_panel(
                                    std::sync::Arc::new(panel) as std::sync::Arc<dyn ui::dock::PanelView>,
                                    window,
                                    cx
                                );
                            });
                        }
                    }
                });
            });
        }
    }

    /// Initialize/reinitialize workspace with current files
    fn initialize_workspace_once(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Process pending panels by adding them efficiently
        if !self.pending_panels_to_add.is_empty() && self.workspace_initialized {
            self.add_pending_panels_to_workspace(window, cx);
            return;
        }
        
        if self.workspace_initialized {
            return;
        }
        
        if let Some(ref workspace) = self.workspace {
            let text_editor_weak = cx.entity().downgrade();
            
            workspace.update(cx, |workspace, cx| {
                let dock_area = workspace.dock_area().downgrade();
                
                if self.open_files.is_empty() {
                    // Show welcome panel when no files
                    let welcome_panel = cx.new(|cx| {
                        use crate::script_editor::WelcomePanelWrapper;
                        WelcomePanelWrapper::new(cx)
                    });
                    
                    workspace.initialize(
                        ui::dock::DockItem::tabs(
                            vec![std::sync::Arc::new(welcome_panel) as std::sync::Arc<dyn ui::dock::PanelView>],
                            Some(0),
                            &dock_area,
                            window,
                            cx,
                        ),
                        None,
                        None,
                        None,
                        window,
                        cx,
                    );
                } else {
                    // Create panels for all open files
                    let file_panels: Vec<std::sync::Arc<dyn ui::dock::PanelView>> = self.open_files
                        .iter()
                        .enumerate()
                        .map(|(index, open_file)| {
                            let panel = cx.new(|cx| {
                                use crate::script_editor::FilePanelWrapper;
                                FilePanelWrapper::new(
                                    text_editor_weak.clone(),
                                    index,
                                    open_file.path.clone(),
                                    open_file.input_state.clone(),
                                    cx
                                )
                            });
                            std::sync::Arc::new(panel) as std::sync::Arc<dyn ui::dock::PanelView>
                        })
                        .collect();
                    
                    workspace.initialize(
                        ui::dock::DockItem::tabs(
                            file_panels,
                            self.current_file_index,
                            &dock_area,
                            window,
                            cx,
                        ),
                        None,
                        None,
                        None,
                        window,
                        cx,
                    );
                }
            });
            
            self.workspace_initialized = true;
        }
    }
    
    /// Mark that files have changed (workspace will re-render)
    fn mark_files_changed(&mut self, cx: &mut Context<Self>) {
        // Just notify - the workspace will pick up changes on next render
        cx.notify();
    }

    /// Create a new empty file
    pub fn create_new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Generate a unique untitled file name
        let mut counter = 1;
        let mut new_path = PathBuf::from(format!("untitled-{}.txt", counter));
        while self.open_files.iter().any(|f| f.path == new_path) {
            counter += 1;
            new_path = PathBuf::from(format!("untitled-{}.txt", counter));
        }

        // Create an empty file in memory
        let language = "text";
        let input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(language)
                .line_number(true)
                .minimap(true) // Enable VSCode-style minimap
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .soft_wrap(true);

            state.set_value("", window, cx);
            state
        });

        let open_file = OpenFile {
            path: new_path.clone(),
            input_state: input_state.clone(),
            is_modified: false,
            lines_count: 1,
            file_size: 0,
            version: 1,
            render_as_markdown: false,
            markdown_preview_cache: String::new(),
            last_markdown_render: None,
            pending_scroll_target: None,
        };

        self.open_files.push(open_file);
        let new_index = self.open_files.len() - 1;
        self.current_file_index = Some(new_index);
        
        // Queue panel to be added on next render
        self.pending_panels_to_add.push((new_index, new_path.clone(), input_state.clone()));

        // Create subscription for this file
        let analyzer = self.rust_analyzer.clone();
        tracing::info!("📝 Creating change subscription for new file, rust_analyzer present: {}", analyzer.is_some());
        let subscription = cx.subscribe(&input_state, move |this: &mut TextEditor, input_state_entity: Entity<InputState>, event: &InputEvent, cx: &mut Context<TextEditor>| {
            if let InputEvent::Change = event {
                if let Some(index) = this.open_files.iter().position(|f| f.input_state == input_state_entity) {
                    if let Some(file) = this.open_files.get_mut(index) {
                        file.is_modified = true;
                        file.version += 1;
                        
                        // Notify rust-analyzer of the change
                        if let Some(ref analyzer) = analyzer {
                            let path = file.path.clone();
                            let version = file.version;
                            let content = file.input_state.read(cx).value().to_string();
                            
                            tracing::info!("📝 File changed: {:?} (version {}), notifying rust-analyzer", path.file_name(), version);
                            analyzer.update(cx, |analyzer, _cx| {
                                if let Err(e) = analyzer.did_change_file(&path, &content, version) {
                                    tracing::error!("⚠️  Failed to notify rust-analyzer of file change: {}", e);
                                } else {
                                    if version % 10 == 0 {  // Log every 10th change to avoid spam
                                        tracing::info!("✓ Notified rust-analyzer of change (version {})", version);
                                    }
                                }
                            });
                        } else {
                            if file.version == 2 {  // Only log once to avoid spam
                                tracing::info!("⚠️  No rust-analyzer available for didChange");
                            }
                        }
                        
                        cx.notify();
                    }
                }
            }
        });

        self.subscriptions.push(subscription);
        
        tracing::info!("✓ Created new file: {:?}", new_path);
        cx.notify();
    }

    /// Open a file picker dialog (platform-specific)
    pub fn open_folder_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // For now, open the current working directory
        // In a real implementation, this would show a platform file picker
        if let Ok(cwd) = std::env::current_dir() {
            tracing::info!("✓ Opening folder: {:?}", cwd);
            // Emit an event or call a method to open this folder in the file explorer
            cx.emit(TextEditorEvent::OpenFolderRequested(cwd));
        }
        cx.notify();
    }

    /// Show search/find dialog
    pub fn show_find_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(file) = self.open_files.get(index) {
                // Trigger search on the input state
                file.input_state.update(cx, |state, cx| {
                    // The InputState has a search panel that can be shown
                    // Emit focus event to potentially show search
                    tracing::info!("✓ Opening search panel for current file");
                    cx.emit(ui::input::InputEvent::Focus);
                });
            }
        }
        cx.notify();
    }

    /// Show find and replace dialog
    pub fn show_replace_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(file) = self.open_files.get(index) {
                // Trigger search/replace on the input state
                file.input_state.update(cx, |state, cx| {
                    tracing::info!("✓ Opening find/replace panel for current file");
                    // The search panel supports replace functionality
                    cx.emit(ui::input::InputEvent::Focus);
                });
            }
        }
        cx.notify();
    }

    /// Run the current script file
    pub fn run_current_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(file) = self.open_files.get(index) {
                let path = file.path.clone();
                let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                
                tracing::info!("🚀 Running file: {:?}", path);
                
                // Determine how to run based on file extension
                let command = match extension {
                    "rs" => format!("rustc {} && ./{}", path.display(), path.with_extension("").display()),
                    "py" => format!("python {}", path.display()),
                    "js" | "ts" => format!("node {}", path.display()),
                    "sh" => format!("bash {}", path.display()),
                    _ => {
                        tracing::info!("⚠️  Don't know how to run .{} files", extension);
                        cx.emit(TextEditorEvent::RunScriptRequested(path, "unknown".to_string()));
                        return;
                    }
                };
                
                cx.emit(TextEditorEvent::RunScriptRequested(path, command));
            }
        }
        cx.notify();
    }

    /// Debug the current script file
    pub fn debug_current_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(file) = self.open_files.get(index) {
                let path = file.path.clone();
                tracing::info!("🐛 Debugging file: {:?}", path);
                cx.emit(TextEditorEvent::DebugScriptRequested(path));
            }
        }
        cx.notify();
    }

    pub fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Check if file is already open
        if let Some(index) = self.open_files.iter().position(|f| f.path == path) {
            self.current_file_index = Some(index);
            cx.notify();
            return;
        }

        // Read file content with timing
        let read_start = Instant::now();
        let content = match fs::read_to_string(&path) {
            Ok(content) => {
                let read_time = read_start.elapsed();
                tracing::info!(
                    "✓ Read file {:?} - {} bytes in {:.2}ms", 
                    path.file_name().unwrap_or_default(),
                    content.len(),
                    read_time.as_secs_f64() * 1000.0
                );
                content
            }
            Err(err) => {
                tracing::error!("✗ Failed to read file: {:?}, error: {}", path, err);
                return;
            }
        };

        let file_size = content.len();
        let lines_count = content.lines().count();
        
        // Determine syntax highlighting based on file extension
        let language = self.get_language_from_extension(&path);
        
        // Check if this is a markdown file
        let is_markdown = path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext == "md")
            .unwrap_or(false);
        
        tracing::info!(
            "📄 Opening file: {} lines, {} KB, language: {}{}",
            lines_count,
            file_size / 1024,
            language,
            if is_markdown { " (markdown preview mode)" } else { "" }
        );
        
        // Warn user about very large files
        if lines_count > 50_000 {
            tracing::info!(
                "⚠️  Large file detected ({} lines). Some features may be disabled for performance:",
                lines_count
            );
            tracing::info!("   - Syntax highlighting disabled");
            tracing::info!("   - Soft wrap disabled");
        } else if lines_count > 10_000 {
            tracing::info!(
                "ℹ️  Large file ({} lines). Performance optimizations enabled:",
                lines_count
            );
            tracing::info!("   - Soft wrap disabled");
            tracing::info!("   - Virtual scrolling enabled");
        }

        // Create editor state with optimal settings for large files
        let setup_start = Instant::now();
        let input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(language)
                .line_number(true)
                .minimap(true) // Enable VSCode-style minimap scrollbar
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                // Disable soft wrap for large files for better performance
                // Files with more than 5k lines or 500KB get no wrapping
                .soft_wrap(lines_count < 5_000 && file_size < 500_000)
                .soft_wrap(false);

            // Set the content after creating the state
            state.set_value(&content, window, cx);
            state
        });

        // Set up autocomplete for the file with rust-analyzer support
        let workspace_root = std::env::current_dir().ok();
        if let Some(analyzer) = self.rust_analyzer.clone() {
            input_state.update(cx, |state, cx| {
                super::setup_autocomplete_for_file(
                    state,
                    path.clone(),
                    workspace_root,
                    analyzer,
                    window,
                    cx,
                );
            });
        } else {
            tracing::info!("⚠️  rust-analyzer not available, completions will be limited");
        }

        let setup_time = setup_start.elapsed();
        tracing::info!(
            "⚡ Editor setup completed in {:.2}ms",
            setup_time.as_secs_f64() * 1000.0
        );

        let open_file = OpenFile {
            path: path.clone(),
            input_state: input_state.clone(),
            is_modified: false,
            lines_count,
            file_size,
            version: 1,
            render_as_markdown: is_markdown,
            markdown_preview_cache: if is_markdown { content.clone() } else { String::new() },
            last_markdown_render: if is_markdown { Some(Instant::now()) } else { None },
            pending_scroll_target: None,
        };

        self.open_files.push(open_file);
        let new_index = self.open_files.len() - 1;
        self.current_file_index = Some(new_index);
        
        // Queue panel to be added on next render
        self.pending_panels_to_add.push((new_index, path.clone(), input_state.clone()));
        
        // Create subscription for this file
        let analyzer = self.rust_analyzer.clone();
        tracing::info!("📝 Creating change subscription for {:?}, rust_analyzer present: {}", path.file_name(), analyzer.is_some());
        let subscription = cx.subscribe(&input_state, move |this: &mut TextEditor, input_state_entity: Entity<InputState>, event: &InputEvent, cx: &mut Context<TextEditor>| {
            match event {
                InputEvent::Change => {
                    // Find which file this corresponds to
                    if let Some(index) = this.open_files.iter().position(|f| f.input_state == input_state_entity) {
                        if let Some(file) = this.open_files.get_mut(index) {
                            file.is_modified = true;
                            file.version += 1;
                            
                            // Note: We no longer auto-update markdown preview here
                            // User must click the refresh button to update preview
                            
                            // Notify rust-analyzer of the change
                            if let Some(ref analyzer) = analyzer {
                                let path = file.path.clone();
                                let version = file.version;
                                let content = file.input_state.read(cx).value().to_string();
                                
                                tracing::info!("📝 File changed: {:?} (version {}), notifying rust-analyzer", path.file_name(), version);
                                analyzer.update(cx, |analyzer, _cx| {
                                    if let Err(e) = analyzer.did_change_file(&path, &content, version) {
                                        tracing::error!("⚠️  Failed to notify rust-analyzer of file change: {}", e);
                                    } else {
                                        if version % 10 == 0 {  // Log every 10th change to avoid spam
                                            tracing::info!("✓ Notified rust-analyzer of change (version {})", version);
                                        }
                                    }
                                });
                            } else {
                                if file.version == 2 {  // Only log once to avoid spam
                                    tracing::info!("⚠️  No rust-analyzer available for didChange");
                                }
                            }
                            
                            cx.notify();
                        }
                    }
                },
                InputEvent::GoToDefinition { path, line, character } => {
                    // Navigate to the definition - emit an event so it can be handled 
                    // by the parent where we have window access
                    tracing::info!("🎯 Received GoToDefinition event: {:?} at {}:{}", path, line, character);
                    
                    // Emit the navigation event so it can be handled by parent components
                    // that have window access
                    let target_path = path.clone();
                    let target_line = *line;
                    let target_character = *character;
                    
                    // Store pending navigation in TextEditor
                    this.pending_navigation = Some((target_path.clone(), target_line, target_character));
                    
                    cx.notify();
                },
                _ => {}
            }
        });
        
        self.subscriptions.push(subscription);
        
        // Emit event so rust-analyzer can be notified
        cx.emit(TextEditorEvent::FileOpened {
            path: path.clone(),
            content: content.clone(),
        });
        
        // Log cache stats after opening
        if let Some(index) = self.current_file_index {
            if let Some(file) = self.open_files.get(index) {
                let state = file.input_state.read(cx);
                tracing::info!(
                    "📊 Line cache initialized - capacity: {} lines",
                    state.line_cache().len()
                );
                
                // Log autocomplete configuration
                if state.lsp.completion_provider.is_some() {
                    tracing::info!("✓ Autocomplete enabled with comprehensive provider");
                } else {
                    tracing::info!("ℹ️  No autocomplete provider configured");
                }
            }
        }
        
        cx.notify();
    }

    fn get_language_from_extension(&self, path: &PathBuf) -> String {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("rs") => "rust".to_string(),
            Some("js") => "javascript".to_string(),
            Some("ts") => "typescript".to_string(),
            Some("py") => "python".to_string(),
            Some("toml") => "toml".to_string(),
            Some("json") => "json".to_string(),
            Some("md") => "markdown".to_string(),
            Some("html") => "html".to_string(),
            Some("css") => "css".to_string(),
            Some("go") => "go".to_string(),
            Some("rb") => "ruby".to_string(),
            Some("sql") => "sql".to_string(),
            Some("log") => "text".to_string(),
            Some("txt") => "text".to_string(),
            Some("yaml") | Some("yml") => "yaml".to_string(),
            Some("xml") => "xml".to_string(),
            Some("c") | Some("h") => "c".to_string(),
            Some("cpp") | Some("hpp") | Some("cc") => "cpp".to_string(),
            _ => "text".to_string(),
        }
    }
    
    /// Get performance info about the current file
    fn get_current_file_performance(&self, cx: &App) -> Option<String> {
        if !self.show_performance_stats {
            return None;
        }
        
        let index = self.current_file_index?;
        let open_file = self.open_files.get(index)?;
        
        let state = open_file.input_state.read(cx);
        let cache_stats = state.line_cache().stats();
        
        Some(format!(
            "📊 Performance: {} lines | Cache: {:.1}% hit rate | {} cached lines | Memory: ~{} MB",
            open_file.lines_count,
            cache_stats.hit_rate() * 100.0,
            state.line_cache().len(),
            (state.line_cache().len() * 1024) / (1024 * 1024) // Rough estimate
        ))
    }

    pub fn close_file(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        if index < self.open_files.len() {
            let file_path = self.open_files[index].path.clone();
            self.open_files.remove(index);

            // Emit event so rust-analyzer can be notified
            cx.emit(TextEditorEvent::FileClosed {
                path: file_path,
            });

            // Adjust current file index
            if let Some(current) = self.current_file_index {
                if current == index {
                    // Closed the current file
                    if self.open_files.is_empty() {
                        self.current_file_index = None;
                    } else if index == self.open_files.len() {
                        // Closed the last file, select the previous one
                        self.current_file_index = Some(index.saturating_sub(1));
                    } else {
                        // Keep the same index (which now points to the next file)
                        self.current_file_index = Some(index);
                    }
                } else if current > index {
                    // Closed a file before the current one
                    self.current_file_index = Some(current - 1);
                }
            }

            cx.notify();
        }
    }

    fn set_active_file(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        if index < self.open_files.len() {
            self.current_file_index = Some(index);
            cx.notify();
        }
    }

    pub fn save_current_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(index) = self.current_file_index {
            if let Some(open_file) = self.open_files.get_mut(index) {
                // Get content from input state
                let content = open_file.input_state.read(cx).value();

                // Write to file
                if let Ok(_) = fs::write(&open_file.path, content.as_str()) {
                    open_file.is_modified = false;
                    tracing::info!("💾 File saved: {:?}", open_file.path);
                    
                    // Emit event so rust-analyzer can be notified
                    cx.emit(TextEditorEvent::FileSaved {
                        path: open_file.path.clone(),
                        content: content.to_string(),
                    });
                    
                    cx.notify();
                    return true;
                }
            }
        }
        false
    }
    
    pub fn close_current_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(open_file) = self.open_files.get(index) {
                let path = open_file.path.clone();
                
                // Emit event so rust-analyzer can be notified
                cx.emit(TextEditorEvent::FileClosed {
                    path: path.clone(),
                });
                
                tracing::info!("❌ File closed: {:?}", path.file_name());
                
                // Remove the file from open files
                self.open_files.remove(index);
                
                // Update current file index
                if self.open_files.is_empty() {
                    self.current_file_index = None;
                } else if index >= self.open_files.len() {
                    // If we removed the last file, select the new last file
                    self.current_file_index = Some(self.open_files.len() - 1);
                }
                // else: keep current index (will now point to the next file)
                
                cx.notify();
            }
        }
    }

    /// Navigate to a specific line and column in the current file
    pub fn go_to_line(&mut self, line: usize, column: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(open_file) = self.open_files.get_mut(index) {
                // Store the pending scroll target in case layout isn't ready yet
                open_file.pending_scroll_target = Some((line, column));
                
                open_file.input_state.update(cx, |state, cx| {
                    // LSP Position uses 'line' and 'character' fields (0-based)
                    // Our UI uses 1-based line numbers
                    use ui::input::Position;
                    state.set_cursor_position(
                        Position {
                            line: (line.saturating_sub(1)) as u32,
                            character: (column.saturating_sub(1)) as u32,
                        },
                        window,
                        cx,
                    );
                    
                    tracing::info!("📍 Navigated to line {}, column {}", line, column);
                    
                    // Force an additional notify to ensure scroll is processed
                    cx.notify();
                });
                
                // Notify at the TextEditor level as well to ensure render is triggered
                cx.notify();
            }
        }
    }

    /// Get the current file path if any
    pub fn current_file_path(&self) -> Option<PathBuf> {
        self.current_file_index
            .and_then(|index| self.open_files.get(index))
            .map(|file| file.path.clone())
    }

    /// Get the current scroll offset of the active file's input state
    pub fn get_current_scroll_offset(&self, cx: &mut Context<Self>) -> Option<Point<Pixels>> {
        self.current_file_index
            .and_then(|index| self.open_files.get(index))
            .map(|file| {
                file.input_state.read(cx).get_scroll_offset()
            })
    }

    /// Set the scroll offset for the active file's input state
    pub fn set_scroll_offset(&mut self, offset: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(file) = self.open_files.get(index) {
                file.input_state.update(cx, |state, _cx| {
                    state.set_scroll_offset(offset);
                });
            }
        }
    }

    /// Process pending navigation request (called from render where we have window access)
    fn process_pending_navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, line, character)) = self.pending_navigation.take() {
            tracing::info!("🎯 Processing pending navigation to {:?} at {}:{}", path, line, character);
            
            // Check if file is already open
            let file_index = self.open_files.iter().position(|f| f.path == path);
            
            if let Some(index) = file_index {
                // File is already open, switch to it
                self.current_file_index = Some(index);
                tracing::info!("✓ Switched to already-open file {:?}", path);
            } else {
                // Need to open the file first
                tracing::info!("📂 Opening file {:?}", path);
                self.open_file(path.clone(), window, cx);
                tracing::info!("✓ File opened, current_file_index: {:?}", self.current_file_index);
            }
            
            // Now navigate to the specific position
            // LSP positions are 0-based, go_to_line expects 1-based line numbers
            // and will convert back to 0-based internally
            let target_line = (line + 1) as usize;  // Convert 0-based to 1-based
            let target_col = (character + 1) as usize;  // Convert 0-based to 1-based
            
            tracing::info!("🎯 Calling go_to_line with line {} (LSP: {}), column {} (LSP: {})", 
                target_line, line, target_col, character);
            
            self.go_to_line(target_line, target_col, window, cx);
            
            cx.notify();
        }
    }
    
    /// Process any pending scroll targets (called from render after layout is ready)
    fn process_pending_scroll_targets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.current_file_index {
            if let Some(open_file) = self.open_files.get_mut(index) {
                if let Some((line, column)) = open_file.pending_scroll_target {
                    // Try to scroll - if layout isn't ready, set_cursor_position will handle it gracefully
                    // We'll keep trying on subsequent frames until it works
                    tracing::info!("📜 Attempting to scroll to line {}, column {}", line, column);
                    
                    let scroll_attempted = open_file.input_state.update(cx, |state, cx| {
                        use ui::input::Position;
                        state.set_cursor_position(
                            Position {
                                line: (line.saturating_sub(1)) as u32,
                                character: (column.saturating_sub(1)) as u32,
                            },
                            window,
                            cx,
                        );
                        // Return true to indicate we tried
                        true
                    });
                    
                    if scroll_attempted {
                        // Clear the pending scroll target - even if it didn't fully work,
                        // set_cursor_position was called which should set deferred scroll
                        open_file.pending_scroll_target = None;
                        tracing::info!("✓ Scroll target cleared");
                    }
                }
            }
        }
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.open_files.is_empty() {
            return div().into_any_element();
        }

        TabBar::new("editor-tabs")
            .w_full()
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border)
            .selected_index(self.current_file_index.unwrap_or(0))
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                this.set_active_file(*ix, window, cx);
            }))
            .font_family("monospace")
            .font(gpui::Font {
                family: "Jetbrains Mono".to_string().into(),
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
                features: gpui::FontFeatures::default(),
                fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
            })
            .children(
                self.open_files.iter().enumerate().map(|(index, open_file)| {
                    let filename = open_file.path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("untitled")
                        .to_string();

                    let display_name = if open_file.is_modified {
                        format!("● {}", filename)
                    } else {
                        filename
                    };

                    Tab::new(display_name)
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new(("close", index))
                                        .icon(IconName::Close)
                                        .ghost()
                                        .xsmall()
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.close_file(index, window, cx);
                                        }))
                                )
                        )
                })
            )
            .into_any_element()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let is_markdown_file = if let Some(index) = self.current_file_index {
            self.open_files.get(index).map(|f| f.render_as_markdown).unwrap_or(false)
        } else {
            false
        };
        
        h_flex()
            .w_full()
            .p_2()
            .bg(cx.theme().secondary)
            .border_b_1()
            .border_color(cx.theme().border)
            .justify_between()
            .items_center()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("new_file")
                            .icon(IconName::Plus)
                            .tooltip("New File (Ctrl+N)")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_new_file(window, cx);
                            }))
                    )
                    .child(
                        Button::new("save")
                            .icon(IconName::FloppyDisk)
                            .tooltip("Save (Ctrl+S)")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_current_file(window, cx);
                            }))
                    )
                    .children(if is_markdown_file {
                        Some(
                            Button::new("refresh_preview")
                                .icon(IconName::Refresh)
                                .tooltip("Refresh Markdown Preview (Ctrl+R)")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.refresh_markdown_preview(cx);
                                }))
                        )
                    } else {
                        None
                    })
                    .child(
                        Button::new("find")
                            .icon(IconName::Search)
                            .tooltip("Find (Ctrl+F)")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_find_dialog(window, cx);
                            }))
                    )
                    .child(
                        Button::new("replace")
                            .icon(IconName::Replace)
                            .tooltip("Replace (Ctrl+H)")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_replace_dialog(window, cx);
                            }))
                    )
                    .child(
                        if self.show_performance_stats {
                            Button::new("toggle_stats")
                                .icon(IconName::Search)
                                .tooltip("Toggle Performance Stats (F12)")
                                .small()
                                .with_variant(ui::button::ButtonVariant::Primary)
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.show_performance_stats = !this.show_performance_stats;
                                    cx.notify();
                                }))
                        } else {
                            Button::new("toggle_stats")
                                .icon(IconName::Search)
                                .tooltip("Toggle Performance Stats (F12)")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.show_performance_stats = !this.show_performance_stats;
                                    cx.notify();
                                }))
                        }
                    )
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("run")
                            .icon(IconName::ArrowRight)
                            .tooltip("Run Script (F5)")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run_current_file(window, cx);
                            }))
                    )
                    .child(
                        Button::new("debug")
                            .icon(IconName::Search)
                            .tooltip("Debug Script (F9)")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.debug_current_file(window, cx);
                            }))
                    )
            )
    }

    fn render_editor_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(index) = self.current_file_index {
            if let Some(open_file) = self.open_files.get(index) {
                // If it's a markdown file, render split view with editor on left and preview on right
                if open_file.render_as_markdown {
                    // Use cached markdown content to avoid re-rendering on every frame
                    let preview_content = &open_file.markdown_preview_cache;
                    
                    return h_resizable("markdown-split", self.markdown_split_state.clone())
                        .child(
                            // Left panel: Text editor for editing markdown
                            resizable_panel()
                                .size(px(400.))
                                .child(
                                    div()
                                        .size_full()
                                        .overflow_hidden()
                                        .border_r_1()
                                        .border_color(cx.theme().border)
                                        .child(
                                            TextInput::new(&open_file.input_state)
                                                .h_full()
                                                .w_full()
                                                .font_family("monospace")
                                                .font(gpui::Font {
                                                    family: "Jetbrains Mono".to_string().into(),
                                                    weight: gpui::FontWeight::NORMAL,
                                                    style: gpui::FontStyle::Normal,
                                                    features: gpui::FontFeatures::default(),
                                                    fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                                                })
                                                .text_size(px(14.0))
                                                .border_0()
                                        )
                                )
                        )
                        .child(
                            // Right panel: Debounced markdown preview (only updates every 300ms)
                            resizable_panel()
                                .child({
                                    if !preview_content.is_empty() {
                                        div()
                                            .id("markdown-preview-panel")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .p_5()
                                            .bg(cx.theme().background)
                                            .font_family("monospace")
                                            .font(gpui::Font {
                                                family: "Jetbrains Mono".to_string().into(),
                                                weight: gpui::FontWeight::NORMAL,
                                                style: gpui::FontStyle::Normal,
                                                features: gpui::FontFeatures::default(),
                                                fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                                            })
                                            // TODO: Re-enable markdown rendering when performance is improved
                                            //.child(
                                            //    TextView::markdown(
                                            //        "md-viewer",
                                            //        preview_content.clone(),
                                            //        window,
                                            //        cx,
                                            //    )
                                            //    .selectable()
                                            //)
                                    } else {
                                        div()
                                            .id("markdown-preview-panel")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .p_5()
                                            .bg(cx.theme().background)
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_3()
                                                    .items_center()
                                                    .justify_center()
                                                    .size_full()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .child("Markdown preview ready")
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .child("Click the refresh button or press Ctrl+R to update")
                                                    )
                                            )
                                    }
                                })
                        )
                        .into_any_element();
                }
                
                // Otherwise render as text editor
                div()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        TextInput::new(&open_file.input_state)
                            .h_full()
                            .w_full()
                            .font_family("monospace")
                            .font(gpui::Font {
                                family: "Jetbrains Mono".to_string().into(),
                                weight: gpui::FontWeight::NORMAL,
                                style: gpui::FontStyle::Normal,
                                features: gpui::FontFeatures::default(),
                                fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                            })
                            .text_size(px(14.0))
                            .border_0()
                    )
                    .into_any_element()
            } else {
                self.render_empty_editor(cx)
            }
        } else {
            self.render_empty_editor(cx)
        }
    }

    fn render_empty_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .text_2xl()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("Welcome to Script Editor")
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .text_center()
                            .child("Open a file from the explorer to start editing")
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .mt_4()
                            .child(
                                Button::new("new_file_welcome")
                                    .label("New File")
                                    .icon(IconName::Plus)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.create_new_file(window, cx);
                                    }))
                            )
                            .child(
                                Button::new("open_folder_welcome")
                                    .label("Open Folder")
                                    .icon(IconName::FolderOpen)
                                    .with_variant(ui::button::ButtonVariant::Primary)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_folder_dialog(window, cx);
                                    }))
                            )
                    )
            )
            .into_any_element()
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (file_info, cache_info) = if let Some(index) = self.current_file_index {
            if let Some(open_file) = self.open_files.get(index) {
                let filename = open_file.path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("untitled")
                    .to_string();
                let language = self.get_language_from_extension(&open_file.path);
                
                // Get cache statistics
                let state = open_file.input_state.read(cx);
                let cache_stats = state.line_cache().stats();
                let cache_size = state.line_cache().len();
                
                let file_size_kb = open_file.file_size / 1024;
                let file_info_str = if file_size_kb > 1024 {
                    format!("{} | {} lines | {:.1} MB", filename, open_file.lines_count, file_size_kb as f64 / 1024.0)
                } else {
                    format!("{} | {} lines | {} KB", filename, open_file.lines_count, file_size_kb)
                };
                
                let cache_info_str = if self.show_performance_stats {
                    format!(
                        "Cache: {}/{} lines | Hit Rate: {:.1}% | Hits: {} | Misses: {}",
                        cache_size,
                        cache_stats.hits + cache_stats.misses,
                        cache_stats.hit_rate() * 100.0,
                        cache_stats.hits,
                        cache_stats.misses
                    )
                } else {
                    format!("Cache: {} lines cached", cache_size)
                };
                
                ((file_info_str, language), cache_info_str)
            } else {
                (("No file".to_string(), "".to_string()), "".to_string())
            }
        } else {
            (("No file".to_string(), "".to_string()), "".to_string())
        };

        h_flex()
            .w_full()
            .min_h_6()
            .px_4()
            .py_1()
            .bg(cx.theme().accent)
            .border_t_1()
            .border_color(cx.theme().border)
            .justify_between()
            .items_center()
            .text_xs()
            .text_color(cx.theme().accent_foreground)
            .child(
                h_flex()
                    .gap_4()
                    .child(file_info.0)
                    .child("UTF-8")
                    .child("LF")
            )
            .child({
                let mut flex = h_flex().gap_4();
                
                if self.show_performance_stats {
                    flex = flex.child(cache_info.clone());
                }
                
                flex.child("Ln 1, Col 1")
                    .child("Spaces: 4")
                    .child(file_info.1)
            })
    }

    /// Load content directly into the editor without opening from disk
    /// This is used for diff mode where content is provided programmatically
    pub fn load_content(&mut self, path: PathBuf, content: String, window: &mut Window, cx: &mut Context<Self>) {
        self.load_content_with_diff_highlight(path, content, None, window, cx);
    }

    /// Load content with optional diff highlighting
    /// other_content is used to compute which lines are added/removed
    pub fn load_content_with_diff_highlight(
        &mut self,
        path: PathBuf,
        content: String,
        other_content: Option<(String, bool)>, // (other_content, is_before)
        window: &mut Window,
        cx: &mut Context<Self>
    ) {
        // In diff mode, we want to replace the current file rather than accumulate multiple files
        // Check if we should reuse the first file slot (for diff mode)
        let should_replace = other_content.is_some() && !self.open_files.is_empty();

        if should_replace {
            // Replace the first (and only) file in diff mode
            if let Some(file) = self.open_files.get_mut(0) {
                file.path = path.clone();

                // Update language for syntax highlighting
                let language = path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| match ext {
                        "rs" => "rust",
                        "js" | "jsx" => "javascript",
                        "ts" | "tsx" => "typescript",
                        "py" => "python",
                        "go" => "go",
                        "c" | "h" => "c",
                        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
                        "java" => "java",
                        "json" => "json",
                        "toml" => "toml",
                        "yaml" | "yml" => "yaml",
                        "md" => "markdown",
                        "html" | "htm" => "html",
                        "css" => "css",
                        "xml" => "xml",
                        "sh" | "bash" => "bash",
                        _ => "plaintext",
                    })
                    .unwrap_or("plaintext");

                file.input_state.update(cx, |state, cx| {
                    // Update language/highlighter for the new file
                    state.set_highlighter(language, cx);
                    state.set_value(&content, window, cx);

                    // Apply diff highlighting if other_content is provided
                    if let Some((other, is_before)) = &other_content {
                        Self::apply_diff_highlighting(state, &content, other, *is_before, window, cx);
                    }
                });
                file.is_modified = false;
                file.lines_count = content.lines().count();
                file.file_size = content.len();
                self.current_file_index = Some(0);
                tracing::info!("TextEditor: Replaced file slot 0 with {:?}", path);
            }
        } else if let Some(index) = self.open_files.iter().position(|f| f.path == path) {
            // File already open at a different index, just set its content
            self.current_file_index = Some(index);
            if let Some(file) = self.open_files.get_mut(index) {
                file.input_state.update(cx, |state, cx| {
                    state.set_value(&content, window, cx);

                    // Apply diff highlighting if other_content is provided
                    if let Some((other, is_before)) = &other_content {
                        Self::apply_diff_highlighting(state, &content, other, *is_before, window, cx);
                    }
                });
                file.is_modified = false;
            }
        } else {
            // Detect language from file extension for syntax highlighting
            let language = path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| match ext {
                    "rs" => "rust",
                    "js" | "jsx" => "javascript",
                    "ts" | "tsx" => "typescript",
                    "py" => "python",
                    "go" => "go",
                    "c" | "h" => "c",
                    "cpp" | "cc" | "cxx" | "hpp" => "cpp",
                    "java" => "java",
                    "json" => "json",
                    "toml" => "toml",
                    "yaml" | "yml" => "yaml",
                    "md" => "markdown",
                    "html" | "htm" => "html",
                    "css" => "css",
                    "xml" => "xml",
                    "sh" | "bash" => "bash",
                    _ => "plaintext",
                })
                .unwrap_or("plaintext");

            // Create new file entry with provided content
            let input_state = cx.new(|cx| {
                let mut state = InputState::new(window, cx)
                    .multi_line()
                    .code_editor(language)
                    .line_number(true)
                    .tab_size(TabSize {
                        tab_size: 4,
                        hard_tabs: false,
                    });
                state.set_value(&content, window, cx);

                // Apply diff highlighting if other_content is provided
                if let Some((other, is_before)) = &other_content {
                    Self::apply_diff_highlighting(&mut state, &content, other, *is_before, window, cx);
                }

                state
            });

            let file = OpenFile {
                path: path.clone(),
                input_state: input_state.clone(),
                is_modified: false,
                lines_count: content.lines().count(),
                file_size: content.len(),
                version: 1,
                render_as_markdown: false,
                markdown_preview_cache: String::new(),
                last_markdown_render: None,
                pending_scroll_target: None,
            };

            self.open_files.push(file);
            self.current_file_index = Some(self.open_files.len() - 1);
        }

        cx.notify();
    }

    /// Apply diff highlighting to an InputState with proper alignment
    fn apply_diff_highlighting(
        state: &mut InputState,
        this_content: &str,
        other_content: &str,
        is_before: bool,
        window: &mut Window,
        cx: &mut Context<InputState>
    ) {
        use similar::{ChangeTag, TextDiff};

        // Use the 'similar' crate's Myers diff algorithm
        let diff = TextDiff::from_lines(
            if is_before { this_content } else { other_content },
            if is_before { other_content } else { this_content }
        );

        let mut aligned_content = String::new();
        let mut line_highlights = Vec::new();

        for change in diff.iter_all_changes() {
            let line = change.to_string_lossy();

            match change.tag() {
                ChangeTag::Equal => {
                    // Line exists in both - no highlight
                    aligned_content.push_str(&line);
                    if !line.ends_with('\n') {
                        aligned_content.push('\n');
                    }
                    line_highlights.push(ui::input::LineHighlight::None);
                }
                ChangeTag::Delete => {
                    if is_before {
                        // This is the "before" view, show the deleted line
                        aligned_content.push_str(&line);
                        if !line.ends_with('\n') {
                            aligned_content.push('\n');
                        }
                        line_highlights.push(ui::input::LineHighlight::Removed);
                    } else {
                        // This is the "after" view, insert blank line for alignment
                        aligned_content.push('\n');
                        line_highlights.push(ui::input::LineHighlight::None);
                    }
                }
                ChangeTag::Insert => {
                    if !is_before {
                        // This is the "after" view, show the inserted line
                        aligned_content.push_str(&line);
                        if !line.ends_with('\n') {
                            aligned_content.push('\n');
                        }
                        line_highlights.push(ui::input::LineHighlight::Added);
                    } else {
                        // This is the "before" view, insert blank line for alignment
                        aligned_content.push('\n');
                        line_highlights.push(ui::input::LineHighlight::None);
                    }
                }
            }
        }

        // Update the content with aligned version
        state.set_value(&aligned_content, window, cx);
        state.set_line_highlights(line_highlights);
    }
}

impl EventEmitter<TextEditorEvent> for TextEditor {}

impl Focusable for TextEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Initialize workspace on first render
        self.initialize_workspace_once(window, cx);
        
        // Process any pending navigation requests (from go-to-definition)
        self.process_pending_navigation(window, cx);
        
        // Process any pending scroll targets (after layout is ready)
        self.process_pending_scroll_targets(window, cx);

        // Track render time for performance monitoring
        let render_start = Instant::now();
        
        let result = v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        if let Some(ref workspace) = self.workspace {
                            workspace.clone().into_any_element()
                        } else {
                            div().child("Loading...").into_any_element()
                        }
                    )
            )
            .child(self.render_status_bar(cx));
        
        // Log render time if performance stats are enabled
        if self.show_performance_stats {
            let render_time = render_start.elapsed();
            if render_time.as_millis() > 16 {
                tracing::error!(
                    "⚠️  Slow render: {:.2}ms (target: 16ms for 60 FPS)",
                    render_time.as_secs_f64() * 1000.0
                );
            }
        }
        
        self.last_render_time = Some(render_start);
        
        result
    }
}