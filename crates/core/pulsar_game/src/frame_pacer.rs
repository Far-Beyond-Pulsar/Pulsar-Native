//! Phase-locking the simulation tick to the display.
//!
//! A tick on a free-running timer finishes at a random point of the frame
//! interval, so the state a frame draws is on average half an interval older
//! than it has to be, and an input waits a further random part of an interval
//! for the tick that consumes it. When the simulation runs at the display's
//! rate (the default, `simulation.tick_rate_hz = 0`) the tick can instead be
//! started so that it *ends just before the next frame samples the world*.
//!
//! The frame thread reports each surface acquire ([`FramePacer::frame_acquired`]:
//! the display-paced moment a frame begins to read the scene) and the tick
//! thread reports what a tick costs ([`FramePacer::tick_finished`]);
//! [`FramePacer::next_tick`] then names the instant the next tick should start:
//! a predicted acquire minus the tick's recent cost and a small margin. It
//! answers `None` (the caller keeps its own timer) whenever the prediction is
//! not trustworthy: no frames yet, frames stopped coming, or a tick rate that
//! is not the display's (a tick is a fixed step of simulated time, so ticking
//! faster than its rate would speed the game up).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Shared between the frame thread and the tick thread; plain atomics.
pub struct FramePacer {
    epoch: Instant,
    /// The last acquire, in nanoseconds after `epoch` plus 1 (0 = none yet).
    last_acquire: AtomicU64,
    /// Smoothed time between two acquires, nanoseconds.
    period: AtomicU64,
    /// Smoothed cost of a tick, nanoseconds, and a slowly decaying high mark.
    tick_cost: AtomicU64,
    tick_high: AtomicU64,
}

/// A tick may not start closer than this to a frame's acquire: what the
/// scheduler's wake-up and the lock hand-over can add.
const MARGIN: Duration = Duration::from_micros(250);

impl FramePacer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            last_acquire: AtomicU64::new(0),
            period: AtomicU64::new(0),
            tick_cost: AtomicU64::new(0),
            tick_high: AtomicU64::new(0),
        })
    }

    fn ns(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch).as_nanos() as u64 + 1
    }

    /// The frame thread acquired its surface texture: the display-paced moment
    /// a frame starts reading the scene.
    pub fn frame_acquired(&self) {
        self.frame_acquired_at(Instant::now());
    }

    fn frame_acquired_at(&self, at: Instant) {
        let now = self.ns(at);
        let previous = self.last_acquire.swap(now, Ordering::AcqRel);
        if previous == 0 || now <= previous {
            return;
        }
        let gap = now - previous;
        let period = self.period.load(Ordering::Acquire);
        let smoothed = if period == 0 {
            gap
        } else if gap > period * 2 || gap * 2 < period {
            // A hitch or a doubled event is not the display's period.
            return;
        } else {
            (period * 15 + gap) / 16
        };
        self.period.store(smoothed, Ordering::Release);
    }

    /// A tick took `cost`.
    pub fn tick_finished(&self, cost: Duration) {
        let cost = cost.as_nanos().min(u128::from(u64::MAX / 4)) as u64;
        let mean = self.tick_cost.load(Ordering::Relaxed);
        self.tick_cost.store(
            if mean == 0 { cost } else { (mean * 7 + cost) / 8 },
            Ordering::Relaxed,
        );
        let high = self.tick_high.load(Ordering::Relaxed);
        // A high mark that forgets slowly: up at once, down by a 64th a tick.
        self.tick_high.store(cost.max(high - high / 64), Ordering::Relaxed);
    }

    /// The smoothed time between frames, when known.
    pub fn period(&self) -> Option<Duration> {
        match self.period.load(Ordering::Acquire) {
            0 => None,
            ns => Some(Duration::from_nanos(ns)),
        }
    }

    /// When the next tick should start so that it ends just before the next
    /// frame samples the world, or `None` to keep a free-running timer.
    /// `previous_start` is when the tick that just ran started, `step` the
    /// tick's fixed simulated step, `now` the current time.
    pub fn next_tick(&self, previous_start: Instant, step: Duration, now: Instant) -> Option<Instant> {
        let acquire = self.last_acquire.load(Ordering::Acquire);
        let period = self.period.load(Ordering::Acquire);
        if acquire == 0 || period < 1_000_000 || period > 100_000_000 {
            return None;
        }
        let now_ns = self.ns(now);
        if now_ns.saturating_sub(acquire) > period * 8 {
            return None; // frames are not being drawn (hidden, occluded)
        }
        let step_ns = step.as_nanos() as u64;
        if step_ns.abs_diff(period) * 10 > period {
            return None; // the tick rate is not the display's
        }
        // Lead: the tick's high mark with half again, and the margin, but never
        // more than half a period (a tick that slow cannot be placed).
        let lead = (self.tick_high.load(Ordering::Relaxed).max(self.tick_cost.load(Ordering::Relaxed)) * 3 / 2
            + MARGIN.as_nanos() as u64)
            .min(period / 2);
        // No sooner than half a period after the previous start (never two ticks
        // for one frame) and not in the past.
        let floor = (self.ns(previous_start) + period / 2).max(now_ns);
        let anchor = acquire as i128 - lead as i128;
        let behind = (floor as i128 - anchor).max(0);
        let frames = (behind + period as i128 - 1) / period as i128;
        let target = anchor + frames * period as i128;
        Some(self.epoch + Duration::from_nanos((target - 1).max(0) as u64))
    }
}

/// Sleep until `at`: the timer for most of it, then a spin for the last
/// stretch, because a sleep wakes a scheduler quantum late and the point of
/// pacing a tick is its phase.
pub fn sleep_until(at: Instant) {
    const SPIN: Duration = Duration::from_micros(700);
    let now = Instant::now();
    if let Some(sleep) = at.checked_duration_since(now).and_then(|d| d.checked_sub(SPIN)) {
        std::thread::sleep(sleep);
    }
    while Instant::now() < at {
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn nothing_is_predicted_before_frames_arrive() {
        let pacer = FramePacer::new();
        let now = Instant::now();
        assert!(pacer.next_tick(now, ms(7), now).is_none());
    }

    #[test]
    fn the_tick_is_placed_just_before_the_next_acquire() {
        let pacer = FramePacer::new();
        let start = Instant::now();
        // Frames every 7 ms; the last acquire was at `start + 70`.
        for frame in 0..=10 {
            pacer.frame_acquired_at(start + ms(frame * 7));
        }
        pacer.tick_finished(Duration::from_micros(400));
        let now = start + ms(71);
        let next = pacer.next_tick(start + ms(64), ms(7), now).expect("paced");
        let next_acquire = start + ms(77);
        let lead = next_acquire.duration_since(next);
        assert!(lead >= Duration::from_micros(250), "{lead:?} margin");
        assert!(lead <= ms(2), "{lead:?} is close to the acquire");
        assert!(next >= now);
    }

    #[test]
    fn a_tick_rate_that_is_not_the_displays_keeps_its_own_timer() {
        let pacer = FramePacer::new();
        let start = Instant::now();
        for frame in 0..=10 {
            pacer.frame_acquired_at(start + ms(frame * 7));
        }
        // 60 Hz simulation on a 143 Hz display.
        assert!(pacer.next_tick(start + ms(64), Duration::from_micros(16_667), start + ms(71)).is_none());
    }

    #[test]
    fn frames_that_stopped_keep_the_timer_too() {
        let pacer = FramePacer::new();
        let start = Instant::now();
        for frame in 0..=10 {
            pacer.frame_acquired_at(start + ms(frame * 7));
        }
        assert!(pacer.next_tick(start + ms(64), ms(7), start + ms(500)).is_none());
    }

    #[test]
    fn a_hitch_does_not_move_the_period() {
        let pacer = FramePacer::new();
        let start = Instant::now();
        for frame in 0..=10 {
            pacer.frame_acquired_at(start + ms(frame * 7));
        }
        let before = pacer.period().unwrap();
        pacer.frame_acquired_at(start + ms(70 + 60));
        assert_eq!(pacer.period().unwrap(), before);
    }
}
