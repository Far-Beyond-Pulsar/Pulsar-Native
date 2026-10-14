//! The toolbox: scene access, the transaction journal and the sandboxes of
//! one editor session, behind the operations the tools and the run extension
//! call.

use std::collections::HashMap;
use std::sync::Arc;

use agent_chat_tools::AgentRunScope;
use anyhow::{anyhow, bail, Result};
use parking_lot::{Mutex, RwLock};
use pulsar_scenedb::World;
use serde_json::Value;

use crate::journal::{author_of, Recorder, TransactionLog};
use crate::sandbox::{Fork, ForkState, SceneDiff};
use crate::settings::{resolve_mode, SandboxMode, SceneSettings};
use crate::source::{SceneSource, SharedScene};
use crate::write_op::WriteOp;

/// What a write did, for the tool result.
#[derive(Clone, Debug)]
pub struct WriteNote {
    /// The edit went into a private sandbox, not the live scene.
    pub sandbox: bool,
    /// `overlay` or `copy` for a sandbox edit, `live` otherwise.
    pub mode: &'static str,
    /// Transaction number in the journal (live edits only).
    pub seq: Option<u64>,
    pub author: String,
}

impl WriteNote {
    pub fn to_json(&self) -> serde_json::Value {
        if self.sandbox {
            serde_json::json!({
                "sandbox": true,
                "sandbox_mode": self.mode,
                "author": self.author,
                "note": "Recorded in your private sandbox. The live scene is unchanged; your changes come back as a diff for the user to apply or discard.",
            })
        } else {
            serde_json::json!({
                "sandbox": false,
                "transaction": self.seq,
                "author": self.author,
                "undoable": true,
            })
        }
    }
}

type ForkHandle = Arc<Mutex<Fork>>;

pub struct SceneToolbox {
    pub(crate) source: Arc<dyn SceneSource>,
    pub(crate) log: TransactionLog,
    pub(crate) forks: Mutex<HashMap<String, ForkHandle>>,
    settings: RwLock<SceneSettings>,
}

pub(crate) fn scene_key(scene: &SharedScene) -> usize {
    Arc::as_ptr(scene) as *const u8 as usize
}

fn run_id_of(scope: &AgentRunScope) -> String {
    if scope.run_id.is_empty() {
        "adhoc".to_string()
    } else {
        scope.run_id.clone()
    }
}

impl SceneToolbox {
    pub fn new(source: Arc<dyn SceneSource>) -> Arc<Self> {
        Arc::new(Self {
            source,
            log: TransactionLog::new(),
            forks: Mutex::new(HashMap::new()),
            settings: RwLock::new(SceneSettings::default()),
        })
    }

    pub fn log(&self) -> &TransactionLog {
        &self.log
    }

    /// The project's sandbox settings (`.pulsar/agent_scene.json`).
    pub fn settings(&self) -> SceneSettings {
        self.settings.read().clone()
    }

    /// Replace the project's sandbox settings; applies to sandboxes created
    /// from now on.
    pub fn set_settings(&self, settings: SceneSettings) {
        *self.settings.write() = settings;
    }

    /// The mode a run with this scope gets right now.
    pub fn mode_for(&self, scope: &AgentRunScope) -> SandboxMode {
        resolve_mode(scope.sandbox_mode.as_deref(), self.settings.read().sandbox_mode)
    }

    /// The sandbox a call must use: the one this run owns (created on first
    /// use for a sandbox run), or the open one of a run above it. `None`
    /// means the live scene: a run without a sandbox, or one whose mode is
    /// `off`.
    fn fork_for(&self, scope: &AgentRunScope) -> Result<Option<ForkHandle>> {
        let run_id = run_id_of(scope);
        {
            let forks = self.forks.lock();
            if let Some(handle) = forks.get(&run_id) {
                return Ok(Some(handle.clone()));
            }
            // A run below a sandbox run stays inside that sandbox.
            if let Some(handle) = forks
                .iter()
                .find(|(key, _)| agent_runtime::run_in_tree(&run_id, key))
                .map(|(_, handle)| handle.clone())
            {
                return Ok(Some(handle));
            }
        }
        if !scope.sandbox {
            return Ok(None);
        }
        let fork = match self.mode_for(scope) {
            SandboxMode::Off => return Ok(None),
            SandboxMode::Overlay => Fork::overlay(&run_id),
            SandboxMode::Copy => {
                let max = self.settings.read().copy_max_objects;
                let scene = self.source.scene().map_err(|e| anyhow!(e))?;
                let db = scene.read();
                Fork::create(&run_id, &db.world, max).map_err(|e| anyhow!(e))?
            }
        };
        // Two parallel tool calls of one run may both get here; the first
        // fork wins so no edit lands in a fork that is then dropped.
        let handle = self
            .forks
            .lock()
            .entry(run_id)
            .or_insert_with(|| Arc::new(Mutex::new(fork)))
            .clone();
        Ok(Some(handle))
    }

    /// Run a read against the scene this run sees: its copy sandbox, the live
    /// scene plus its own pending edits (overlay), or the live scene.
    pub fn read<R>(&self, scope: &AgentRunScope, f: impl FnOnce(&World) -> Result<R>) -> Result<R> {
        if let Some(fork) = self.fork_for(scope)? {
            let fork = fork.lock();
            match &fork.state {
                ForkState::Copy(world) => return f(world),
                ForkState::Overlay(overlay) if !overlay.is_empty() => {
                    let scene = self.source.scene().map_err(|e| anyhow!(e))?;
                    let mut db = scene.write();
                    return overlay.view(&mut db.world, &*self.source, f);
                }
                ForkState::Overlay(_) => {}
            }
        }
        let scene = self.source.scene().map_err(|e| anyhow!(e))?;
        let db = scene.read();
        f(&db.world)
    }

    /// Run an edit as one transaction. On error everything the call wrote is
    /// rolled back. A live edit is journaled under `agent:<run id>`; a
    /// sandbox edit only changes the sandbox (a private world, or the
    /// overlay's list of recorded ops).
    pub fn write_op(&self, scope: &AgentRunScope, mut op: WriteOp) -> Result<(Value, WriteNote)> {
        let run_id = run_id_of(scope);
        let author = author_of(&run_id);
        if let Some(handle) = self.fork_for(scope)? {
            let mut fork = handle.lock();
            if fork.finished {
                bail!(
                    "This sandbox is closed: its run has finished and its diff is waiting for review."
                );
            }
            let fork = &mut *fork;
            let mode = match fork.state {
                ForkState::Copy(_) => "copy",
                ForkState::Overlay(_) => "overlay",
            };
            let done = match &mut fork.state {
                ForkState::Copy(world) => {
                    let mut rec = Recorder::new();
                    match op.exec(world, &mut rec, &*self.source) {
                        Ok(done) => done,
                        Err(err) => {
                            if let Err(re) = rec.rollback(world) {
                                tracing::error!("[scene_tools] sandbox rollback failed: {re}");
                            }
                            return Err(err);
                        }
                    }
                }
                ForkState::Overlay(overlay) => {
                    let scene = self.source.scene().map_err(|e| anyhow!(e))?;
                    let mut db = scene.write();
                    overlay.write(&mut db.world, &*self.source, op)?
                }
            };
            let (value, summary) = done;
            fork.ops.push(summary);
            return Ok((value, WriteNote { sandbox: true, mode, seq: None, author }));
        }
        let scene = self.source.scene().map_err(|e| anyhow!(e))?;
        let key = scene_key(&scene);
        let mut db = scene.write();
        let mut rec = Recorder::new();
        match op.exec(&mut db.world, &mut rec, &*self.source) {
            Ok((value, summary)) => {
                let seq = self.log.record(&run_id, op.tool(), summary.clone(), key, &db.world, rec);
                drop(db);
                if seq.is_some() {
                    self.source.scene_edited(&summary);
                }
                Ok((value, WriteNote { sandbox: false, mode: "live", seq, author }))
            }
            Err(err) => {
                if let Err(re) = rec.rollback(&mut db.world) {
                    tracing::error!("[scene_tools] rollback failed: {re}");
                }
                Err(err)
            }
        }
    }

    // ── Run-tree operations (used by the run extension) ──────────────────────

    /// Undo the scene edits of `root` and its descendants and drop their
    /// sandboxes. Edits to objects the user changed since are left alone and
    /// reported as an error. Returns the notes for the revert summary.
    pub fn revert_tree(&self, root: &str) -> Result<Vec<String>, String> {
        self.revert_tree_with(root, false)
    }

    /// [`Self::revert_tree`]; with `force` the agent's edits are undone even
    /// over objects the user changed since.
    pub fn revert_tree_with(&self, root: &str, force: bool) -> Result<Vec<String>, String> {
        let mut notes = Vec::new();
        let dropped: Vec<String> = {
            let mut forks = self.forks.lock();
            let keys: Vec<String> = forks
                .keys()
                .filter(|k| agent_runtime::run_in_tree(k, root))
                .cloned()
                .collect();
            for key in &keys {
                forks.remove(key);
            }
            keys
        };
        if !dropped.is_empty() {
            notes.push(format!("Discarded {} scene sandbox(es).", dropped.len()));
        }
        if !self.log.has_undo(root) {
            return Ok(notes);
        }
        let scene = self.source.scene()?;
        let key = scene_key(&scene);
        let outcome = {
            let mut db = scene.write();
            self.log.revert_tree(root, &mut db.world, key, force)
        };
        if outcome.reverted > 0 {
            self.source.scene_edited(&format!("reverted agent:{root}"));
            notes.push(format!(
                "Undid {} scene transaction(s) ({} object(s)) made by agent:{root}.",
                outcome.reverted, outcome.objects
            ));
        }
        let mut problems = outcome.failed;
        if !outcome.conflicts.is_empty() {
            problems.extend(outcome.conflicts);
            problems.push(
                "Those objects were edited by someone else after the agent; they were left as they are. \
                 Revert with force to overwrite them."
                    .to_string(),
            );
        }
        if problems.is_empty() {
            Ok(notes)
        } else {
            Err(format!("{} {}", notes.join(" "), problems.join("; ")))
        }
    }

    /// The run `run_id` finished: close its sandbox (if it has one), compute
    /// the diff and return the report section. An unchanged sandbox is
    /// dropped.
    pub fn finish_sandbox(&self, run_id: &str) -> Vec<(String, String)> {
        let Some(handle) = self.forks.lock().get(run_id).cloned() else {
            return Vec::new();
        };
        let mut fork = handle.lock();
        let mut ops = fork.ops.clone();
        let computed: Result<(SceneDiff, Option<crate::sandbox::SceneImage>), String> =
            match &fork.state {
                ForkState::Copy(_) => Ok((fork.compute_diff(), None)),
                ForkState::Overlay(overlay) => self
                    .source
                    .scene()
                    .map_err(|e| e.to_string())
                    .and_then(|scene| {
                        let mut db = scene.write();
                        overlay
                            .finish(&mut db.world, &*self.source)
                            .map_err(|e| e.to_string())
                    })
                    .map(|(base, now, skipped)| {
                        for note in skipped {
                            ops.push(format!("NOT APPLICABLE to the current scene, skipped: {note}"));
                        }
                        (SceneDiff::compute(&base, &now), Some(base))
                    }),
            };
        let diff = match computed {
            Ok((diff, base)) => {
                if let Some(base) = base {
                    fork.base = base;
                }
                diff
            }
            Err(err) => {
                drop(fork);
                self.forks.lock().remove(run_id);
                return vec![(
                    "scene_diff".to_string(),
                    format!("The sandbox diff could not be computed and was dropped: {err}"),
                )];
            }
        };
        let text = diff.render_text(run_id, &ops);
        let empty = diff.is_empty();
        fork.finished = true;
        fork.review = Some((
            format!("Scene sandbox {run_id}: {}", diff.headline()),
            text.clone(),
        ));
        fork.diff = Some(diff);
        drop(fork);
        if empty {
            self.forks.lock().remove(run_id);
            return vec![(
                "scene_diff".to_string(),
                "The sandbox run made no scene changes.".to_string(),
            )];
        }
        vec![("scene_diff".to_string(), text)]
    }

    /// Sandboxes whose run finished and whose diff is waiting.
    pub fn pending(&self) -> Vec<(String, String, String)> {
        let handles: Vec<(String, ForkHandle)> = self
            .forks
            .lock()
            .iter()
            .map(|(key, handle)| (key.clone(), handle.clone()))
            .collect();
        let mut out: Vec<(String, String, String)> = handles
            .into_iter()
            .filter_map(|(key, handle)| {
                let fork = handle.lock();
                let (title, text) = fork.review.clone().filter(|_| fork.finished)?;
                Some((key, title, text))
            })
            .collect();
        out.sort();
        out
    }

    /// Does `root` or a run below it own a sandbox diff waiting for review?
    pub fn has_pending_in(&self, root: &str) -> bool {
        let handles: Vec<ForkHandle> = self
            .forks
            .lock()
            .iter()
            .filter(|(key, _)| agent_runtime::run_in_tree(key, root))
            .map(|(_, handle)| handle.clone())
            .collect();
        handles.iter().any(|h| {
            let fork = h.lock();
            fork.finished && fork.review.is_some()
        })
    }

    /// Replay a finished sandbox's diff into the live scene as one
    /// transaction attributed to the sandbox's run.
    pub fn apply_sandbox(&self, key: &str, force: bool) -> Result<String, String> {
        let handle = self
            .forks
            .lock()
            .get(key)
            .cloned()
            .ok_or_else(|| format!("No sandbox '{key}'. It may already be applied or discarded."))?;
        let fork = handle.lock();
        if !fork.finished {
            return Err(format!(
                "Sandbox '{key}' is still being edited by its run; wait for the run to finish."
            ));
        }
        let diff = fork
            .diff
            .clone()
            .ok_or_else(|| "The sandbox has no diff.".to_string())?;
        let scene = self.source.scene()?;
        let scene_id = scene_key(&scene);
        let mut db = scene.write();
        let mut rec = Recorder::new();
        match crate::sandbox::apply_diff(&mut db.world, &mut rec, &fork.base, &diff, force) {
            Ok(report) => {
                let summary = format!(
                    "applied sandbox {key}: +{} -{} ~{}",
                    report.added, report.removed, report.changed
                );
                let seq = self.log.record(key, "sandbox_apply", summary.clone(), scene_id, &db.world, rec);
                drop(db);
                drop(fork);
                self.forks.lock().remove(key);
                if seq.is_some() {
                    self.source.scene_edited(&summary);
                }
                Ok(format!(
                    "Applied sandbox {key} to the live scene as one transaction{}: {} added, {} removed, {} changed objects.",
                    seq.map(|s| format!(" (#{s}, author {})", author_of(key)))
                        .unwrap_or_default(),
                    report.added,
                    report.removed,
                    report.changed
                ))
            }
            Err(err) => {
                if let Err(re) = rec.rollback(&mut db.world) {
                    tracing::error!("[scene_tools] apply rollback failed: {re}");
                }
                Err(err)
            }
        }
    }

    /// Drop a finished sandbox without applying it.
    pub fn discard_sandbox(&self, key: &str) -> Result<String, String> {
        match self.forks.lock().remove(key) {
            Some(_) => Ok(format!("Discarded sandbox {key}; the live scene was not changed.")),
            None => Err(format!("No sandbox '{key}'. It may already be applied or discarded.")),
        }
    }

    pub fn sandbox_exists(&self, key: &str) -> bool {
        self.forks.lock().contains_key(key)
    }

    /// The mode of the open sandbox `key`, if there is one.
    pub fn sandbox_mode_of(&self, key: &str) -> Option<SandboxMode> {
        self.forks.lock().get(key).map(|h| h.lock().mode())
    }
}
