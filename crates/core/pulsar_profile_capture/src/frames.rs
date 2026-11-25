//! Frames: what delimits one, found in the capture or named by the caller.

use crate::buffer::Capture;

/// The scopes that delimit a frame, most useful first: the packaged game's
/// render frame, the renderer's own frame scope (editor viewport), the
/// profiler's frame marker, the simulation tick, a Play-in-Editor tick.
pub const CANDIDATES: &[&str] = &[
    "Game::Frame",
    "helio_frame",
    "__FRAME_MARKER__",
    "TickLoop::tick",
    "PIE::pie_tick",
];

/// What to call a frame.
#[derive(Clone, Debug, Default)]
pub struct FrameSpec {
    /// The scope name; `None` picks the first of [`CANDIDATES`] present.
    pub scope: Option<String>,
    /// Only this thread's scopes of that name.
    pub thread_id: Option<u64>,
}

/// One frame of the chosen scope.
#[derive(Clone, Debug)]
pub struct Frame {
    pub index: usize,
    pub thread_id: u64,
    pub start_ns: u64,
    /// The scope's own duration: the frame's cost.
    pub cost_ns: u64,
    /// Start to the next frame's start (the cost for the last frame): what a
    /// player perceives, waits outside the scope included.
    pub period_ns: u64,
    /// The recorded event, `None` for a marker-derived frame.
    pub event: Option<usize>,
}

impl Frame {
    pub fn end_ns(&self) -> u64 {
        self.start_ns + self.cost_ns
    }

    /// The window the frame owns: to the next frame's start.
    pub fn window_end_ns(&self) -> u64 {
        self.start_ns + self.period_ns.max(self.cost_ns)
    }
}

/// The frames of `capture` and the scope name used, or `None` when nothing
/// delimits one.
pub fn find(capture: &Capture, spec: &FrameSpec) -> Option<(String, Vec<Frame>)> {
    let names: Vec<&str> = match &spec.scope {
        Some(s) => vec![s.as_str()],
        None => CANDIDATES.to_vec(),
    };
    for name in names {
        let mut frames: Vec<Frame> = capture
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| e.name == name && spec.thread_id.is_none_or(|t| t == e.thread_id))
            .map(|(i, e)| {
                // A frame marker is published when the frame ends.
                let marker = e.name == "__FRAME_MARKER__";
                Frame {
                    index: 0,
                    thread_id: e.thread_id,
                    start_ns: if marker {
                        e.start_ns.saturating_sub(e.duration_ns)
                    } else {
                        e.start_ns
                    },
                    cost_ns: e.duration_ns,
                    period_ns: e.duration_ns,
                    event: (!marker).then_some(i),
                }
            })
            .collect();
        // One frame stream: the thread with the most of them.
        if let Some(thread) = busiest_thread(&frames) {
            frames.retain(|f| f.thread_id == thread);
        }
        let needed = if spec.scope.is_some() { 1 } else { 2 };
        if frames.len() < needed {
            continue;
        }
        frames.sort_by_key(|f| f.start_ns);
        for i in 0..frames.len() {
            frames[i].index = i;
            if let Some(next) = frames.get(i + 1).map(|f| f.start_ns) {
                frames[i].period_ns = next.saturating_sub(frames[i].start_ns);
            }
        }
        return Some((name.to_owned(), frames));
    }
    None
}

fn busiest_thread(frames: &[Frame]) -> Option<u64> {
    let mut counts: Vec<(u64, usize)> = Vec::new();
    for f in frames {
        match counts.iter_mut().find(|(t, _)| *t == f.thread_id) {
            Some((_, n)) => *n += 1,
            None => counts.push((f.thread_id, 1)),
        }
    }
    counts.into_iter().max_by_key(|&(_, n)| n).map(|(t, _)| t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ev, game};

    #[test]
    fn frames_are_found_and_periods_measured() {
        let capture = Capture {
            events: game(5, 16, 10, Some((2, 3))),
            ..Default::default()
        };
        let (name, frames) = find(&capture, &FrameSpec::default()).unwrap();
        assert_eq!(name, "Game::Frame");
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0].period_ns, 16_000_000);
        assert_eq!(frames[2].period_ns, 48_000_000);
        assert_eq!(frames[2].cost_ns, 30_000_000);
    }

    #[test]
    fn a_marker_frame_ends_at_the_marker() {
        let capture = Capture {
            events: vec![
                ev("__FRAME_MARKER__", 1, 100_000_000, 16_000_000, 0),
                ev("__FRAME_MARKER__", 1, 116_000_000, 16_000_000, 0),
            ],
            ..Default::default()
        };
        let (_, frames) = find(&capture, &FrameSpec::default()).unwrap();
        assert_eq!(frames[0].start_ns, 84_000_000);
        assert_eq!(frames[0].period_ns, 16_000_000);
    }

    #[test]
    fn nothing_to_delimit() {
        let capture = Capture {
            events: vec![ev("x", 1, 0, 1, 0)],
            ..Default::default()
        };
        assert!(find(&capture, &FrameSpec::default()).is_none());
    }
}
