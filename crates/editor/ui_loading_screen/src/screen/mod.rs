use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine_backend::services::RustAnalyzerManager;
use gpui::*;

use crate::recent_projects::update_recent_projects;
use crate::tasks::{LoadingEvent, TaskStatus, TASKS};

mod components;
use components::*;

const MIN_DISPLAY_DURATION: Duration = Duration::from_secs(2);

pub struct LoadingScreen {
    project_name: String,
    project_path: PathBuf,
    statuses: Vec<TaskStatus>,
    progress: f32,
    message: String,
    all_done: bool,
    opened_editor: bool,
    anim_tick: u32,
    created_at: std::time::Instant,
    on_complete: Arc<dyn Fn(PathBuf, &mut App) + Send + Sync>,
    splash: Option<Arc<RenderImage>>,
    rx: std::sync::mpsc::Receiver<LoadingEvent>,
    running_since: Option<Instant>,
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
            created_at: Instant::now(),
            running_since: Some(Instant::now()),
            on_complete,
            splash: splash_handle(),
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
            self.running_since = Some(Instant::now());
            self.message = TASKS[next].0.to_string();
        } else {
            self.running_since = None;
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
        while let Ok(LoadingEvent::TaskDone { idx, elapsed, detail }) = self.rx.try_recv() {
            self.advance(idx, elapsed, detail);
            cx.notify();
        }

        if !self.all_done {
            self.anim_tick = self.anim_tick.wrapping_add(1);
            window.request_animation_frame();
        }

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

        let running_elapsed = self.running_since.map(|t| t.elapsed());

        div()
            .id("loading-root")
            .size_full()
            .relative()
            .bg(gpui::black())
            .children(splash_background(&self.splash))
            .child(vignette_overlay())
            .child(top_tint())
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .px(px(32.0))
                            .pt(px(24.0))
                            .pb(px(0.0))
                            .gap(px(16.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_end()
                                    .justify_between()
                                    .w_full()
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
                                    .child(task_list_widget(self.anim_tick, &self.statuses, running_elapsed)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::white().opacity(0.45))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(self.message.clone()),
                            ),
                    )
                    .child(progress_bar_widget(self.progress, self.anim_tick)),
            )
    }
}
