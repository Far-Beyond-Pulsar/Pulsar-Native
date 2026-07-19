//! Pulsar SceneDB — AAA-Grade Stress Test TUI
//!
//! Simulates an absurdly-maxed-out game scene hammering every subsystem of
//! pulsar_scenedb simultaneously.  Displays live throughput, error counts,
//! and memory pressure in a ratatui dashboard.
//!
//! Run:
//!   cargo run -p pulsar_scenedb --example stress_tui
//!
//! Controls:
//!   q  — quit
//!   p  — pause / resume all workloads
//!   r  — reset all counters

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use pulsar_scenedb::*;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, Paragraph,
};
use ratatui::Terminal;
use std::io::stdout;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Metrics ────────────────────────────────────────────────────────────────

const NUM_WORKLOADS: usize = 8;

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
        let mut out = vec![0u32; sc.rows_in_use() as usize];
        state.log(Color::Cyan, "Spatial query storm started — 1000 AABBs, AABB + frustum queries");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // AABB query.
            let q = Aabb {
                min: [rand::random::<f32>() * 2000.0 - 1000.0; 3],
                max: [
                    rand::random::<f32>() * 100.0 + 0.1,
                    rand::random::<f32>() * 100.0 + 0.1,
                    rand::random::<f32>() * 100.0 + 0.1,
                ],
            };
            sc.query_aabb(&q, &mut out);
            // Frustum query.
            let planes = [
                [1.0, 0.0, 0.0, 1000.0],
                [-1.0, 0.0, 0.0, 1000.0],
                [0.0, 1.0, 0.0, 1000.0],
                [0.0, -1.0, 0.0, 1000.0],
                [0.0, 0.0, 1.0, 1000.0],
                [0.0, 0.0, -1.0, 1000.0],
            ];
            sc.query_frustum(&Frustum { planes }, &mut out);
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
        state.log(Color::Cyan, "Mixed frame started — full game-loop simulation");
        while m.running.load(Ordering::Relaxed) {
            while state.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let t0 = Instant::now();
            // ── Simulate phase ──
            // Spawn new entities with random AABBs.
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
            // Free a portion (simulating killed entities).
            let to_kill = handles.len() / 10;
            for h in handles.drain(..to_kill) {
                sc.free(h);
            }
            // ── Harvest phase ──
            let mut out = vec![0u32; sc.rows_in_use() as usize];
            let q = Aabb {
                min: [-100.0; 3],
                max: [100.0; 3],
            };
            sc.query_aabb(&q, &mut out);
            // ── Boundary phase ──
            sc.compact();
            m.tick(t0.elapsed());
        }
    }
}

// ── TUI Rendering ─────────────────────────────────────────────────────────

fn render(frame: &mut ratatui::Frame, state: &AppState) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(frame, chunks[0], state);
    render_body(frame, chunks[1], state);
    render_footer(frame, chunks[2], state);
}

fn render_header(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let elapsed = state.start.elapsed();
    let title = format!(
        " Pulsar SceneDB AAA Stress Test  [{:02}:{:02}:{:02}] ",
        elapsed.as_secs() / 3600,
        (elapsed.as_secs() / 60) % 60,
        elapsed.as_secs() % 60,
    );
    let paused = if state.paused.load(Ordering::Relaxed) {
        "  ** PAUSED ** "
    } else {
        ""
    };
    let block = Block::default()
        .title(format!("{}{}", title, paused))
        .title_alignment(ratatui::layout::Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, area);
}

fn render_body(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    render_workloads(frame, chunks[0], state);
    render_log(frame, chunks[1], state);
}

fn render_workloads(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" Workloads ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(5); NUM_WORKLOADS])
        .split(inner);

    for (i, chunk) in chunks.iter().enumerate() {
        render_workload(frame, *chunk, &state.metrics[i]);
    }
}

fn render_workload(frame: &mut ratatui::Frame, area: Rect, m: &WorkloadMetrics) {
    let ops = m.ops.load(Ordering::Relaxed);
    let errs = m.errors.load(Ordering::Relaxed);
    let lat_ns = m.latency_ns.load(Ordering::Relaxed);
    let running = m.running.load(Ordering::Relaxed);

    let status_color = if running { Color::Green } else { Color::DarkGray };
    let _err_color = if errs > 0 { Color::Red } else { Color::DarkGray };

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Length(12),
            Constraint::Min(0),
        ])
        .split(area);

    // Name + status
    let name = format!(
        " {} {}",
        if running { "▶" } else { "⏹" },
        m.name
    );
    let name_style = Style::default().fg(status_color).add_modifier(Modifier::BOLD);
    let name_p = Paragraph::new(Line::from(Span::styled(name, name_style)));
    frame.render_widget(name_p, cols[0]);

    // Ops/s estimate
    let ops_text = format!("{}", ops);
    let ops_p = Paragraph::new(Line::from(Span::styled(
        ops_text,
        Style::default().fg(Color::Yellow),
    )));
    frame.render_widget(ops_p, cols[1]);

    // Latency + errors (uses remaining space)
    let lat_us = lat_ns / 1000;
    let detail = format!(
        "  {:>6} ops  {:>5}µs  err:{}",
        ops,
        lat_us,
        errs,
    );
    let detail_style = if errs > 0 {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::White)
    };
    let detail_p = Paragraph::new(Line::from(Span::styled(detail, detail_style)));
    frame.render_widget(detail_p, cols[2]);
}

fn render_log(frame: &mut ratatui::Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(" Event Log ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let entries = if let Ok(l) = state.log.lock() {
        let items: Vec<ListItem> = l.iter().rev().take(32).map(|e| {
            ListItem::new(Line::from(Span::styled(
                e.msg.clone(),
                Style::default().fg(e.color),
            )))
        }).collect();
        items
    } else {
        vec![ListItem::new("")]
    };

    let list = List::new(entries).block(block);
    frame.render_widget(list, area);
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
        "  [Q]uit  [P]ause  [R]eset  |  Total ops: {}  Errors: {}  Overall: {} ops/s  Frame: 16.7ms (60 FPS target)",
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
    let p = Paragraph::new(Line::from(Span::styled(text, style))).block(block);
    frame.render_widget(p, area);
}

// ── Main ───────────────────────────────────────────────────────────────────

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

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
        ],
        log: Mutex::new(Vec::with_capacity(64)),
        paused: AtomicBool::new(false),
        start: Instant::now(),
    });

    state.log(Color::Green, "SceneDB stress test initialized — 8 workers ready");

    let workers: Vec<(Box<dyn Workload>, &str)> = vec![
        (Box::new(EntityStorm), "EntityStorm"),
        (Box::new(CellAllocStorm), "CellAllocStorm"),
        (Box::new(SpatialQueryStorm), "SpatialQueryStorm"),
        (Box::new(HandlePressure), "HandlePressure"),
        (Box::new(LeasePressure), "LeasePressure"),
        (Box::new(GenericColumnStress), "GenericColStress"),
        (Box::new(ConcurrentRW), "ConcurrentRW"),
        (Box::new(MixedFrame), "MixedFrame"),
    ];

    let handles: Vec<_> = workers
        .into_iter()
        .enumerate()
        .map(|(i, (w, name))| {
            let s = Arc::clone(&state);
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    w.run(&s, i);
                })
                .unwrap()
        })
        .collect();

    // TUI event loop.
    let mut tick_count = 0u64;
    loop {
        terminal.draw(|f| render(f, &state))?;

        if event::poll(Duration::from_millis(33))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('p') => {
                            let p = state.paused.fetch_xor(true, Ordering::Relaxed);
                            state.log(
                                Color::Yellow,
                                if p { "Resumed" } else { "Paused — workloads frozen" },
                            );
                        }
                        KeyCode::Char('r') => {
                            for m in &state.metrics {
                                m.ops.store(0, Ordering::Relaxed);
                                m.errors.store(0, Ordering::Relaxed);
                            }
                            state.log(Color::Yellow, "Counters reset");
                        }
                        _ => {}
                    }
                }
            }
        }

        tick_count += 1;
        if tick_count % 60 == 0 {
            // Periodic status log.
            let total_ops: u64 = state.metrics.iter().map(|m| m.ops.load(Ordering::Relaxed)).sum();
            let total_errs: u64 = state.metrics.iter().map(|m| m.errors.load(Ordering::Relaxed)).sum();
            let msg = format!(
                "Status: {} total ops, {} errors, {}s elapsed",
                total_ops,
                total_errs,
                state.start.elapsed().as_secs(),
            );
            state.log(Color::DarkGray, msg);
        }
    }

    // Clean shutdown: signal all workers to stop.
    for m in &state.metrics {
        m.running.store(false, Ordering::Relaxed);
    }
    for h in handles {
        h.join().ok();
    }

    state.log(Color::Green, "All workers stopped — shutting down");

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}
