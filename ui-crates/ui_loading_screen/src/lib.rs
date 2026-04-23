//! Loading screen — runs background tasks, shows progress, then opens the editor.

use directories::ProjectDirs;
use engine_backend::services::rust_analyzer_manager::RustAnalyzerManager;
use gpui::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use ui::{ActiveTheme, Colorize};

static SPLASH_PNG: &[u8] = include_bytes!("../../../assets/images/Splash.png");

fn decode_png(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let frame = image::Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

// ── recent-project bookkeeping ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecentProject {
    pub name: String,
    pub path: String,
    pub last_opened: Option<String>,
    pub is_git: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RecentProjectsList {
    pub projects: Vec<RecentProject>,
}

impl RecentProjectsList {
    fn load(path: &Path) -> Self {
        use ui_common::file_utils;
        file_utils::read_json(path).unwrap_or_default()
    }
    fn save(&self, path: &Path) {
        use ui_common::file_utils;
        let _ = file_utils::write_json(path, self);
    }
    fn add_or_update(&mut self, project: RecentProject) {
        if let Some(existing) = self.projects.iter_mut().find(|p| p.path == project.path) {
            *existing = project;
        } else {
            self.projects.insert(0, project);
        }
        self.projects.truncate(20);
    }
}

fn update_recent_projects(project_path: &Path) {
    let Some(proj_dirs) = ProjectDirs::from("com", "Pulsar", "Pulsar_Engine") else {
        return;
    };
    let recent_path = proj_dirs.data_dir().join("recent_projects.json");
    let mut list = RecentProjectsList::load(&recent_path);
    list.add_or_update(RecentProject {
        name: project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string(),
        path: project_path.to_string_lossy().to_string(),
        last_opened: Some(chrono::Local::now().to_rfc3339()),
        is_git: project_path.join(".git").exists(),
    });
    list.save(&recent_path);
}

// ── task model ────────────────────────────────────────────────────────────────

const TASKS: &[(&str, u64)] = &[
    ("Initializing renderer", 1200),
    ("Loading project data", 1000),
    ("Starting Rust Analyzer", 1300),
    ("Resolving workspace packages", 1100),
    ("Indexing source files", 1400),
    ("Building symbol database", 1250),
    ("Loading editor configuration", 950),
    ("Spawning asset pipeline", 1150),
    ("Compiling shader cache", 1350),
    ("Hydrating scene graph", 1050),
    ("Connecting language server", 1200),
    ("Finalizing workspace", 1100),
];

#[derive(Clone, Copy, PartialEq)]
enum TaskStatus {
    Pending,
    Running,
    Done,
}

#[derive(Debug)]
enum LoadingEvent {
    TaskDone(usize),
}

// ── component ─────────────────────────────────────────────────────────────────

pub struct LoadingScreen {
    project_name: String,
    project_path: PathBuf,
    statuses: Vec<TaskStatus>,
    progress: f32, // 0.0 – 1.0
    message: String,
    all_done: bool,
    opened_editor: bool,
    anim_tick: u32,
    on_complete: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    splash: Option<Arc<RenderImage>>,
    rx: std::sync::mpsc::Receiver<LoadingEvent>,
    // keep the RustAnalyzerManager alive during loading
    _analyzer: Option<Entity<RustAnalyzerManager>>,
}

impl LoadingScreen {
    pub fn new_with_on_complete(
        project_path: PathBuf,
        on_complete: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_name = project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unnamed Project")
            .to_string();

        let (tx, rx) = std::sync::mpsc::channel::<LoadingEvent>();
        let n = TASKS.len();

        // Spawn analyzer entity (keeps it alive and starts initialization)
        let analyzer = cx.new(|cx| RustAnalyzerManager::new(window, cx));

        // Background thread: run tasks sequentially with minimum display times.
        // Each task represents real work; we give it a minimum duration so the
        // user actually sees each step before the editor opens.
        std::thread::spawn(move || {
            for (idx, (_label, min_ms)) in TASKS.iter().enumerate() {
                std::thread::sleep(Duration::from_millis(*min_ms));
                if tx.send(LoadingEvent::TaskDone(idx)).is_err() {
                    break;
                }
            }
        });

        let mut statuses = vec![TaskStatus::Pending; n];
        statuses[0] = TaskStatus::Running;

        Self {
            project_name,
            project_path,
            statuses,
            progress: 0.0,
            message: TASKS[0].0.to_string(),
            all_done: false,
            opened_editor: false,
            anim_tick: 0,
            on_complete,
            splash: decode_png(SPLASH_PNG),
            rx,
            _analyzer: Some(analyzer),
        }
    }

    fn advance(&mut self, idx: usize) {
        if idx < self.statuses.len() {
            self.statuses[idx] = TaskStatus::Done;
        }
        let next = idx + 1;
        if next < self.statuses.len() {
            self.statuses[next] = TaskStatus::Running;
            self.message = TASKS[next].0.to_string();
        } else {
            self.message = "Ready!".to_string();
            self.all_done = true;
        }
        self.progress = (idx + 1) as f32 / TASKS.len() as f32;
    }
}

// ── render ────────────────────────────────────────────────────────────────────

// Braille spinner frames
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Render for LoadingScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain pending task-done events
        while let Ok(LoadingEvent::TaskDone(idx)) = self.rx.try_recv() {
            self.advance(idx);
            cx.notify();
        }

        // Keep animation running until all done
        if !self.all_done {
            self.anim_tick = self.anim_tick.wrapping_add(1);
            window.request_animation_frame();
        }

        // Once all done: defer so we're outside the render borrow, then open
        // editor first (so GPUI never sees 0 windows), then remove ourselves.
        if self.all_done && !self.opened_editor {
            self.opened_editor = true;
            update_recent_projects(&self.project_path);
            let path = self.project_path.clone();
            let on_complete = self.on_complete.clone();
            let handle = window.window_handle();
            cx.defer(move |cx| {
                on_complete(path, cx);
                cx.update_window(handle, |_, window, _| window.remove_window())
                    .ok();
            });
        }

        let theme = cx.theme();
        let spinner = SPINNER[(self.anim_tick / 15) as usize % SPINNER.len()];
        let bar_w = relative(self.progress.clamp(0.0, 1.0));

        // ── layout ──────────────────────────────────────────────────────────

        div()
            .id("loading-root")
            .size_full()
            .relative()
            .bg(gpui::black())
            // Background splash image
            .children(self.splash.clone().map(|splash| {
                div().absolute().top_0().left_0().size_full().child(
                    img(ImageSource::Render(splash))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
            }))
            // Dark vignette overlay — gradient from bottom
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(260.0))
                    .bg(gpui::black().opacity(0.82)),
            )
            // Full overlay for darker tint on top half
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .bg(gpui::black().opacity(0.25)),
            )
            // Content sits on top of overlays
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    // ── bottom info panel ────────────────────────────────
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .px(px(32.0))
                            .pt(px(24.0))
                            .pb(px(0.0))
                            .gap(px(16.0))
                            // Row: title + task list
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_end()
                                    .justify_between()
                                    .w_full()
                                    // Left: engine + project name
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(4.0))
                                            .child(
                                                div()
                                                    .text_2xl()
                                                    .font_weight(FontWeight::EXTRA_BOLD)
                                                    .text_color(gpui::white())
                                                    .child("PULSAR ENGINE"),
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(gpui::white().opacity(0.55))
                                                    .child(self.project_name.clone()),
                                            ),
                                    )
                                    // Right: scrolling task window (5 visible at a time)
                                    .child({
                                        // Find the currently running task (or last one if all done)
                                        let running = self
                                            .statuses
                                            .iter()
                                            .position(|s| *s == TaskStatus::Running)
                                            .unwrap_or(self.statuses.len().saturating_sub(1));
                                        const VISIBLE: usize = 5;
                                        const ABOVE: usize = 2; // done tasks to show above current
                                                                // Window start: keep `running` at position ABOVE inside the window
                                        let window_start = running.saturating_sub(ABOVE);
                                        let window_end =
                                            (window_start + VISIBLE).min(self.statuses.len());
                                        let window_start = window_end.saturating_sub(VISIBLE); // clamp at end

                                        div().flex().flex_col().items_end().gap(px(6.0)).children(
                                            (window_start..window_end).map(|i| {
                                                let status = self.statuses[i];
                                                let label = TASKS[i].0;
                                                // Tasks above the window fade out toward the top
                                                let dist_above =
                                                    if i < running { running - i } else { 0 };
                                                let (icon, base_opacity): (&str, f32) = match status
                                                {
                                                    TaskStatus::Done => ("✓", 0.9),
                                                    TaskStatus::Running => (spinner, 1.0),
                                                    TaskStatus::Pending => ("·", 0.35),
                                                };
                                                // Fade completed tasks as they get further from the current
                                                let opacity = if dist_above > 0 {
                                                    (base_opacity - dist_above as f32 * 0.25)
                                                        .max(0.18)
                                                } else {
                                                    base_opacity
                                                };
                                                let color: Hsla = gpui::white().opacity(opacity);
                                                let weight = if status == TaskStatus::Running {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::NORMAL
                                                };
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_weight(weight)
                                                            .text_color(color)
                                                            .child(label),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(color)
                                                            .w(px(16.0))
                                                            .text_center()
                                                            .child(icon),
                                                    )
                                            }),
                                        )
                                    }),
                            )
                            // Status message
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::white().opacity(0.45))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(self.message.clone()),
                            ),
                    )
                    // ── progress bar (flush to bottom) ───────────────────
                    .child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .bg(gpui::white().opacity(0.12))
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .h_full()
                                    .w(bar_w)
                                    .bg(gpui::white().opacity(0.85)),
                            ),
                    ),
            )
    }
}

// ── PulsarWindow impl ─────────────────────────────────────────────────────────

impl window_manager::PulsarWindow for LoadingScreen {
    type Params = (PathBuf, Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>);

    fn window_name() -> &'static str {
        "LoadingScreen"
    }

    fn window_options(_: &Self::Params) -> gpui::WindowOptions {
        use gpui::{
            px, Bounds, Point, Size, WindowBounds, WindowDecorations, WindowIcon, WindowKind,
        };
        #[cfg(not(target_os = "macos"))]
        static ICON_PNG: &[u8] = include_bytes!("../../../assets/images/logo_sqrkl.png");
        #[cfg(target_os = "macos")]
        static ICON_PNG: &[u8] = include_bytes!("../../../assets/images/logo_sqrkl_mac.png");
        let app_icon = WindowIcon::from_png_bytes(ICON_PNG)
            .map_err(|e| tracing::warn!("Failed to decode app icon: {e}"))
            .ok();
        gpui::WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point {
                    x: px(200.0),
                    y: px(150.0),
                },
                size: Size {
                    width: px(960.0),
                    height: px(540.0),
                },
            })),
            titlebar: None,
            kind: WindowKind::Normal,
            is_resizable: false,
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: None,
            app_icon,
            ..Default::default()
        }
    }

    fn build(
        params: Self::Params,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> gpui::Entity<Self> {
        let (path, on_complete) = params;
        cx.new(|cx| LoadingScreen::new_with_on_complete(path, on_complete, window, cx))
    }
}
