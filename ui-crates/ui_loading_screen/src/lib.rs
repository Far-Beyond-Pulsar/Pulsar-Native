//! Loading Screen Component
//!
//! Full-featured loading screen from LoadingWindow

use gpui::*;
use gpui::Hsla;
use ui::{ActiveTheme, Colorize, Root};
use std::path::{Path, PathBuf};
use std::time::Duration;
use engine_backend::services::rust_analyzer_manager::{RustAnalyzerManager, AnalyzerStatus, AnalyzerEvent};
use engine_state::{EngineContext, WindowRequest};
use ui_entry::entry_screen::recent_projects::{RecentProjectsList, RecentProject};
use directories::ProjectDirs;

/// Helper function to create a loading screen component wrapped in Root
pub fn create_loading_component(
    project_path: PathBuf,
    window_id: u64,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Root> {
    let loading_screen = cx.new(|cx| LoadingScreen::new_with_window_id(project_path, window_id, window, cx));
    cx.new(|cx| Root::new(loading_screen.into(), window, cx))
}

pub struct LoadingScreen {
    project_path: PathBuf,
    project_name: String,
    loading_tasks: Vec<LoadingTask>,
    current_task_index: usize,
    progress: f32,
    rust_analyzer: Option<Entity<RustAnalyzerManager>>,
    analyzer_ready: bool,
    initial_tasks_complete: bool,
    _analyzer_subscription: Option<Subscription>,
    analyzer_message: String,
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

impl EventEmitter<LoadingComplete> for LoadingScreen {}

impl LoadingScreen {
    pub fn new(project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_window_id(project_path, 0, window, cx)
    }

    pub fn new_with_window_id(project_path: PathBuf, window_id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let project_name = project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unnamed Project")
            .to_string();
        
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

        let mut loading_screen = Self {
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

        let analyzer = cx.new(|cx| RustAnalyzerManager::new(window, cx));
        loading_screen.rust_analyzer = Some(analyzer.clone());
        loading_screen.start_loading(window, cx);
        loading_screen
    }

    fn start_loading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_init_tasks(window, cx);
    }

    fn start_init_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading_tasks[0].status = TaskStatus::InProgress;
        self.progress = 0.0;
        self.analyzer_message = "Initializing renderer...".to_string();
        cx.notify();
        
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_tasks[0].status = TaskStatus::Completed;
                this.progress = 33.0;
                this.loading_tasks[1].status = TaskStatus::InProgress;
                this.analyzer_message = "Loading project data...".to_string();
                cx.notify();
            });
            
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let _ = this.update(cx, |this, cx| {
                this.loading_tasks[1].status = TaskStatus::Completed;
                this.progress = 66.0;
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
                this.check_completion(cx);
            });
        }).detach();
    }


    fn check_completion(&mut self, cx: &mut Context<Self>) {
        if self.initial_tasks_complete {
            let project_path = self.project_path.clone();
            let rust_analyzer = self.rust_analyzer.clone().expect("Rust Analyzer should be initialized");

            // Update recent projects list
            update_recent_projects(&project_path);

            if let Some(engine_context) = EngineContext::global() {
                engine_context.request_window(WindowRequest::ProjectEditor {
                    project_path: project_path.to_string_lossy().to_string(),
                });

                engine_context.request_window(WindowRequest::CloseWindow {
                    window_id: self.window_id,
                });
            }

            cx.emit(LoadingComplete {
                project_path,
                rust_analyzer,
            });
        }
    }
}

impl Render for LoadingScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        
        let relative_w = relative(match self.progress {
            v if v < 0.0 => 0.0,
            v if v > 100.0 => 1.0,
            v => v / 100.0,
        });

        div()
            .id("loading-screen")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .child(
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
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(
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

/// Update recent projects list when a project is successfully loaded
fn update_recent_projects(project_path: &Path) {
    // Get recent projects path
    let Some(proj_dirs) = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine") else {
        tracing::warn!("Failed to get project directories for recent projects update");
        return;
    };

    let data_dir = proj_dirs.data_dir();
    let recent_projects_path = data_dir.join("recent_projects.json");

    // Load existing recent projects
    let mut recent_projects = RecentProjectsList::load(&recent_projects_path);

    // Create RecentProject entry
    let project = RecentProject {
        name: project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string(),
        path: project_path.to_string_lossy().to_string(),
        last_opened: Some(chrono::Local::now().to_rfc3339()),
        is_git: project_path.join(".git").exists(),
    };

    // Add/update and save
    recent_projects.add_or_update(project);
    recent_projects.save(&recent_projects_path);
    tracing::debug!("Updated recent projects for: {}", project_path.display());
}
