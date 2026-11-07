//! `cargo run --bin web` — builds every WASM demo and serves them locally.
//!
//! Two modes:
//!   * **Interactive** (default, when stdout is a TTY): a fullscreen TUI shows
//!     per-demo build progress; press Enter when done to serve on
//!     http://127.0.0.1:8000. Arrow keys navigate, Enter views live logs, Q quits.
//!   * **Headless** (`--headless`, or when `CI` is set / stdout is not a TTY):
//!     builds every demo serially, streams a plain log, writes the site to
//!     `target/wasm-prebuilt`, and exits non-zero if any demo failed. This is
//!     what CI runs — there is no shell build script.
//!
//! The builder is the single source of truth for the published site: it writes
//! each demo's landing page and the master index itself, after each wasm-pack
//! build, so the HTML can never be clobbered or go missing.

use std::io::{BufRead, BufReader, IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

// ── Demo catalogue ──────────────────────────────────────────────────────────
// The one source of truth for which demos exist and how their landing pages
// read. Adding a demo means adding a feature in helio-web-demos and an entry
// here — nothing else.

struct Demo {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    controls: &'static str,
}

const DEMOS: &[Demo] = &[
    Demo { name: "render_v2_basic",     title: "Basic Render",       description: "Three lit cubes — the minimal Helio render pipeline.",                          controls: "WASD / Space / Shift — fly &nbsp;·&nbsp; Mouse — look &nbsp;·&nbsp; Click — grab cursor" },
    Demo { name: "render_v2_sky",       title: "Volumetric Sky",     description: "Real-time atmospheric scattering with a moving sun.",                           controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Q/E — rotate sun &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "debug_shapes",        title: "Infinite Grid Debug Scene", description: "Procedural infinite editor grid with depth-occluded debug props and continuous LOD.", controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "indoor_room",         title: "Indoor Room",        description: "Furnished room with a flickering point light.",                                controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "indoor_corridor",     title: "Indoor Corridor",    description: "40 m corridor with overhead fluorescent lighting.",                            controls: "WASD/Space/Shift — walk &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "outdoor_night",       title: "Outdoor Night",      description: "City block under streetlamps at night.",                                       controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "outdoor_canyon",      title: "Outdoor Canyon",     description: "Desert canyon with a campfire and a dynamic sky.",                              controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Q/E — sun &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "indoor_cathedral",    title: "Indoor Cathedral",   description: "Gothic nave lit by candles and flickering torch sconces.",                     controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "indoor_server_room",  title: "Indoor Server Room", description: "Datacenter server racks with animated LED indicators.",                        controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "outdoor_city",        title: "Outdoor City",       description: "Procedural night city with street lamps and rooftop beacons.",                 controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "outdoor_volcano",     title: "Outdoor Volcano",    description: "Active lava field with pulsing vent glow.",                                    controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "space_station",       title: "Space Station",      description: "Orbiting station with solar arrays and navigation lights.",                    controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "light_benchmark",     title: "Light Benchmark",    description: "128 animated point lights — deferred lighting stress test.",                   controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "hlfs_benchmark",      title: "HLFS Compute Lighting", description: "Hierarchical light-field compute injection and propagation.",               controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; +/- — light intensity &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "sdf_demo",            title: "SDF Demo",           description: "Signed-distance field clipmap with live sphere edits.",                        controls: "WASD/Space/Shift — move &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "rc_benchmark",        title: "Radiance Cascades",  description: "Cornell box global illumination benchmark.",                                   controls: "+/- — intensity &nbsp;·&nbsp; WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "load_fbx",            title: "Load FBX",           description: "FBX asset loading (placeholder scene on WASM).",                               controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "load_fbx_embedded",   title: "Load FBX (Embedded)", description: "FBX loaded from bytes embedded at compile time.",                             controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "ship_flight",         title: "Ship Flight",        description: "6-DoF spaceship through an asteroid field.",                                   controls: "WASD — thrust &nbsp;·&nbsp; Q/E — roll &nbsp;·&nbsp; Space/Shift — lift &nbsp;·&nbsp; Mouse — aim" },
    Demo { name: "simple_graph",        title: "Simple Graph",       description: "Minimal fly-camera around a lit unit cube.",                                   controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "outdoor_rocks",       title: "Outdoor Rocks",      description: "Scattered rocks with an embedded FBX ship and dynamic sun.",                   controls: "WASD/Space/Shift — fly &nbsp;·&nbsp; Q/E — sun &nbsp;·&nbsp; Mouse — look" },
    Demo { name: "editor_demo",         title: "Editor Demo",        description: "BVH ray-picking and transform gizmo (translate / rotate / scale).",            controls: "RMB hold — fly &nbsp;·&nbsp; LMB — pick/drag &nbsp;·&nbsp; G/R/S — gizmo mode &nbsp;·&nbsp; Ctrl+D — duplicate &nbsp;·&nbsp; Del — delete &nbsp;·&nbsp; Tab — grid" },
    Demo { name: "vhs_backrooms",       title: "VHS Backrooms",      description: "Procedural backrooms maze with an injected VHS camcorder post-process shader.", controls: "WASD/Space/Shift — move &nbsp;·&nbsp; R — regenerate &nbsp;·&nbsp; Mouse — look" },
];

// ── HTML templates ──────────────────────────────────────────────────────────

/// Full landing page for a single demo, written into that demo's output dir.
fn demo_page_html(d: &Demo) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} — Helio</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    html, body {{ width: 100%; height: 100%; overflow: hidden; background: #000; font-family: system-ui, sans-serif; }}
    #loading {{ position: fixed; inset: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; background: #000; color: #aaa; font-size: 14px; gap: 16px; transition: opacity 0.4s; z-index: 10; }}
    #loading.hidden {{ opacity: 0; pointer-events: none; }}
    .spinner {{ width: 32px; height: 32px; border: 3px solid #333; border-top-color: #888; border-radius: 50%; animation: spin 0.8s linear infinite; }}
    @keyframes spin {{ to {{ transform: rotate(360deg); }} }}
    #controls {{ position: fixed; bottom: 10px; left: 50%; transform: translateX(-50%); color: #555; font-size: 11px; pointer-events: none; transition: opacity 1s; white-space: nowrap; }}
    #controls.fade {{ opacity: 0; }}
    a.back {{ position: fixed; top: 10px; left: 12px; color: #444; font-size: 11px; text-decoration: none; transition: color 0.2s; }}
    a.back:hover {{ color: #aaa; }}
  </style>
</head>
<body>
  <div id="loading"><div class="spinner"></div><span>Loading {title}…</span></div>
  <div id="controls">{controls}</div>
  <a class="back" href="../index.html">← all demos</a>
  <script type="module">
    import init from './helio_web_demos.js';
    try {{
      await init();
    }} catch (e) {{
      document.getElementById('loading').innerHTML =
        '<span style="color:#c44">Failed to load WASM: ' + e + '</span>';
      throw e;
    }}
    const ctrl = document.getElementById('controls');
    setTimeout(() => ctrl.classList.add('fade'), 5000);
  </script>
</body>
</html>
"#,
        title = d.title,
        controls = d.controls,
    )
}

/// Master listing page linking to every demo.
fn index_page_html(demos: &[Demo]) -> String {
    let cards: String = demos
        .iter()
        .map(|d| {
            format!(
                r#"    <a class="card" href="{name}/">
      <div class="card-title">{title}</div>
      <div class="card-desc">{description}</div>
    </a>
"#,
                name = d.name,
                title = d.title,
                description = d.description,
            )
        })
        .collect();

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Helio — Web Demos</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; margin: 0; padding: 0; }}
    html {{ background: #0a0a0c; color: #ccc; font-family: system-ui, sans-serif; }}
    body {{ max-width: 960px; margin: 0 auto; padding: 48px 20px; }}
    h1 {{ font-size: 28px; font-weight: 700; color: #eee; margin-bottom: 6px; }}
    .subtitle {{ color: #555; font-size: 14px; margin-bottom: 40px; }}
    .grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 16px; }}
    .card {{ display: flex; flex-direction: column; gap: 6px; background: #111; border: 1px solid #1e1e22; border-radius: 8px; padding: 18px 20px; text-decoration: none; color: inherit; transition: border-color 0.15s, background 0.15s; }}
    .card:hover {{ background: #161618; border-color: #3a3a44; }}
    .card-title {{ font-size: 15px; font-weight: 600; color: #ddd; }}
    .card-desc  {{ font-size: 12px; color: #666; line-height: 1.5; }}
  </style>
</head>
<body>
  <h1>Helio Web Demos</h1>
  <p class="subtitle">Real-time rendering demos compiled to WebAssembly.</p>
  <div class="grid">
{cards}  </div>
</body>
</html>
"#,
        cards = cards,
    )
}

// ── Build status (TUI) ────────────────────────────────────────────────────────

#[derive(Clone)]
enum Status {
    Pending,
    Building,
    Success(u64), // size in KiB
    Failed,
}

struct BuildState {
    log: Arc<Mutex<Vec<String>>>,
}

struct App {
    builds: Vec<BuildState>,
    list_state: ListState,
    log_scroll: usize,
    show_log: bool,
    total: usize,
}

impl App {
    fn new() -> Self {
        let total = DEMOS.len();
        let builds = (0..total)
            .map(|_| BuildState {
                log: Arc::new(Mutex::new(Vec::new())),
            })
            .collect();
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            builds,
            list_state,
            log_scroll: 0,
            show_log: false,
            total,
        }
    }

    fn status_icon(frame: u64, s: &Status) -> &'static str {
        let spinners = &["◴", "◷", "◶", "◵", "◐", "◑", "◒", "◓"][..];
        match s {
            Status::Pending => "  ",
            Status::Building => spinners[(frame as usize) % spinners.len()],
            Status::Success(_) => "✓",
            Status::Failed => "✗",
        }
    }

    fn status_color(s: &Status) -> Color {
        match s {
            Status::Pending => Color::DarkGray,
            Status::Building => Color::Cyan,
            Status::Success(_) => Color::Green,
            Status::Failed => Color::Red,
        }
    }

    fn status_text(s: &Status) -> String {
        match s {
            Status::Pending | Status::Building => String::new(),
            Status::Success(kb) => format!("  {} KiB", kb),
            Status::Failed => "  FAILED".into(),
        }
    }
}

// ── C-compiler resolution ─────────────────────────────────────────────────────

/// Which C compiler to hand wasm-pack. meshopt and other C deps must be built
/// with a clang that can target wasm32; macOS system clang (and Linux gcc) can't.
enum WasmCc {
    /// The default `cc` already targets wasm32 — don't override CC.
    Default,
    /// Use this compiler (sets CC for the build).
    Override(String),
    /// No wasm-capable compiler found — the build cannot succeed.
    Missing,
}

/// Can `cc` compile a trivial file for wasm32-unknown-unknown?
fn cc_targets_wasm(cc: &str) -> bool {
    let dir = std::env::temp_dir();
    let src = dir.join("helio_wasm_cc_test.c");
    let obj = dir.join("helio_wasm_cc_test.o");
    if std::fs::write(&src, "int x;\n").is_err() {
        return false;
    }
    let ok = Command::new(cc)
        .arg("--target=wasm32-unknown-unknown")
        .arg("-c")
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&obj);
    ok
}

/// Resolve a wasm-capable C compiler: honor an explicit CC that works, else the
/// default `cc`, else a plain `clang` (Linux/CI), else Homebrew LLVM (macOS).
fn resolve_wasm_cc() -> WasmCc {
    if let Ok(cc) = std::env::var("CC") {
        let cc = cc.trim().to_string();
        if !cc.is_empty() && cc_targets_wasm(&cc) {
            return WasmCc::Override(cc);
        }
    }
    if cc_targets_wasm("cc") {
        return WasmCc::Default;
    }
    // LLVM `clang` targets wasm32 out of the box; present on GitHub's Linux runners.
    if cc_targets_wasm("clang") {
        return WasmCc::Override("clang".into());
    }
    // macOS: Homebrew LLVM clang.
    if let Ok(out) = Command::new("brew").args(["--prefix", "llvm"]).output() {
        if out.status.success() {
            let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let clang = format!("{prefix}/bin/clang");
            if Path::new(&clang).exists() && cc_targets_wasm(&clang) {
                return WasmCc::Override(clang);
            }
        }
    }
    WasmCc::Missing
}

// ── Build ─────────────────────────────────────────────────────────────────────

/// Build a single demo with wasm-pack straight into `out_dir`, streaming the
/// process output into `log`, then write the demo's landing page. Blocks until
/// the build finishes. Returns whether the demo built successfully. `cc`, when
/// set, overrides the C compiler for the build.
fn build_demo(
    demo: &Demo,
    out_dir: &Path,
    manifest_dir: &Path,
    cc: Option<&str>,
    log: &Arc<Mutex<Vec<String>>>,
    running: &Arc<AtomicBool>,
) -> bool {
    log.lock()
        .unwrap()
        .push(format!("[{}] Starting wasm-pack build...", demo.name));

    // Build into a per-demo out-dir so runs never share pkg/. `--out-dir` must
    // come before the trailing feature args, or wasm-pack forwards it to cargo
    // (which rejects it as an unknown flag).
    let _ = std::fs::remove_dir_all(out_dir);
    let mut cmd = Command::new("wasm-pack");
    cmd.arg("build")
        .arg("--out-dir")
        .arg(out_dir)
        .args([
            "--release",
            "--target",
            "web",
            "--no-default-features",
            "--features",
            demo.name,
        ])
        .current_dir(manifest_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Only override CC when we resolved a specific wasm-capable compiler; never
    // pass an empty CC, which would break meshopt's C build.
    if let Some(cc) = cc {
        cmd.env("CC", cc);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut log_guard = log.lock().unwrap();
            log_guard.push(format!("Failed to spawn wasm-pack: {e}"));
            log_guard.push("FAILED".into());
            return false;
        }
    };

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        log.lock().unwrap().push(line);
    }
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        log.lock().unwrap().push(line);
    }

    let success = child.wait().ok().map(|s| s.success()).unwrap_or(false);

    // wasm-pack built straight into out_dir; write the landing page ourselves so
    // it can never be clobbered or go missing.
    let built = success
        && out_dir.join("helio_web_demos_bg.wasm").exists()
        && std::fs::write(out_dir.join("index.html"), demo_page_html(demo)).is_ok();

    let mut log_guard = log.lock().unwrap();
    if built {
        let size_kb = out_dir
            .join("helio_web_demos_bg.wasm")
            .metadata()
            .ok()
            .map(|m| m.len() / 1024)
            .unwrap_or(0);
        log_guard.push(format!("OK ({size_kb} KiB)"));
    } else {
        log_guard.push("FAILED".into());
    }
    built
}

// ── Entry point ────────────────────────────────────────────────────────────────

/// Headless mode is used by CI and any non-interactive run.
fn headless_requested() -> bool {
    std::env::args()
        .skip(1)
        .any(|a| a == "--headless" || a == "--ci")
        || std::env::var_os("CI").is_some()
        || !std::io::stdout().is_terminal()
}

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out_base = manifest_dir.join("target/wasm-prebuilt");

    // Resolve a wasm-capable C compiler up front so a clear message can be
    // printed (and CI can fail fast) if none exists.
    let cc: Option<String> = match resolve_wasm_cc() {
        WasmCc::Default => None,
        WasmCc::Override(c) => {
            eprintln!("Using wasm-capable C compiler: {c}");
            Some(c)
        }
        WasmCc::Missing => {
            eprintln!("No C compiler can target wasm32-unknown-unknown.");
            eprintln!("meshopt and other C deps need a clang with the wasm32 target.");
            eprintln!("Install LLVM and retry (macOS):");
            eprintln!("  brew install llvm && export CC=\"$(brew --prefix llvm)/bin/clang\"");
            eprintln!("On Linux, install clang (e.g. `apt-get install clang`).");
            std::process::exit(1);
        }
    };

    // The master index is static; write it up front so it's always present.
    let _ = std::fs::create_dir_all(&out_base);
    let _ = std::fs::write(out_base.join("index.html"), index_page_html(DEMOS));

    if headless_requested() {
        std::process::exit(run_headless(&manifest_dir, &out_base, cc.as_deref()));
    }
    run_tui(manifest_dir, out_base, cc);
}

/// Build every demo serially, logging plainly. Returns a process exit code.
fn run_headless(manifest_dir: &Path, out_base: &Path, cc: Option<&str>) -> i32 {
    let running = Arc::new(AtomicBool::new(true));
    println!("Building {} WASM demos (release)…", DEMOS.len());
    let mut failed: Vec<&str> = Vec::new();

    for demo in DEMOS {
        let out_dir = out_base.join(demo.name);
        let log = Arc::new(Mutex::new(Vec::new()));
        print!("  {:<20} ", demo.name);
        let _ = std::io::stdout().flush();

        let ok = build_demo(demo, &out_dir, manifest_dir, cc, &log, &running);
        if ok {
            let size_kb = out_dir
                .join("helio_web_demos_bg.wasm")
                .metadata()
                .ok()
                .map(|m| m.len() / 1024)
                .unwrap_or(0);
            println!("OK ({size_kb} KiB)");
        } else {
            println!("FAILED");
            eprintln!("──── {} build log ────", demo.name);
            for line in log.lock().unwrap().iter() {
                eprintln!("{line}");
            }
            eprintln!("──── end {} log ────", demo.name);
            failed.push(demo.name);
        }
    }

    if failed.is_empty() {
        println!("\nAll {} demos built → {}", DEMOS.len(), out_base.display());
        0
    } else {
        eprintln!("\n{} demo(s) FAILED: {}", failed.len(), failed.join(", "));
        1
    }
}

/// Interactive TUI: build in the background, then optionally serve.
fn run_tui(manifest_dir: PathBuf, out_base: PathBuf, cc: Option<String>) {
    enable_raw_mode().unwrap();
    std::io::stdout().execute(EnterAlternateScreen).unwrap();
    let mut terminal =
        Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout())).unwrap();
    terminal.clear().unwrap();

    let mut app = App::new();
    let running = Arc::new(AtomicBool::new(true));

    // ── Build worker ─────────────────────────────────────────────────────────
    // Every demo is the same crate built with a different feature flag against
    // one shared cargo target dir, so they cannot build concurrently: parallel
    // runs serialize on cargo's target lock and still race on the shared output
    // wasm. A single worker walks the list one demo at a time while the TUI
    // stays responsive.
    let jobs: Vec<(&'static Demo, PathBuf, Arc<Mutex<Vec<String>>>)> = DEMOS
        .iter()
        .enumerate()
        .map(|(i, demo)| {
            (
                demo,
                out_base.join(demo.name),
                Arc::clone(&app.builds[i].log),
            )
        })
        .collect();
    {
        let manifest_dir = manifest_dir.clone();
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            for (demo, out_dir, log) in jobs {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                build_demo(demo, &out_dir, &manifest_dir, cc.as_deref(), &log, &running);
            }
        });
    }

    // ── TUI event loop ────────────────────────────────────────────────────
    let tick = std::time::Duration::from_millis(100);
    let all_done = Arc::new(AtomicBool::new(false));
    let serve_started = Arc::new(AtomicBool::new(false));

    'main: loop {
        let all_done_val = all_done.load(Ordering::Relaxed);

        terminal.draw(|f| ui(f, &mut app, all_done_val)).unwrap();

        if event::poll(tick).unwrap() {
            if let Event::Key(key) = event::read().unwrap() {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc if !app.show_log => break 'main,
                        KeyCode::Down => {
                            if app.show_log {
                                app.log_scroll = app.log_scroll.saturating_add(1);
                            } else {
                                let i = app.list_state.selected().unwrap_or(0);
                                if i + 1 < app.total {
                                    app.list_state.select(Some(i + 1));
                                }
                            }
                        }
                        KeyCode::Up => {
                            if app.show_log {
                                app.log_scroll = app.log_scroll.saturating_sub(1);
                            } else {
                                let i = app.list_state.selected().unwrap_or(0);
                                if i > 0 {
                                    app.list_state.select(Some(i - 1));
                                }
                            }
                        }
                        KeyCode::Enter if !all_done_val || app.show_log => {
                            app.show_log = !app.show_log;
                            app.log_scroll = 0;
                        }
                        KeyCode::Enter => {
                            // All done, Enter starts the server.
                            serve_started.store(true, Ordering::Relaxed);
                            break 'main;
                        }
                        KeyCode::Backspace | KeyCode::Esc if app.show_log => {
                            app.show_log = false;
                        }
                        _ => {}
                    }
                }
            }
        }

        if !all_done_val {
            let done = app
                .builds
                .iter()
                .filter(|b| {
                    let last = b.log.lock().unwrap().last().cloned().unwrap_or_default();
                    last.starts_with("OK") || last == "FAILED"
                })
                .count();
            if done == app.total {
                all_done.store(true, Ordering::Relaxed);
            }
        }
    }

    running.store(false, Ordering::Relaxed);
    disable_raw_mode().unwrap();
    std::io::stdout().execute(LeaveAlternateScreen).unwrap();

    if serve_started.load(Ordering::Relaxed) || all_done.load(Ordering::Relaxed) {
        println!("\nServing at http://127.0.0.1:8000/");
        serve(&out_base, "127.0.0.1:8000");
    }
}

// ── UI rendering ───────────────────────────────────────────────────────────

fn ui(f: &mut Frame, app: &App, all_done: bool) {
    if app.show_log {
        log_ui(f, app);
    } else {
        list_ui(f, app, all_done);
    }
}

fn list_ui(f: &mut Frame, app: &App, all_done: bool) {
    let total = app.total;
    let done = app
        .builds
        .iter()
        .filter(|b| {
            let last = b.log.lock().unwrap().last().cloned().unwrap_or_default();
            last.starts_with("OK") || last == "FAILED"
        })
        .count();
    let ok = app
        .builds
        .iter()
        .filter(|b| {
            b.log
                .lock()
                .unwrap()
                .last()
                .cloned()
                .unwrap_or_default()
                .starts_with("OK")
        })
        .count();
    let fail = done.saturating_sub(ok);
    let pct = if total > 0 {
        done as f64 / total as f64
    } else {
        0.0
    };
    let frame = app
        .builds
        .iter()
        .map(|b| b.log.lock().unwrap().len() as u64)
        .sum::<u64>();

    let bar_width = f.area().width.saturating_sub(4) as usize;
    let filled = (pct * bar_width as f64).round() as usize;
    let empty = bar_width.saturating_sub(filled);
    let bar = format!("{}│{}", "█".repeat(filled), "░".repeat(empty));

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(f.area());

    // Header bar
    let mut header_spans = vec![
        Span::styled(" Helio ", Style::new().fg(Color::Magenta).bold()),
        Span::raw("│ "),
        Span::raw(format!("{total} demos  ")),
        Span::styled(format!("✔ {ok}"), Style::new().fg(Color::Green).bold()),
        Span::raw("  "),
        Span::styled(format!("✘ {fail}"), Style::new().fg(Color::Red).bold()),
        Span::raw(format!("  {done}/{total}  ")),
        Span::styled(bar, Style::default().fg(Color::Cyan)),
    ];
    if all_done {
        header_spans.push(Span::raw("  │  "));
        header_spans.push(Span::styled(
            "http://127.0.0.1:8000/",
            Style::new().fg(Color::Green).bold(),
        ));
    }
    let header = Paragraph::new(Line::from(header_spans)).block(Block::default());
    f.render_widget(header, layout[0]);

    // Demo list
    let items: Vec<ListItem> = app
        .builds
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let last = b.log.lock().unwrap().last().cloned().unwrap_or_default();
            let status = if last.starts_with("OK") {
                Status::Success(
                    last.trim_start_matches("OK (")
                        .trim_end_matches(" KiB)")
                        .parse()
                        .unwrap_or(0),
                )
            } else if last == "FAILED" {
                Status::Failed
            } else if !b.log.lock().unwrap().is_empty() {
                Status::Building
            } else {
                Status::Pending
            };
            let icon = App::status_icon(frame, &status);
            let color = App::status_color(&status);
            let text = App::status_text(&status);
            let name = DEMOS[i].name;
            let is_selected = app.list_state.selected() == Some(i);
            let bg = if is_selected {
                Color::DarkGray
            } else {
                Color::Reset
            };
            let fg = if is_selected {
                Color::White
            } else {
                Color::Reset
            };
            let icon_style = Style::default()
                .fg(color)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            let name_style = Style::default().fg(fg).bg(bg);
            let text_style = Style::default().fg(color).bg(bg);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", icon), icon_style),
                Span::styled(name, name_style),
                Span::styled(text, text_style),
            ]))
            .style(Style::default().bg(bg))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Demos "))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, layout[1], &mut app.list_state.clone());
}

fn log_ui(f: &mut Frame, app: &App) {
    let idx = app.list_state.selected().unwrap_or(0);
    let name = DEMOS[idx].name;
    let log = app.builds[idx].log.lock().unwrap();
    let lines: Vec<Line> = log.iter().map(|l| Line::from(Span::raw(l))).collect();

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(f.area());

    let title = format!(" Build Log: {name}  ");
    let log_widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .scroll((app.log_scroll as u16, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(log_widget, layout[1]);

    let footer = Paragraph::new(Line::from(vec![
        Span::raw(" ↑↓ Scroll  "),
        Span::styled("Enter/BS", Style::new().bold()),
        Span::raw(" Close  "),
        Span::styled("Q", Style::new().bold()),
        Span::raw(" Quit"),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, layout[0]);
}

// ── HTTP server ─────────────────────────────────────────────────────────────

fn serve(root: &Path, addr: &str) {
    let server = tiny_http::Server::http(addr).unwrap();
    let root = root.to_path_buf();
    println!("\nServing at http://{addr}/");
    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let path = {
            let stripped = url.trim_start_matches('/');
            let candidate = root.join(stripped);
            if candidate.is_dir() {
                candidate.join("index.html")
            } else {
                candidate
            }
        };

        let (status, contents) = match std::fs::read(&path) {
            Ok(data) => (tiny_http::StatusCode(200), data),
            Err(_) => (tiny_http::StatusCode(404), b"404 Not Found\n".to_vec()),
        };

        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let cts = mime_for(ext);
        let response = tiny_http::Response::from_data(contents)
            .with_status_code(status)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], cts.as_bytes()).unwrap(),
            );
        let _ = request.respond(response);
    }
}

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "application/javascript",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}
