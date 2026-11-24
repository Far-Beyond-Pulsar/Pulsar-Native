//! Input latency probe: where the time between an input event and the frame
//! that shows its effect goes.
//!
//! One sample follows the oldest input not yet shown through four stamps:
//!
//! 1. `input`  - the OS event reaches the window's event handler
//!    ([`note_input`]),
//! 2. `tick`   - the first tick that began after it has finished
//!    ([`tick_done`]; the tick drains the input into actions, scripts,
//!    movement and physics and writes the transforms to the scene),
//! 3. `redraw` - the first frame that started after that tick
//!    ([`frame_started`]; it flushes the scene to the GPU and builds the camera),
//! 4. `present`- that frame's `present` call returned ([`frame_presented`]).
//!
//! A sample is logged (target `pulsar_game::latency`) at most four times a
//! second, with the time of every hop in milliseconds and the frame's own
//! duration, and the last samples are kept for [`summary`]. The probe is
//! always on: two atomics and a clock read per input, per tick and per frame.
//!
//! The stamp after `present` is the earliest the display can show the frame; a
//! deep present queue (`desired_maximum_frame_latency`) or vsync adds to what
//! the person sees, and is not visible here: `present_queue_frames` in the
//! summary is the configured depth, to add `depth x frame time` to the figure.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static EPOCH: OnceLock<Instant> = OnceLock::new();
static INPUT: AtomicU64 = AtomicU64::new(0);
static TICK: AtomicU64 = AtomicU64::new(0);
static FRAME: AtomicU64 = AtomicU64::new(0);
static LAST_LOG: AtomicU64 = AtomicU64::new(0);
static SAMPLES: Mutex<Vec<Sample>> = Mutex::new(Vec::new());

/// One input followed to its frame, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Input event to the end of the tick that consumed it.
    pub to_tick: f32,
    /// End of that tick to the start of the next frame.
    pub to_frame: f32,
    /// The frame's own cost: start to `present` returning.
    pub frame: f32,
    /// Input event to `present` returning.
    pub total: f32,
}

fn now() -> u64 {
    // +1 so a stamp is never 0 (0 means "none").
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

fn ms(from: u64, to: u64) -> f32 {
    to.saturating_sub(from) as f32 / 1.0e6
}

/// An input event arrived. Only the oldest unshown input is followed.
pub fn note_input() {
    let _ = INPUT.compare_exchange(0, now(), Ordering::AcqRel, Ordering::Relaxed);
}

/// A tick finished: the input noted before it began has been consumed. The
/// stamp is kept only for an input that is already waiting.
pub fn tick_done(started: u64) {
    let input = INPUT.load(Ordering::Acquire);
    if input != 0 && input <= started && TICK.load(Ordering::Acquire) == 0 {
        TICK.store(now(), Ordering::Release);
    }
}

/// The clock reading to pass to [`tick_done`] when the tick finishes.
pub fn tick_started() -> u64 {
    now()
}

/// A frame started; returns the stamp for [`frame_presented`].
pub fn frame_started() -> u64 {
    let t = now();
    let tick = TICK.load(Ordering::Acquire);
    if tick != 0 && tick <= t {
        FRAME.store(t, Ordering::Release);
    }
    t
}

/// The frame's `present` returned: close the sample, if this frame carries one.
pub fn frame_presented(frame_start: u64) {
    let input = INPUT.load(Ordering::Acquire);
    let tick = TICK.load(Ordering::Acquire);
    let frame = FRAME.load(Ordering::Acquire);
    if input == 0 || tick == 0 || frame != frame_start {
        return;
    }
    let end = now();
    let sample = Sample {
        to_tick: ms(input, tick),
        to_frame: ms(tick, frame),
        frame: ms(frame, end),
        total: ms(input, end),
    };
    INPUT.store(0, Ordering::Release);
    TICK.store(0, Ordering::Release);
    FRAME.store(0, Ordering::Release);
    {
        let mut samples = SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
        if samples.len() >= 240 {
            samples.remove(0);
        }
        samples.push(sample);
    }
    let last = LAST_LOG.load(Ordering::Relaxed);
    if end.saturating_sub(last) > 250_000_000
        && LAST_LOG
            .compare_exchange(last, end, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        tracing::info!(
            target: "pulsar_game::latency",
            input_to_tick_ms = sample.to_tick,
            tick_to_frame_ms = sample.to_frame,
            frame_ms = sample.frame,
            input_to_present_ms = sample.total,
            "input latency"
        );
    }
}

/// The kept samples' mean and worst case per hop.
pub fn summary() -> Option<(Sample, Sample)> {
    let samples = SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
    summarize(&samples)
}

fn summarize(samples: &[Sample]) -> Option<(Sample, Sample)> {
    if samples.is_empty() {
        return None;
    }
    let n = samples.len() as f32;
    let mut mean = Sample { to_tick: 0.0, to_frame: 0.0, frame: 0.0, total: 0.0 };
    let mut worst = mean;
    for s in samples {
        mean.to_tick += s.to_tick / n;
        mean.to_frame += s.to_frame / n;
        mean.frame += s.frame / n;
        mean.total += s.total / n;
        worst.to_tick = worst.to_tick.max(s.to_tick);
        worst.to_frame = worst.to_frame.max(s.to_frame);
        worst.frame = worst.frame.max(s.frame);
        worst.total = worst.total.max(s.total);
    }
    Some((mean, worst))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_mean_and_worst() {
        let a = Sample { to_tick: 2.0, to_frame: 4.0, frame: 6.0, total: 12.0 };
        let b = Sample { to_tick: 4.0, to_frame: 8.0, frame: 10.0, total: 22.0 };
        let (mean, worst) = summarize(&[a, b]).unwrap();
        assert_eq!(mean, Sample { to_tick: 3.0, to_frame: 6.0, frame: 8.0, total: 17.0 });
        assert_eq!(worst, b);
        assert!(summarize(&[]).is_none());
    }
}
