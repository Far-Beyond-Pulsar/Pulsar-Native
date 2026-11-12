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
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let published = frames_published.load(Ordering::Acquire);
                let changed = published != last_published;
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
