//! Late-latching of camera input.
//!
//! The input thread samples the keyboard and mouse on its own timer (every
//! ~2 ms while the camera is captured) and writes into `CameraInput`; the render
//! thread reads that snapshot once, at the start of each frame. So the state a
//! frame renders is `poll period + jitter` old before rendering even begins:
//! measured at 1.4 ms median and 2.4 ms p90, roughly a fifth of the whole
//! input-to-photon chain (6.5 ms median).
//!
//! Instead of moving the polling (the input thread owns the cursor capture and
//! reset, which must stay single-owner), the render thread asks the input thread
//! for one fresh poll immediately before the frame reads the camera, and waits a
//! bounded time for it. Same polling code, same thread, just sampled at the last
//! possible moment.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// Upper bound on how long a frame waits for the fresh sample. A normal poll is
/// ~0.1 ms; if the input thread is descheduled the frame renders with the
/// previous snapshot rather than stalling.
const MAX_WAIT: Duration = Duration::from_micros(400);

/// One viewport's input thread, as the render threads see it. Every viewport
/// has its own thread and so its own latch; `latch_now` wakes whichever is
/// capturing (the cursor is captured by at most one at a time).
pub(super) struct InputLatch {
    /// Bumped at the end of every input-thread iteration.
    generation: AtomicU64,
    /// Whether the input thread is currently capturing (rotating/panning). No
    /// point waking it otherwise: an idle iteration samples nothing.
    capturing: AtomicBool,
    thread: Mutex<Option<Thread>>,
}

impl InputLatch {
    fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            capturing: AtomicBool::new(false),
            thread: Mutex::new(None),
        }
    }

    /// Called by the input thread every iteration with the current capture
    /// state, and with `false` when it exits.
    pub(super) fn set_capturing(&self, capturing: bool) {
        self.capturing.store(capturing, Ordering::Release);
    }
}

static LATCHES: Mutex<Vec<Arc<InputLatch>>> = Mutex::new(Vec::new());

/// Called by an input thread once at startup so the render threads can wake
/// it. Returns the thread's latch; hand it to [`unregister_input_thread`] when
/// the thread exits.
pub(super) fn register_input_thread() -> Arc<InputLatch> {
    let latch = Arc::new(InputLatch::new());
    if let Ok(mut slot) = latch.thread.lock() {
        *slot = Some(std::thread::current());
    }
    if let Ok(mut latches) = LATCHES.lock() {
        latches.push(latch.clone());
    }
    latch
}

/// Forget an input thread that is exiting.
pub(super) fn unregister_input_thread(latch: &Arc<InputLatch>) {
    latch.set_capturing(false);
    if let Ok(mut latches) = LATCHES.lock() {
        latches.retain(|other| !Arc::ptr_eq(other, latch));
    }
}

/// Marks the end of one input-thread iteration when dropped, on every exit path
/// of the loop body (including the early `continue`s).
pub(super) struct PollDone(pub(super) Arc<InputLatch>);

impl Drop for PollDone {
    fn drop(&mut self) {
        self.0.generation.fetch_add(1, Ordering::Release);
    }
}

/// Ask the capturing input thread for one fresh sample and wait (bounded) for
/// it.
///
/// Call from a render thread immediately before the frame reads `CameraInput`.
/// A no-op unless a camera is being captured and its input thread is registered.
pub(crate) fn latch_now() {
    let capturing: Vec<Arc<InputLatch>> = match LATCHES.lock() {
        Ok(latches) => latches
            .iter()
            .filter(|latch| latch.capturing.load(Ordering::Acquire))
            .cloned()
            .collect(),
        Err(_) => return,
    };
    for latch in capturing {
        latch_one(&latch);
    }
}

fn latch_one(latch: &InputLatch) {
    let Some(thread) = latch.thread.lock().ok().and_then(|slot| slot.clone()) else {
        return;
    };

    let before = latch.generation.load(Ordering::Acquire);
    // The input thread parks between polls; this wakes it now instead of at its
    // next 2 ms deadline. If it is mid-iteration the token makes its next park
    // return immediately, and the current iteration's completion (also a bump)
    // is at worst a few hundred microseconds old.
    thread.unpark();

    let start = Instant::now();
    while latch.generation.load(Ordering::Acquire) == before {
        if start.elapsed() >= MAX_WAIT {
            return;
        }
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    /// A stand-in input thread that would only poll every 200 ms on its own.
    /// `latch_now` must get a fresh iteration out of it almost immediately, and
    /// must not wait at all when nothing is being captured.
    #[test]
    fn latch_wakes_the_input_thread_instead_of_waiting_for_its_timer() {
        let stop = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicU64::new(0));
        let (stop_t, polls_t) = (stop.clone(), polls.clone());
        let (latch_tx, latch_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let latch = register_input_thread();
            latch_tx.send(latch.clone()).unwrap();
            while !stop_t.load(Ordering::Acquire) {
                std::thread::park_timeout(Duration::from_millis(200));
                let _done = PollDone(latch.clone());
                polls_t.fetch_add(1, Ordering::AcqRel);
            }
        });
        let latch = latch_rx.recv().unwrap();

        // Not capturing: returns at once, does not wake the thread.
        latch.set_capturing(false);
        let start = Instant::now();
        latch_now();
        assert!(
            start.elapsed() < Duration::from_millis(5),
            "idle latch must be free"
        );

        // Capturing: a fresh iteration happens well inside the 200 ms timer.
        latch.set_capturing(true);
        let mut worst = Duration::ZERO;
        for _ in 0..20 {
            let polls_before = polls.load(Ordering::Acquire);
            let start = Instant::now();
            latch_now();
            let took = start.elapsed();
            worst = worst.max(took);
            if polls.load(Ordering::Acquire) == polls_before {
                // The frame is allowed to use the previous sample after the
                // 400 us deadline. The wake must still produce a fresh poll.
                assert!(
                    took >= MAX_WAIT,
                    "latch returned before polling or timing out"
                );
                let deadline = Instant::now() + Duration::from_millis(50);
                while polls.load(Ordering::Acquire) == polls_before && Instant::now() < deadline {
                    std::thread::yield_now();
                }
                assert!(
                    polls.load(Ordering::Acquire) > polls_before,
                    "latch did not wake the input thread"
                );
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            worst < Duration::from_millis(50),
            "latch waited on the 200 ms timer instead of waking the thread ({worst:?})"
        );

        latch.set_capturing(false);
        stop.store(true, Ordering::Release);
        latch.thread.lock().unwrap().as_ref().unwrap().unpark();
        unregister_input_thread(&latch);
        handle.join().unwrap();
    }
}
