//! Loading and switching levels, the same for every host (windowed, headless,
//! Play-in-Editor).
//!
//! A level is asked for with [`TickLoop::request_level`] (scripts: the
//! `game::load_level` native). Each tick first advances the request:
//!
//! 1. **Prepare**, on a worker, while the level being played runs on: read,
//!    parse, migrate and validate the file, scan the classes, resolve the
//!    roles and the render pipeline. A failure here leaves the level being
//!    played exactly as it is and is reported.
//! 2. **Commit**, at the tick boundary: the old level ends completely
//!    ([`TickLoop::end_level`]): possession, the game mode, its roles and
//!    scripts, its components' `end_play`, its objects (their GPU rows and
//!    sublevels with them) and what plugins keep for a level
//!    (`LevelEndRegistration`: physics and its gravity, sounds). The game
//!    instance, its components, timers and variables, global scripts and
//!    native script actors go on. The new level's render pipeline is selected.
//! 3. **Hydrate**, [`HYDRATION_BUDGET`] a tick. Nothing of gameplay runs
//!    until the level is whole: no system, component or script ticks on a
//!    partial level. The renderer keeps drawing. Then the level's roles and
//!    world settings become the session's, and the next script phase starts
//!    its scripts and roles and tells the game instance (`on_level_loaded`).
//!
//! A blocking loop ([`TickLoop::set_blocking_level_loads`], the headless
//! runner and tests) does all of it inside one tick, so a run is the same
//! whatever the machine.
//!
//! How others do it: Unreal's `OpenLevel` ("hard travel") destroys the
//! `UWorld` and keeps the `UGameInstance`, loading behind a loading screen;
//! Unity's `SceneManager.LoadSceneAsync` loads on a worker and activates in
//! one frame, with `DontDestroyOnLoad` objects surviving.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use engine_backend::scene::{
    EditorCamera, HydrationProgress, LevelHydration, PreparedLevelFile, RuntimeLevel, SceneWorldExt,
};
use engine_backend::subsystems::render::helio_renderer::pipelines;
use pulsar_content::ContentRoot;
use pulsar_scenedb::{Entity, World};

use super::TickLoop;
use crate::game_control::LevelEvent;

/// What one tick spends hydrating a level, so the frame keeps drawing and
/// answering input while it arrives.
pub const HYDRATION_BUDGET: Duration = Duration::from_millis(4);

/// What the prepare worker hands back.
struct Prepared {
    prepared: PreparedLevelFile,
    roles: pulsar_traits::WorldRoles,
    pipeline: Option<String>,
}

#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    Preparing {
        level: String,
        result: Receiver<Result<Prepared, String>>,
    },
    Hydrating {
        level: String,
        hydration: Box<LevelHydration>,
        roles: pulsar_traits::WorldRoles,
        editor_camera: Option<EditorCamera>,
    },
}

/// The level requests of one tick loop; see the module doc.
#[derive(Default)]
pub(crate) struct LevelFlow {
    /// The latest level asked for and not started yet.
    requested: Option<String>,
    stage: Stage,
    /// The level in the world, content-relative.
    current: Option<String>,
    /// Whether a level has been put into this world: only then is there an
    /// old level to end.
    active: bool,
    /// Levels read from another file than their own (Play-in-Editor: the
    /// open level is read from the snapshot of the unsaved editor world).
    sources: Vec<(String, PathBuf)>,
    blocking: bool,
    last_error: Option<String>,
}

impl LevelFlow {
    /// Whether a level is in the world.
    pub(super) fn has_level(&self) -> bool {
        self.active
    }
}

/// Whether two content-relative level paths name the same level.
fn same_level(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// `level` as a content-relative path: forward slashes, and a path inside the
/// content root made relative to it.
pub(crate) fn level_name(root: &Path, level: &str) -> Result<String, String> {
    let slashed = level.trim().replace('\\', "/");
    if slashed.is_empty() {
        return Err("no level was named".into());
    }
    let root = root.to_string_lossy().replace('\\', "/");
    let root = root.trim_end_matches('/');
    let inside = (!root.is_empty()
        && slashed.len() > root.len()
        && slashed.is_char_boundary(root.len())
        && same_level(&slashed[..root.len()], root)
        && slashed.as_bytes()[root.len()] == b'/')
        .then(|| slashed[root.len() + 1..].to_owned());
    let relative = inside.unwrap_or(slashed);
    pulsar_content::normalize_rel(&relative)
        .ok_or_else(|| format!("`{level}` is not a path inside the project"))
}

/// The slow, file-reading half of a switch. Runs on a worker.
fn prepare(content: &ContentRoot, name: &str, source: Option<PathBuf>) -> Result<Prepared, String> {
    profiling::profile_scope!("level-prepare");
    let root = content.root().to_path_buf();
    let path = match source {
        Some(path) => path,
        None => {
            if !content.exists(name) {
                return Err("there is no level file of that name".into());
            }
            content.path(name)
        }
    };
    // The project's file index is what components resolve an asset id through
    // while they hydrate on the tick thread. Its first open scans the project;
    // opened here, the tick finds it ready.
    if let Some(project) = engine_state::get_project_path() {
        profiling::profile_scope!("level-prepare: project file index");
        let _ = engine_fs::open_project_file_index(Path::new(&project));
    }
    let registry = pulsar_class::ClassRegistry::scan(&root);
    let prepared = RuntimeLevel::prepare(&path, &registry).map_err(|e| e.to_string())?;
    prepared.validate().map_err(|e| e.to_string())?;
    let json = engine_fs::virtual_fs::read_to_string(&path).map_err(|e| format!("{e:#}"))?;
    let roles = pulsar_traits::WorldRoles::for_level(&json, &root);
    let pipeline = crate::standalone::level_pipeline_for(&root, Some(&json));
    Ok(Prepared {
        prepared,
        roles,
        pipeline,
    })
}

/// Every scene object of `world`, by root tree, except the trees of `keep`.
fn level_trees(world: &World, keep: &HashSet<Entity>) -> (Vec<Entity>, HashSet<Entity>) {
    let roots: Vec<Entity> = world
        .children_of(None)
        .into_iter()
        .filter(|root| !keep.contains(root))
        // A native script actor's owner is the game code's, not the level's.
        .filter(|root| world.get::<crate::scripts::ScriptTag>(*root).is_none())
        .collect();
    let mut objects = HashSet::new();
    let mut next: Vec<Entity> = roots.clone();
    while let Some(object) = next.pop() {
        if keep.contains(&object) || !objects.insert(object) {
            continue;
        }
        next.extend(world.children_of(Some(object)));
    }
    (roots, objects)
}

impl TickLoop {
    /// Ask for the level `level` (content-relative, or a path inside the
    /// content). It replaces the level being played; asking again before it
    /// started replaces the earlier request. See the `tick::levels` module doc.
    pub fn request_level(&mut self, level: impl Into<String>) {
        let level = level.into();
        tracing::info!(%level, "Level requested");
        self.levels.requested = Some(level);
    }

    /// The level asked for and not started yet, content-relative when it is a
    /// path of the content.
    pub fn requested_level(&self) -> Option<String> {
        let requested = self.levels.requested.as_deref()?;
        let name = self
            .content_for_levels()
            .and_then(|content| level_name(content.root(), requested).ok());
        Some(name.unwrap_or_else(|| requested.to_owned()))
    }

    /// The level in the world, content-relative; `None` before the first one.
    pub fn current_level(&self) -> Option<&str> {
        self.levels.current.as_deref()
    }

    /// Whether a level request is waiting or being loaded.
    pub fn level_load_pending(&self) -> bool {
        self.levels.requested.is_some() || !matches!(self.levels.stage, Stage::Idle)
    }

    /// Load levels inside the tick that finds the request, however long it
    /// takes (a headless run, a test), instead of on a worker across ticks.
    pub fn set_blocking_level_loads(&mut self, blocking: bool) {
        self.levels.blocking = blocking;
    }

    /// Read the level `level` from `file` whenever it is loaded (Play-in-Editor:
    /// the open level is the snapshot of the editor's world, unsaved edits
    /// included).
    pub fn set_level_source(&mut self, level: impl Into<String>, file: impl Into<PathBuf>) {
        let level = level.into();
        self.levels
            .sources
            .retain(|(name, _)| !same_level(name, &level));
        self.levels.sources.push((level, file.into()));
    }

    /// The host put `level` into the world itself (Play-in-Editor adopts the
    /// editor's world): its roles are set already ([`Self::load_world_roles`]),
    /// and a later switch ends it like any level.
    pub fn adopt_open_level(&mut self, level: &str) {
        self.levels.current = Some(level.to_owned());
        self.levels.active = true;
        if let Some(driver) = &self.scripts {
            driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .begin_level(level.to_owned());
        }
    }

    /// Load every requested level now, blocking: for a host that has nothing
    /// to draw yet (the headless runner, a test). Returns why the last one
    /// failed.
    pub fn process_level_requests(&mut self) -> Result<(), String> {
        let blocking = std::mem::replace(&mut self.levels.blocking, true);
        self.levels.last_error = None;
        while self.level_load_pending() {
            self.advance_level_flow();
        }
        self.levels.blocking = blocking;
        match self.levels.last_error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Advance the level requests at a tick boundary. `true` while a level is
    /// partly in the world: the tick must run nothing of gameplay.
    pub(super) fn advance_level_flow(&mut self) -> bool {
        loop {
            match std::mem::take(&mut self.levels.stage) {
                Stage::Idle => {
                    let Some(requested) = self.levels.requested.take() else {
                        return false;
                    };
                    self.start_preparing(requested);
                    if matches!(self.levels.stage, Stage::Idle) {
                        // It failed at once; a newer request may be waiting.
                        continue;
                    }
                }
                Stage::Preparing { level, result } => {
                    // A newer request replaces this one: its result is dropped.
                    if self.levels.requested.is_some() {
                        continue;
                    }
                    let received = if self.levels.blocking {
                        result.recv().map_err(|_| TryRecvError::Disconnected)
                    } else {
                        result.try_recv()
                    };
                    let received = match received {
                        Ok(received) => received,
                        Err(TryRecvError::Empty) => {
                            self.levels.stage = Stage::Preparing { level, result };
                            return false;
                        }
                        Err(TryRecvError::Disconnected) => {
                            Err("the loader thread ended without a result".into())
                        }
                    };
                    match received {
                        Ok(prepared) => self.commit_level(level, prepared),
                        Err(error) => self.level_failed(level, error),
                    }
                }
                Stage::Hydrating {
                    level,
                    mut hydration,
                    roles,
                    editor_camera,
                } => {
                    let budget = if self.levels.blocking {
                        Duration::MAX
                    } else {
                        HYDRATION_BUDGET
                    };
                    let stepped = {
                        let mut store = self.scene_store.write();
                        hydration.step(&mut store.world, budget)
                    };
                    match stepped {
                        Ok(HydrationProgress::More) => {
                            self.levels.stage = Stage::Hydrating {
                                level,
                                hydration,
                                roles,
                                editor_camera,
                            };
                            if !self.levels.blocking {
                                return true;
                            }
                        }
                        Ok(HydrationProgress::Done(extras)) => {
                            self.finish_level(level, roles, extras.editor_camera);
                            if !self.levels.blocking {
                                return false;
                            }
                        }
                        Err(error) => {
                            // The old level is gone already (what validation
                            // cannot see failed): the game runs with what
                            // loaded.
                            self.level_failed(level.clone(), error.to_string());
                            self.finish_level(level, roles, editor_camera);
                            if !self.levels.blocking {
                                return false;
                            }
                        }
                    }
                }
            }
        }
    }

    fn content_for_levels(&self) -> Option<ContentRoot> {
        pulsar_content::current().or_else(|| {
            let driver = self.scripts.as_ref()?;
            let root = driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .project_root()
                .to_path_buf();
            Some(ContentRoot::project(root))
        })
    }

    fn start_preparing(&mut self, requested: String) {
        let Some(content) = self.content_for_levels() else {
            return self.level_failed(requested, "no game content is installed".into());
        };
        let name = match level_name(content.root(), &requested) {
            Ok(name) => name,
            Err(error) => return self.level_failed(requested, error),
        };
        let source = self
            .levels
            .sources
            .iter()
            .find(|(level, _)| same_level(level, &name))
            .map(|(_, file)| file.clone());
        let (send, result) = std::sync::mpsc::channel();
        if self.levels.blocking {
            let _ = send.send(prepare(&content, &name, source));
        } else {
            let worker_name = name.clone();
            let spawned = editor_task_queue::spawn_thread(
                editor_task_queue::ThreadSpec::new(
                    "level-prepare",
                    editor_task_queue::ThreadRole::Io,
                )
                .owner("pulsar_game"),
                move || {
                    let _ = send.send(prepare(&content, &worker_name, source));
                },
            );
            if let Err(error) = spawned {
                return self.level_failed(name, format!("no thread to load it: {error}"));
            }
        }
        self.levels.stage = Stage::Preparing {
            level: name,
            result,
        };
    }

    fn commit_level(&mut self, level: String, prepared: Prepared) {
        profiling::profile_scope!("TickLoop::commit_level");
        let Prepared {
            prepared,
            roles,
            pipeline,
        } = prepared;
        let editor_camera = prepared.editor_camera();
        let hydration = match prepared.begin_hydration() {
            Ok(hydration) => hydration,
            Err(error) => return self.level_failed(level, error.to_string()),
        };
        if self.levels.active {
            self.end_level();
        }
        if let Some(driver) = &self.scripts {
            driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hold_level_roles();
        }
        tracing::info!(pipeline = ?pipeline, "Level render pipeline selected");
        pipelines::set_level_pipeline(pipeline);
        self.control.publish_level(LevelEvent::Begun {
            level: level.clone(),
        });
        self.levels.stage = Stage::Hydrating {
            level,
            hydration: Box::new(hydration),
            roles,
            editor_camera,
        };
    }

    fn finish_level(
        &mut self,
        level: String,
        roles: pulsar_traits::WorldRoles,
        editor_camera: Option<EditorCamera>,
    ) {
        {
            let mut store = self.scene_store.write();
            if self.levels.blocking {
                // A blocking load has the whole level, sublevels and assets,
                // before its first tick.
                engine_backend::scene::sublevel_display::sync_level_instances(&mut store.world);
                engine_backend::scene::resolve_level_assets(&mut store.world);
            } else {
                // Displays of other levels are requested from workers.
                engine_backend::scene::sublevel_display::sync_level_instances_deferred(
                    &mut store.world,
                );
            }
        }
        // The watch for moved static objects starts over: the load placed them.
        self.static_moves = None;
        for message in roles.diagnostics() {
            tracing::warn!("{message}");
        }
        self.set_world_roles(roles);
        self.levels.current = Some(level.clone());
        self.levels.active = true;
        if let Some(driver) = &self.scripts {
            driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .begin_level(level.clone());
        }
        tracing::info!(level = %level, "Level loaded");
        self.control.publish_level(LevelEvent::Loaded {
            level,
            editor_camera,
        });
    }

    fn level_failed(&mut self, level: String, error: String) {
        let message = format!("Load Level '{level}' failed: {error}");
        tracing::error!("{message}");
        if self.collect_problems {
            self.script_problems.push(pulsar_events::ScriptProblem {
                severity: pulsar_events::ProblemSeverity::Error,
                message: message.clone(),
                ..Default::default()
            });
        }
        // With no level in the world, the game draws its fallback pipeline.
        if !self.levels.active {
            pipelines::set_level_pipeline(crate::standalone::no_level_pipeline());
        }
        self.levels.last_error = Some(message);
        self.control
            .publish_level(LevelEvent::Failed { level, error });
    }

    /// End the level in the world completely, and nothing else: possession,
    /// the game mode's `on_game_end`, the level's scripts (`end_play`) and
    /// roles, its components' `end_play` (in the world, the components still
    /// in it), its objects, and what plugins keep for a level. The game
    /// instance and its components, global scripts, timers they own and
    /// native script actors go on.
    pub(super) fn end_level(&mut self) {
        profiling::profile_scope!("TickLoop::end_level");
        let mut store = self.scene_store.write();
        let world = &mut store.world;
        let mut keep = HashSet::new();
        if let Some(driver) = &self.scripts {
            let mut driver = driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let report = driver.end_level(world);
            self.script_stats.absorb(&report);
            if self.collect_problems && report.has_problems() {
                self.script_problems.extend(driver.problems(&report));
            }
            keep = driver.game_instance_objects(world);
        }
        let (roots, objects) = level_trees(world, &keep);
        let ended = self
            .component_runtime
            .end_owned(world, &self.events, &|owner| objects.contains(&owner));
        // What the ends queued leaves the world's outboxes before the owners go.
        pulsar_world_registry::flush_component_events(world, &self.events);
        for root in roots {
            if world.is_alive(root) {
                world.despawn_tree(root);
            }
        }
        pulsar_world_registry::process_component_removals_mut(
            world,
            &self.events,
            &mut self.component_runtime,
        );
        // The old level's settings go before plugins rebuild what they keep.
        self.component_runtime.set_settings(serde_json::Value::Null);
        for hook in engine_backend::scene::game_hooks::level_end_hooks() {
            (hook.level_ended)(world, self.component_runtime.scope_mut());
        }
        drop(store);
        self.world_roles = pulsar_traits::WorldRoles::default();
        self.static_moves = None;
        tracing::info!(
            level = self.levels.current.as_deref().unwrap_or(""),
            objects = objects.len(),
            components = ended,
            "Level ended"
        );
        self.levels.current = None;
        self.levels.active = false;
    }

    /// Drop a level request that is waiting or being loaded (the session is
    /// ending).
    pub(super) fn cancel_level_requests(&mut self) {
        self.levels.requested = None;
        self.levels.stage = Stage::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_names_are_relative_to_the_content() {
        let root = Path::new("C:/Games/My Game");
        for given in [
            "scene/a.level",
            "./scene/a.level",
            "scene\\a.level",
            "C:\\Games\\My Game\\scene\\a.level",
        ] {
            assert_eq!(
                level_name(root, given).as_deref(),
                Ok("scene/a.level"),
                "{given}"
            );
        }
        assert!(level_name(root, "").is_err());
        assert!(level_name(root, "../other/a.level").is_err());
        assert!(level_name(root, "D:/elsewhere/a.level").is_err());
    }
}
