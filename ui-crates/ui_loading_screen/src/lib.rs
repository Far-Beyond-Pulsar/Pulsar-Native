//! Loading Screen Component
//!
//! Full-featured loading screen from LoadingWindow

use gpui::*;
use gpui::Hsla;
use ui::{ActiveTheme, Colorize, Root};
use std::path::{Path, PathBuf};
use std::time::Duration;
use engine_backend::services::rust_analyzer_manager::{RustAnalyzerManager, AnalyzerStatus, AnalyzerEvent};
use engine_state::{EngineContext, WindowRequest, WindowContext};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

// we used to pull these types from ui_entry, but that created a cyclic
// dependency. The definitions are small so we duplicate them here.

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecentProject {
    pub name: String,
    pub path: String,
    pub last_opened: Option<String>,
    pub is_git: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct RecentProjectsList {
    pub projects: Vec<RecentProject>,
}

impl RecentProjectsList {
    fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    fn add_or_update(&mut self, project: RecentProject) {
        if let Some(existing) = self.projects.iter_mut().find(|p| p.path == project.path) {
            *existing = project;
        } else {
            self.projects.insert(0, project);
        }
        if self.projects.len() > 20 {
            self.projects.truncate(20);
        }
    }

    fn remove(&mut self, path: &str) {
        self.projects.retain(|p| p.path != path);
    }
}

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
    // raw pointer to allow closing window from async task
    window_ptr: *mut Window,
    // flag to open editor only once
    opened_editor: bool,
    // background thread channel receiver for progress events
    progress_rx: std::sync::mpsc::Receiver<LoadingEvent>,
}

#[derive(Clone)]
struct LoadingTask {
    name: String,
    status: TaskStatus,
}

// message sent from the timer thread to the UI
#[derive(Debug)]
enum LoadingEvent {
    TaskDone(usize),
    FrameRequest,
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

        // create progress channel
        let (tx, rx) = std::sync::mpsc::channel();
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
            window_ptr: window as *mut Window,
            opened_editor: false,
            progress_rx: rx,
        };

        let analyzer = cx.new(|cx| RustAnalyzerManager::new(window, cx));
        loading_screen.rust_analyzer = Some(analyzer.clone());
        // mark first task
        loading_screen.loading_tasks[0].status = TaskStatus::InProgress;
        loading_screen.analyzer_message = "Initializing renderer...".to_string();
        // spawn background thread for updates
        let tx_clone = tx.clone();
        std::thread::spawn(move || {
            println!("[bg] task1 start");
            std::thread::sleep(Duration::from_millis(100));
            tx_clone.send(LoadingEvent::TaskDone(0)).ok();

            println!("[bg] task2 start");
            std::thread::sleep(Duration::from_millis(100));
            tx_clone.send(LoadingEvent::TaskDone(1)).ok();

            println!("[bg] task3 start");
            std::thread::sleep(Duration::from_millis(100));
            tx_clone.send(LoadingEvent::TaskDone(2)).ok();
        });

        loading_screen
    }
}

impl Render for LoadingScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // keep the animation loop running regardless of events
        _window.request_animation_frame();
        println!("[render] called, progress={} task={} initial_done={}", self.progress, self.current_task_index, self.initial_tasks_complete);
        // process any pending progress events
        while let Ok(ev) = self.progress_rx.try_recv() {
            println!("[render] got event {:?}", ev);
            match ev {
                LoadingEvent::TaskDone(idx) => {
                    if idx < self.loading_tasks.len() {
                        self.loading_tasks[idx].status = TaskStatus::Completed;
                        if idx + 1 < self.loading_tasks.len() {
                            self.loading_tasks[idx + 1].status = TaskStatus::InProgress;
                        }
                        self.progress = ((idx + 1) as f32 / self.loading_tasks.len() as f32) * 100.0;
                        self.analyzer_message = if idx + 1 < self.loading_tasks.len() {
                            self.loading_tasks[idx + 1].name.clone()
                        } else {
                            "Ready!".to_string()
                        };
                        cx.notify();
                    }
                    if idx + 1 == self.loading_tasks.len() {
                        self.initial_tasks_complete = true;
                    }
                }
                LoadingEvent::FrameRequest => {
                    _window.request_animation_frame();
                }
            }
        }
        // once all tasks done, open editor & schedule closing
        if self.initial_tasks_complete && !self.opened_editor {
            self.opened_editor = true;
            if let Some(engine_context) = EngineContext::global() {
                let pathbuf = self.project_path.clone();
                let ec = engine_context.clone();
                let wid2 = ec.next_window_id();
                ec.register_window(
                    wid2,
                    WindowContext::new(
                        wid2,
                        WindowRequest::ProjectEditor { project_path: pathbuf.to_string_lossy().to_string() },
                    ),
                );
                let opts = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(100.0), px(100.0)),
                        size(px(800.0), px(600.0)),
                    ))),
                    titlebar: None,
                    kind: WindowKind::Normal,
                    is_resizable: true,
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    ..Default::default()
                };
                cx.open_window(opts, move |window, cx| {
                    crate::create_loading_component(pathbuf.clone(), wid2, window, cx)
                })
                .expect("failed to open project editor");

                let close_id = self.window_id;
                let ec2 = engine_context.clone();
                let ptr = self.window_ptr;
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    unsafe { (&mut *ptr).remove_window(); }
                    ec2.unregister_window(&close_id);
                });
            }
        }
        // request a frame every render call to keep loop alive
        _window.request_animation_frame();

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
