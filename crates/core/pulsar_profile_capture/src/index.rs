//! The capture indexed by thread: nesting rebuilt from the timestamps, so
//! every scope knows its parent and its own time (its duration less its
//! children's).

use std::collections::BTreeMap;

use profiling::ProfileEvent;

use crate::buffer::Capture;

/// Events whose name starts with this are markers (frame times, latency
/// samples), not scopes.
pub const MARKER_PREFIX: &str = "__";

/// One thread's scopes in start order.
pub struct ThreadTrack {
    pub id: u64,
    pub name: String,
    /// Indices into the capture's events: by start, a longer scope before a
    /// shorter one that starts with it (so a parent precedes its children).
    pub order: Vec<usize>,
}

pub struct Index<'a> {
    pub capture: &'a Capture,
    pub threads: Vec<ThreadTrack>,
    /// Parent scope (an event index) per event; `None` for a root or a marker.
    pub parent: Vec<Option<usize>>,
    /// Nesting depth rebuilt from the timestamps (the recorded depth can be
    /// off for work continued on another thread).
    pub depth: Vec<u32>,
    /// Own time: duration less the children's.
    pub self_ns: Vec<u64>,
    /// First scope start and last scope end.
    pub span: (u64, u64),
}

impl<'a> Index<'a> {
    pub fn new(capture: &'a Capture) -> Self {
        let events = &capture.events;
        let mut by_thread: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        let mut names: BTreeMap<u64, String> = BTreeMap::new();
        let mut start = u64::MAX;
        let mut end = 0u64;
        for (i, e) in events.iter().enumerate() {
            if is_marker(e) {
                continue;
            }
            by_thread.entry(e.thread_id).or_default().push(i);
            if let Some(name) = e.thread_name.as_deref().or(e.track_name.as_deref()) {
                names.entry(e.thread_id).or_insert_with(|| name.to_owned());
            }
            start = start.min(e.start_ns);
            end = end.max(e.start_ns.saturating_add(e.duration_ns));
        }
        if start == u64::MAX {
            start = 0;
        }
        let mut parent = vec![None; events.len()];
        let mut depth = vec![0u32; events.len()];
        let mut self_ns: Vec<u64> = events.iter().map(|e| e.duration_ns).collect();
        let mut threads = Vec::new();
        for (id, mut order) in by_thread {
            order.sort_by(|&a, &b| {
                events[a]
                    .start_ns
                    .cmp(&events[b].start_ns)
                    .then(events[b].duration_ns.cmp(&events[a].duration_ns))
            });
            let mut stack: Vec<(u64, usize)> = Vec::new();
            for &i in &order {
                let e = &events[i];
                while stack.last().is_some_and(|&(end, _)| end <= e.start_ns) {
                    stack.pop();
                }
                if let Some(&(_, p)) = stack.last() {
                    parent[i] = Some(p);
                    depth[i] = stack.len() as u32;
                    self_ns[p] = self_ns[p].saturating_sub(e.duration_ns);
                }
                stack.push((e.start_ns.saturating_add(e.duration_ns), i));
            }
            let name = names
                .remove(&id)
                .unwrap_or_else(|| format!("thread-{id}"));
            threads.push(ThreadTrack { id, name, order });
        }
        Self {
            capture,
            threads,
            parent,
            depth,
            self_ns,
            span: (start, end.max(start)),
        }
    }

    pub fn event(&self, i: usize) -> &ProfileEvent {
        &self.capture.events[i]
    }

    pub fn thread(&self, id: u64) -> Option<&ThreadTrack> {
        self.threads.iter().find(|t| t.id == id)
    }

    pub fn thread_name(&self, id: u64) -> String {
        self.thread(id)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| format!("thread-{id}"))
    }

    /// Every scope event index.
    pub fn scopes(&self) -> impl Iterator<Item = usize> + '_ {
        self.threads.iter().flat_map(|t| t.order.iter().copied())
    }

    /// Children of `i` in start order (scans the thread's order after `i`).
    pub fn children(&self, i: usize) -> Vec<usize> {
        let e = self.event(i);
        let Some(track) = self.thread(e.thread_id) else {
            return Vec::new();
        };
        let Some(pos) = track.order.iter().position(|&x| x == i) else {
            return Vec::new();
        };
        let end = e.start_ns.saturating_add(e.duration_ns);
        track.order[pos + 1..]
            .iter()
            .copied()
            .take_while(|&c| self.event(c).start_ns < end)
            .filter(|&c| self.parent[c] == Some(i))
            .collect()
    }

    /// The capture's window in milliseconds.
    pub fn window_ms(&self) -> f64 {
        crate::stats::ms(self.span.1 - self.span.0)
    }
}

pub fn is_marker(e: &ProfileEvent) -> bool {
    e.name.starts_with(MARKER_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ev, game};

    #[test]
    fn nesting_and_self_time_are_rebuilt() {
        let capture = Capture {
            events: game(1, 16, 10, None),
            ..Default::default()
        };
        let index = Index::new(&capture);
        let by_name = |n: &str| capture.events.iter().position(|e| e.name == n).unwrap();
        let frame = by_name("Game::Frame");
        let render = by_name("render");
        let wait = by_name("wait present");
        assert_eq!(index.parent[render], Some(frame));
        assert_eq!(index.parent[wait], Some(render));
        assert_eq!(index.depth[wait], 2);
        // render is 7 ms, wait is 3.5 ms: render's own time is 3.5 ms.
        assert_eq!(index.self_ns[render], 3_500_000);
        assert_eq!(index.self_ns[frame], 3_000_000);
        assert_eq!(index.threads.len(), 2);
    }

    #[test]
    fn markers_are_not_scopes() {
        let capture = Capture {
            events: vec![ev("__FRAME_MARKER__", 1, 0, 10, 0), ev("a", 1, 0, 10, 0)],
            ..Default::default()
        };
        assert_eq!(Index::new(&capture).scopes().count(), 1);
    }
}
