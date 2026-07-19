//! Pulsar SceneDB — AAA-Grade Stress Test TUI
//!
//! Interactive dashboard with keyboard navigation, drill-down workload
//! details, and scrollable event log.
//!
//! Run:  cargo run -p pulsar_scenedb --example stress_tui
//!
//! Controls:
//!   Tab         — cycle focus: workloads ↔ log
//!   ↑/↓         — move cursor (workloads) / scroll (log)
//!   Enter / d   — toggle detail view for highlighted workload
//!   1-8         — open detail for workload N
//!   Esc         — back from detail / quit
//!   q           — quit
//!   p           — pause / resume
//!   r           — reset counters
//!   Home        — scroll log to top
//!   End         — scroll log to bottom
//!   PgUp / PgDn — scroll log by page

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use pulsar_scenedb::*;
use pulsar_scenedb_derive::SceneStore;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Sparkline,
};
use ratatui::Terminal;
use std::io::stdout;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── UI State ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Focus { Workloads, Log }

struct UiState {
    focus: Focus,
    selected: Option<usize>,
    highlight: usize,
    log_scroll: usize,
}

// ── Metrics ────────────────────────────────────────────────────────────────

const NUM_WORKLOADS: usize = 9;

struct WorkloadMetrics {
    name: &'static str,
    desc: &'static str,
    ops: AtomicU64,
    errors: AtomicU64,
    running: AtomicBool,
    latency_ns: AtomicU64,
    spark_data: Mutex<Vec<u64>>,
}

impl WorkloadMetrics {
    fn new(name: &'static str, desc: &'static str) -> Self {
        Self {
            name,
            desc,
            ops: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            running: AtomicBool::new(true),
            latency_ns: AtomicU64::new(0),
            spark_data: Mutex::new(Vec::new()),
        }
    }

    fn tick(&self, dur: Duration) {
        self.ops.fetch_add(1, Ordering::Relaxed);
        self.latency_ns.store(dur.as_nanos() as u64, Ordering::Relaxed);
        if let Ok(mut sd) = self.spark_data.lock() {
            sd.push(dur.as_micros() as u64);
            if sd.len() > 120 {
                sd.remove(0);
            }
        }
    }

    fn error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }
}

struct LogEntry {
    msg: String,
    color: Color,
}

struct AppState {
    metrics: [WorkloadMetrics; NUM_WORKLOADS],
    log: Mutex<Vec<LogEntry>>,
    paused: AtomicBool,
    start: Instant,
}

impl AppState {
    fn log(&self, color: Color, msg: impl Into<String>) {
        if let Ok(mut l) = self.log.lock() {
            l.push(LogEntry { msg: msg.into(), color });
            if l.len() > 64 {
                l.remove(0);
            }
        }
    }
}

// ── GPU Test Component (exercises #[derive(SceneStore)] + #[gpu] fields) ────

/// SceneStore component with mixed GPU/CPU fields.
///
/// When `gpu` feature is on the `#[cfg_attr]` expands to `#[gpu]`, and the
/// derive macro generates a `GpuColumnSet` impl (descriptors + write path).
/// When `gpu` is off the attributes vanish and `SceneColumnSet` is tested alone.
#[derive(Copy, Clone, SceneStore)]
struct GpuTestComponent {
    #[cfg_attr(feature = "gpu", gpu)]
    x: f32,
    #[cfg_attr(feature = "gpu", gpu)]
    y: f64,
    health: u16,
    #[cfg_attr(feature = "gpu", gpu(mirror = Once))]
    color: u32,
}

// ── Workload trait ─────────────────────────────────────────────────────────

trait Workload: Send {
    fn run(&self, state: &AppState, idx: usize);
}

// ── Workload 1: Entity Storm ───────────────────────────────────────────────

struct EntityStorm;
impl Workload for EntityStorm {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mut world = World::new();
        let mut entities = Vec::with_capacity(10_000);
        state.log(Color::Cyan, "Entity storm started — spawning 10k entities per batch");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // Spawn a batch.
            for _ in 0..1000 {
                if entities.len() >= 10_000 {
                    break;
                }
                entities.push(world.spawn());
            }
            // Despawn a portion.
            let n = entities.len().min(200);
            for e in entities.drain(..n) {
                world.despawn(e);
            }
            // Spawn more to keep pressure up.
            for _ in 0..n {
                if entities.len() < 10_000 {
                    entities.push(world.spawn());
                }
            }
            m.tick(t0.elapsed());
        }
    }
}

// ── Workload 2: Cell Alloc Storm ──────────────────────────────────────────

struct CellAllocStorm;
impl Workload for CellAllocStorm {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mut cells: Vec<CellStorage> = Vec::new();
        state.log(Color::Cyan, "Cell alloc storm started — 128 concurrent cells");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // Hammer a random cell: alloc / free / compact cycle.
            let cell_idx = (m.ops.load(Ordering::Relaxed) as usize) % cells.len().max(1);
            if !cells.is_empty() {
                let c = &mut cells[cell_idx];
                if let Some(h) = c.alloc() {
                    let row = c.row_of(h).unwrap_or(0) as usize;
                    if row < c.rows_in_use() as usize {
                        c.user_column_mut::<f32>(0)[row] = 1.0;
                    }
                } else {
                    // Cell full — compact and try again.
                    let before = c.rows_in_use();
                    c.compact();
                    if c.rows_in_use() < before {
                        m.tick(t0.elapsed());
                        continue;
                    }
                }
            }
            // Occasionally create a new cell.
            if cells.len() < 128 && m.ops.load(Ordering::Relaxed) % 500 == 0 {
                if let Ok(c) = CellStorage::new(&[ColumnDesc::of::<f32>()], 256) {
                    cells.push(c);
                }
            }
            // Occasionally free handles to create compaction pressure.
            if !cells.is_empty() {
                let idx = m.ops.load(Ordering::Relaxed) as usize % cells.len();
                let c = &mut cells[idx];
                if let Some(h) = c.alloc() {
                    c.free(h);
                }
            }
            m.tick(t0.elapsed());
        }
    }
}

// ── Workload 3: Spatial Query Storm ────────────────────────────────────────

struct SpatialQueryStorm;
impl Workload for SpatialQueryStorm {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mut sc = SpatialCell::new(1024).unwrap();
        // Fill with random AABBs (capacity is 1024).
        for _ in 0..1000 {
            let min = [rand::random::<f32>() * 1000.0 - 500.0; 3];
            let max = [
                min[0] + rand::random::<f32>() * 50.0 + 0.1,
                min[1] + rand::random::<f32>() * 50.0 + 0.1,
                min[2] + rand::random::<f32>() * 50.0 + 0.1,
            ];
            sc.alloc(Aabb { min, max });
        }
        // Pre-allocated scratch buffers — expand under load, retract when idle.
        let mut out: Vec<u32> = Vec::with_capacity(1024);
        let mut liveness_words: Vec<u64> = Vec::with_capacity(1024 / 64);
        let mut iter = 0u64;
        state.log(Color::Cyan, "Spatial query storm started — 1000 AABBs, AABB + frustum queries");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            let len = sc.rows_in_use() as usize;
            let n_words = len.div_ceil(64);

            // Expand buffers on demand.
            if out.len() < len {
                out.resize(len, 0);
            }
            if liveness_words.len() < n_words {
                liveness_words.resize(n_words, 0);
            }

            // Capture liveness snapshot without allocation.
            let lw = sc.liveness().words();
            for (i, w) in lw.iter().enumerate().take(n_words) {
                liveness_words[i] = w.load(std::sync::atomic::Ordering::Relaxed);
            }

            // AABB query.
            let q = Aabb {
                min: [rand::random::<f32>() * 2000.0 - 1000.0; 3],
                max: [
                    rand::random::<f32>() * 100.0 + 0.1,
                    rand::random::<f32>() * 100.0 + 0.1,
                    rand::random::<f32>() * 100.0 + 0.1,
                ],
            };
            sc.query_aabb_in(&q, &liveness_words[..n_words], &mut out[..len]);
            // Frustum query.
            let planes = [
                [1.0, 0.0, 0.0, 1000.0],
                [-1.0, 0.0, 0.0, 1000.0],
                [0.0, 1.0, 0.0, 1000.0],
                [0.0, -1.0, 0.0, 1000.0],
                [0.0, 0.0, 1.0, 1000.0],
                [0.0, 0.0, -1.0, 1000.0],
            ];
            sc.query_frustum_in(&Frustum { planes }, &liveness_words[..n_words], &mut out[..len]);

            // Retract buffers every 256 iterations if over-allocated >2x.
            iter += 1;
            if iter & 0xFF == 0 {
                if liveness_words.len() > n_words.max(16) * 2 {
                    liveness_words.truncate(n_words.max(16));
                    liveness_words.shrink_to_fit();
                }
                if out.len() > len.max(1024) * 2 {
                    out.truncate(len.max(1024));
                    out.shrink_to_fit();
                }
            }

            m.tick(t0.elapsed());
        }
    }
}

// ── Workload 4: Handle Pressure ───────────────────────────────────────────

struct HandlePressure;
impl Workload for HandlePressure {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mut reg = HandleRegistry::new();
        let mut handles = Vec::with_capacity(1024);
        state.log(Color::Cyan, "Handle pressure started — cycling 1024 slots at max rate");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // Allocate a batch.
            for _ in 0..256 {
                if handles.len() < 1024 {
                    handles.push(reg.allocate(handles.len() as u32));
                }
            }
            // Free a batch.
            for h in handles.drain(..handles.len().min(128)) {
                reg.free(h);
            }
            // Verify stale handles are rejected.
            for i in 0..handles.len().min(10) {
                let h = handles[i];
                if !reg.is_live(h) {
                    // Might have been freed — expected.
                }
            }
            m.tick(t0.elapsed());
        }
    }
}

// ── Workload 5: Lease Pressure ────────────────────────────────────────────

struct LeasePressure;
impl Workload for LeasePressure {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mask = LeaseMask::new();
        state.log(Color::Cyan, "Lease pressure started — hammering 64-slot pool");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // Exhaust the pool, then release.
            let mut leases = Vec::with_capacity(64);
            while let Some(l) = mask.acquire() {
                leases.push(l);
            }
            // Verify exhaustion.
            assert!(mask.acquire().is_none());
            drop(leases);
            m.tick(t0.elapsed());
        }
    }
}

// ── Workload 6: GenericColumn Stress ───────────────────────────────────────
//
// This is the most important workload: it specifically targets the init-bit
// desync bug in GenericColumn::swap.  Under heavy concurrent swap pressure
// the desync triggers UB via assume_init_ref() on uninitialized memory.

struct GenericColumnStress;
impl Workload for GenericColumnStress {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mut columns: Vec<GenericColumn<Box<i32>>> = (0..64)
            .map(|_| GenericColumn::<Box<i32>>::new(128))
            .collect();
        state.log(
            Color::Cyan,
            "GenericColumn stress started — 64 columns × 128 slots, hammering swap",
        );
        let _rng = rand::thread_rng();
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            let col_idx = (m.ops.load(Ordering::Relaxed) as usize) % columns.len();
            let col = &mut columns[col_idx];
            // Phase 1: push new rows.
            for i in 0..4 {
                col.set(i, Box::new(i as i32));
            }
            // Phase 2: free some, then swap aggressively to desync bits.
            col.free(1);
            col.free(3);
            col.swap(0, 1);
            col.swap(2, 3);
            // Phase 3: read — this is where UB surfaces under Miri.
            let _ = col.get(0);
            let _ = col.get(2);
            // Phase 4: clean up before next iteration.
            for i in 0..4 {
                let _ = col.free(i);
            }
            m.tick(t0.elapsed());
        }
    }
}

// ── Workload 7: Concurrent Read/Write ──────────────────────────────────────

struct ConcurrentRW;
impl Workload for ConcurrentRW {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mask = Arc::new(LivenessMask::new(8192));
        let mut readers = Vec::new();
        state.log(Color::Cyan, "Concurrent RW started — 1 writer + 4 readers on LivenessMask");
        let running = Arc::new(AtomicBool::new(true));
        for _ in 0..4 {
            let m2 = Arc::clone(&mask);
            let r = Arc::clone(&running);
            readers.push(std::thread::spawn(move || {
                while r.load(Ordering::Relaxed) {
                    // Reader: iterate all rows, compute live count.
                    let mut _count = 0u32;
                    for row in 0..8192 {
                        if m2.is_live(row) {
                            _count += 1;
                        }
                    }
                    std::hint::spin_loop();
                }
            }));
        }
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // Writer: toggle random rows.
            for _ in 0..100 {
                let row = (m.ops.load(Ordering::Relaxed) as u32 % 8192) as u32;
                if row % 2 == 0 {
                    mask.set_live(row);
                } else {
                    mask.set_dead(row);
                }
            }
            // Read live count (stale due to relaxed atomics — that's the point).
            let _ = mask.live_count();
            m.tick(t0.elapsed());
        }
        running.store(false, Ordering::Relaxed);
        for r in readers {
            r.join().ok();
        }
    }
}

// ── Workload 8: Mixed Frame (Full Game Sim) ───────────────────────────────
//
// Simulates one complete game frame:
//   1. Simulate phase:    alloc new entities, update transforms
//   2. Harvest phase:     spatial queries for culling
//   3. Boundary phase:    compact dead entities

struct MixedFrame;
impl Workload for MixedFrame {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        let mut sc = SpatialCell::new(1024).unwrap();
        let mut handles = Vec::with_capacity(1024);
        let mut out: Vec<u32> = Vec::with_capacity(1024);
        let mut liveness_words: Vec<u64> = Vec::with_capacity(1024 / 64);
        state.log(Color::Cyan, "Mixed frame started — full game-loop simulation");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // ── Simulate phase ──
            for _ in 0..50 {
                if handles.len() < 900 {
                    let min = [rand::random::<f32>() * 100.0 - 50.0; 3];
                    let max = [
                        min[0] + rand::random::<f32>() * 10.0 + 0.1,
                        min[1] + rand::random::<f32>() * 10.0 + 0.1,
                        min[2] + rand::random::<f32>() * 10.0 + 0.1,
                    ];
                    if let Some(h) = sc.alloc(Aabb { min, max }) {
                        handles.push(h);
                    }
                }
            }
            let to_kill = handles.len() / 10;
            for h in handles.drain(..to_kill) {
                sc.free(h);
            }
            // ── Harvest phase (zero-alloc) ──
            let len = sc.rows_in_use() as usize;
            let n_words = len.div_ceil(64);
            if out.len() < len { out.resize(len, 0); }
            if liveness_words.len() < n_words { liveness_words.resize(n_words, 0); }
            let lw = sc.liveness().words();
            for (i, w) in lw.iter().enumerate().take(n_words) {
                liveness_words[i] = w.load(std::sync::atomic::Ordering::Relaxed);
            }
            let q = Aabb { min: [-100.0; 3], max: [100.0; 3] };
            sc.query_aabb_in(&q, &liveness_words[..n_words], &mut out[..len]);
            // ── Boundary phase ──
            sc.compact();
            m.tick(t0.elapsed());
        }
    }
}

// ── Workload 9: GPU Component Stress ───────────────────────────────────────
//
// Hammers the #[derive(SceneStore)] macro output — column creation, Pod
// round-trip, and (when `gpu` feature is on) GpuColumnSet::gpu_columns()
// descriptor verification.  This is a key failure point: the init-bit desync
// in swap, the stride-budget check, and the GpuColumnSet path all live here.

struct GpuComponentStress;
impl Workload for GpuComponentStress {
    fn run(&self, state: &AppState, idx: usize) {
        let m = &state.metrics[idx];
        state.log(Color::Cyan, "GPU component stress started — SceneStore derive + #[gpu] fields");
        let mut cell = GpuTestComponent::create_cell(256).unwrap();
        let mut handles = Vec::with_capacity(256);
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();

            #[cfg(feature = "gpu")]
            {
                let descs = GpuTestComponent::gpu_columns();
                assert_eq!(descs.len(), 3);
                assert_eq!(descs[0].buffer_name, "x");
                assert_eq!(descs[0].mode, MirrorMode::DirtyTracked);
                assert_eq!(descs[1].buffer_name, "y");
                assert_eq!(descs[1].mode, MirrorMode::DirtyTracked);
                assert_eq!(descs[2].buffer_name, "color");
                assert_eq!(descs[2].mode, MirrorMode::Once);
            }

            // Allocate batch.
            let start = handles.len();
            for _ in 0..32 {
                if let Some(h) = cell.alloc() {
                    let row = cell.row_of(h).unwrap() as usize;
                    let x = m.ops.load(Ordering::Relaxed) as f32;
                    let y = (m.ops.load(Ordering::Relaxed) as f64) * 0.5;
                    // Write via Pod column access — tests the SceneColumnSet layout.
                    if let Some(col) = cell.column_for_mut::<f32>() {
                        col[row] = x;
                    }
                    if let Some(col) = cell.column_for_mut::<f64>() {
                        col[row] = y;
                    }
                    if let Some(col) = cell.column_for_mut::<u16>() {
                        col[row] = 42;
                    }
                    if let Some(col) = cell.column_for_mut::<u32>() {
                        col[row] = 0xFF00_00FF;
                    }
                    handles.push(h);
                } else {
                    break;
                }
            }

            // Read-back verification.
            for &h in &handles[start..] {
                let row = cell.row_of(h).unwrap() as usize;
                if let Some(col) = cell.column_for::<f32>() {
                    let _ = col[row];
                }
            }

            // Free a portion and compact under pressure.
            let to_free = handles.len().min(16);
            for h in handles.drain(..to_free) {
                cell.free(h);
            }
            if handles.len() < 64 && handles.capacity() > 128 {
                cell.compact();
            }

            m.tick(t0.elapsed());
        }
    }
}

// ── TUI Rendering ─────────────────────────────────────────────────────────

fn render(frame: &mut ratatui::Frame, state: &AppState, ui: &UiState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, chunks[0], state, ui);
    if let Some(idx) = ui.selected {
        render_workload_detail(frame, chunks[1], state, idx);
    } else {
        render_body(frame, chunks[1], state, ui);
    }
    render_footer(frame, chunks[2], state);
}

fn render_header(frame: &mut ratatui::Frame, area: Rect, state: &AppState, ui: &UiState) {
    let elapsed = state.start.elapsed();
    let mut title = format!(
        " Pulsar SceneDB AAA Stress Test  [{:02}:{:02}:{:02}] ",
        elapsed.as_secs() / 3600,
        (elapsed.as_secs() / 60) % 60,
        elapsed.as_secs() % 60,
    );
    if state.paused.load(Ordering::Relaxed) {
        title.push_str("  ** PAUSED ** ");
    }
    if let Some(idx) = ui.selected {
        title.push_str(&format!("  [{} detail] ", state.metrics[idx].name));
    }
    let focus_hint = match ui.focus {
        Focus::Workloads => " [Workloads]",
        Focus::Log => " [Log]",
    };
    title.push_str(focus_hint);
    let block = Block::default()
        .title(title)
        .title_alignment(ratatui::layout::Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, area);
}

fn render_body(frame: &mut ratatui::Frame, area: Rect, state: &AppState, ui: &UiState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    render_workloads(frame, chunks[0], state, ui);
    render_log(frame, chunks[1], state, ui.log_scroll);
}

fn render_workload_detail(frame: &mut ratatui::Frame, area: Rect, state: &AppState, idx: usize) {
    let m = &state.metrics[idx];
    let ops = m.ops.load(Ordering::Relaxed);
    let errs = m.errors.load(Ordering::Relaxed);
    let lat_ns = m.latency_ns.load(Ordering::Relaxed);
    let running = m.running.load(Ordering::Relaxed);
    let lat_us = lat_ns / 1000;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Length(5), Constraint::Min(0)])
        .split(area);

    // Sparkline
    let spark_data: Vec<u64> = m.spark_data.lock().ok().map(|sd| sd.clone()).unwrap_or_default();
    let spark = Sparkline::default()
        .block(Block::default().title(" Latency (µs) ").borders(Borders::ALL).border_type(BorderType::Rounded))
        .data(&spark_data)
        .style(Style::default().fg(Color::Magenta));
    frame.render_widget(spark, chunks[0]);

    // Stats line
    let status = if running { "▶ Running" } else { "⏹ Stopped" };
    let status_color = if running { Color::Green } else { Color::DarkGray };
    let stats = format!(
        "  {}  |  Ops: {}  Latency: {}µs  Errors: {}  |  {}",
        status, ops, lat_us, errs, m.desc,
    );
    let s = if errs > 0 { Style::default().fg(Color::Red) } else { Style::default().fg(status_color) };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(stats, s))).block(
            Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)),
        chunks[1],
    );

    // Log panel below detail, with scroll.
    render_log(frame, chunks[2], state, 0);
}

fn render_workloads(frame: &mut ratatui::Frame, area: Rect, state: &AppState, ui: &UiState) {
    let focused = ui.focus == Focus::Workloads;
    let border_color = if focused { Color::Cyan } else { Color::White };
    let block = Block::default()
        .title(format!(" Workloads {} ", if focused { "◀" } else { "" }))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(5); NUM_WORKLOADS])
        .split(inner);

    for (i, chunk) in chunks.iter().enumerate() {
        let is_highlighted = i == ui.highlight;
        render_workload(frame, *chunk, &state.metrics[i], is_highlighted, focused);
    }
}

fn render_workload(frame: &mut ratatui::Frame, area: Rect, m: &WorkloadMetrics, highlighted: bool, focused: bool) {
    let ops = m.ops.load(Ordering::Relaxed);
    let errs = m.errors.load(Ordering::Relaxed);
    let lat_ns = m.latency_ns.load(Ordering::Relaxed);
    let running = m.running.load(Ordering::Relaxed);

    let bg = if highlighted && focused {
        Style::default().bg(Color::DarkGray)
    } else if highlighted {
        Style::default().bg(Color::Rgb(40, 40, 40))
    } else {
        Style::default()
    };

    let status_color = if running { Color::Green } else { Color::DarkGray };

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Length(12),
            Constraint::Min(0),
        ])
        .split(area);

    let name = format!(
        " {} {}",
        if running { "▶" } else { "⏹" },
        m.name
    );
    let name_style = bg.fg(status_color).add_modifier(Modifier::BOLD);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(name, name_style))), cols[0]);

    let ops_text = format!("{}", ops);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(ops_text, bg.fg(Color::Yellow)))),
        cols[1],
    );

    let lat_us = lat_ns / 1000;
    let detail = format!("  {:>6} ops  {:>5}µs  err:{}", ops, lat_us, errs);
    let detail_color = if errs > 0 { Color::Red } else { Color::White };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(detail, bg.fg(detail_color)))),
        cols[2],
    );
}

fn render_log(frame: &mut ratatui::Frame, area: Rect, state: &AppState, scroll: usize) {
    let block = Block::default()
        .title(format!(
            " Event Log [{} entries] ",
            state.log.lock().map(|l| l.len()).unwrap_or(0),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(area);

    let (entries, total) = if let Ok(l) = state.log.lock() {
        let visible = inner.height.max(1) as usize;
        let total = l.len();
        // scroll=0 means bottom (latest); positive means offset from bottom.
        let end = if scroll >= total { total } else { total - scroll };
        let start = end.saturating_sub(visible);
        let items = l[start..end].iter().rev().map(|e| {
            ListItem::new(Line::from(Span::styled(e.msg.clone(), Style::default().fg(e.color))))
        }).collect();
        (items, total)
    } else {
        (vec![ListItem::new("")], 0)
    };

    frame.render_widget(List::new(entries).block(block), area);

    if total > 0 {
        let visible = inner.height.max(1) as usize;
        let end = if scroll >= total { total } else { total - scroll };
        let position = end.saturating_sub(visible);
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(total).position(position);
        frame.render_stateful_widget(scrollbar, inner, &mut scrollbar_state);
    }
}

fn render_footer(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let total_ops: u64 = state.metrics.iter().map(|m| m.ops.load(Ordering::Relaxed)).sum();
    let total_errs: u64 = state.metrics.iter().map(|m| m.errors.load(Ordering::Relaxed)).sum();
    let elapsed = state.start.elapsed().as_secs_f64();
    let overall_ops_s = if elapsed > 0.0 {
        (total_ops as f64 / elapsed) as u64
    } else {
        0
    };

    let text = format!(
        "  [Q]uit  [P]ause  [R]eset  |  Total ops: {}  Errors: {}  Overall: {} ops/s",
        total_ops, total_errs, overall_ops_s,
    );
    let style = if total_errs > 0 {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double);
    frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))).block(block), area);
}

// ── Main ───────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

    let state = Arc::new(AppState {
        metrics: [
            WorkloadMetrics::new("EntityStorm", "World spawn/despawn"),
            WorkloadMetrics::new("CellAllocStorm", "CellStorage alloc/free/compact"),
            WorkloadMetrics::new("SpatialQueryStorm", "AABB + frustum queries"),
            WorkloadMetrics::new("HandlePressure", "HandleRegistry gen cycling"),
            WorkloadMetrics::new("LeasePressure", "LeaseMask acquire/release"),
            WorkloadMetrics::new("GenericColStress", "GenericColumn swap desync"),
            WorkloadMetrics::new("ConcurrentRW", "Multi-threaded LivenessMask"),
            WorkloadMetrics::new("MixedFrame", "Full game-loop sim"),
            WorkloadMetrics::new("GpuComponentStress", "SceneStore #[gpu] fields + column ops"),
        ],
        log: Mutex::new(Vec::with_capacity(64)),
        paused: AtomicBool::new(false),
        start: Instant::now(),
    });

    state.log(Color::Green, "SceneDB stress test initialized — 9 workers ready");

    let workers: Vec<(Box<dyn Workload>, &str)> = vec![
        (Box::new(EntityStorm), "EntityStorm"),
        (Box::new(CellAllocStorm), "CellAllocStorm"),
        (Box::new(SpatialQueryStorm), "SpatialQueryStorm"),
        (Box::new(HandlePressure), "HandlePressure"),
        (Box::new(LeasePressure), "LeasePressure"),
        (Box::new(GenericColumnStress), "GenericColStress"),
        (Box::new(ConcurrentRW), "ConcurrentRW"),
        (Box::new(MixedFrame), "MixedFrame"),
        (Box::new(GpuComponentStress), "GpuComponentStress"),
    ];

    let handles: Vec<_> = workers
        .into_iter()
        .enumerate()
        .map(|(i, (w, name))| {
            let s = Arc::clone(&state);
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || w.run(&s, i))
                .unwrap()
        })
        .collect();

    // Dedicated input thread — never touches the draw path.
    let (tx, rx) = mpsc::channel::<Event>();
    std::thread::Builder::new()
        .name("tui-input".to_string())
        .spawn(move || {
            loop { match event::read() { Ok(ev) => { let _ = tx.send(ev); } Err(_) => break } }
        })
        .unwrap();

    // ── Draw loop ──────────────────────────────────────────────────────────
    let mut ui = UiState {
        focus: Focus::Workloads,
        selected: None,
        highlight: 0,
        log_scroll: 0,
    };
    let mut should_quit = false;
    while !should_quit {
        // Drain queued events.
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match key.code {
                        KeyCode::Char('q') => should_quit = ui.selected.is_none(),
                        KeyCode::Esc => {
                            if ui.selected.is_some() { ui.selected = None; }
                            else { should_quit = true; }
                        }
                        KeyCode::Tab => {
                            ui.focus = match ui.focus {
                                Focus::Workloads => Focus::Log,
                                Focus::Log => Focus::Workloads,
                            };
                        }
                        KeyCode::Up => match ui.focus {
                            Focus::Workloads => {
                                if ui.selected.is_none() {
                                    ui.highlight = ui.highlight.saturating_sub(1);
                                }
                            }
                            Focus::Log => {
                                let total = state.log.lock().map(|l| l.len()).unwrap_or(0);
                                if ui.log_scroll < total { ui.log_scroll += 1; }
                            }
                        },
                        KeyCode::Down => match ui.focus {
                            Focus::Workloads => {
                                if ui.selected.is_none() {
                                    ui.highlight = (ui.highlight + 1).min(NUM_WORKLOADS - 1);
                                }
                            }
                            Focus::Log => {
                                ui.log_scroll = ui.log_scroll.saturating_sub(1);
                            }
                        },
                        KeyCode::Enter => {
                            if ui.focus == Focus::Workloads && ui.selected.is_none() {
                                ui.selected = Some(ui.highlight);
                            }
                        }
                        KeyCode::Char('d') => {
                            if ui.focus == Focus::Workloads && ui.selected.is_none() {
                                ui.selected = Some(ui.highlight);
                            }
                        }
                        KeyCode::Home => {
                            if ui.focus == Focus::Log {
                                let total = state.log.lock().map(|l| l.len()).unwrap_or(0);
                                ui.log_scroll = total;
                            }
                        }
                        KeyCode::End => { ui.log_scroll = 0; }
                        KeyCode::PageUp => {
                            if ui.focus == Focus::Log {
                                ui.log_scroll = ui.log_scroll.saturating_add(10);
                            }
                        }
                        KeyCode::PageDown => {
                            if ui.focus == Focus::Log {
                                ui.log_scroll = ui.log_scroll.saturating_sub(10);
                            }
                        }
                        KeyCode::Char('p') => {
                            let p = state.paused.fetch_xor(true, Ordering::Relaxed);
                            state.log(Color::Yellow, if p { "Resumed" } else { "Paused — workloads frozen" });
                        }
                        KeyCode::Char('r') => {
                            for m in &state.metrics { m.ops.store(0, Ordering::Relaxed); m.errors.store(0, Ordering::Relaxed); }
                            state.log(Color::Yellow, "Counters reset");
                        }
                        _ => {
                            // 1-8 quick-jump to workload detail.
                            if let KeyCode::Char(c) = key.code {
                                if let Some(n) = c.to_digit(10) {
                                    let idx = n as usize - 1;
                                    if idx < NUM_WORKLOADS {
                                        ui.selected = Some(idx);
                                        ui.focus = Focus::Workloads;
                                    }
                                }
                            }
                        }
                    }
                }
                Event::Resize(..) => { let _ = terminal.clear(); }
                _ => {}
            }
        }
        if should_quit { break; }

        terminal.draw(|f| render(f, &state, &ui))?;

        // Status log every ~3 seconds (every 90 draws at 30 FPS).
        // Use a static counter to avoid Atomic overhead in the hot loop.
        {
            // Draw counter stored as a local — incremented each frame.
            // We use a small AtomicU64 for the periodic log to keep it simple.
            static DRAW_COUNTER: AtomicU64 = AtomicU64::new(0);
            let c = DRAW_COUNTER.fetch_add(1, Ordering::Relaxed);
            if c % 90 == 0 {
                let total_ops: u64 = state.metrics.iter().map(|m| m.ops.load(Ordering::Relaxed)).sum();
                let total_errs: u64 = state.metrics.iter().map(|m| m.errors.load(Ordering::Relaxed)).sum();
                state.log(
                    Color::DarkGray,
                    format!("Status: {} total ops, {} errors, {}s elapsed",
                        total_ops, total_errs, state.start.elapsed().as_secs()),
                );
            }
        }

        std::thread::sleep(Duration::from_millis(33));
    }

    // Clean shutdown.
    for m in &state.metrics { m.running.store(false, Ordering::Relaxed); }
    for h in handles { h.join().ok(); }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
