//! Aggregation over a loaded session: self time and per-function statistics.

use crate::trace_data::TraceFrame;
use std::collections::HashMap;

/// Self time of every span, indexed like `TraceFrame::spans`: its duration
/// minus the time spent in the spans directly nested inside it.
///
/// Spans are walked per thread in start order with a stack of open spans, so
/// the cost is one sort of each thread's spans, not a scan per span.
pub fn compute_self_times(frame: &TraceFrame) -> Vec<u64> {
    let mut self_ns: Vec<u64> = frame.spans.iter().map(|span| span.duration_ns).collect();

    let mut by_thread: HashMap<u64, Vec<u32>> = HashMap::new();
    for (index, span) in frame.spans.iter().enumerate() {
        by_thread.entry(span.thread_id).or_default().push(index as u32);
    }

    for (_, mut indices) in by_thread {
        // Parents first: earlier start, then longer, then shallower.
        indices.sort_unstable_by(|&a, &b| {
            let (a, b) = (&frame.spans[a as usize], &frame.spans[b as usize]);
            a.start_ns
                .cmp(&b.start_ns)
                .then(b.duration_ns.cmp(&a.duration_ns))
                .then(a.depth.cmp(&b.depth))
        });
        let mut open: Vec<u32> = Vec::new();
        for index in indices {
            let span = &frame.spans[index as usize];
            while let Some(&top) = open.last() {
                if frame.spans[top as usize].end_ns() <= span.start_ns {
                    open.pop();
                } else {
                    break;
                }
            }
            if let Some(&parent) = open.last() {
                let slot = &mut self_ns[parent as usize];
                *slot = slot.saturating_sub(span.duration_ns);
            }
            open.push(index);
        }
    }
    self_ns
}

/// One row of the statistics table.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionStats {
    pub name: String,
    pub calls: usize,
    pub total_ns: u64,
    pub self_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
}

impl FunctionStats {
    pub fn avg_ns(&self) -> u64 {
        self.total_ns / self.calls.max(1) as u64
    }
}

/// Statistics per span name over the whole session.
pub fn aggregate(frame: &TraceFrame, self_ns: &[u64]) -> Vec<FunctionStats> {
    let mut by_name: HashMap<&str, FunctionStats> = HashMap::new();
    for (index, span) in frame.spans.iter().enumerate() {
        let entry = by_name
            .entry(span.name.as_str())
            .or_insert_with(|| FunctionStats {
                name: span.name.clone(),
                calls: 0,
                total_ns: 0,
                self_ns: 0,
                min_ns: u64::MAX,
                max_ns: 0,
            });
        entry.calls += 1;
        entry.total_ns = entry.total_ns.saturating_add(span.duration_ns);
        entry.self_ns = entry
            .self_ns
            .saturating_add(self_ns.get(index).copied().unwrap_or(span.duration_ns));
        entry.min_ns = entry.min_ns.min(span.duration_ns);
        entry.max_ns = entry.max_ns.max(span.duration_ns);
    }
    by_name.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace_data::TraceSpan;

    fn span(name: &str, thread: u64, start: u64, duration: u64, depth: u32) -> TraceSpan {
        TraceSpan {
            name: name.to_string(),
            start_ns: start,
            duration_ns: duration,
            depth,
            thread_id: thread,
            color_index: 0,
        }
    }

    fn frame(spans: Vec<TraceSpan>) -> TraceFrame {
        let mut frame = TraceFrame::new();
        for span in spans {
            frame.add_span(span);
        }
        frame
    }

    #[test]
    fn self_time_excludes_direct_children_only() {
        let frame = frame(vec![
            span("root", 1, 0, 100, 0),
            span("child", 1, 10, 40, 1),
            span("grandchild", 1, 20, 10, 2),
            span("child2", 1, 60, 20, 1),
        ]);
        let self_ns = compute_self_times(&frame);
        let by_name = |name: &str| {
            let index = frame.spans.iter().position(|s| s.name == name).unwrap();
            self_ns[index]
        };
        assert_eq!(by_name("root"), 100 - 40 - 20);
        assert_eq!(by_name("child"), 40 - 10);
        assert_eq!(by_name("grandchild"), 10);
        assert_eq!(by_name("child2"), 20);
    }

    #[test]
    fn threads_do_not_subtract_from_each_other() {
        let frame = frame(vec![span("a", 1, 0, 50, 0), span("b", 2, 10, 20, 0)]);
        assert_eq!(compute_self_times(&frame), vec![50, 20]);
    }

    #[test]
    fn sibling_starting_at_the_parent_end_is_not_a_child() {
        let frame = frame(vec![span("a", 1, 0, 10, 0), span("b", 1, 10, 10, 0)]);
        assert_eq!(compute_self_times(&frame), vec![10, 10]);
    }

    #[test]
    fn aggregate_sums_calls_total_and_self() {
        let frame = frame(vec![
            span("f", 1, 0, 100, 0),
            span("g", 1, 10, 30, 1),
            span("g", 1, 50, 10, 1),
        ]);
        let self_ns = compute_self_times(&frame);
        let mut stats = aggregate(&frame, &self_ns);
        stats.sort_by(|a, b| a.name.cmp(&b.name));
        let f = &stats[0];
        assert_eq!((f.calls, f.total_ns, f.self_ns), (1, 100, 60));
        let g = &stats[1];
        assert_eq!((g.calls, g.total_ns, g.self_ns), (2, 40, 40));
        assert_eq!((g.min_ns, g.max_ns, g.avg_ns()), (10, 30, 20));
    }
}
