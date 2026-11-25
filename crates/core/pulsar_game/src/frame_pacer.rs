//! Placing the simulation tick just before the frame that draws it.
//!
//! A tick on a free-running timer finishes at a random point of the frame
//! interval, so the state a frame draws is on average half an interval older
//! than it has to be (more when the display refreshes faster than the game
//! draws: ticks every 6 ms against frames every 8.5 ms left the state 4.3 ms
//! old at the frame's start, measured). A tick is a fixed step of simulated
//! time, so how many run is not negotiable, but *when* each one runs inside its
//! window is: tick `k` is due at `start + k * step` and may run any time up to
//! one step later. The pacer picks the moment in that window at which the tick
//! *ends just before the next frame reads the world*.
//!
//! The frame thread reports when it starts to read the world
//! ([`FramePacer::sampling`]) and the tick thread what a tick costs
//! ([`FramePacer::tick_finished`]); [`FramePacer::next_start`] then answers the
//! instant the next tick should begin. It answers `None` (the caller keeps its
//! timer) when the prediction is not trustworthy: no frames yet, or frames
//! stopped coming (hidden, occluded). The tick's schedule never drifts: the
//! caller advances its due time by the step whatever the pacer says.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Shared between the frame thread and the tick thread; plain atomics.
pub struct FramePacer {
    epoch: Instant,
    /// The last time the frame began to read the world, in nanoseconds after
    /// `epoch` plus 1 (0 = never).
    last_sample: AtomicU64,
    /// Smoothed time between two reads, nanoseconds.
    period: AtomicU64,
    /// Smoothed cost of a tick, nanoseconds, and a slowly decaying high mark.
    tick_cost: AtomicU64,
    tick_high: AtomicU64,
    /// Smoothed absolute error of the last prediction of a sample time, ns:
    /// what the margin must cover for the tick not to end after the frame has
    /// started to read.
    jitter: AtomicU64,
}

/// A tick may not end closer than this to the frame's read: what the
/// scheduler's wake-up and the lock hand-over can add.
const MARGIN: Duration = Duration::from_micros(200);

impl FramePacer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: Instant::now(),
            last_sample: AtomicU64::new(0),
            period: AtomicU64::new(0),
            tick_cost: AtomicU64::new(0),
            tick_high: AtomicU64::new(0),
            jitter: AtomicU64::new(0),
        })
    }

    fn ns(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch).as_nanos() as u64 + 1
    }

    /// The frame thread is about to read the world (step the scene and take the
    /// camera): the moment the tick before it should have ended.
    pub fn sampling(&self) {
        self.sampling_at(Instant::now());
    }

    fn sampling_at(&self, at: Instant) {
        let now = self.ns(at);
        let previous = self.last_sample.swap(now, Ordering::AcqRel);
        if previous == 0 || now <= previous {
            return;
        }
        let gap = now - previous;
        let period = self.period.load(Ordering::Acquire);
        if period == 0 {
            self.period.store(gap, Ordering::Release);
            return;
        }
        // How far off the prediction was, then the period itself; a hitch or a
        // doubled event is neither.
        let error = gap.abs_diff(period);
        if gap > period * 3 || gap * 3 < period {
            return;
        }
        let jitter = self.jitter.load(Ordering::Relaxed);
        self.jitter.store((jitter * 7 + error) / 8, Ordering::Relaxed);
        self.period.store((period * 15 + gap) / 16, Ordering::Release);
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

    /// When the tick due at `due` should start, within `due ..= due + window`:
    /// so that it ends just before the next frame reads the world, or `None` to
    /// run it when due. `now` is the current time.
    pub fn next_start(&self, due: Instant, window: Duration, now: Instant) -> Option<Instant> {
        let last = self.last_sample.load(Ordering::Acquire);
        let period = self.period.load(Ordering::Acquire);
        if last == 0 || period < 500_000 || period > 100_000_000 {
            return None;
        }
        let now_ns = self.ns(now);
        if now_ns.saturating_sub(last) > period * 8 {
            return None; // frames are not being drawn (hidden, occluded)
        }
        let cost = self
            .tick_high
            .load(Ordering::Relaxed)
            .max(self.tick_cost.load(Ordering::Relaxed));
        // End this long before the read: the tick's high mark with half again,
        // what the prediction has been off by, and the margin.
        let lead = cost * 3 / 2 + self.jitter.load(Ordering::Relaxed) + MARGIN.as_nanos() as u64;
        let due_ns = self.ns(due).max(now_ns);
        // The first predicted read the tick can still end before.
        let first = last as i128 + period as i128;
        let needed = due_ns as i128 + lead as i128;
        let frames_ahead = ((needed - first).max(0) + period as i128 - 1) / period as i128;
        let read = first + frames_ahead * period as i128;
        let start = read - lead as i128;
        let latest = self.ns(due) as i128 + window.as_nanos() as i128;
        let start = start.clamp(self.ns(due) as i128, latest.max(self.ns(due) as i128));
        Some(self.epoch + Duration::from_nanos((start - 1).max(0) as u64))
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

    fn paced(period_ms: u64, frames: u64) -> (Arc<FramePacer>, Instant) {
        let pacer = FramePacer::new();
        let start = Instant::now();
        for frame in 0..frames {
            pacer.sampling_at(start + ms(frame * period_ms));
        }
        pacer.tick_finished(Duration::from_micros(400));
        (pacer, start)
    }

    #[test]
    fn nothing_is_predicted_before_frames_arrive() {
        let pacer = FramePacer::new();
        let now = Instant::now();
        assert!(pacer.next_start(now, ms(7), now).is_none());
    }

    #[test]
    fn a_display_rate_tick_ends_just_before_the_next_read() {
        // Frames every 7 ms, the last read at 70; the tick due at 71.
        let (pacer, start) = paced(7, 11);
        let due = start + ms(71);
        let at = pacer.next_start(due, ms(7), due).expect("paced");
        let read = start + ms(77);
        let lead = read.duration_since(at);
        assert!(lead >= Duration::from_micros(200), "{lead:?} margin");
        assert!(lead <= ms(2), "{lead:?} is close to the read");
        assert!(at >= due);
    }

    #[test]
    fn frames_slower_than_ticks_put_the_tick_late_in_its_window_but_never_past_it() {
        // Frames every 8.5 ms (8, 9, ...), ticks due every 6 ms.
        let pacer = FramePacer::new();
        let start = Instant::now();
        for frame in 0..40u64 {
            pacer.sampling_at(start + Duration::from_micros(frame * 8500));
        }
        pacer.tick_finished(Duration::from_micros(300));
        let last = 39 * 8500;
        // Due at 6 ms after the last read: the next read (8.5 ms) is 2.5 ms
        // away, so the tick waits for it, inside its 6 ms window.
        let due = start + Duration::from_micros(last + 6000);
        let at = pacer.next_start(due, ms(6), due).unwrap();
        assert!(at > due, "waits for the frame");
        assert!(at <= due + ms(6), "never later than a step past due");
        let read = start + Duration::from_micros(last + 8500);
        assert!(read.duration_since(at) < ms(2), "ends just before the read");
        // Due right after a read, the next read is 8.5 ms away but the window
        // is 6 ms: it runs at the end of the window, not later.
        let due = start + Duration::from_micros(last + 100);
        let at = pacer.next_start(due, ms(6), due).unwrap();
        assert!(at <= due + ms(6));
    }

    #[test]
    fn frames_that_stopped_keep_the_timer() {
        let (pacer, start) = paced(7, 11);
        assert!(pacer.next_start(start + ms(500), ms(7), start + ms(500)).is_none());
    }

    #[test]
    fn a_hitch_does_not_move_the_period() {
        let (pacer, start) = paced(7, 11);
        let before = pacer.period().unwrap();
        pacer.sampling_at(start + ms(70 + 60));
        assert_eq!(pacer.period().unwrap(), before);
    }

    #[test]
    fn a_noisy_frame_clock_widens_the_margin() {
        let steady = paced(7, 40).0;
        let noisy = FramePacer::new();
        let start = Instant::now();
        let mut at = 0;
        for frame in 0..40u64 {
            at += if frame % 2 == 0 { 5 } else { 9 };
            noisy.sampling_at(start + ms(at));
        }
        noisy.tick_finished(Duration::from_micros(400));
        assert!(noisy.jitter.load(Ordering::Relaxed) > steady.jitter.load(Ordering::Relaxed));
    }
}
