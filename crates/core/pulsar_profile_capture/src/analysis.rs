//! The analyses. Each takes a finished [`Capture`] (through an [`Analyzer`],
//! which indexes it once and finds its frames) and answers with JSON.
//!
//! Times are milliseconds. "Self" time is a scope's duration less its
//! children's; "total" is its duration. A frame is one `Game::Frame` (or the
//! scope the caller names, see [`crate::frames`]); the frame's "period" is its
//! start to the next frame's start, which is what the player perceives, and its
//! "cost" is the scope's own duration: a frame whose period is far above its
//! cost spends the difference outside the instrumented work (a frame limiter,
//! the compositor, the OS).

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};

use crate::buffer::Capture;
use crate::frames::{self, Frame, FrameSpec};
use crate::index::{is_marker, Index};
use crate::latency;
use crate::stats::{ms, round3, Summary};

/// Names containing one of these (lower case) are time spent waiting rather
/// than working.
pub const WAIT_WORDS: &[&str] = &[
    "wait", "lock", "fence", "present", "acquire", "sleep", "idle", "park", "block", "recv",
    "poll_events", "vsync",
];

pub fn is_wait_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    WAIT_WORDS.iter().any(|w| lower.contains(w))
}

pub struct Analyzer<'a> {
    pub index: Index<'a>,
    /// The scope that delimits frames, when the capture has one.
    pub frame_scope: Option<String>,
    pub frames: Vec<Frame>,
}

/// How [`Analyzer::top_scopes`] groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    /// One row per scope name, all threads together.
    Name,
    /// One row per (thread, scope name).
    ThreadAndName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    SelfTime,
    Total,
    Max,
    Count,
    MeanCall,
}

impl Sort {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "self" => Self::SelfTime,
            "total" => Self::Total,
            "max" => Self::Max,
            "count" => Self::Count,
            "mean" | "mean_call" => Self::MeanCall,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TopParams {
    pub group: Group,
    pub sort: Sort,
    pub limit: usize,
    /// Case-insensitive substring of the thread name.
    pub thread: Option<String>,
    /// Case-insensitive substring of the scope name.
    pub name: Option<String>,
}

impl Default for TopParams {
    fn default() -> Self {
        Self {
            group: Group::Name,
            sort: Sort::SelfTime,
            limit: 25,
            thread: None,
            name: None,
        }
    }
}

#[derive(Default)]
struct Acc {
    thread_id: Option<u64>,
    calls: Vec<f64>,
    total_ns: u64,
    self_ns: u64,
    per_frame: HashMap<usize, f64>,
}

impl<'a> Analyzer<'a> {
    pub fn new(capture: &'a Capture, spec: &FrameSpec) -> Self {
        let (frame_scope, frames) = match frames::find(capture, spec) {
            Some((name, frames)) => (Some(name), frames),
            None => (None, Vec::new()),
        };
        Self {
            index: Index::new(capture),
            frame_scope,
            frames,
        }
    }

    fn capture(&self) -> &Capture {
        self.index.capture
    }

    /// The frame whose window holds `ns`.
    fn frame_at(&self, ns: u64) -> Option<usize> {
        let pos = self.frames.partition_point(|f| f.start_ns <= ns);
        let f = self.frames.get(pos.checked_sub(1)?)?;
        (ns < f.window_end_ns()).then_some(f.index)
    }

    /// What the capture holds, for the head of every answer.
    pub fn overview(&self) -> Value {
        let c = self.capture();
        let costs: Vec<f64> = self.frames.iter().map(|f| ms(f.cost_ns)).collect();
        json!({
            "capture_id": c.id,
            "target": { "pid": c.target.pid, "kind": c.target.kind, "name": c.target.name },
            "events": c.events.len(),
            "window_ms": round3(self.index.window_ms()),
            "threads": self.index.threads.len(),
            "frame_scope": self.frame_scope,
            "frames": self.frames.len(),
            "frame_cost_ms": Summary::of(&costs).to_json(),
            "dropped_by_buffer": c.dropped_overflow,
            "dropped_by_target": c.dropped_in_target,
            "complete": c.dropped_overflow == 0 && c.dropped_in_target == 0,
        })
    }

    // ---- top scopes ---------------------------------------------------------

    pub fn top_scopes(&self, p: &TopParams) -> Value {
        let ix = &self.index;
        let mut accs: BTreeMap<(Option<u64>, String), Acc> = BTreeMap::new();
        for i in ix.scopes() {
            let e = ix.event(i);
            if let Some(t) = &p.thread {
                if !ix
                    .thread_name(e.thread_id)
                    .to_ascii_lowercase()
                    .contains(&t.to_ascii_lowercase())
                {
                    continue;
                }
            }
            if let Some(n) = &p.name {
                if !e.name.to_ascii_lowercase().contains(&n.to_ascii_lowercase()) {
                    continue;
                }
            }
            let key = (
                (p.group == Group::ThreadAndName).then_some(e.thread_id),
                e.name.clone(),
            );
            let acc = accs.entry(key).or_default();
            acc.thread_id = (p.group == Group::ThreadAndName).then_some(e.thread_id);
            acc.calls.push(ms(e.duration_ns));
            acc.total_ns += e.duration_ns;
            acc.self_ns += ix.self_ns[i];
            if let Some(f) = self.frame_at(e.start_ns) {
                *acc.per_frame.entry(f).or_default() += ms(e.duration_ns);
            }
        }
        let frames = self.frames.len().max(1) as f64;
        let mut rows: Vec<(String, Acc)> = accs.into_iter().map(|((_, n), a)| (n, a)).collect();
        let key = |a: &Acc| -> f64 {
            match p.sort {
                Sort::SelfTime => a.self_ns as f64,
                Sort::Total => a.total_ns as f64,
                Sort::Max => a.calls.iter().copied().fold(0.0, f64::max),
                Sort::Count => a.calls.len() as f64,
                Sort::MeanCall => a.total_ns as f64 / a.calls.len().max(1) as f64,
            }
        };
        rows.sort_by(|a, b| key(&b.1).total_cmp(&key(&a.1)));
        let window = ms(ix.span.1 - ix.span.0).max(1e-9);
        let out: Vec<Value> = rows
            .iter()
            .take(p.limit)
            .map(|(name, a)| {
                let per_frame_totals: Vec<f64> = a.per_frame.values().copied().collect();
                let worst_frame = a
                    .per_frame
                    .iter()
                    .max_by(|x, y| x.1.total_cmp(y.1))
                    .map(|(f, _)| *f);
                let mut row = json!({
                    "name": name,
                    "calls": a.calls.len(),
                    "total_ms": round3(ms(a.total_ns)),
                    "self_ms": round3(ms(a.self_ns)),
                    "self_pct_of_window": round3(ms(a.self_ns) / window * 100.0),
                    "per_frame_mean_ms": round3(ms(a.total_ns) / frames),
                    "per_frame_self_mean_ms": round3(ms(a.self_ns) / frames),
                    "calls_per_frame": round3(a.calls.len() as f64 / frames),
                    "per_call_ms": Summary::of(&a.calls).to_json(),
                    "per_frame_total_ms": Summary::of(&per_frame_totals).to_json(),
                    "worst_frame": worst_frame,
                });
                if let Some(t) = a.thread_id {
                    row["thread"] = json!(ix.thread_name(t));
                }
                row
            })
            .collect();
        json!({
            "overview": self.overview(),
            "group": if p.group == Group::Name { "name" } else { "thread_and_name" },
            "rows_total": rows.len(),
            "scopes": out,
            "worst_frames": self.worst_frames(5),
        })
    }

    /// The `n` frames with the longest period, each with its biggest scopes.
    pub fn worst_frames(&self, n: usize) -> Vec<Value> {
        let mut order: Vec<&Frame> = self.frames.iter().collect();
        order.sort_by_key(|f| std::cmp::Reverse(f.period_ns));
        order
            .into_iter()
            .take(n)
            .map(|f| {
                json!({
                    "frame": f.index,
                    "at_ms": round3(ms(f.start_ns - self.index.span.0)),
                    "period_ms": round3(ms(f.period_ns)),
                    "cost_ms": round3(ms(f.cost_ns)),
                    "top_scopes": self.top_in_window(f.start_ns, f.window_end_ns(), 4),
                })
            })
            .collect()
    }

    /// The scopes with the most own time that start in `[from, to)`, any thread.
    fn top_in_window(&self, from: u64, to: u64, n: usize) -> Vec<Value> {
        let ix = &self.index;
        let mut sums: BTreeMap<(u64, &str), u64> = BTreeMap::new();
        for i in ix.scopes() {
            let e = ix.event(i);
            if e.start_ns >= from && e.start_ns < to {
                *sums.entry((e.thread_id, e.name.as_str())).or_default() += ix.self_ns[i];
            }
        }
        let mut v: Vec<_> = sums.into_iter().collect();
        v.sort_by_key(|&(_, ns)| std::cmp::Reverse(ns));
        v.into_iter()
            .take(n)
            .map(|((t, name), ns)| {
                json!({ "name": name, "thread": ix.thread_name(t), "self_ms": round3(ms(ns)) })
            })
            .collect()
    }

    // ---- one frame ------------------------------------------------------------

    /// `frame`: an index, or `None` for the one with the longest period (or
    /// cost, when `by_cost`).
    pub fn frame_breakdown(
        &self,
        frame: Option<usize>,
        by_cost: bool,
        max_depth: usize,
        min_ms: f64,
    ) -> Result<Value, String> {
        if self.frames.is_empty() {
            return Err("no frames in this capture: name the frame scope (frame_scope) or capture while the game renders".into());
        }
        let f = match frame {
            Some(i) => self
                .frames
                .get(i)
                .ok_or_else(|| format!("frame {i} is out of range (0..{})", self.frames.len()))?,
            None => self
                .frames
                .iter()
                .max_by_key(|f| if by_cost { f.cost_ns } else { f.period_ns })
                .expect("frames is not empty"),
        };
        let ix = &self.index;
        let origin = f.start_ns;
        let tree = match f.event {
            Some(root) => self.node(root, origin, max_depth, min_ms),
            None => {
                let roots: Vec<Value> = ix
                    .thread(f.thread_id)
                    .map(|t| {
                        t.order
                            .iter()
                            .copied()
                            .filter(|&i| {
                                ix.parent[i].is_none()
                                    && ix.event(i).start_ns >= f.start_ns
                                    && ix.event(i).start_ns < f.window_end_ns()
                            })
                            .map(|i| self.node(i, origin, max_depth, min_ms))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({ "name": "(frame)", "total_ms": round3(ms(f.cost_ns)), "children": roots })
            }
        };
        let path = f.event.map(|root| self.critical_path(root)).unwrap_or_default();

        let mut others = Vec::new();
        for t in &ix.threads {
            if t.id == f.thread_id {
                continue;
            }
            let mut busy = 0u64;
            let mut items: Vec<(u64, u64, usize)> = Vec::new();
            for &i in &t.order {
                let e = ix.event(i);
                let (s, en) = (e.start_ns, e.start_ns + e.duration_ns);
                let (cs, ce) = (s.max(f.start_ns), en.min(f.window_end_ns()));
                if ce <= cs {
                    continue;
                }
                if ix.parent[i].is_none() {
                    busy += ce - cs;
                }
                if ix.depth[i] <= 1 {
                    items.push((cs, ce, i));
                }
            }
            if busy == 0 {
                continue;
            }
            items.sort_by_key(|&(s, e, _)| std::cmp::Reverse(e - s));
            others.push(json!({
                "thread": t.name,
                "busy_ms": round3(ms(busy)),
                "scopes": items.iter().filter(|(s, e, _)| ms(e - s) >= min_ms).take(10).map(|&(s, e, i)| json!({
                    "name": ix.event(i).name,
                    "depth": ix.depth[i],
                    "offset_ms": round3(ms(s - origin)),
                    "ms_in_frame": round3(ms(e - s)),
                })).collect::<Vec<_>>(),
            }));
        }
        Ok(json!({
            "overview": self.overview(),
            "frame": {
                "index": f.index,
                "thread": ix.thread_name(f.thread_id),
                "at_ms": round3(ms(f.start_ns - ix.span.0)),
                "cost_ms": round3(ms(f.cost_ns)),
                "period_ms": round3(ms(f.period_ns)),
                "outside_scope_ms": round3(ms(f.period_ns.saturating_sub(f.cost_ns))),
            },
            "tree": tree,
            "critical_path": path,
            "other_threads": others,
        }))
    }

    fn node(&self, i: usize, origin: u64, max_depth: usize, min_ms: f64) -> Value {
        let ix = &self.index;
        let e = ix.event(i);
        let kids = ix.children(i);
        let mut shown = Vec::new();
        let (mut hidden_n, mut hidden_ns) = (0usize, 0u64);
        for c in kids {
            let ce = ix.event(c);
            if ms(ce.duration_ns) < min_ms || ix.depth[c] as usize > max_depth {
                hidden_n += 1;
                hidden_ns += ce.duration_ns;
            } else {
                shown.push(self.node(c, origin, max_depth, min_ms));
            }
        }
        let mut v = json!({
            "name": e.name,
            "offset_ms": round3(ms(e.start_ns.saturating_sub(origin))),
            "total_ms": round3(ms(e.duration_ns)),
            "self_ms": round3(ms(ix.self_ns[i])),
            "children": shown,
        });
        if let Some(loc) = &e.location {
            v["location"] = json!(loc);
        }
        if hidden_n > 0 {
            v["hidden"] = json!({ "scopes": hidden_n, "ms": round3(ms(hidden_ns)) });
        }
        v
    }

    /// From `root`, the longest child at each level down to a leaf.
    fn critical_path(&self, root: usize) -> Vec<Value> {
        let ix = &self.index;
        let mut path = Vec::new();
        let mut at = Some(root);
        while let Some(i) = at {
            let e = ix.event(i);
            path.push(json!({
                "name": e.name,
                "total_ms": round3(ms(e.duration_ns)),
                "self_ms": round3(ms(ix.self_ns[i])),
                "wait": is_wait_name(&e.name),
            }));
            at = ix
                .children(i)
                .into_iter()
                .max_by_key(|&c| ix.event(c).duration_ns);
        }
        path
    }

    // ---- frame times ----------------------------------------------------------

    pub fn frame_times(&self, threshold_ms: Option<f64>, series_limit: usize, hitch_limit: usize) -> Result<Value, String> {
        if self.frames.is_empty() {
            return Err("no frames in this capture".into());
        }
        // The last frame's period is only its cost: leave it out of the period figures.
        let periods: Vec<f64> = self.frames[..self.frames.len().saturating_sub(1).max(1)]
            .iter()
            .map(|f| ms(f.period_ns))
            .collect();
        let costs: Vec<f64> = self.frames.iter().map(|f| ms(f.cost_ns)).collect();
        let ps = Summary::of(&periods);
        let threshold = threshold_ms.unwrap_or_else(|| (ps.p50 * 2.0).max(ps.p50 + 8.0));
        let edges = [2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.7, 20.0, 25.0, 33.4, 50.0, 100.0];
        let mut counts = vec![0usize; edges.len() + 1];
        for p in &periods {
            counts[edges.iter().position(|e| p < e).unwrap_or(edges.len())] += 1;
        }
        let mut hist = Vec::new();
        let mut lo = 0.0;
        for (i, c) in counts.iter().enumerate() {
            let label = match edges.get(i) {
                Some(hi) => format!("{lo}-{hi}"),
                None => format!(">{lo}"),
            };
            hist.push(json!({ "ms": label, "frames": c, "pct": round3(*c as f64 / periods.len().max(1) as f64 * 100.0) }));
            if let Some(hi) = edges.get(i) {
                lo = *hi;
            }
        }
        let stride = periods.len().div_ceil(series_limit.max(1)).max(1);
        let series: Vec<f64> = periods
            .chunks(stride)
            .map(|c| round3(c.iter().copied().fold(0.0, f64::max)))
            .collect();
        let mut hitches: Vec<&Frame> = self.frames[..self.frames.len() - 1]
            .iter()
            .filter(|f| ms(f.period_ns) > threshold)
            .collect();
        let hitch_count = hitches.len();
        hitches.sort_by_key(|f| std::cmp::Reverse(f.period_ns));
        let hitch_rows: Vec<Value> = hitches
            .into_iter()
            .take(hitch_limit)
            .map(|f| {
                json!({
                    "frame": f.index,
                    "at_ms": round3(ms(f.start_ns - self.index.span.0)),
                    "period_ms": round3(ms(f.period_ns)),
                    "cost_ms": round3(ms(f.cost_ns)),
                    "outside_scope_ms": round3(ms(f.period_ns.saturating_sub(f.cost_ns))),
                    "ran": self.top_in_window(f.start_ns, f.window_end_ns(), 5),
                })
            })
            .collect();
        let mean_period = ps.mean.max(1e-9);
        Ok(json!({
            "overview": self.overview(),
            "frame_period_ms": ps.to_json(),
            "frame_cost_ms": Summary::of(&costs).to_json(),
            "fps_mean": round3(1000.0 / mean_period),
            "fps_1pct_low": round3(1000.0 / ps.p99.max(1e-9)),
            "histogram": hist,
            "hitch_threshold_ms": round3(threshold),
            "hitches": hitch_count,
            "worst_hitches": hitch_rows,
            "series_ms": series,
            "series_stride_frames": stride,
            "other_frame_streams": self.other_streams(),
        }))
    }

    /// The period and cost of the other frame-like scopes (the simulation
    /// tick next to the render frame).
    fn other_streams(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for name in frames::CANDIDATES {
            if Some(*name) == self.frame_scope.as_deref() {
                continue;
            }
            let spec = FrameSpec {
                scope: Some((*name).into()),
                thread_id: None,
            };
            if let Some((n, fr)) = frames::find(self.capture(), &spec) {
                if fr.len() < 2 {
                    continue;
                }
                let periods: Vec<f64> = fr[..fr.len() - 1].iter().map(|f| ms(f.period_ns)).collect();
                let costs: Vec<f64> = fr.iter().map(|f| ms(f.cost_ns)).collect();
                out.push(json!({
                    "scope": n,
                    "count": fr.len(),
                    "period_ms": Summary::of(&periods).to_json(),
                    "cost_ms": Summary::of(&costs).to_json(),
                }));
            }
        }
        out
    }

    // ---- threads ----------------------------------------------------------------

    pub fn threads(&self) -> Value {
        let ix = &self.index;
        let window = (ix.span.1 - ix.span.0).max(1);
        let mut rows = Vec::new();
        for t in &ix.threads {
            let mut intervals: Vec<(u64, u64)> = Vec::new();
            let mut waits = 0u64;
            let mut work: BTreeMap<&str, (u64, usize)> = BTreeMap::new();
            let mut names = std::collections::BTreeSet::new();
            for &i in &t.order {
                let e = ix.event(i);
                names.insert(e.name.as_str());
                if ix.parent[i].is_none() {
                    intervals.push((e.start_ns, e.start_ns + e.duration_ns));
                }
                if is_wait_name(&e.name) {
                    waits += ix.self_ns[i];
                } else {
                    let w = work.entry(e.name.as_str()).or_default();
                    w.0 += ix.self_ns[i];
                    w.1 += 1;
                }
            }
            let busy = union_len(&mut intervals);
            let mut top: Vec<_> = work.into_iter().collect();
            top.sort_by_key(|(_, (ns, _))| std::cmp::Reverse(*ns));
            rows.push(json!({
                "thread": t.name,
                "id": t.id,
                "gpu": t.name == "GPU",
                "scopes": t.order.len(),
                "distinct_scopes": names.len(),
                "busy_ms": round3(ms(busy)),
                "idle_ms": round3(ms(window.saturating_sub(busy))),
                "utilisation_pct": round3(busy as f64 / window as f64 * 100.0),
                "wait_scope_self_ms": round3(ms(waits)),
                "top_work_scopes": top.iter().take(5).map(|(n, (ns, c))| json!({
                    "name": n, "self_ms": round3(ms(*ns)), "calls": c,
                })).collect::<Vec<_>>(),
            }));
        }
        rows.sort_by(|a, b| {
            b["busy_ms"].as_f64().partial_cmp(&a["busy_ms"].as_f64()).unwrap_or(std::cmp::Ordering::Equal)
        });
        json!({
            "overview": self.overview(),
            "note": "busy = union of the thread's outermost scopes; time between them is idle or uninstrumented. wait_scope_self_ms is the own time of scopes named like a wait (wait, lock, fence, present, acquire, sleep, idle...).",
            "threads": rows,
            "waits": self.waits(),
        })
    }

    /// Wait-like scopes by total self time, any thread.
    fn waits(&self) -> Vec<Value> {
        let ix = &self.index;
        let mut sums: BTreeMap<(u64, &str), (u64, usize, u64)> = BTreeMap::new();
        for i in ix.scopes() {
            let e = ix.event(i);
            if is_wait_name(&e.name) {
                let s = sums.entry((e.thread_id, e.name.as_str())).or_default();
                s.0 += ix.self_ns[i];
                s.1 += 1;
                s.2 = s.2.max(e.duration_ns);
            }
        }
        let mut v: Vec<_> = sums.into_iter().collect();
        v.sort_by_key(|(_, (ns, _, _))| std::cmp::Reverse(*ns));
        v.into_iter()
            .take(15)
            .map(|((t, n), (ns, c, max))| {
                json!({ "thread": ix.thread_name(t), "scope": n, "self_ms": round3(ms(ns)), "calls": c, "max_ms": round3(ms(max)) })
            })
            .collect()
    }

    // ---- GPU --------------------------------------------------------------------

    pub fn gpu(&self) -> Value {
        let c = self.capture();
        let is_gpu = |e: &profiling::ProfileEvent| {
            e.thread_name.as_deref() == Some("GPU") || e.track_name.as_deref() == Some("GPU")
        };
        let gpu: Vec<&profiling::ProfileEvent> = c.events.iter().filter(|e| is_gpu(e) && !is_marker(e)).collect();
        if gpu.is_empty() {
            return json!({
                "overview": self.overview(),
                "available": false,
                "reason": "the capture holds no GPU events. Helio publishes per-pass GPU times (timestamp queries read back asynchronously) as events on the `GPU` track while a recording is on, from the editor viewport renderer (`helio_gpu_profiler_update`, `helio_emit_gpu_passes`). A process that renders through another path publishes none.",
            });
        }
        let mut totals = Vec::new();
        let mut passes: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let mut meta = BTreeMap::new();
        for e in &gpu {
            if e.depth == 0 {
                totals.push(ms(e.duration_ns));
            } else {
                passes.entry(e.name.as_str()).or_default().push(ms(e.duration_ns));
            }
            if let Some(m) = &e.metadata {
                for kv in m.split(';') {
                    if let Some((k, v)) = kv.split_once('=') {
                        meta.insert(k.to_owned(), v.to_owned());
                    }
                }
            }
        }
        let total_ms: f64 = totals.iter().sum();
        let mut rows: Vec<(&str, Vec<f64>)> = passes.into_iter().collect();
        rows.sort_by(|a, b| b.1.iter().sum::<f64>().total_cmp(&a.1.iter().sum::<f64>()));
        json!({
            "overview": self.overview(),
            "available": true,
            "gpu_frames": totals.len(),
            "gpu_frame_ms": Summary::of(&totals).to_json(),
            "passes": rows.iter().map(|(n, v)| json!({
                "pass": n,
                "mean_ms": round3(v.iter().sum::<f64>() / totals.len().max(1) as f64),
                "per_sample_ms": Summary::of(v).to_json(),
                "share_pct": round3(v.iter().sum::<f64>() / total_ms.max(1e-9) * 100.0),
            })).collect::<Vec<_>>(),
            "last_readback": { "lag_frames": meta.get("lag_frames"), "readback_drops": meta.get("readback_drops"), "query_overflows": meta.get("query_overflows") },
            "note": "pass times are timestamp-query durations; their placement on the timeline is an estimate.",
        })
    }

    // ---- latency ------------------------------------------------------------------

    pub fn latency(&self) -> Value {
        let c = self.capture();
        let samples: Vec<&profiling::ProfileEvent> = c.events.iter().filter(|e| latency::is_sample(e)).collect();
        if samples.is_empty() {
            return json!({
                "overview": self.overview(),
                "available": false,
                "reason": "the capture holds no `__LATENCY__` samples: the game publishes one per followed input while it is recorded, so press keys or move the mouse during the capture.",
            });
        }
        let mut order: Vec<String> = Vec::new();
        let mut stages: HashMap<String, Vec<f64>> = HashMap::new();
        let mut totals = Vec::new();
        for s in &samples {
            totals.push(ms(s.duration_ns));
            for (k, v) in latency::parse_stages(s.metadata.as_deref().unwrap_or("")) {
                if !order.contains(&k) {
                    order.push(k.clone());
                }
                stages.entry(k).or_default().push(v);
            }
        }
        let total = Summary::of(&totals);
        let mut worst: Vec<&&profiling::ProfileEvent> = samples.iter().collect();
        worst.sort_by_key(|e| std::cmp::Reverse(e.duration_ns));
        json!({
            "overview": self.overview(),
            "available": true,
            "samples": samples.len(),
            "input_to_present_ms": total.to_json(),
            "stages": order.iter().map(|k| {
                let s = Summary::of(&stages[k]);
                json!({ "stage": k, "mean_ms": round3(s.mean), "p50_ms": round3(s.p50), "p95_ms": round3(s.p95), "p99_ms": round3(s.p99), "worst_ms": round3(s.max),
                        "share_of_mean_pct": round3(s.mean / total.mean.max(1e-9) * 100.0) })
            }).collect::<Vec<_>>(),
            "worst_samples": worst.iter().take(5).map(|e| json!({
                "at_ms": round3(ms(e.start_ns.saturating_sub(self.index.span.0))),
                "total_ms": round3(ms(e.duration_ns)),
                "stages": latency::parse_stages(e.metadata.as_deref().unwrap_or("")).into_iter().map(|(k, v)| json!({k: v})).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "note": "the chain ends when present() returns; the display adds the present queue depth times the frame time (and vsync) on top.",
        })
    }

    // ---- gaps ----------------------------------------------------------------------

    /// Where instrumentation is missing: scopes that are mostly their own time
    /// (nothing inside them is scoped), and how much of each frame sits in the
    /// frame scope's own time.
    pub fn gaps(&self, min_self_ms: f64, limit: usize) -> Value {
        let ix = &self.index;
        let mut sums: BTreeMap<(u64, &str), (u64, u64, usize)> = BTreeMap::new();
        for i in ix.scopes() {
            let e = ix.event(i);
            let s = sums.entry((e.thread_id, e.name.as_str())).or_default();
            s.0 += ix.self_ns[i];
            s.1 += e.duration_ns;
            s.2 += 1;
        }
        let frames = self.frames.len().max(1) as f64;
        let mut rows: Vec<_> = sums
            .into_iter()
            .filter(|(_, (s, t, _))| ms(*s) >= min_self_ms && *s * 10 >= *t * 6)
            .collect();
        rows.sort_by_key(|(_, (s, _, _))| std::cmp::Reverse(*s));
        let frame_self = self.frame_scope.as_deref().map(|n| {
            let (mut own, mut total) = (0u64, 0u64);
            for i in ix.scopes() {
                if ix.event(i).name == n {
                    own += ix.self_ns[i];
                    total += ix.event(i).duration_ns;
                }
            }
            json!({
                "scope": n,
                "own_ms_per_frame": round3(ms(own) / frames),
                "of_frame_cost_pct": round3(own as f64 / total.max(1) as f64 * 100.0),
                "meaning": "time inside the frame scope that no inner scope covers",
            })
        });
        let shallow: Vec<Value> = ix
            .threads
            .iter()
            .filter(|t| t.order.len() > 20)
            .filter_map(|t| {
                let depth = t.order.iter().map(|&i| ix.depth[i]).max().unwrap_or(0);
                let distinct: std::collections::BTreeSet<&str> = t.order.iter().map(|&i| ix.event(i).name.as_str()).collect();
                (depth == 0 && distinct.len() <= 2).then(|| json!({ "thread": t.name, "scopes": t.order.len(), "distinct": distinct.len() }))
            })
            .collect();
        json!({
            "overview": self.overview(),
            "frame_scope_self": frame_self,
            "mostly_own_time": rows.iter().take(limit).map(|((t, n), (s, tot, c))| json!({
                "thread": ix.thread_name(*t),
                "scope": n,
                "self_ms_total": round3(ms(*s)),
                "self_ms_per_frame": round3(ms(*s) / frames),
                "self_pct_of_scope": round3(*s as f64 / (*tot).max(1) as f64 * 100.0),
                "calls": c,
                "advice": "60% or more of this scope is its own time: scope the steps inside it to see which one costs",
            })).collect::<Vec<_>>(),
            "threads_with_flat_instrumentation": shallow,
            "threads_total": ix.threads.len(),
            "note": "a thread that does work but has no scopes does not appear at all; compare with profile_threads and the Threads view (threads_list)",
        })
    }
}

/// Length of the union of intervals.
fn union_len(intervals: &mut [(u64, u64)]) -> u64 {
    intervals.sort_unstable();
    let (mut total, mut end) = (0u64, 0u64);
    for &(s, e) in intervals.iter() {
        let s = s.max(end);
        if e > s {
            total += e - s;
            end = e;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ev, game};

    fn cap(events: Vec<profiling::ProfileEvent>) -> Capture {
        Capture {
            events,
            ..Default::default()
        }
    }

    #[test]
    fn top_scopes_rank_by_self_time_and_know_the_frames() {
        let c = cap(game(10, 16, 10, None));
        let a = Analyzer::new(&c, &FrameSpec::default());
        let v = a.top_scopes(&TopParams::default());
        let rows = v["scopes"].as_array().unwrap();
        // render 3.5 ms own + wait 3.5 ms own: both above Game::Frame's 3 ms.
        assert_eq!(rows[0]["self_ms"], 35.0);
        assert_eq!(v["overview"]["frames"], 10);
        let wait = rows.iter().find(|r| r["name"] == "wait present").unwrap();
        assert_eq!(wait["calls"], 10);
        assert_eq!(wait["per_frame_mean_ms"], 3.5);
        let by_thread = a.top_scopes(&TopParams {
            group: Group::ThreadAndName,
            thread: Some("t2".into()),
            ..Default::default()
        });
        assert_eq!(by_thread["scopes"].as_array().unwrap().len(), 1);
        assert_eq!(by_thread["scopes"][0]["thread"], "t2");
    }

    #[test]
    fn the_worst_frame_is_found_and_its_breakdown_has_a_critical_path() {
        let c = cap(game(10, 16, 10, Some((6, 4))));
        let a = Analyzer::new(&c, &FrameSpec::default());
        let v = a.frame_breakdown(None, false, 8, 0.0).unwrap();
        assert_eq!(v["frame"]["index"], 6);
        assert_eq!(v["frame"]["cost_ms"], 40.0);
        let path: Vec<&str> = v["critical_path"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(path, ["Game::Frame", "render", "wait present"]);
        assert_eq!(v["critical_path"][2]["wait"], true);
        assert_eq!(v["other_threads"][0]["thread"], "t2");
        assert!(a.frame_breakdown(Some(99), false, 8, 0.0).is_err());
    }

    #[test]
    fn frame_times_report_hitches_with_what_ran() {
        let c = cap(game(20, 16, 10, Some((10, 5))));
        let a = Analyzer::new(&c, &FrameSpec::default());
        let v = a.frame_times(None, 100, 5).unwrap();
        assert_eq!(v["hitches"], 1);
        assert_eq!(v["worst_hitches"][0]["frame"], 10);
        assert_eq!(v["worst_hitches"][0]["period_ms"], 80.0);
        assert!(v["worst_hitches"][0]["ran"].as_array().unwrap().len() >= 3);
        assert_eq!(v["frame_period_ms"]["p50"], 16.0);
        assert!(v["histogram"].as_array().unwrap().len() > 5);
    }

    #[test]
    fn threads_report_busy_idle_and_waits() {
        let c = cap(game(10, 16, 10, None));
        let a = Analyzer::new(&c, &FrameSpec::default());
        let v = a.threads();
        let t1 = v["threads"].as_array().unwrap().iter().find(|t| t["thread"] == "t1").unwrap();
        assert_eq!(t1["wait_scope_self_ms"], 35.0);
        let util = t1["utilisation_pct"].as_f64().unwrap();
        assert!(util > 55.0 && util < 70.0, "{util}");
        assert_eq!(v["waits"][0]["scope"], "wait present");
    }

    #[test]
    fn gpu_and_latency_say_so_when_absent_and_aggregate_when_present() {
        let mut events = game(5, 16, 10, None);
        let c = cap(events.clone());
        let a = Analyzer::new(&c, &FrameSpec::default());
        assert_eq!(a.gpu()["available"], false);
        assert_eq!(a.latency()["available"], false);

        let mut root = ev("helio_frame (estimated GPU placement)", 0, 1_000_000_000, 6_000_000, 0);
        root.thread_name = Some("GPU".into());
        let mut pass = ev("Lighting", 0, 1_000_000_000, 4_000_000, 1);
        pass.thread_name = Some("GPU".into());
        events.extend([root, pass]);
        for (i, total) in [10_000_000u64, 30_000_000].into_iter().enumerate() {
            let mut s = ev(latency::EVENT_NAME, 1, 1_000_000_000 + i as u64 * 50_000_000, total, 0);
            s.metadata = Some(latency::encode_stages(&[("os_to_tick", total as f32 / 2e6), ("present", total as f32 / 2e6)]));
            events.push(s);
        }
        let c = cap(events);
        let a = Analyzer::new(&c, &FrameSpec::default());
        let g = a.gpu();
        assert_eq!(g["available"], true);
        assert_eq!(g["passes"][0]["pass"], "Lighting");
        let l = a.latency();
        assert_eq!(l["samples"], 2);
        assert_eq!(l["input_to_present_ms"]["max"], 30.0);
        assert_eq!(l["stages"][0]["stage"], "os_to_tick");
        assert_eq!(l["stages"][0]["mean_ms"], 10.0);
    }

    #[test]
    fn gaps_flag_scopes_that_are_mostly_their_own_time() {
        let c = cap(game(10, 16, 10, None));
        let a = Analyzer::new(&c, &FrameSpec::default());
        let v = a.gaps(1.0, 10);
        let names: Vec<&str> = v["mostly_own_time"].as_array().unwrap().iter().map(|r| r["scope"].as_str().unwrap()).collect();
        assert!(names.contains(&"wait present"));
        assert!(!names.contains(&"Game::Frame"));
        assert_eq!(v["frame_scope_self"]["scope"], "Game::Frame");
    }

    #[test]
    fn an_empty_capture_does_not_panic() {
        let c = cap(Vec::new());
        let a = Analyzer::new(&c, &FrameSpec::default());
        assert!(a.frame_times(None, 10, 5).is_err());
        let _ = a.top_scopes(&TopParams::default());
        let _ = a.threads();
        let _ = a.gaps(0.0, 5);
    }
}
