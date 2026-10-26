use super::*;

/// Fallback target when the platform doesn't report a refresh rate.
const FALLBACK_REFRESH_HZ: f64 = 60.0;

/// Frame pacer for the Helio render thread: one frame per display refresh.
///
/// Presenting faster cannot show more frames — the compositor promotes at
/// most one Helio frame per refresh and surplus frames are overwritten in the
/// triple buffer's `ready` slot — so the thread sleeps until each refresh's
/// deadline. A frame that runs long is simply late: the next deadline is
/// re-anchored to now (no burst to catch up) and the following frame is
/// paced at the refresh again.
///
/// The rate never ratchets down. An adaptive target that dropped after a
/// burst of late frames (a flight's generation, a big edit) and recovered
/// only after a long run of on-time ones locked the viewport at a fraction
/// of the refresh long after the work was gone: 24-38 fps with the GPU
/// busy 7 ms a frame. The producer stays bounded without it:
/// `wait_for_frame_consumed` keeps the thread at most one frame ahead of
/// the compositor.
pub(super) struct FramePacer {
    /// The rate we aim at: the display refresh, unless overridden.
    pub(super) target_hz: f64,
    /// `PULSAR_VIEWPORT_FPS` pinned the target; display changes are ignored.
    target_overridden: bool,
    /// When the next frame is due.
    next_deadline: Instant,
    /// When the previous frame began; bounds how early a UI-aligned start may be.
    last_start: Instant,
}

impl FramePacer {
    pub(super) fn new(refresh_hz: Option<f64>) -> Self {
        // `PULSAR_VIEWPORT_FPS` overrides the target; `0` disables pacing and
        // lets `wait_for_frame_consumed` be the only thing governing the rate.
        let override_hz = std::env::var("PULSAR_VIEWPORT_FPS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v >= 0.0);

        let mut pacer = Self::with_target(match override_hz {
            Some(hz) => hz,
            None => refresh_hz.unwrap_or(FALLBACK_REFRESH_HZ),
        });
        pacer.target_overridden = override_hz.is_some();
        pacer
    }

    /// Retarget at a newly reported display refresh rate. On Wayland the window
    /// isn't associated with an output until the compositor sends
    /// `wl_surface.enter`, which typically lands after the viewport's first
    /// render — so the rate is often unknown when the render thread starts.
    pub(super) fn set_display_refresh(&mut self, refresh_hz: f64) {
        if self.target_overridden || refresh_hz <= 0.0 || refresh_hz == self.target_hz {
            return;
        }
        tracing::info!(
            "[VIEWPORT PACER] display refresh {:.0} -> {:.0} Hz",
            self.target_hz,
            refresh_hz
        );
        self.target_hz = refresh_hz;
    }

    /// Build a pacer aiming at `target_hz`, ignoring the environment (tests).
    pub(super) fn with_target(target_hz: f64) -> Self {
        Self {
            target_hz,
            target_overridden: false,
            next_deadline: Instant::now(),
            last_start: Instant::now(),
        }
    }

    pub(super) fn budget(&self) -> Option<Duration> {
        if self.target_hz <= 0.0 {
            None
        } else {
            Some(Duration::from_secs_f64(1.0 / self.target_hz))
        }
    }

    /// Run one frame with no pacing (the profiler's "uncap frame rate" option).
    /// Re-anchors the deadline to now so that turning the cap back on resumes at
    /// the target rate instead of bursting frames to catch up on the deadlines it
    /// skipped.
    pub(super) fn skip_wait(&mut self) {
        self.next_deadline = Instant::now();
    }

    /// Sleep until this frame is due. Returns immediately when uncapped or when
    /// the deadline has already passed.
    pub(super) fn wait_for_next_frame(&mut self) {
        let Some(budget) = self.budget() else {
            return;
        };
        let now = Instant::now();
        if self.next_deadline > now {
            // Anchoring to the previous deadline (rather than sleeping a fixed
            // amount after the render) keeps the average rate on target and
            // absorbs overshoot from Windows' coarse timer granularity on the
            // following frame.
            std::thread::sleep(self.next_deadline - now);
        }
        self.next_deadline = Self::next(self.next_deadline, budget, Instant::now());
    }

    /// Like [`wait_for_next_frame`](Self::wait_for_next_frame), but begin the
    /// frame when the UI thread begins its own.
    ///
    /// Helio publishes into a surface the UI composites at the *end* of its
    /// frame. Started on a free-running timer, Helio's frame landed after the
    /// UI had finished layout/prepaint/paint, so its CPU work and queue submit
    /// contended with the UI's present instead of hiding under the UI's
    /// CPU-bound phase. Starting on the UI's frame begin moves it there.
    ///
    /// `seen` is the UI frame count at the previous start. A UI frame that
    /// began since then satisfies the wait at once. Never earlier than half a
    /// period after the last start (so an uncapped UI cannot drive Helio past
    /// twice the refresh), never later than the normal deadline (so an idle UI
    /// leaves the timer pacing exactly as before). A start ahead of the
    /// deadline re-anchors the cadence to the UI's phase.
    pub(super) fn wait_for_next_frame_aligned(&mut self, seen: &mut u64) {
        let Some(budget) = self.budget() else {
            return;
        };
        let earliest = self.last_start + budget / 2;
        let now = Instant::now();
        if earliest > now {
            std::thread::sleep(earliest - now);
        }
        let deadline = self.next_deadline;
        let now = Instant::now();
        if deadline > now && gpui::ui_frame_signal::ui_frame_seq() == *seen {
            gpui::ui_frame_signal::wait_for_ui_frame_after(*seen, deadline - now);
        }
        *seen = gpui::ui_frame_signal::ui_frame_seq();
        let start = Instant::now();
        self.next_deadline = if start < deadline {
            start + budget
        } else {
            Self::next(deadline, budget, start)
        };
        self.last_start = start;
    }

    /// The deadline after `deadline` at `now`: a budget on, or a budget from
    /// now when the frame ran more than a budget late (resync instead of a
    /// burst of back-to-back frames).
    fn next(deadline: Instant, budget: Duration, now: Instant) -> Instant {
        let next = deadline + budget;
        if next < now {
            now + budget
        } else {
            next
        }
    }
}

#[cfg(test)]
mod pacer_tests {
    use super::{FramePacer, FALLBACK_REFRESH_HZ};
    use std::time::{Duration, Instant};

    /// The UI frame signal is process-global; tests that drive it take turns.
    static SIGNAL_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn aims_at_the_display_refresh_rate() {
        assert_eq!(FramePacer::with_target(144.0).target_hz, 144.0);
        assert_eq!(FramePacer::with_target(240.0).target_hz, 240.0);
    }

    #[test]
    fn falls_back_when_the_platform_reports_no_refresh_rate() {
        let pacer = FramePacer::with_target(FALLBACK_REFRESH_HZ);
        assert_eq!(pacer.target_hz, 60.0);
    }

    #[test]
    fn a_late_frame_resyncs_without_a_burst() {
        let budget = Duration::from_millis(16);
        let start = Instant::now();
        // On time: the next refresh.
        assert_eq!(
            FramePacer::next(start, budget, start + Duration::from_millis(5)),
            start + budget
        );
        // Far behind: a budget from now, not the missed deadlines.
        let now = start + Duration::from_millis(100);
        assert_eq!(FramePacer::next(start, budget, now), now + budget);
    }

    #[test]
    fn late_frames_never_lower_the_rate() {
        let mut pacer = FramePacer::with_target(144.0);
        for _ in 0..100 {
            pacer.next_deadline = Instant::now() - Duration::from_millis(50);
            pacer.wait_for_next_frame();
        }
        assert_eq!(pacer.target_hz, 144.0);
    }

    #[test]
    fn a_late_display_report_raises_the_fallback_target() {
        // Wayland: no output at startup, then `wl_surface.enter` reports 165 Hz.
        let mut pacer = FramePacer::with_target(FALLBACK_REFRESH_HZ);
        pacer.set_display_refresh(165.0);
        assert_eq!(pacer.target_hz, 165.0);
    }

    #[test]
    fn an_env_override_ignores_display_reports() {
        let mut pacer = FramePacer::with_target(30.0);
        pacer.target_overridden = true;
        pacer.set_display_refresh(144.0);
        assert_eq!(pacer.target_hz, 30.0);
    }

    #[test]
    fn a_ui_frame_begin_starts_the_frame_ahead_of_the_deadline() {
        let _serial = SIGNAL_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut pacer = FramePacer::with_target(10.0);
        pacer.last_start = Instant::now() - Duration::from_secs(1);
        pacer.next_deadline = Instant::now() + Duration::from_millis(400);
        let mut seen = gpui::ui_frame_signal::ui_frame_seq();
        let ui = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(30));
            gpui::ui_frame_signal::mark_ui_frame_begin();
        });
        let t = Instant::now();
        pacer.wait_for_next_frame_aligned(&mut seen);
        ui.join().unwrap();
        assert!(
            t.elapsed() < Duration::from_millis(250),
            "woke on the UI frame, not the deadline"
        );
        // The cadence follows the UI's phase from here.
        assert!(pacer.next_deadline < Instant::now() + Duration::from_millis(150));
    }

    #[test]
    fn an_idle_ui_leaves_the_timer_pacing_alone() {
        let _serial = SIGNAL_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut pacer = FramePacer::with_target(50.0);
        pacer.last_start = Instant::now() - Duration::from_secs(1);
        pacer.next_deadline = Instant::now() + Duration::from_millis(40);
        let mut seen = gpui::ui_frame_signal::ui_frame_seq();
        let t = Instant::now();
        pacer.wait_for_next_frame_aligned(&mut seen);
        assert!(t.elapsed() >= Duration::from_millis(35));
    }

    #[test]
    fn an_uncapped_pacer_stays_uncapped() {
        // `PULSAR_VIEWPORT_FPS=0` hands pacing entirely to the consumer-side
        // backpressure.
        let mut pacer = FramePacer::with_target(0.0);
        assert!(pacer.budget().is_none());
        pacer.wait_for_next_frame();
        assert_eq!(pacer.target_hz, 0.0);
    }
}
