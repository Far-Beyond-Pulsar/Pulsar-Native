//! The standalone game entry point: what a generated project's `main.rs`
//! calls, for `cargo run` and for packaged games alike.
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
//! The project's game instance class (`game_instance` in
//! `Pulsar/project.json`) starts first, before any level
//! ([`start_game_instance`]). Then the startup level loads: `startup_level`
//! from `Pulsar/project.json`, or, when the key is absent, the editor's
//! `project.default_map` setting (dev builds) and the usual default level
//! files. An empty `startup_level` is no level: the game runs with none until
//! a script asks for one (`game::load_level`). A level the game instance asks
//! for in `on_init` replaces the startup level.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use pulsar_content::ContentRoot;
/// The generated `main` installs the project's content key, which opens its
/// packed content, before [`run`] finds it.
pub use pulsar_content::{install_content_key, ContentKey};
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
    /// Publish the process for the editor's profiler
    /// (`--pulsar-profile`, or the `PULSAR_PROFILE` environment variable).
    pub profile: bool,
}

/// Headless frames when `--frames` is not given.
pub const DEFAULT_HEADLESS_FRAMES: u64 = 60;

/// Prefix of the line a headless run prints its report on (stdout), for
/// tools and CI to find.
pub const HEADLESS_REPORT_PREFIX: &str = "PULSAR_HEADLESS_REPORT ";

pub const USAGE: &str =
    "usage: <game> [--headless] [--frames N] [--content DIR] [--pulsar-profile]\n\
    \n  --headless       run the game loop without a window or renderer\
    \n  --frames N       with --headless: run N ticks, then exit (default 60)\
    \n  --content DIR    read content from DIR instead of <exe dir>/Content or the project\
    \n  --pulsar-profile let the editor's profiler record this process";

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
                Some((flag, value)) if flag.starts_with("--") => {
                    (flag.to_owned(), Some(value.to_owned()))
                }
                _ => (arg.clone(), None),
            };
            let mut value = |name: &str| {
                inline
                    .clone()
                    .or_else(|| args.next())
                    .ok_or_else(|| format!("{name} needs a value\n{USAGE}"))
            };
            match flag.as_str() {
                "--headless" => options.headless = true,
                "--frames" => {
                    let raw = value("--frames")?;
                    options.frames = Some(
                        raw.parse()
                            .map_err(|_| format!("--frames: `{raw}` is not a number\n{USAGE}"))?,
                    );
                }
                "--content" => options.content = Some(PathBuf::from(value("--content")?)),
                profiling::remote::ARG_FLAG => options.profile = true,
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
        Some(dir) => {
            ContentRoot::open(dir).map_err(|e| format!("--content {}: {e}", dir.display()))?
        }
        None => ContentRoot::discover().map_err(|e| e.to_string())?,
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
/// live only in the pak; load it through `engine_fs::virtual_fs`). `None`
/// when the project says none (an empty `startup_level`) or has none.
pub fn startup_level(content: &ContentRoot) -> Option<PathBuf> {
    let settings = content.settings();
    match settings.startup_choice() {
        pulsar_content::StartupChoice::None => return None,
        pulsar_content::StartupChoice::Level(rel) => {
            if content.exists(rel) {
                return Some(content.path(rel));
            }
            tracing::warn!(
                level = rel,
                "Startup level from Pulsar/project.json not found; trying the default level"
            );
        }
        pulsar_content::StartupChoice::Default => {}
    }
    // Dev builds: the editor's own setting (`.pulsar/project/*.toml`).
    let configured = (!content.is_packaged())
        .then(|| engine_state::settings::ProjectSettings::new(content.root()))
        .flatten()
        .and_then(|ps| {
            ps.load_all();
            ps.get("project", "default_map")?
                .as_str()
                .ok()
                .map(str::to_owned)
        });
    pulsar_content::default_level(configured.as_deref(), |rel| content.exists(rel))
        .map(|rel| content.path(&rel))
}

/// Start the game instance `content`'s settings name, if any: before any
/// level (`TickLoop::start_game_instance`). Every host calls it after the
/// project's setup enabled scripting.
pub fn start_game_instance(game: &mut TickLoop, content: &ContentRoot) {
    if let Some(class) = content.settings().game_instance_class() {
        game.start_game_instance(class);
    }
}

/// [`start_game_instance`], then ask for the startup level unless the game
/// instance already asked for one: how a standalone game starts. Returns the
/// level that will load first, content-relative.
pub fn start_game(game: &mut TickLoop, content: &ContentRoot) -> Option<String> {
    start_game_instance(game, content);
    if let Some(requested) = game.requested_level() {
        return Some(requested);
    }
    let level = startup_level(content).and_then(|path| content.relative(&path));
    match &level {
        Some(level) => {
            tracing::info!(level = %level, "Startup level");
            game.request_level(level.clone());
        }
        None => tracing::info!("No startup level: the game starts with no level"),
    }
    level
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
            // CLI output: bad arguments are reported to the person at the terminal.
            // style: allow println - standalone runner output is its interface
            eprintln!("{message}");
            return 2;
        }
    };
    match run_with(options, setup) {
        Ok(Some(report)) => {
            // The headless report on stdout is the process interface (a parent parses it).
            // style: allow println - standalone runner output is its interface
            println!("{HEADLESS_REPORT_PREFIX}{}", report.to_json());
            i32::from(!report.ok())
        }
        Ok(None) => 0,
        Err(message) => {
            tracing::error!("{message}");
            // CLI output: the person at the terminal reads this; the print is the feature.
            // style: allow println - standalone runner output is its interface
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
    #[cfg(feature = "alloc-profile")]
    crate::alloc_profile::start_reporter();
    // A shipped game doesn't edit shaders: keep Helio's file watcher (a
    // thread polling 20x a second, plus every shader's source held for
    // rebuilds) out of it unless the developer asked for it. Set before any
    // other thread exists, as the variable is read once.
    if std::env::var_os("HELIO_SHADER_HOT_RELOAD").is_none() {
        std::env::set_var("HELIO_SHADER_HOT_RELOAD", "0");
    }
    let content = install_content(&options)?;
    if options.profile || profiling::remote::requested() {
        let published = profiling::remote::publish_process(
            profiling::remote::TargetDescription {
                kind: "game".into(),
                name: game_name(),
                project: content.root().display().to_string(),
            },
            editor_task_queue::service_spawner("profiling"),
        );
        if published {
            tracing::info!("Profiling enabled: the editor's profiler can record this process");
        }
    }
    // Several copies launched together (multiplayer testing) split the cores
    // between them instead of each sizing its pool to the whole machine.
    let copies = std::env::var("PULSAR_INSTANCE_COUNT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(1);
    let threads = (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        / copies)
        .max(1);
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
        fullscreen: settings.window.fullscreen,
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
    /// The first level loaded, content-relative (`None`: no level).
    pub level: Option<String>,
    /// The level in the world at the end, content-relative.
    pub final_level: Option<String>,
    /// Every level that finished loading, in order.
    pub levels_loaded: Vec<String>,
    /// The game asked to quit (the run ends there).
    pub quit: bool,
    pub frames: u64,
    pub scripts: ScriptStats,
    /// Every script instance alive after the last frame.
    pub instances: Vec<InstanceReport>,
    /// Script errors, link errors and dropped calls.
    pub problems: Vec<pulsar_events::ScriptProblem>,
    /// What the plugins report about the world after the last frame, by
    /// key (`ContentReportRegistration`: the meshes' `meshes`, how many
    /// mesh components and how many bound).
    pub world: BTreeMap<String, serde_json::Value>,
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
pub fn run_headless(
    game: &mut TickLoop,
    content: &ContentRoot,
    frames: u64,
) -> Result<HeadlessReport, String> {
    run_headless_with(game, content, frames, HeadlessHooks::default())
}

/// Hooks of a headless run, for tests and tools that watch it: the same run,
/// observed.
#[derive(Default)]
pub struct HeadlessHooks<'a> {
    /// Once the level is loaded and no tick has run yet (subscribe to the
    /// level's entities here).
    pub after_load: Option<Box<dyn FnOnce(&mut TickLoop) + 'a>>,
    /// After every tick, with its index.
    pub after_tick: Option<Box<dyn FnMut(&mut TickLoop, u64) + 'a>>,
}

/// [`run_headless`] with [`HeadlessHooks`].
pub fn run_headless_with(
    game: &mut TickLoop,
    content: &ContentRoot,
    frames: u64,
    mut hooks: HeadlessHooks<'_>,
) -> Result<HeadlessReport, String> {
    prepare_engine(content)?;
    // No window, no renderer: a headless session, whatever built the loop.
    game.set_session_kind(pulsar_world_registry::SessionKind::Headless);
    // Levels load whole inside the tick that asks for them, so a run does the
    // same whatever the machine.
    game.set_blocking_level_loads(true);
    let control = game.control();
    let settings = content.settings();
    let mut levels_loaded = Vec::new();
    let mut take_loaded = |control: &crate::game_control::GameControl| {
        for event in control.take_level_events() {
            if let crate::game_control::LevelEvent::Loaded { level, .. } = event {
                levels_loaded.push(level);
            }
        }
    };
    game.collect_script_problems(true);
    let level = start_game(game, content);
    // The first level is in the world before the first tick.
    game.process_level_requests()?;
    take_loaded(&control);
    if let Some(after_load) = hooks.after_load.take() {
        after_load(game);
    }
    let mut ran = 0;
    for index in 0..frames {
        if control.quit_requested() {
            tracing::info!(frame = index, "The game quit");
            break;
        }
        game.tick_once();
        ran += 1;
        take_loaded(&control);
        if let Some(after_tick) = hooks.after_tick.as_mut() {
            after_tick(game, index);
        }
    }
    let mut report = HeadlessReport {
        content: content.root().display().to_string(),
        packaged: content.is_packaged(),
        pak: content.pak().is_some(),
        profile: settings.profile.as_str().to_owned(),
        level,
        final_level: game.current_level().map(str::to_owned),
        levels_loaded,
        quit: control.quit_requested(),
        frames: ran,
        scripts: game.script_stats().clone(),
        instances: Vec::new(),
        problems: game.take_script_problems(),
        world: engine_backend::scene::game_hooks::content_reports(&game.scene_store.read().world),
    };
    if let Some(driver) = &game.scripts {
        let driver = driver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            report.instances.push(InstanceReport {
                id: id.clone(),
                class,
                variables,
            });
        }
    }
    // Components end first, while the world is alive, then scripts.
    game.end_session();
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

/// The render pipeline a game runs for the level whose file text is `level`
/// (its World Settings), or with no level the plugins' fallback
/// (`pipelines::FallbackPipeline`, so a menu before any level still draws).
/// A level that names no pipeline runs none: the renderer says so on screen.
/// Offers the project's render graphs first (`content_root`, plugin folders
/// included) when one is named; building them learns Helio's pins once.
/// Reads the disk: the level flow calls it on its worker.
pub fn level_pipeline_for(content_root: &std::path::Path, level: Option<&str>) -> Option<String> {
    use engine_backend::subsystems::render::helio_renderer::pipelines;
    let requested = match level {
        Some(json) => pipelines::render_pipeline_of_level(json),
        None => pipelines::fallback_pipeline(),
    };
    if requested.is_some() {
        let report = pipelines::refresh_project_pipelines(content_root);
        for (graph, error) in &report.failed {
            tracing::warn!(graph = %graph, "Render graph not offered: {error}");
        }
    }
    requested
}

/// The pipeline of a game with no level: see [`level_pipeline_for`].
pub(crate) fn no_level_pipeline() -> Option<String> {
    engine_backend::subsystems::render::helio_renderer::pipelines::fallback_pipeline()
}

/// Choose the render pipeline for the level file at `level` (see
/// [`level_pipeline_for`]), for the renderers built after this: every host
/// calls it before it builds one, since a game's renderer takes its pipeline
/// when it is made (`SceneRendererOptions::game`). A level switch selects its
/// own (`tick::levels`).
pub fn select_level_pipeline(content_root: &std::path::Path, level: Option<&std::path::Path>) {
    use engine_backend::subsystems::render::helio_renderer::pipelines;
    let json = level.and_then(|path| {
        engine_fs::virtual_fs::read_to_string(path)
            .map_err(|error| {
                tracing::warn!(level = %path.display(), "Level not read for its render pipeline: {error}");
            })
            .ok()
    });
    let requested = level_pipeline_for(content_root, json.as_deref());
    tracing::info!(pipeline = ?requested, "Level render pipeline selected");
    pipelines::set_level_pipeline(requested);
}

/// [`select_level_pipeline`] for the level `game` is about to load, or for no
/// level.
pub(crate) fn select_requested_level_pipeline(game: &TickLoop, content: &ContentRoot) {
    let level = game
        .requested_level()
        .map(|level| content.path(&level))
        .filter(|path| engine_fs::virtual_fs::is_file(path));
    select_level_pipeline(content.root(), level.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_options_parse() {
        assert_eq!(
            LaunchOptions::from_args(Vec::<String>::new()).unwrap(),
            LaunchOptions::default()
        );
        let options =
            LaunchOptions::from_args(["--headless", "--frames", "5", "--content=/x"]).unwrap();
        assert!(options.headless);
        assert_eq!(options.frames, Some(5));
        assert_eq!(options.content.as_deref(), Some(std::path::Path::new("/x")));
        assert!(
            LaunchOptions::from_args(["--pulsar-profile"])
                .unwrap()
                .profile
        );
        assert!(!LaunchOptions::from_args(["--headless"]).unwrap().profile);
        assert!(LaunchOptions::from_args(["--frames"]).is_err());
        assert!(LaunchOptions::from_args(["--frames", "many"]).is_err());
        assert!(LaunchOptions::from_args(["--bogus"]).is_err());
    }
}
