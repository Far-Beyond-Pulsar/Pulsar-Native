//! LoadingScreen component — struct, constructor, Render, and PulsarWindow impl.

use gpui::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use ui::ActiveTheme;

use engine_backend::services::RustAnalyzerManager;

use crate::recent_projects::update_recent_projects;
use crate::tasks::{LoadingEvent, TaskStatus, TASKS};

static SPLASH_PNG: &[u8] = include_bytes!("../../../assets/images/Splash.png");

fn decode_png(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let rgba = image::load_from_memory(bytes).ok()?.into_rgba8();
    let frame = image::Frame::new(rgba);
    Some(Arc::new(RenderImage::new(smallvec::smallvec![frame])))
}

// Braille spinner frames
const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const MIN_DISPLAY_DURATION: Duration = Duration::from_secs(2);

pub struct LoadingScreen {
    project_name: String,
    project_path: PathBuf,
    statuses: Vec<TaskStatus>,
    progress: f32, // 0.0 – 1.0
    message: String,
    all_done: bool,
    opened_editor: bool,
    anim_tick: u32,
    created_at: std::time::Instant,
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

        let analyzer = cx.new(|cx| RustAnalyzerManager::new(window, cx));

        // Pre-loading work runs immediately in a sub-thread, in parallel with the
        // visual task display, so that by the time the animation finishes:
        //   • the default scene directory is guaranteed to exist on disk
        //   • the default.level file is in the OS page cache (if it exists)
        //   • the palette file list is ready in memory
        //
        // This means PulsarApp::new_internal never has to do blocking disk I/O on
        // the GPUI main thread — the transition from loading screen to editor is
        // essentially instant.
        // Background thread: run every task in order, measure real wall-clock
        // time for each one, and report results back to the UI thread.
        // There are no artificial sleeps — the loading screen advances as fast
        // as the work actually takes.
        let project_path_for_thread = project_path.clone();
        std::thread::spawn(move || {
            let project = project_path_for_thread.as_path();
            for (idx, (label, task_fn)) in TASKS.iter().enumerate() {
                let result = task_fn(project);
                tracing::info!(
                    "[Loading] {:>3}ms  {}{}",
                    result.elapsed.as_millis(),
                    label,
                    result
                        .detail
                        .as_deref()
                        .map(|d| format!(" — {d}"))
                        .unwrap_or_default(),
                );
                if tx
                    .send(LoadingEvent::TaskDone {
                        idx,
                        elapsed: result.elapsed,
                        detail: result.detail,
                    })
                    .is_err()
                {
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
            created_at: std::time::Instant::now(),
            on_complete,
            splash: decode_png(SPLASH_PNG),
            rx,
            _analyzer: Some(analyzer),
        }
    }

    fn advance(&mut self, idx: usize, elapsed: Duration, _detail: Option<String>) {
        if idx < self.statuses.len() {
            self.statuses[idx] = TaskStatus::Done(elapsed);
        }
        let next = idx + 1;
        if next < self.statuses.len() {
            self.statuses[next] = TaskStatus::Running;
            self.message = TASKS[next].0.to_string();
        } else {
            let total_ms: u128 = self
                .statuses
                .iter()
                .map(|s| match s {
                    TaskStatus::Done(d) => d.as_millis(),
                    _ => 0,
                })
                .sum();
            self.message = format!("Ready! ({total_ms}ms total)");
            self.all_done = true;
        }
        self.progress = (idx + 1) as f32 / TASKS.len() as f32;
    }
}

impl Render for LoadingScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain pending task-done events
        while let Ok(LoadingEvent::TaskDone { idx, elapsed, detail }) = self.rx.try_recv() {
            self.advance(idx, elapsed, detail);
            cx.notify();
        }

        // Keep animation running until all done
        if !self.all_done {
            self.anim_tick = self.anim_tick.wrapping_add(1);
            window.request_animation_frame();
        }

        // Once all done: wait MIN_DISPLAY_DURATION so the splash is always
        // visible for at least 2s, then defer the transition to the editor.
        if self.all_done && !self.opened_editor {
            let elapsed = self.created_at.elapsed();
            if elapsed < MIN_DISPLAY_DURATION {
                window.request_animation_frame();
            } else {
                self.opened_editor = true;
                update_recent_projects(&self.project_path);
                let path = self.project_path.clone();
                let on_complete = self.on_complete.clone();
                let handle = window.window_handle();
                cx.defer(move |cx| {
                    on_complete(path, cx);
                    cx.update_window(handle, |_, window, _| window.remove_window());
                });
            }
        }

        let _theme = cx.theme();
        let spinner = SPINNER[(self.anim_tick / 15) as usize % SPINNER.len()];
        let bar_w = relative(self.progress.clamp(0.0, 1.0));

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
                                        let running = self
                                            .statuses
                                            .iter()
                                            .position(|s| *s == TaskStatus::Running)
                                            .unwrap_or(self.statuses.len().saturating_sub(1));
                                        const VISIBLE: usize = 5;
                                        const ABOVE: usize = 2;
                                        let window_start = running.saturating_sub(ABOVE);
                                        let window_end =
                                            (window_start + VISIBLE).min(self.statuses.len());
                                        let window_start = window_end.saturating_sub(VISIBLE);

                                        div().flex().flex_col().items_end().gap(px(6.0)).children(
                                            (window_start..window_end).map(|i| {
                                                let status = self.statuses[i];
                                                let label = TASKS[i].0;
                                                let dist_above = running.saturating_sub(i);
                                                let (icon, base_opacity): (&str, f32) =
                                                    match status {
                                                        TaskStatus::Done(_) => ("✓", 0.9),
                                                        TaskStatus::Running => (spinner, 1.0),
                                                        TaskStatus::Pending => ("·", 0.35),
                                                    };
                                                let opacity = if dist_above > 0 {
                                                    (base_opacity - dist_above as f32 * 0.25)
                                                        .max(0.18)
                                                } else {
                                                    base_opacity
                                                };
                                                let color: Hsla = gpui::white().opacity(opacity);
                                                let is_running =
                                                    matches!(status, TaskStatus::Running);
                                                let weight = if is_running {
                                                    FontWeight::SEMIBOLD
                                                } else {
                                                    FontWeight::NORMAL
                                                };
                                                // Show real elapsed time for completed tasks.
                                                let timing_label: Option<String> =
                                                    if let TaskStatus::Done(d) = status {
                                                        Some(format!("{}ms", d.as_millis()))
                                                    } else {
                                                        None
                                                    };
                                                div()
                                                    .flex()
                                                    .flex_row()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .children(timing_label.map(|t| {
                                                        div()
                                                            .text_xs()
                                                            .font_weight(FontWeight::NORMAL)
                                                            .text_color(
                                                                gpui::white().opacity(opacity * 0.6),
                                                            )
                                                            .child(t)
                                                    }))
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
            window_background: gpui::WindowBackgroundAppearance::Opaque,
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
