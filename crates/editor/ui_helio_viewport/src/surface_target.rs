//! The UI triple buffer as a render runtime's output.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine_backend::services::render_runtimes::FrameTarget;
use gpui::WgpuSurfaceHandle;

/// Fallback refresh when the platform reports none.
pub const FALLBACK_REFRESH_HZ: f64 = 60.0;

/// How long a published frame may wait for the compositor before the next one
/// replaces it anyway: the fast-blit presentation path never advances the
/// composited generation, so an unbounded wait would freeze the view.
pub const FRAME_CONSUMER_WAIT_TIMEOUT: Duration = Duration::from_millis(100);

/// A [`WgpuSurfaceHandle`] (wgpui's triple buffer) as a runtime's output. The
/// runtime draws into its back buffer under its submit permission, publishes
/// the frame silently and bumps `frames_published`, which the view's frame
/// pump watches to repaint only that view.
pub struct SurfaceTarget {
    surface: WgpuSurfaceHandle,
    frames_published: Arc<AtomicU64>,
    /// The display's refresh in millihertz, written by the view (0: unknown).
    display_refresh_millihertz: Arc<AtomicU32>,
    published_at: Mutex<Option<Instant>>,
}

impl SurfaceTarget {
    pub fn new(
        surface: WgpuSurfaceHandle,
        frames_published: Arc<AtomicU64>,
        display_refresh_millihertz: Arc<AtomicU32>,
    ) -> Self {
        // wgpui holds the device's lost and error callbacks; the renderer
        // reads the loss from wgpui's flag instead of taking them.
        engine_backend::services::device_loss::DeviceLoss::adopt(
            surface.device(),
            surface.device_lost_flag(),
        );
        // The window's device is the engine's: GPU work with no surface of its
        // own (the render graph pin probe) runs on it, not on a second one.
        engine_backend::services::render_runtimes::global()
            .offer_device(surface.device().clone(), surface.queue().clone());
        Self {
            surface,
            frames_published,
            display_refresh_millihertz,
            published_at: Mutex::new(None),
        }
    }
}

impl FrameTarget for SurfaceTarget {
    fn device(&self) -> wgpu::Device {
        self.surface.device().clone()
    }

    fn queue(&self) -> wgpu::Queue {
        self.surface.queue().clone()
    }

    fn format(&self) -> wgpu::TextureFormat {
        self.surface.format()
    }

    fn size(&self) -> (u32, u32) {
        self.surface.size()
    }

    fn is_lost(&self) -> bool {
        self.surface.is_device_lost()
    }

    fn ready(&self) -> bool {
        // At most one frame ahead of the compositor: rendering faster than it
        // consumes only piles up submissions nothing shows.
        !self.surface.has_unconsumed_frame()
            || self
                .published_at
                .lock()
                .ok()
                .and_then(|at| *at)
                .is_none_or(|at| at.elapsed() >= FRAME_CONSUMER_WAIT_TIMEOUT)
    }

    fn set_ready_waker(&self, waker: Option<Arc<dyn Fn() + Send + Sync>>) {
        // The compositor taking the published frame is what `ready` waits on.
        self.surface.set_consumed_waker(waker);
    }

    fn frame_interval(&self) -> Duration {
        // The profiler's "uncap frame rate" recording option lifts the target.
        if profiling::uncap_frame_rate() {
            return Duration::ZERO;
        }
        let millihertz = self.display_refresh_millihertz.load(Ordering::Relaxed);
        let hz = if millihertz > 0 {
            f64::from(millihertz) / 1000.0
        } else {
            FALLBACK_REFRESH_HZ
        };
        Duration::from_secs_f64(1.0 / hz.max(1.0))
    }

    fn draw(
        &self,
        draw: &mut dyn FnMut(&wgpu::TextureView, u32, u32) -> Option<wgpu::SubmissionIndex>,
    ) -> bool {
        let submission = {
            // Permission to submit on the shared device, held across the
            // render so a window resize (which reconfigures the swapchain on an
            // idle queue) cannot race it.
            let _guard = self.surface.submit_guard();
            let Some((view, (width, height))) = self.surface.back_view_with_size() else {
                gpui::render_stats::count("helio: no back buffer (skipped)");
                return false;
            };
            draw(&view, width, height)
        };
        let Some(submission) = submission else {
            return false;
        };
        // Silent present: publish for the compositor without a window redraw
        // from this thread; the view's pump repaints just this view when it
        // sees `frames_published` move (Release pairs with its Acquire).
        self.surface.present_synced_silent(submission);
        if let Ok(mut at) = self.published_at.lock() {
            *at = Some(Instant::now());
        }
        self.frames_published.fetch_add(1, Ordering::Release);
        self.surface.request_frame();
        true
    }
}
