//! The bounded buffer a recording fills, and the finished capture.

use std::collections::VecDeque;

use profiling::ProfileEvent;

/// What was recorded.
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub pid: u32,
    /// `"game"`, `"editor"`, ...
    pub kind: String,
    pub name: String,
    pub project: String,
}

/// A recording in progress. Holds the newest `capacity` events: a recording
/// longer than the buffer loses its start, never its end, and never grows past
/// the bound whatever the target publishes.
pub struct CaptureBuffer {
    events: VecDeque<ProfileEvent>,
    capacity: usize,
    /// Events pushed out because the buffer was full.
    pub dropped_overflow: u64,
}

impl CaptureBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::new(),
            capacity: capacity.max(1),
            dropped_overflow: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Append a batch, dropping the oldest events to stay within the bound.
    pub fn extend(&mut self, batch: impl IntoIterator<Item = ProfileEvent>) {
        for event in batch {
            if self.events.len() == self.capacity {
                self.events.pop_front();
                self.dropped_overflow += 1;
            }
            self.events.push_back(event);
        }
    }

    /// Close the recording.
    pub fn finish(self, id: u64, target: Target, dropped_in_target: u64, requested_ms: u64) -> Capture {
        Capture {
            id,
            target,
            events: self.events.into(),
            dropped_overflow: self.dropped_overflow,
            dropped_in_target,
            requested_ms,
        }
    }
}

/// A finished recording.
#[derive(Clone, Debug, Default)]
pub struct Capture {
    pub id: u64,
    pub target: Target,
    pub events: Vec<ProfileEvent>,
    /// Events the buffer pushed out (the recording was longer than it holds).
    pub dropped_overflow: u64,
    /// Events the target dropped because its ring was full before the viewer read them.
    pub dropped_in_target: u64,
    /// The duration asked for, in milliseconds (0: until stopped).
    pub requested_ms: u64,
}

impl Capture {
    /// First event start and last event end, in nanoseconds.
    pub fn span_ns(&self) -> Option<(u64, u64)> {
        let start = self.events.iter().map(|e| e.start_ns).min()?;
        let end = self
            .events
            .iter()
            .map(|e| e.start_ns.saturating_add(e.duration_ns))
            .max()?;
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ev;

    #[test]
    fn the_buffer_keeps_the_newest_events() {
        let mut buffer = CaptureBuffer::new(3);
        buffer.extend((0..5).map(|i| ev("a", 1, i * 10, 5, 0)));
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.dropped_overflow, 2);
        let capture = buffer.finish(1, Target::default(), 0, 0);
        assert_eq!(capture.events[0].start_ns, 20);
        assert_eq!(capture.span_ns(), Some((20, 45)));
    }
}
