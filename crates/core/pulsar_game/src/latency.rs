//! Input latency probe: where the time between an input event and the frame
//! that shows its effect goes.
//!
//! One sample follows the oldest input not yet shown through these stamps, in
//! chain order (the stage between two stamps is named for what happens in it):
//!
//! | stamp            | taken                                                        | stage before it   |
//! |------------------|--------------------------------------------------------------|-------------------|
//! | `os_event`       | the OS event reaches the window's event handler ([`note_input`]) |                |
//! | `tick_start`     | the first tick that began after it starts ([`tick_started`])  | `wait_for_tick`   |
//! | `systems_done`   | the schedule and component phase (input drain, character, physics handoff) finished ([`tick_systems_done`]) | `input_and_systems` |
//! | `scripts_done`   | the script phase finished ([`tick_scripts_done`])             | `scripts`         |
//! | `tick_done`      | the tick finished, the change window closed ([`tick_done`])   | `scene_write`     |
//! | `frame_start`    | the first frame that started after the tick ([`frame_started`]) | `wait_for_frame` |
//! | `acquired`       | the surface texture was acquired ([`frame_stage`])            | `acquire`         |
//! | `prepared`       | the scene is flushed to the GPU and the frame extracted       | `extract_upload`  |
//! | `rendered`       | the frame was recorded and submitted                          | `render_submit`   |
//! | `presented`      | `present` returned ([`frame_presented`])                      | `present`         |
//!
//! A sample is logged (target `pulsar_game::latency`) at most four times a
//! second, the last 240 are kept for [`summary`], and while the profiler is
//! recording every sample is also published as scopes on the `Input latency`
//! track (a root `Latency::input_to_present`, one child per stage), so the
//! chain is in the same recording as everything else and the `profile_latency`
//! MCP tool reads it. The probe is always on: an atomic and
//! a clock read per stamp.
//!
//! `os_event` is the moment the OS delivered the input to this process, taken on the
//! raw-input thread ([`crate::pointer`]) the instant it arrives, not when the event
//! loop gets to it. The event loop is busy rendering most of the time, so what it
//! would have added (the stage the chain used to hide) is measured on its own: each
//! raw input the event loop later hands over reports its wait ([`note_handler_delay`]),
//! published as `Latency::os_to_handler` on the same track and kept for
//! [`handler_summary`].
//!
//! The stamp after `present` is the earliest the display can show the frame; a
//! deep present queue (`desired_maximum_frame_latency`) or vsync adds to what
//! the person sees, and is not visible here: add `depth x frame time`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The stages, in chain order; stage `i` is the time from stamp `i` to stamp
/// `i + 1`.
pub const STAGES: [&str; 9] = [
    "wait_for_tick",
    "input_and_systems",
    "scripts",
    "scene_write",
    "wait_for_frame",
    "acquire",
    "extract_upload",
    "render_submit",
    "present",
];

/// Stamp slots: `OS_EVENT` .. `PRESENTED`.
const STAMPS: usize = STAGES.len() + 1;
const OS_EVENT: usize = 0;
const TICK_START: usize = 1;
const SYSTEMS_DONE: usize = 2;
const SCRIPTS_DONE: usize = 3;
const TICK_DONE: usize = 4;
const FRAME_START: usize = 5;
const ACQUIRED: usize = 6;
const PREPARED: usize = 7;
const RENDERED: usize = 8;

/// A frame stage reported through [`frame_stage`].
#[derive(Clone, Copy, Debug)]
pub enum FrameStage {
    /// The surface texture was acquired.
    Acquired,
    /// The scene was flushed and the frame's camera and views extracted.
    Prepared,
    /// The frame was recorded and submitted.
    Rendered,
}

static EPOCH: OnceLock<Instant> = OnceLock::new();
#[allow(clippy::declare_interior_mutable_const)]
const ZERO: AtomicU64 = AtomicU64::new(0);
/// 0 = not taken. One chain is followed at a time.
static SLOTS: [AtomicU64; STAMPS] = [ZERO; STAMPS];
static LAST_LOG: AtomicU64 = AtomicU64::new(0);
static SAMPLES: Mutex<Vec<Sample>> = Mutex::new(Vec::new());

/// One input followed to its frame, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Per stage, in [`STAGES`] order.
    pub stage_ms: [f32; STAGES.len()],
    /// Input event to `present` returning.
    pub total: f32,
    /// The OS event's wall-clock time (nanoseconds since the Unix epoch).
    pub at_unix_ns: u64,
}

/// The probe's clock reading (nanoseconds since its epoch, never 0): the stamp a
/// thread takes when an input arrives and passes to [`note_input_at`].
pub fn stamp() -> u64 {
    now()
}

fn now() -> u64 {
    // +1 so a stamp is never 0 (0 means "none").
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

fn ms(from: u64, to: u64) -> f32 {
    to.saturating_sub(from) as f32 / 1.0e6
}

fn load(slot: usize) -> u64 {
    SLOTS[slot].load(Ordering::Acquire)
}

/// An input event arrived. Only the oldest unshown input is followed.
pub fn note_input() {
    note_input_at(now());
}

/// An input event arrived at `at` ([`stamp`]), possibly some time ago: the OS
/// delivered it to a thread that took the stamp at once. Only the oldest
/// unshown input is followed.
pub fn note_input_at(at: u64) {
    let _ = SLOTS[OS_EVENT].compare_exchange(0, at, Ordering::AcqRel, Ordering::Relaxed);
}

/// The clock reading to pass to the tick's stage calls; the tick has begun.
/// The stamp is kept only for an input that is already waiting and a chain
/// whose tick has not been taken yet.
pub fn tick_started() -> u64 {
    let t = now();
    let input = load(OS_EVENT);
    if input != 0 && input <= t && load(TICK_DONE) == 0 {
        SLOTS[TICK_START].store(t, Ordering::Release);
    }
    t
}

/// Whether the tick that began at `started` is the one being followed.
fn followed(started: u64) -> bool {
    load(TICK_START) == started && load(TICK_DONE) == 0
}

/// The schedule and component phase of the tick that began at `started` finished.
pub fn tick_systems_done(started: u64) {
    if followed(started) {
        SLOTS[SYSTEMS_DONE].store(now(), Ordering::Release);
    }
}

/// The script phase of the tick that began at `started` finished.
pub fn tick_scripts_done(started: u64) {
    if followed(started) {
        SLOTS[SCRIPTS_DONE].store(now(), Ordering::Release);
    }
}

/// A tick finished: the input noted before it began has been consumed.
pub fn tick_done(started: u64) {
    if followed(started) {
        let t = now();
        // A tick that skipped a phase reports it as zero length.
        for slot in [SYSTEMS_DONE, SCRIPTS_DONE] {
            if load(slot) == 0 {
                SLOTS[slot].store(t, Ordering::Release);
            }
        }
        SLOTS[TICK_DONE].store(t, Ordering::Release);
    }
}

/// A frame started; returns the stamp for [`frame_stage`] and [`frame_presented`].
pub fn frame_started() -> u64 {
    let t = now();
    let tick = load(TICK_DONE);
    if tick != 0 && tick <= t && load(FRAME_START) == 0 {
        SLOTS[FRAME_START].store(t, Ordering::Release);
    }
    t
}

/// A stage of the frame that began at `frame_start` finished.
pub fn frame_stage(frame_start: u64, stage: FrameStage) {
    if load(FRAME_START) != frame_start {
        return;
    }
    let slot = match stage {
        FrameStage::Acquired => ACQUIRED,
        FrameStage::Prepared => PREPARED,
        FrameStage::Rendered => RENDERED,
    };
    SLOTS[slot].store(now(), Ordering::Release);
}

/// The frame's `present` returned: close the sample, if this frame carries one.
pub fn frame_presented(frame_start: u64) {
    if load(OS_EVENT) == 0 || load(TICK_DONE) == 0 || load(FRAME_START) != frame_start {
        return;
    }
    let end = now();
    let mut stamps = [0u64; STAMPS];
    for (i, s) in stamps.iter_mut().enumerate().take(STAMPS - 1) {
        *s = load(i);
    }
    stamps[STAMPS - 1] = end;
    for slot in &SLOTS {
        slot.store(0, Ordering::Release);
    }
    // A stage that was not reported (a frame path without that step) takes the
    // previous stamp: zero length rather than a hole in the chain.
    for i in 1..STAMPS {
        if stamps[i] == 0 {
            stamps[i] = stamps[i - 1];
        }
    }
    let mut stage_ms = [0.0f32; STAGES.len()];
    for (i, m) in stage_ms.iter_mut().enumerate() {
        *m = ms(stamps[i], stamps[i + 1]);
    }
    let total = ms(stamps[OS_EVENT], end);
    let at_unix_ns = unix_ns().saturating_sub((total * 1.0e6) as u64);
    let sample = Sample {
        stage_ms,
        total,
        at_unix_ns,
    };
    {
        let mut samples = SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
        if samples.len() >= 240 {
            samples.remove(0);
        }
        samples.push(sample);
    }
    publish(&sample);
    let last = LAST_LOG.load(Ordering::Relaxed);
    if end.saturating_sub(last) > 250_000_000
        && LAST_LOG
            .compare_exchange(last, end, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        tracing::info!(
            target: "pulsar_game::latency",
            input_to_present_ms = sample.total,
            wait_for_tick_ms = sample.stage_ms[0],
            input_and_systems_ms = sample.stage_ms[1],
            scripts_ms = sample.stage_ms[2],
            scene_write_ms = sample.stage_ms[3],
            wait_for_frame_ms = sample.stage_ms[4],
            acquire_ms = sample.stage_ms[5],
            extract_upload_ms = sample.stage_ms[6],
            render_submit_ms = sample.stage_ms[7],
            present_ms = sample.stage_ms[8],
            "input latency"
        );
    }
}

fn unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// The track the chain is published on (the profiler's `track_name`), shown
/// as its own lane; `ui_flamegraph`'s `profile_latency` reads it back.
pub const TRACK: &str = "Input latency";
/// The root scope of one chain; each stage is a child named `Latency::<stage>`.
pub const ROOT: &str = "Latency::input_to_present";
/// The lane's pseudo thread id (a track, not an OS thread).
const TRACK_THREAD: u64 = 1_000_001;

/// Hand the chain to the profiler while it records (one atomic load
/// otherwise), as ordinary scopes: a root and one child per stage, on their
/// own track, so it lands in the same recording as everything else.
fn publish(sample: &Sample) {
    let profiler = profiling::init_profiler();
    if !profiler.is_enabled() {
        return;
    }
    let event = |scope_id, parent, name: String, start_ns: u64, ms: f32, depth| profiling::ProfileEvent {
        scope_id,
        parent_scope_id: parent,
        parent_name: parent.map(|_| ROOT.to_owned()),
        name,
        thread_id: TRACK_THREAD,
        thread_name: Some(TRACK.into()),
        process_id: profiler.get_process_id(),
        start_ns,
        duration_ns: (ms * 1.0e6) as u64,
        depth,
        location: None,
        metadata: None,
        track_name: Some(TRACK.into()),
    };
    let root = profiling::allocate_scope_id();
    profiler.submit_event(event(root, None, ROOT.into(), sample.at_unix_ns, sample.total, 0));
    let mut at = sample.at_unix_ns;
    for (stage, ms) in STAGES.iter().zip(sample.stage_ms) {
        let id = profiling::allocate_scope_id();
        profiler.submit_event(event(id, Some(root), format!("Latency::{stage}"), at, ms, 1));
        at += (ms * 1.0e6) as u64;
    }
}

/// Waits of the event loop, in milliseconds (the last 240): how long raw input
/// sat in the OS queue before the event loop handed it over.
static HANDLER_WAITS: Mutex<Vec<f32>> = Mutex::new(Vec::new());
/// The root scope of one event-loop wait on the `Input latency` track.
pub const HANDLER_ROOT: &str = "Latency::os_to_handler";

/// The event loop handed over an input the raw-input thread saw at `os_event`
/// ([`stamp`]): the wait between the two is the stage a handler-stamped chain
/// cannot see.
pub fn note_handler_delay(os_event: u64) {
    let end = now();
    let wait = ms(os_event, end);
    {
        let mut waits = HANDLER_WAITS.lock().unwrap_or_else(|p| p.into_inner());
        if waits.len() >= 240 {
            waits.remove(0);
        }
        waits.push(wait);
    }
    let profiler = profiling::init_profiler();
    if !profiler.is_enabled() {
        return;
    }
    profiler.submit_event(profiling::ProfileEvent {
        scope_id: profiling::allocate_scope_id(),
        parent_scope_id: None,
        parent_name: None,
        name: HANDLER_ROOT.into(),
        thread_id: TRACK_THREAD,
        thread_name: Some(TRACK.into()),
        process_id: profiler.get_process_id(),
        start_ns: unix_ns().saturating_sub((wait * 1.0e6) as u64),
        duration_ns: (wait * 1.0e6) as u64,
        depth: 0,
        location: None,
        metadata: None,
        track_name: Some(TRACK.into()),
    });
}

/// The kept event-loop waits' mean and worst case, in milliseconds.
pub fn handler_summary() -> Option<(f32, f32)> {
    let waits = HANDLER_WAITS.lock().unwrap_or_else(|p| p.into_inner());
    if waits.is_empty() {
        return None;
    }
    let mean = waits.iter().sum::<f32>() / waits.len() as f32;
    Some((mean, waits.iter().copied().fold(0.0, f32::max)))
}

/// The kept samples' mean and worst case per stage and in total.
pub fn summary() -> Option<(Sample, Sample)> {
    let samples = SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
    summarize(&samples)
}

fn summarize(samples: &[Sample]) -> Option<(Sample, Sample)> {
    if samples.is_empty() {
        return None;
    }
    let n = samples.len() as f32;
    let mut mean = Sample {
        stage_ms: [0.0; STAGES.len()],
        total: 0.0,
        at_unix_ns: 0,
    };
    let mut worst = mean;
    for s in samples {
        for i in 0..STAGES.len() {
            mean.stage_ms[i] += s.stage_ms[i] / n;
            worst.stage_ms[i] = worst.stage_ms[i].max(s.stage_ms[i]);
        }
        mean.total += s.total / n;
        worst.total = worst.total.max(s.total);
    }
    Some((mean, worst))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(stage: f32) -> Sample {
        Sample {
            stage_ms: [stage; STAGES.len()],
            total: stage * STAGES.len() as f32,
            at_unix_ns: 0,
        }
    }

    #[test]
    fn summary_is_mean_and_worst() {
        let (mean, worst) = summarize(&[sample(1.0), sample(3.0)]).unwrap();
        assert_eq!(mean.stage_ms[4], 2.0);
        assert_eq!(mean.total, 18.0);
        assert_eq!(worst.stage_ms[0], 3.0);
        assert!(summarize(&[]).is_none());
    }

    /// The whole chain, one thread, the global slots: an input is followed
    /// through a tick and a frame and the sample's stages add up to its total.
    #[test]
    fn a_chain_is_followed_end_to_end() {
        for slot in &SLOTS {
            slot.store(0, Ordering::Release);
        }
        SAMPLES.lock().unwrap().clear();
        note_input();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let tick = tick_started();
        tick_systems_done(tick);
        tick_scripts_done(tick);
        tick_done(tick);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let frame = frame_started();
        frame_stage(frame, FrameStage::Acquired);
        frame_stage(frame, FrameStage::Prepared);
        frame_stage(frame, FrameStage::Rendered);
        frame_presented(frame);
        let (mean, _) = summary().expect("a sample was kept");
        assert!(mean.stage_ms[0] >= 3.0, "wait_for_tick {:?}", mean.stage_ms);
        assert!(mean.stage_ms[4] >= 2.0, "wait_for_frame {:?}", mean.stage_ms);
        let sum: f32 = mean.stage_ms.iter().sum();
        assert!((sum - mean.total).abs() < 0.05, "{sum} vs {}", mean.total);
        // The chain restarts with the next input.
        assert_eq!(load(OS_EVENT), 0);
    }
}
