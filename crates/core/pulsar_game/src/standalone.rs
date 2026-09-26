//! The standalone game entry point: what a generated project's `main.rs`
//! calls, for `cargo run` and for packaged games alike (#926).
//!
//! ```rust,ignore
//! fn main() {
//!     std::process::exit(pulsar_game::standalone::run(engine_main::setup));
//! }
//! ```
//!
//! [`run`] reads the command line ([`LaunchOptions`]), finds the game's
//! content ([`pulsar_content::ContentRoot::discover`]: `<exe dir>/Content`
//! for a packaged game, the project around the executable for a dev
//! build), installs it (a pak is mounted into `engine_fs::virtual_fs`),
//! runs the project's `setup()` and then either opens the game window or,
//! with `--headless --frames N`, runs `N` fixed-step ticks of the same
//! [`TickLoop`] with no window or renderer and prints a
//! [`HeadlessReport`]. Nothing about the project's location is compiled
//! into the game.
//!
//! The level loaded at startup is `startup_level` from
//! `Pulsar/project.json`; dev builds fall back to the editor's
//! `project.default_map` setting and the usual default level files.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use pulsar_content::ContentRoot;
use pulsar_core::TickMode;
use serde::Serialize;

use crate::tick::{ScriptStats, TickLoop};

/// Command-line options of a standalone game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Run without a window or renderer.
    pub headless: bool,
    /// With `headless`: how many ticks to run (default 60).
    pub frames: Option<u64>,
    /// Use this directory as the content (cooked content or a project)
    /// instead of discovering it.
    pub content: Option<PathBuf>,
}

/// Headless frames when `--frames` is not given.
pub const DEFAULT_HEADLESS_FRAMES: u64 = 60;

/// Prefix of the line a headless run prints its report on (stdout), for
/// tools and CI to find.
pub const HEADLESS_REPORT_PREFIX: &str = "PULSAR_HEADLESS_REPORT ";

pub const USAGE: &str = "usage: <game> [--headless] [--frames N] [--content DIR]\n\
    \n  --headless       run the game loop without a window or renderer\
    \n  --frames N       with --headless: run N ticks, then exit (default 60)\
    \n  --content DIR    read content from DIR instead of <exe dir>/Content or the project";

impl LaunchOptions {
    /// Parse arguments (without the program name).
    pub fn from_args<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut options = Self::default();
        let mut args = args.into_iter().map(Into::into);
        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((flag, value)) if flag.starts_with("--") => (flag.to_owned(), Some(value.to_owned())),
                _ => (arg.clone(), None),
            };
            let mut value = |name: &str| {
                inline.clone().or_else(|| args.next()).ok_or_else(|| format!("{name} needs a value\n{USAGE}"))
            };
            match flag.as_str() {
                "--headless" => options.headless = true,
                "--frames" => {
                    let raw = value("--frames")?;
                    options.frames =
                        Some(raw.parse().map_err(|_| format!("--frames: `{raw}` is not a number\n{USAGE}"))?);
                }
                "--content" => options.content = Some(PathBuf::from(value("--content")?)),
                "--help" | "-h" => return Err(USAGE.to_owned()),
                other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
            }
        }
        Ok(options)
    }

    /// Parse this process's arguments.
    pub fn from_env() -> Result<Self, String> {
        Self::from_args(std::env::args().skip(1))
    }
}

/// Find and install the game's content: `options.content` when given,
/// else [`ContentRoot::discover`].
pub fn install_content(options: &LaunchOptions) -> Result<ContentRoot, String> {
    let content = match &options.content {
        Some(dir) => ContentRoot::open(dir).map_err(|e| format!("--content {}: {e}", dir.display()))?,
        None => ContentRoot::discover()?,
    };
    content.install();
    Ok(content)
}

/// Engine globals a game session needs before any level loads: settings
/// schemas, the engine context, and the directory asset loaders resolve
/// `assets/...` paths against.
pub fn prepare_engine(content: &ContentRoot) -> Result<(), String> {
    pulsar_settings::register_all_settings(engine_state::settings::global_config());
    engine_state::EngineContext::new().set_global();
    let assets = content
        .asset_root()
        .map_err(|e| format!("cannot prepare assets of {}: {e}", content.root().display()))?;
    engine_state::set_project_path(assets.display().to_string());
    Ok(())
}

/// The level to load at startup, as a path under the content root (it may
/// live only in the pak; load it through `engine_fs::virtual_fs`).
pub fn startup_level(content: &ContentRoot) -> Option<PathBuf> {
    let settings = content.settings();
    if let Some(rel) = settings.startup_level.as_deref().filter(|l| !l.is_empty()) {
        if content.exists(rel) {
            return Some(content.path(rel));
        }
        tracing::warn!(level = rel, "Startup level from Pulsar/project.json not found");
    }
    if !content.is_packaged() {
        // Dev builds: the editor's own setting (`.pulsar/project/*.toml`).
        let configured = engine_state::settings::ProjectSettings::new(content.root()).and_then(|ps| {
            ps.load_all();
            ps.get("project", "default_map")?.as_str().ok().map(str::to_owned)
        });
        if let Some(rel) = configured.filter(|m| !m.is_empty()) {
            if content.exists(&rel) {
                return Some(content.path(&rel));
            }
            tracing::warn!(level = %rel, "project.default_map not found; trying the default level files");
        }
    }
    ["scene/default.level", "scenes/default.level", "scenes/default_level.json"]
        .into_iter()
        .find(|rel| content.exists(rel))
        .map(|rel| content.path(rel))
}

/// The game's `setup()`: registers actors and turns on scripting (the
/// generated `engine_main::setup`).
pub type Setup = fn(&mut TickLoop) -> Result<(), String>;

/// Run the game with this process's command line. Returns the exit code:
/// 0 on success, 1 when setup, content or (headless) scripts failed, 2 for
/// bad arguments.
pub fn run(setup: impl FnOnce(&mut TickLoop) -> Result<(), String>) -> i32 {
    let options = match LaunchOptions::from_env() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return 2;
        }
    };
    match run_with(options, setup) {
        Ok(Some(report)) => {
            println!("{HEADLESS_REPORT_PREFIX}{}", report.to_json());
            i32::from(!report.ok())
        }
        Ok(None) => 0,
        Err(message) => {
            tracing::error!("{message}");
            eprintln!("{message}");
            1
        }
    }
}

/// [`run`] with explicit options. Headless runs return their report;
/// windowed runs return `None` once the window closes.
pub fn run_with(
    options: LaunchOptions,
    setup: impl FnOnce(&mut TickLoop) -> Result<(), String>,
) -> Result<Option<HeadlessReport>, String> {
    let content = install_content(&options)?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    if options.headless {
        let mut game = TickLoop::new(TickMode::Fixed { dt: HEADLESS_STEP }, threads);
        setup(&mut game).map_err(|e| format!("Level setup failed: {e}"))?;
        let frames = options.frames.unwrap_or(DEFAULT_HEADLESS_FRAMES);
        return run_headless(&mut game, &content, frames).map(Some);
    }
    let mut game = TickLoop::new(TickMode::default(), threads);
    setup(&mut game).map_err(|e| format!("Level setup failed: {e}"))?;
    let settings = content.settings();
    let window = crate::window::WindowDescriptor {
        title: settings.window_title(&game_name()),
        width: settings.window.width,
        height: settings.window.height,
        editor_mode: false,
    };
    let event_loop = winit::event_loop::EventLoop::with_user_event()
        .build()
        .map_err(|e| format!("Failed to create the event loop: {e}"))?;
    game.run_with_content(event_loop, window, content);
    Ok(None)
}

/// The simulation step of a headless run.
pub const HEADLESS_STEP: Duration = Duration::from_micros(16_667);

fn game_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Pulsar Game".into())
}

/// One script instance at the end of a headless run.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct InstanceReport {
    pub id: String,
    pub class: String,
    /// Its scalar variables (bool, int, float, string), by name.
    pub variables: BTreeMap<String, serde_json::Value>,
}

/// What a headless run did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct HeadlessReport {
    /// The content root, and whether it is packaged / from a pak.
    pub content: String,
    pub packaged: bool,
    pub pak: bool,
    pub profile: String,
    /// The startup level, content-relative.
    pub level: Option<String>,
    pub frames: u64,
    pub scripts: ScriptStats,
    /// Every script instance alive after the last frame.
    pub instances: Vec<InstanceReport>,
    /// Script errors, link errors and dropped calls.
    pub problems: Vec<pulsar_events::ScriptProblem>,
}

impl HeadlessReport {
    /// No script or load errors.
    pub fn ok(&self) -> bool {
        self.problems.is_empty() && self.scripts.script_errors == 0 && self.scripts.load_errors == 0
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
    }

    /// The instance of class `class`, if exactly one exists.
    pub fn instance_of(&self, class: &str) -> Option<&InstanceReport> {
        let mut found = self.instances.iter().filter(|i| i.class == class);
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }
}

/// Load the startup level into `game`'s world and run `frames` ticks with
/// no window or renderer, then end every script (`end_play`). The same
/// [`TickLoop::tick_once`] the windowed game's tick thread runs.
pub fn run_headless(game: &mut TickLoop, content: &ContentRoot, frames: u64) -> Result<HeadlessReport, String> {
    prepare_engine(content)?;
    let settings = content.settings();
    let level = startup_level(content);
    if let Some(path) = &level {
        load_level(game, content, path)?;
    } else {
        tracing::warn!("No startup level: running an empty world");
    }
    game.collect_script_problems(true);
    for _ in 0..frames {
        game.tick_once();
    }
    let mut report = HeadlessReport {
        content: content.root().display().to_string(),
        packaged: content.is_packaged(),
        pak: content.pak().is_some(),
        profile: settings.profile.as_str().to_owned(),
        level: level.as_deref().and_then(|p| content.relative(p)),
        frames,
        scripts: game.script_stats().clone(),
        instances: Vec::new(),
        problems: game.take_script_problems(),
    };
    if let Some(driver) = &game.scripts {
        let driver = driver.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let runtime = driver.runtime();
        for id in runtime.instance_ids() {
            let class = runtime.class_of(id).unwrap_or_default().to_owned();
            let variables = runtime
                .class_variables(&class)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(name, _)| {
                    let value = match runtime.variable(id, &name)? {
                        pulsar_script_vm::Value::Bool(b) => serde_json::json!(b),
                        pulsar_script_vm::Value::Int(i) => serde_json::json!(i),
                        pulsar_script_vm::Value::Float(f) => serde_json::json!(f),
                        pulsar_script_vm::Value::Str(s) => serde_json::json!(s.to_string()),
                        _ => return None,
                    };
                    Some((name, value))
                })
                .collect();
            report.instances.push(InstanceReport { id: id.clone(), class, variables });
        }
    }
    game.end_scripts();
    report.problems.extend(game.take_script_problems());
    tracing::info!(
        frames,
        started = report.scripts.started,
        spawned = report.scripts.spawned,
        problems = report.problems.len(),
        "Headless run finished"
    );
    Ok(report)
}

/// Hydrate the level at `path` (under the content root) into `game`'s
/// world, resolving placed classes against the content's classes.
pub fn load_level(game: &mut TickLoop, content: &ContentRoot, path: &std::path::Path) -> Result<(), String> {
    let registry = pulsar_class::ClassRegistry::scan(content.root());
    let name = content.relative(path).unwrap_or_else(|| path.display().to_string());
    {
        let mut store = game.scene_store.write();
        engine_backend::scene::RuntimeLevel::load_into_with_classes(path, &mut store.world, &registry)
            .map_err(|e| format!("Failed to load level {name}: {e}"))?;
    }
    if let Some(driver) = &game.scripts {
        driver.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).set_level_name(name.clone());
    }
    tracing::info!(level = %name, "Level loaded");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_options_parse() {
        assert_eq!(LaunchOptions::from_args(Vec::<String>::new()).unwrap(), LaunchOptions::default());
        let options = LaunchOptions::from_args(["--headless", "--frames", "5", "--content=/x"]).unwrap();
        assert!(options.headless);
        assert_eq!(options.frames, Some(5));
        assert_eq!(options.content.as_deref(), Some(std::path::Path::new("/x")));
        assert!(LaunchOptions::from_args(["--frames"]).is_err());
        assert!(LaunchOptions::from_args(["--frames", "many"]).is_err());
        assert!(LaunchOptions::from_args(["--bogus"]).is_err());
    }
}
