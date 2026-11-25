//! Synthetic events for the tests.

use profiling::ProfileEvent;

/// A scope on `thread` starting at `start_ns` for `dur_ns` at `depth`.
pub fn ev(name: &str, thread: u64, start_ns: u64, dur_ns: u64, depth: u32) -> ProfileEvent {
    ProfileEvent {
        scope_id: start_ns + thread,
        parent_scope_id: None,
        name: name.into(),
        thread_id: thread,
        thread_name: Some(format!("t{thread}")),
        process_id: 1,
        parent_name: None,
        start_ns,
        duration_ns: dur_ns,
        depth,
        location: None,
        metadata: None,
        track_name: None,
    }
}

/// `frames` frames on thread 1 every `period_ms`: a `Game::Frame` of `cost_ms`
/// holding `render` (70 % of the cost) holding `wait present` (half of that),
/// plus a `Physics::Step` of 1 ms on thread 2 per frame. Frame `slow.0` takes
/// `slow.1` times as long.
pub fn game(frames: u64, period_ms: u64, cost_ms: u64, slow: Option<(u64, u64)>) -> Vec<ProfileEvent> {
    let mut out = Vec::new();
    let mut t = 1_000_000_000u64;
    for i in 0..frames {
        let factor = match slow {
            Some((at, f)) if at == i => f,
            _ => 1,
        };
        let cost = cost_ms * 1_000_000 * factor;
        let period = period_ms * 1_000_000 * factor;
        out.push(ev("Game::Frame", 1, t, cost, 0));
        let render = cost * 7 / 10;
        out.push(ev("render", 1, t + 100_000, render, 1));
        out.push(ev("wait present", 1, t + 100_000, render / 2, 2));
        out.push(ev("Physics::Step", 2, t + 200_000, 1_000_000, 0));
        t += period;
    }
    out
}
