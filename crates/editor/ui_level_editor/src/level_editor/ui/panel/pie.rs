//! Play-In-Editor (issue #243) build orchestration.
//!
//! `begin_pie`/`end_pie` are the shared entry points for both the `PlayScene`
//! action handler and the toolbar's "Start Simulation" button
//! (`toolbar::playback_controls`); the rest of this module is the dylib build
//! pipeline they drive (scaffolding regeneration, source-staleness fastpath,
//! crate-name probing). Re-exported at the `panel` level so existing
//! `crate::level_editor::ui::panel::begin_pie` paths keep compiling.

use gpui::{App, Window};
use rust_i18n::t;
use std::path::Path;
use std::sync::Arc;
use ui::{notification::Notification, ContextModal as _};

use crate::level_editor::state::{LevelEditorState, PieStartRequest};

// ── Play In Editor build helpers (issue #243) ───────────────────────────────

/// Enter play mode and kick off the Play-In-Editor build.
///
/// Free function (not a panel method) so BOTH the `PlayScene` action handler and
/// the toolbar "Start Simulation" button (`playback_controls`) can share one
/// code path — they previously diverged, and only the action handler had PiE.
pub(crate) fn begin_pie(
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    window: &mut Window,
    cx: &mut App,
) {
    {
        let mut st = shared_state.write();
        // Play pressed again before the viewport processed a Stop: the
        // game keeps running and nothing is restored.
        if st.play.pie.restore_after_stop {
            st.play.pie.stop_requested = false;
            st.play.pie.restore_after_stop = false;
            st.play.pie.stop_requested_at = None;
        }
        // Snapshot the scene (also flips to play mode). A Play while a game
        // runs keeps the pre-Play snapshot.
        st.scene.enter_play_mode();
    }

    let Some(root) = engine_state::get_project_path().map(std::path::PathBuf::from) else {
        window.push_notification(
            Notification::error(t!("Notification.Title.PlayInEditor").to_string())
                .message(t!("Notification.Message.NoProjectOpenForPlay").to_string()),
            cx,
        );
        return;
    };

    tracing::info!(project = %root.display(), "PiE: starting build for Play");

    // Reflect unsaved edits: write the live SceneDb to a temp level file.
    let scene_path = root.join("target").join("pie").join("play.level");
    let save_result = {
        let state = shared_state.read();
        let world = state.scene.world();
        crate::level_editor::scene_edit::level_io::save_to_file(&world, &scene_path)
    };
    if let Err(e) = save_result {
        window.push_notification(
            Notification::error(t!("Notification.Title.PlayInEditor").to_string()).message(
                t!(
                    "Notification.Message.FailedToWriteScene",
                    error => e.to_string()
                )
                .to_string(),
            ),
            cx,
        );
        return;
    }

    // Native hot reload (#653): pressing Play while a game runs rebuilds
    // and swaps the library WITHOUT dropping the world — the viewport stops
    // the old host only once the new build is in hand.
    let (reload, loaded_artifact) = {
        let mut st = shared_state.write();
        st.play.pie.building = true;
        st.play.pie.stop_requested = false;
        st.play.pie.last_error = None;
        st.play.pie.pending_start = None;
        (st.play.pie.active, st.play.pie.loaded_artifact.clone())
    };
    if !reload {
        // A new session: the problems panel forgets the last one's.
        shared_state.write().play.pie.problems.clear();
        pulsar_events::publish_script_problems_cleared();
    }

    if reload {
        tracing::info!("PiE: game already running — this Play is a NATIVE HOT RELOAD");
    }

    window.push_notification(
        Notification::info(t!("Notification.Title.PlayInEditor").to_string())
            .message(t!("Notification.Message.BuildingGame").to_string()),
        cx,
    );

    let shared = shared_state.clone();
    let _ = std::thread::Builder::new()
        .name("pie-build".into())
        .spawn(move || {
            let result = build_pie_dylib(&root, &scene_path, reload, loaded_artifact.as_ref());
            let mut st = shared.write();
            st.play.pie.building = false;
            match result {
                Ok(req) => st.play.pie.pending_start = Some(req),
                Err(e) => {
                    tracing::error!("PiE build failed: {e}");
                    st.play.pie.last_error = Some(e);
                }
            }
        });
}

/// Ask the viewport to tear down the embedded game, then exit play mode.
///
/// With a game running, the editor world is restored only after the game
/// shut down (#925): the viewport stops it on its next frame (scripts get
/// `end_play` while the world still holds their objects, and nothing
/// ticks after), then restores the pre-Play snapshot, which also removes
/// what scripts spawned. Without a running game it restores now.
pub(crate) fn end_pie(shared_state: Arc<parking_lot::RwLock<LevelEditorState>>) {
    let mut st = shared_state.write();
    st.play.pie.pending_start = None;
    st.play.pie.building = false;
    st.play.pie.pause_request = None;
    st.play.pie.step_request = 0;
    if st.play.pie.active {
        st.play.pie.stop_requested = true;
        st.play.pie.restore_after_stop = true;
        st.play.pie.stop_requested_at = Some(std::time::Instant::now());
    } else {
        st.play.pie.stop_requested = true;
        st.scene.exit_play_mode();
    }
}

/// How long a Stop waits for the viewport to shut the game down before the
/// editor world is restored anyway (the Game tab is not being drawn).
pub(crate) const STOP_RESTORE_FALLBACK: std::time::Duration = std::time::Duration::from_millis(750);

/// Finish a Stop: restore the editor world if the game is gone (called by
/// the viewport right after it stopped the game), or, as a fallback, once
/// [`STOP_RESTORE_FALLBACK`] passed without a viewport doing it. Returns
/// whether it restored.
pub(crate) fn finish_stop(state: &mut LevelEditorState, game_stopped: bool) -> bool {
    if !state.play.pie.restore_after_stop {
        return false;
    }
    let overdue = state
        .play
        .pie
        .stop_requested_at
        .is_some_and(|at| at.elapsed() >= STOP_RESTORE_FALLBACK);
    if !game_stopped && !overdue {
        return false;
    }
    if !game_stopped {
        tracing::warn!("PiE: no viewport stopped the game in time; restoring the editor world anyway");
    }
    state.play.pie.restore_after_stop = false;
    state.play.pie.stop_requested_at = None;
    state.play.pie.paused = false;
    state.scene.exit_play_mode();
    true
}

/// Regenerate the project scaffolding and build it as a `cdylib`, returning what
/// the viewport needs to load the embedded game. Runs on a background thread.
///
/// `reload` marks a native hot reload (#653): an earlier session is still
/// running and its world state must survive the swap. The flag only rides the
/// request — a failed rebuild leaves the old game untouched either way.
///
/// Fastpath: if a release library already exists and no `.rs`/`.toml` under the
/// project is newer than it, skip regeneration + `cargo build` entirely and reuse
/// the last-built artifact — pressing Play with no source changes is instant.
fn build_pie_dylib(
    root: &Path,
    scene_path: &Path,
    reload: bool,
    loaded_artifact: Option<&(std::path::PathBuf, std::time::SystemTime)>,
) -> Result<PieStartRequest, String> {
    // PiE uses the release library (faster at runtime, and matches the artifact
    // `cargo build --release` / `cargo run --release` produce).
    let release = true;

    // Script preflight (#656): every scripting language plugin validates its
    // saved classes. Bad scripts stop Play here instead of surfacing as
    // runtime failures inside the embedded game.
    if let Some(plugins) = plugin_manager::global() {
        if let Err(summary) = plugins.read().validate_scripts(root) {
            tracing::error!("PiE blocked by script validation:\n{summary}");
            return Err(summary);
        }
    }

    // Fastpath — reuse the existing artifact when nothing changed. Needs the
    // crate name, which needs a manifest; if it's missing we fall through to a
    // full build that generates it.
    if let Ok(crate_name) = read_crate_name(root) {
        let dylib_path =
            engine_backend::services::PieHost::output_dylib_path(root, &crate_name, release);
        if dylib_path.exists() && !any_source_newer(root, &dylib_path) {
            // #833: script classes are data (their compiled modules are
            // reloaded in place); only native sources need cargo. With the
            // running game loaded from this very artifact, Play again only
            // reloads the classes.
            let scripts_only = reload && same_artifact(&dylib_path, loaded_artifact);
            tracing::info!(
                lib = %dylib_path.display(),
                scripts_only,
                "PiE fastpath: no .rs/.toml changes since last build — reusing artifact"
            );
            return Ok(PieStartRequest {
                dylib_path,
                project_root: root.to_path_buf(),
                scene_path: scene_path.to_path_buf(),
                reload,
                scripts_only,
            });
        }
    }

    // Slow path — regenerate scaffolding (src/lib.rs + the cdylib manifest) and
    // build.
    engine_backend::services::ensure_core_bootstrap(root)?;

    let output = std::process::Command::new("cargo")
        .arg("build")
        .arg("--lib")
        .arg("--release")
        .current_dir(root)
        .output()
        .map_err(|e| format!("Failed to spawn cargo: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Keep the message bounded — the full log is on stderr/tracing.
        let tail: String = stderr.lines().rev().take(20).collect::<Vec<_>>().join("\n");
        return Err(format!("cargo build --lib --release failed:\n{tail}"));
    }

    let crate_name = read_crate_name(root)?;
    let dylib_path =
        engine_backend::services::PieHost::output_dylib_path(root, &crate_name, release);
    if !dylib_path.exists() {
        return Err(format!(
            "Build succeeded but library not found at {}",
            dylib_path.display()
        ));
    }

    Ok(PieStartRequest {
        dylib_path,
        project_root: root.to_path_buf(),
        scene_path: scene_path.to_path_buf(),
        reload,
        scripts_only: false,
    })
}

/// Whether `dylib` is the library the running game was loaded from,
/// unchanged since.
fn same_artifact(dylib: &Path, loaded: Option<&(std::path::PathBuf, std::time::SystemTime)>) -> bool {
    let Some((path, mtime)) = loaded else { return false };
    path == dylib && artifact_mtime(dylib).is_some_and(|now| now == *mtime)
}

/// The modification time of a built library.
pub(crate) fn artifact_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The asset updates that reload every script class of the project at
/// `root` in a running game (Play pressed again with only script changes).
pub(crate) fn class_reload_events(root: &Path) -> Vec<plugin_editor_api::AssetUpdated> {
    pulsar_class::ClassRegistry::scan(root)
        .entries()
        .iter()
        .map(|entry| {
            plugin_editor_api::AssetUpdated::new(plugin_editor_api::AssetKind::Blueprint)
                .with_id(entry.id.as_str())
                .with_path(entry.dir.clone())
        })
        .collect()
}

/// Whether any `.rs` or `.toml` file under `root` is newer than `artifact`.
/// Skips `target/` and `.git/`. A missing/unreadable artifact counts as "newer"
/// so the caller rebuilds.
fn any_source_newer(root: &Path, artifact: &Path) -> bool {
    let Ok(artifact_mtime) = std::fs::metadata(artifact).and_then(|m| m.modified()) else {
        return true;
    };
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let name = entry.file_name();
                if name == "target" || name == ".git" {
                    continue;
                }
                stack.push(entry.path());
            } else if file_type.is_file() {
                let path = entry.path();
                let is_source = path
                    .extension()
                    .map(|e| e == "rs" || e == "toml")
                    .unwrap_or(false);
                if is_source {
                    if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                        if mtime > artifact_mtime {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

/// Read the `[package] name` from the project's `Cargo.toml`.
fn read_crate_name(root: &Path) -> Result<String, String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("Failed to read Cargo.toml: {e}"))?;
    for line in toml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(value) = rest.strip_prefix('=') {
                let name = value.trim().trim_matches('"').trim();
                if !name.is_empty() {
                    return Ok(name.to_string());
                }
            }
        }
    }
    Err("Could not find package name in Cargo.toml".to_string())
}
#[cfg(test)]
mod tests {
    use super::*;

    /// #833: saving and compiling a Blueprint writes only data files
    /// (graph, variables, prefab, the compiled module), so Play reuses the
    /// built library without cargo; a native source change does not.
    #[test]
    fn script_only_changes_do_not_need_cargo() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src/classes/Door/events/.build")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "// lib").unwrap();
        let artifact = root.path().join("target/release/libgame.so");
        std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        // The artifact is built after the sources.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&artifact, b"lib").unwrap();
        assert!(!any_source_newer(root.path(), &artifact));

        std::thread::sleep(std::time::Duration::from_millis(20));
        let class = root.path().join("src/classes/Door");
        for file in ["graph_save.json", "vars_save.json", "prefab.json", "class.json", "events/.build/module.json"] {
            std::fs::write(class.join(file), "{}").unwrap();
        }
        assert!(!any_source_newer(root.path(), &artifact), "script data never needs a rebuild");

        let loaded = (artifact.clone(), artifact_mtime(&artifact).unwrap());
        assert!(same_artifact(&artifact, Some(&loaded)), "Play again reloads scripts only");
        assert!(!same_artifact(&artifact, None), "no running game: a normal start");

        std::fs::write(root.path().join("src/lib.rs"), "// changed").unwrap();
        assert!(any_source_newer(root.path(), &artifact), "native changes still rebuild");
    }

    #[test]
    fn class_reload_events_name_every_class() {
        let root = tempfile::tempdir().unwrap();
        for (name, guid) in [("Door", "door-guid"), ("Lamp", "lamp-guid")] {
            let dir = root.path().join("src/classes").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("class.json"), format!("{{\"class_id\":\"{guid}\"}}")).unwrap();
            std::fs::write(dir.join("graph_save.json"), "{}").unwrap();
        }
        let mut ids: Vec<String> = class_reload_events(root.path()).into_iter().filter_map(|e| e.id).collect();
        ids.sort();
        assert_eq!(ids, ["door-guid", "lamp-guid"]);
    }
}
