use gpui::*;
use gpui::Hsla;
use ui::{ActiveTheme, Colorize};
use std::path::PathBuf;
use std::time::Duration;
use engine_backend::services::rust_analyzer_manager::{RustAnalyzerManager, AnalyzerStatus, AnalyzerEvent};

pub struct LoadingWindow {
    project_path: PathBuf,
    project_name: String,
    loading_tasks: Vec<LoadingTask>,
    current_task_index: usize,
    progress: f32,
    rust_analyzer: Option<Entity<RustAnalyzerManager>>,
    analyzer_ready: bool,
    initial_tasks_complete: bool,
    _analyzer_subscription: Option<Subscription>,
    /// Current analyzer work message (shown at bottom left)
    analyzer_message: String,
    /// Window ID for closing this window
    window_id: u64,
}

#[derive(Clone)]
struct LoadingTask {
    name: String,
    status: TaskStatus,
}

#[derive(Clone, PartialEq)]
enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

pub struct LoadingComplete {
    pub project_path: PathBuf,
    pub rust_analyzer: Entity<RustAnalyzerManager>,
}

impl EventEmitter<LoadingComplete> for LoadingWindow {}

impl LoadingWindow {
    pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_window_id(project_path, 0, window, cx)
    }

    pub fn new_with_window_id(project_path: PathBuf, window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Extract project name from path
        let project_name = project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unnamed Project")
            .to_string();
        
        //TODO: These should be fetched dynamically based on rust analyzer's output
        // Define all loading tasks
        let loading_tasks = vec![
            LoadingTask {
                name: "Initializing renderer...".to_string(),
                status: TaskStatus::Pending,
            },
            LoadingTask {
                name: "Loading project data...".to_string(),
                status: TaskStatus::Pending,
            },
            LoadingTask {
                name: "Starting Rust Analyzer...".to_string(),
                status: TaskStatus::Pending,
            },
        ];

        let mut loading_window = Self {
            project_path: project_path.clone(),
            project_name,
            loading_tasks,
            current_task_index: 0,
            progress: 0.0,
            rust_analyzer: None,
            analyzer_ready: false,
            initial_tasks_complete: false,
            _analyzer_subscription: None,
            analyzer_message: String::new(),
            window_id,
        };

        // Initialize rust analyzer but DON'T start it yet
        // We'll start it in the background after opening the editor
        let analyzer = cx.new(|cx| RustAnalyzerManager::new(window, cx));
        loading_window.rust_analyzer = Some(analyzer.clone());

        // Start initial quick tasks (no analyzer polling needed)
        loading_window.start_loading(window, cx);

        loading_window
    }

    fn start_loading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Start all initialization tasks asynchronously
        self.start_init_tasks(window, cx);
    }


    fn start_init_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Task 0: Renderer init (immediate)
        self.loading_tasks[0].status = TaskStatus::InProgress;
        self.progress = 0.0;
        self.analyzer_message = "Initializing renderer...".to_string();
        cx.notify();
        
        cx.spawn_in(window, async move |this, cx| {
            // Minimal delay just to show the task
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_tasks[0].status = TaskStatus::Completed;
                this.progress = 33.0;
                
                // Task 1: Project data
                this.loading_tasks[1].status = TaskStatus::InProgress;
                this.analyzer_message = "Loading project data...".to_string();
                cx.notify();
            });
            
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_tasks[1].status = TaskStatus::Completed;
                this.progress = 66.0;
                
                // Task 2: Starting analyzer (but not waiting for it)
                this.loading_tasks[2].status = TaskStatus::InProgress;
                this.analyzer_message = "Opening editor...".to_string();
                cx.notify();
            });
            
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_tasks[2].status = TaskStatus::Completed;
                this.progress = 100.0;
                this.initial_tasks_complete = true;
                this.analyzer_message = "Ready!".to_string();
                cx.notify();
                
                // Open editor immediately - analyzer will start in background
                this.check_completion(cx);
            });
        }).detach();
    }
    


    
    fn check_completion(&mut self, cx: &mut Context<Self>) {
        // Complete immediately when initial tasks are done - don't wait for analyzer
        if self.initial_tasks_complete {
            tracing::info!("🎉 Loading complete! Opening editor window...");
            let project_path = self.project_path.clone();
            let rust_analyzer = self.rust_analyzer.clone().expect("Rust Analyzer should be initialized");

            // Request editor window to be opened and close this splash
            if let Some(engine_state) = crate::EngineState::global() {
                tracing::info!("📝 Requesting editor window for: {:?}", project_path);
                engine_state.request_window(crate::WindowRequest::ProjectEditor {
                    project_path: project_path.to_string_lossy().to_string(),
                });

                // Close this splash window
                tracing::info!("🔚 Closing splash window (ID: {})", self.window_id);
                engine_state.request_window(crate::WindowRequest::CloseWindow {
                    window_id: self.window_id,
                });
            }

            // Emit completion event (in case anyone else needs it)
            cx.emit(LoadingComplete {
                project_path,
                rust_analyzer,
            });
        }
    }
}

impl Render for LoadingWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        
        // Calculate relative width for progress bar (Story crate style)
        let relative_w = relative(match self.progress {
            v if v < 0.0 => 0.0,
            v if v > 100.0 => 1.0,
            v => v / 100.0,
        });

        div()
            .id("loading-window")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(
                // Background image layer
                div()
                    .absolute()
                    .size_full()
                    .child(
                        img("images/Splash.png")
                            .size_full()
                            .object_fit(gpui::ObjectFit::Cover)
                    )
            )
            .child(
                // Main content area (centered)
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .flex_1()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_4()
                            .child(
                                // Logo/Title with project name (with background)
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap_1()
                                    .px_6()
                                    .py_4()
                                    .rounded_lg()
                                    .bg(gpui::black().opacity(0.5))
                                    .child(
                                        div()
                                            .text_xl()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(theme.foreground)
                                            .child("Pulsar Engine")
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(theme.muted_foreground)
                                            .child(self.project_name.clone())
                                    )
                            )
                            .child(
                                // Task list (with background)
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .mt_4()
                                    .px_6()
                                    .py_4()
                                    .rounded_lg()
                                    .bg(gpui::black().opacity(0.5))
                                    .children(
                                        self.loading_tasks.iter().map(|task| {
                                            let color = match task.status {
                                                TaskStatus::Pending => theme.muted_foreground,
                                                TaskStatus::InProgress => theme.accent,
                                                TaskStatus::Completed => theme.success_foreground,
                                            };
                                            let icon = match task.status {
                                                TaskStatus::Pending => "○",
                                                TaskStatus::InProgress => "◐",
                                                TaskStatus::Completed => "●",
                                            };

                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .text_color(color)
                                                        .child(icon)
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(color)
                                                        .child(task.name.clone())
                                                )
                                        })
                                    )
                            )
                    )
            )
            .child(
                // Bottom section with current work and progress bar
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(
                        // Current work indicator (bottom left, can truncate) with background
                        div()
                            .px_4()
                            .pb_2()
                            .w_full()
                            .overflow_hidden()
                            .child(
                                div()
                                    .px_4()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(gpui::black().opacity(0.5))
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .child(
                                        // Show the analyzer message if available, otherwise show current task
                                        if !self.analyzer_message.is_empty() {
                                            self.analyzer_message.clone()
                                        } else {
                                            self.loading_tasks.iter()
                                                .find(|t| t.status == TaskStatus::InProgress)
                                                .map(|t| t.name.clone())
                                                .unwrap_or_else(|| "Initializing...".to_string())
                                        }
                                    )
                            )
                    )
                    .child(
                        // Progress bar at the very bottom edge
                        div()
                            .w_full()
                            .h(px(4.))
                            .relative()
                            .bg(theme.border)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .h_full()
                                    .w(relative_w)
                                    .bg(Hsla::parse_hex("#c2c2c8ff").unwrap())
                            )
                    )
            )
    }
}
