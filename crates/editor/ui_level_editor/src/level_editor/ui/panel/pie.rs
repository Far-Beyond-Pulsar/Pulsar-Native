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
    // Snapshot the scene (also flips to play mode).
    shared_state.write().scene.enter_play_mode();

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
    let reload = {
        let mut st = shared_state.write();
        st.play.pie.building = true;
        st.play.pie.stop_requested = false;
        st.play.pie.last_error = None;
        st.play.pie.pending_start = None;
        st.play.pie.active
    };

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
            let result = build_pie_dylib(&root, &scene_path, reload);
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
pub(crate) fn end_pie(shared_state: Arc<parking_lot::RwLock<LevelEditorState>>) {
    {
        let mut st = shared_state.write();
        st.play.pie.stop_requested = true;
        st.play.pie.pending_start = None;
        st.play.pie.building = false;
    }
    shared_state.write().scene.exit_play_mode();
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
            tracing::info!(
                lib = %dylib_path.display(),
                "PiE fastpath: no .rs/.toml changes since last build — reusing artifact"
            );
            return Ok(PieStartRequest {
                dylib_path,
                project_root: root.to_path_buf(),
                scene_path: scene_path.to_path_buf(),
                reload,
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
    })
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