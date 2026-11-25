//! What editor views that show a Helio renderer share: the UI triple buffer as
//! a render runtime's output ([`SurfaceTarget`]) and the pump that repaints a
//! view when its runtime published a frame ([`PublishedFramePump`]). The
//! runtimes themselves (who draws, when, on which thread) are the engine's
//! (`engine_backend::services::render_runtimes`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, Window};

mod surface_target;
pub use surface_target::{SurfaceTarget, FALLBACK_REFRESH_HZ, FRAME_CONSUMER_WAIT_TIMEOUT};

/// Repaints a GPUI view when its Helio worker publishes a new surface frame.
/// The worker uses `Release` and the pump uses `Acquire` so the surface publish
/// happens before the view is notified.
#[derive(Default)]
pub struct PublishedFramePump {
    started: bool,
}

impl PublishedFramePump {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start<T: 'static>(
        &mut self,
        frames_published: Arc<AtomicU64>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        if self.started {
            return;
        }
        self.started = true;
        cx.spawn_in(window, async move |this, cx| {
            let mut last_published = 0;
            // Polls at a frame's pace while frames arrive and backs off to
            // a tenth of that when none do (a hidden or minimized view, a
            // runtime the frame policy has put to sleep): a view that shows
            // nothing new keeps no 60 Hz timer running.
            let mut quiet = 0u32;
            loop {
                let wait = if quiet > 60 { 100 } else { 16 };
                cx.background_executor()
                    .timer(Duration::from_millis(wait))
                    .await;
                let published = frames_published.load(Ordering::Acquire);
                let changed = published != last_published;
                quiet = if changed { 0 } else { quiet.saturating_add(1) };
                last_published = published;
                let alive = this.update_in(cx, |_this, _window, cx| {
                    if changed {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Where `window` stands for the render runtimes' frame policy
/// (`render_runtimes::PacingPolicy`).
pub fn presence_of(window: &Window) -> engine_backend::services::render_runtimes::Presence {
    engine_backend::services::render_runtimes::Presence {
        focused: window.is_window_active(),
        minimized: window.is_window_minimized(),
    }
}

/// Tells `runtime` where the window showing it stands, now and whenever the
/// window is activated or deactivated (focus, minimize, restore), so the
/// service's frame policy draws it at the pace its window deserves: the one
/// place a view says this, hold the returned subscription as long as the view
/// shows the runtime.
pub fn follow_window<V: 'static>(
    runtime: engine_backend::services::render_runtimes::RuntimeHandle,
    window: &mut Window,
    cx: &mut Context<V>,
) -> gpui::Subscription {
    runtime.set_presence(presence_of(window));
    cx.observe_window_activation(window, move |_, window, _| {
        runtime.set_presence(presence_of(window));
    })
}
