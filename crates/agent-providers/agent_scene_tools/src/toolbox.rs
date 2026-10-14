//! The toolbox: scene access, the transaction journal and the sandboxes of
//! one editor session, behind the operations the tools and the run extension
//! call.

use std::collections::HashMap;
use std::sync::Arc;

use agent_chat_tools::AgentRunScope;
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use pulsar_scenedb::World;

use crate::journal::{author_of, Recorder, TransactionLog};
use crate::sandbox::Fork;
use crate::source::{SceneSource, SharedScene};

/// What a write did, for the tool result.
#[derive(Clone, Debug)]
pub struct WriteNote {
    /// The edit went into a private sandbox, not the live scene.
    pub sandbox: bool,
    /// Transaction number in the journal (live edits only).
    pub seq: Option<u64>,
    pub author: String,
}

impl WriteNote {
    pub fn to_json(&self) -> serde_json::Value {
        if self.sandbox {
            serde_json::json!({
                "sandbox": true,
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
        })
    }

    pub fn log(&self) -> &TransactionLog {
        &self.log
    }

    /// The sandbox a call must use: the one this run owns (created on first
    /// use for a sandbox run), or the open one of a run above it.
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
        let scene = self.source.scene().map_err(|e| anyhow!(e))?;
        let fork = {
            let db = scene.read();
            Fork::create(&run_id, &db.world).map_err(|e| anyhow!(e))?
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

    /// Run a read against the scene this run sees (its sandbox, or live).
    pub fn read<R>(&self, scope: &AgentRunScope, f: impl FnOnce(&World) -> Result<R>) -> Result<R> {
        if let Some(fork) = self.fork_for(scope)? {
            let fork = fork.lock();
            return f(&fork.world);
        }
        let scene = self.source.scene().map_err(|e| anyhow!(e))?;
        let db = scene.read();
        f(&db.world)
    }

    /// Run an edit as one transaction. On error everything the call wrote is
    /// rolled back. A live edit is journaled under `agent:<run id>`; a
    /// sandbox edit only changes the sandbox.
    pub fn write<R>(
        &self,
        scope: &AgentRunScope,
        tool: &str,
        f: impl FnOnce(&mut World, &mut Recorder, &dyn SceneSource) -> Result<(R, String)>,
    ) -> Result<(R, WriteNote)> {
        let run_id = run_id_of(scope);
        let author = author_of(&run_id);
        if let Some(fork) = self.fork_for(scope)? {
            let mut fork = fork.lock();
            if fork.finished {
                bail!(
                    "This sandbox is closed: its run has finished and its diff is waiting for review."
                );
            }
            let mut rec = Recorder::new();
            return match f(&mut fork.world, &mut rec, &*self.source) {
                Ok((value, summary)) => {
                    fork.ops.push(summary);
                    Ok((value, WriteNote { sandbox: true, seq: None, author }))
                }
                Err(err) => {
                    if let Err(re) = rec.rollback(&mut fork.world) {
                        tracing::error!("[scene_tools] sandbox rollback failed: {re}");
                    }
                    Err(err)
                }
            };
        }
        let scene = self.source.scene().map_err(|e| anyhow!(e))?;
        let key = scene_key(&scene);
        let mut db = scene.write();
        let mut rec = Recorder::new();
        match f(&mut db.world, &mut rec, &*self.source) {
            Ok((value, summary)) => {
                let seq = self.log.record(&run_id, tool, summary, key, rec);
                Ok((value, WriteNote { sandbox: false, seq, author }))
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
    /// sandboxes. Returns the notes for the revert summary.
    pub fn revert_tree(&self, root: &str) -> Result<Vec<String>, String> {
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
        let mut db = scene.write();
        let outcome = self.log.revert_tree(root, &mut db.world, key);
        if outcome.reverted > 0 {
            notes.push(format!(
                "Undid {} scene transaction(s) ({} object(s)) made by agent:{root}.",
                outcome.reverted, outcome.objects
            ));
        }
        if outcome.failed.is_empty() {
            Ok(notes)
        } else {
            Err(format!(
                "{} {}",
                notes.join(" "),
                outcome.failed.join("; ")
            ))
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
        let diff = fork.compute_diff();
        let text = diff.render_text(run_id, &fork.ops);
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
                let seq = self.log.record(key, "sandbox_apply", summary.clone(), scene_id, rec);
                drop(db);
                drop(fork);
                self.forks.lock().remove(key);
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
}
