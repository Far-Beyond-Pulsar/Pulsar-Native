use super::*;


/// Fallback target when the platform doesn't report a refresh rate.
const FALLBACK_REFRESH_HZ: f64 = 60.0;

/// Never pace slower than this, however badly frames are missing. Below it the
/// viewport stops feeling interactive and dropping further doesn't help.
const MIN_TARGET_HZ: f64 = 20.0;

/// Adaptive frame pacer for the Helio render thread.
///
/// Starts at the display's refresh rate — presenting faster cannot show more
/// frames, since the compositor promotes at most one Helio frame per refresh
/// and surplus frames are overwritten in the triple buffer's `ready` slot — and
/// backs off when the renderer can't sustain it, recovering when it can.
///
/// Backing off is not cosmetic. A thread that keeps aiming at a rate it cannot
/// hit spends every frame late, never sleeps, and turns into the unbounded
/// producer that `wait_for_frame_consumed` exists to prevent. Dropping the
/// target restores the idle gap between frames.
pub(super) struct FramePacer {
    /// The rate we'd like to hit — the display refresh, unless overridden.
    ceiling_hz: f64,
    /// The rate we're currently aiming at, `<= ceiling_hz`.
    pub(super) target_hz: f64,
    /// When the next frame is due.
    next_deadline: Instant,
    /// Consecutive frames that overran their budget.
    late_streak: u32,
    /// Consecutive frames that met their deadline at the current target.
    on_time_streak: u32,
}

impl FramePacer {
    /// Frames in a row that must overrun before dropping the target.
    const LATE_STREAK_TO_DROP: u32 = 8;
    /// Frames in a row that must meet their deadline before testing the ceiling
    /// again. Recovery is a single jump; if the ceiling is unstable, the
    /// late-frame logic backs it off again instead of slowly curving upward.
    const ON_TIME_STREAK_TO_RAISE: u32 = 30;
    /// Multiplier applied on each drop.
    const DROP_FACTOR: f64 = 0.8;

    pub(super) fn new(refresh_hz: Option<f64>) -> Self {
        // `PULSAR_VIEWPORT_FPS` overrides the ceiling; `0` disables pacing and
        // lets `wait_for_frame_consumed` be the only thing governing the rate.
        let override_hz = std::env::var("PULSAR_VIEWPORT_FPS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v >= 0.0);

        Self::with_ceiling(match override_hz {
            Some(hz) => hz,
            None => refresh_hz.unwrap_or(FALLBACK_REFRESH_HZ),
        })
    }

    /// Build a pacer aiming at `ceiling_hz`, ignoring the environment. Split out
    /// from [`new`](Self::new) so the adaptation logic is testable without an
    /// ambient `PULSAR_VIEWPORT_FPS` changing the outcome.
    pub(super) fn with_ceiling(ceiling_hz: f64) -> Self {
        Self {
            ceiling_hz,
            target_hz: ceiling_hz,
            next_deadline: Instant::now(),
            late_streak: 0,
            on_time_streak: 0,
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
    /// skipped, and clears the adaptation streaks that no longer describe anything.
    pub(super) fn skip_wait(&mut self) {
        self.next_deadline = Instant::now();
        self.late_streak = 0;
        self.on_time_streak = 0;
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
            self.on_time_streak = self.on_time_streak.saturating_add(1);
            self.late_streak = 0;
        } else {
            // Missed the deadline: the previous frame ran long.
            self.late_streak = self.late_streak.saturating_add(1);
            self.on_time_streak = 0;
        }

        self.next_deadline += budget;
        let now = Instant::now();
        if self.next_deadline < now {
            // More than a frame behind — resync instead of trying to catch up
            // with a burst of back-to-back frames.
            self.next_deadline = now + budget;
        }

        self.adapt();
    }

    pub(super) fn adapt(&mut self) {
        if self.target_hz <= 0.0 {
            return;
        }

        if self.late_streak >= Self::LATE_STREAK_TO_DROP {
            let dropped = (self.target_hz * Self::DROP_FACTOR).max(MIN_TARGET_HZ);
            if dropped < self.target_hz {
                tracing::debug!(
                    "[VIEWPORT PACER] target {:.0} -> {:.0} Hz (missed {} frames in a row)",
                    self.target_hz,
                    dropped,
                    self.late_streak
                );
                self.target_hz = dropped;
            }
            self.late_streak = 0;
        } else if self.on_time_streak >= Self::ON_TIME_STREAK_TO_RAISE
            && self.target_hz < self.ceiling_hz
        {
            tracing::debug!(
                "[VIEWPORT PACER] target {:.0} -> {:.0} Hz (stability window passed)",
                self.target_hz,
                self.ceiling_hz
            );
            self.target_hz = self.ceiling_hz;
            self.on_time_streak = 0;
        }
    }
}

#[cfg(test)]
mod pacer_tests {
    use super::{FramePacer, FALLBACK_REFRESH_HZ, MIN_TARGET_HZ};

    /// Drive `adapt` as if `n` frames in a row missed their deadline.
    fn run_late_frames(pacer: &mut FramePacer, n: u32) {
        for _ in 0..n {
            pacer.late_streak += 1;
            pacer.on_time_streak = 0;
            pacer.adapt();
        }
    }

    /// Drive `adapt` as if `n` frames in a row met their deadline.
    fn run_on_time_frames(pacer: &mut FramePacer, n: u32) {
        for _ in 0..n {
            pacer.on_time_streak += 1;
            pacer.late_streak = 0;
            pacer.adapt();
        }
    }

    #[test]
    fn starts_at_the_display_refresh_rate() {
        assert_eq!(FramePacer::with_ceiling(144.0).target_hz, 144.0);
        assert_eq!(FramePacer::with_ceiling(240.0).target_hz, 240.0);
    }

    #[test]
    fn falls_back_when_the_platform_reports_no_refresh_rate() {
        // `new(None)` with no override must land on the fallback, not on zero.
        let pacer = FramePacer::with_ceiling(FALLBACK_REFRESH_HZ);
        assert_eq!(pacer.target_hz, 60.0);
    }

    #[test]
    fn holds_the_target_while_frames_are_on_time() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_on_time_frames(&mut pacer, 1000);
        assert_eq!(pacer.target_hz, 144.0, "should never exceed the ceiling");
    }

    #[test]
    fn a_short_burst_of_late_frames_does_not_drop_the_target() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP - 1);
        assert_eq!(pacer.target_hz, 144.0);
    }

    #[test]
    fn drops_the_target_after_sustained_late_frames() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP);
        assert!(
            pacer.target_hz < 144.0,
            "expected a drop, got {}",
            pacer.target_hz
        );
    }

    #[test]
    fn never_drops_below_the_floor() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        // Far more late frames than could ever be needed to bottom out.
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP * 500);
        assert!(
            pacer.target_hz >= MIN_TARGET_HZ,
            "target {} fell below the floor {}",
            pacer.target_hz,
            MIN_TARGET_HZ
        );
    }

    #[test]
    fn recovers_toward_the_ceiling_but_never_past_it() {
        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP * 6);
        let dropped = pacer.target_hz;
        assert!(dropped < 144.0);

        run_on_time_frames(&mut pacer, FramePacer::ON_TIME_STREAK_TO_RAISE * 100);
        assert!(
            pacer.target_hz > dropped,
            "expected recovery from {}, got {}",
            dropped,
            pacer.target_hz
        );
        assert_eq!(
            pacer.target_hz, 144.0,
            "sustained headroom should return all the way to the display refresh"
        );
    }

    #[test]
    fn recovery_waits_then_jumps_to_the_ceiling() {
        assert!(FramePacer::ON_TIME_STREAK_TO_RAISE > FramePacer::LATE_STREAK_TO_DROP);

        let mut pacer = FramePacer::with_ceiling(144.0);
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP);
        let dropped = pacer.target_hz;

        run_on_time_frames(&mut pacer, FramePacer::ON_TIME_STREAK_TO_RAISE - 1);
        assert_eq!(pacer.target_hz, dropped);

        run_on_time_frames(&mut pacer, 1);
        assert_eq!(pacer.target_hz, 144.0);
    }

    #[test]
    fn an_uncapped_pacer_stays_uncapped() {
        // `PULSAR_VIEWPORT_FPS=0` hands pacing entirely to the consumer-side
        // backpressure; adapt must not resurrect a target from zero.
        let mut pacer = FramePacer::with_ceiling(0.0);
        assert!(pacer.budget().is_none());
        run_late_frames(&mut pacer, FramePacer::LATE_STREAK_TO_DROP * 4);
        assert!(pacer.budget().is_none());
        assert_eq!(pacer.target_hz, 0.0);
    }
}
