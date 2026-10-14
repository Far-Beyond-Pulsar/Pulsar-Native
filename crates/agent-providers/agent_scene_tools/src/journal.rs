//! The transaction journal: every agent edit of the live scene is one
//! transaction that records the state it replaced, attributed to the run
//! (`agent:<run_id>`), so it can be undone exactly.
//!
//! SceneDB's change journals (`open_change_cursor` / `read_changes`) tell a
//! reader *that* a component changed; they carry no previous values, and
//! SceneDB has no undo of its own. The transaction therefore keeps its own
//! inverse: a typed snapshot of every object it touched, taken before the
//! first write to that object (the first capture wins), or `None` for an
//! object it created. Undoing writes the snapshots back (and removes the
//! created objects), the same way the level editor's undo history restores a
//! command's before-state, so the GPU mirror, subscriptions and change
//! journals see an ordinary write.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use pulsar_scene_model::SceneWorldExt;
use pulsar_scenedb::World;

use crate::snapshot::{
    apply_snapshots, snapshot_object, snapshot_subtree, ObjectSnapshot, SceneIndex,
};

/// Collects the inverse of one tool call while it runs.
#[derive(Default)]
pub struct Recorder {
    before: Vec<(String, Option<ObjectSnapshot>)>,
    seen: HashSet<String>,
}

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Did the call touch anything?
    pub fn is_empty(&self) -> bool {
        self.before.is_empty()
    }

    /// Ids touched so far, in first-touch order.
    pub fn touched(&self) -> Vec<String> {
        self.before.iter().map(|(id, _)| id.clone()).collect()
    }

    /// Record the current state of the existing object `id` (just that
    /// object; descendants are untouched by the edit). Call before writing.
    pub fn note_existing(&mut self, world: &World, index: &SceneIndex, id: &str) {
        if self.seen.contains(id) {
            return;
        }
        if let Some(entity) = index.entity(id) {
            if let Some(snapshot) = snapshot_object(world, index, entity) {
                self.seen.insert(id.to_string());
                self.before.push((id.to_string(), Some(snapshot)));
            }
        }
    }

    /// Record `id` and everything below it (a delete removes the subtree).
    pub fn note_subtree(&mut self, world: &World, index: &SceneIndex, id: &str) {
        let Some(root) = index.entity(id) else {
            return;
        };
        for snapshot in snapshot_subtree(world, index, root) {
            if self.seen.insert(snapshot.id.clone()) {
                self.before.push((snapshot.id.clone(), Some(snapshot)));
            }
        }
    }

    /// Record that `id` did not exist before the call (it is created).
    pub fn note_new(&mut self, id: &str) {
        if self.seen.insert(id.to_string()) {
            self.before.push((id.to_string(), None));
        }
    }

    /// Undo everything recorded, in place. Used when the call fails halfway
    /// so a rejected edit leaves nothing behind.
    pub fn rollback(&self, world: &mut World) -> Result<(), String> {
        restore_before(world, &self.before)
    }

    pub(crate) fn into_before(self) -> Vec<(String, Option<ObjectSnapshot>)> {
        self.before
    }
}

/// Write `before` back: existing objects take their snapshot (re-created if
/// they were deleted), then objects the transaction created are removed.
/// Restoring first matters: an object the transaction moved under a new
/// object must leave it before that object is removed with its children.
pub(crate) fn restore_before(
    world: &mut World,
    before: &[(String, Option<ObjectSnapshot>)],
) -> Result<(), String> {
    let index = SceneIndex::build(world);
    let mut ids: HashMap<String, pulsar_scenedb::Entity> = index
        .all_ordered()
        .into_iter()
        .filter_map(|e| index.id_of(e).map(|id| (id.to_string(), e)))
        .collect();
    let snapshots: Vec<ObjectSnapshot> = before.iter().filter_map(|(_, s)| s.clone()).collect();
    apply_snapshots(world, &mut ids, &snapshots)?;
    for (id, snapshot) in before.iter().rev() {
        if snapshot.is_none() {
            if let Some(entity) = world.entity_for(id) {
                world.despawn_tree(entity);
            }
        }
    }
    Ok(())
}

/// One committed agent edit.
pub struct SceneTransaction {
    pub seq: u64,
    /// `agent:<run_id>`.
    pub author: String,
    pub run_id: String,
    pub tool: String,
    pub summary: String,
    /// Stable ids of every object the transaction touched.
    pub touched: Vec<String>,
    pub at_ms: u64,
    pub reverted: bool,
    /// Identity of the scene it ran against (see [`TransactionLog::record`]).
    scene_key: usize,
    before: Vec<(String, Option<ObjectSnapshot>)>,
}

/// What [`TransactionLog::revert_tree`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RevertOutcome {
    pub reverted: usize,
    pub objects: usize,
    /// Transactions that could not be undone and why.
    pub failed: Vec<String>,
}

const MAX_TRANSACTIONS: usize = 1_000;

/// All transactions of one toolbox, newest last.
#[derive(Default)]
pub struct TransactionLog {
    items: Mutex<Vec<SceneTransaction>>,
    next: AtomicU64,
}

pub fn author_of(run_id: &str) -> String {
    format!("agent:{run_id}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

impl TransactionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Commit the recorder as a transaction. `scene_key` identifies the scene
    /// the edit ran against so an undo never writes into a different level.
    /// Returns the sequence number, or `None` when nothing was touched.
    pub fn record(
        &self,
        run_id: &str,
        tool: &str,
        summary: String,
        scene_key: usize,
        recorder: Recorder,
    ) -> Option<u64> {
        if recorder.is_empty() {
            return None;
        }
        let touched = recorder.touched();
        let seq = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let mut items = self.items.lock();
        items.push(SceneTransaction {
            seq,
            author: author_of(run_id),
            run_id: run_id.to_string(),
            tool: tool.to_string(),
            summary,
            touched,
            at_ms: now_ms(),
            reverted: false,
            scene_key,
            before: recorder.into_before(),
        });
        if items.len() > MAX_TRANSACTIONS {
            let overflow = items.len() - MAX_TRANSACTIONS;
            items.drain(..overflow);
        }
        Some(seq)
    }

    /// Does `root` or any run below it have a transaction still applied?
    pub fn has_undo(&self, root: &str) -> bool {
        self.items
            .lock()
            .iter()
            .any(|t| !t.reverted && agent_runtime::run_in_tree(&t.run_id, root))
    }

    /// `(seq, author, tool, summary)` of the applied transactions of a run
    /// tree, oldest first.
    pub fn list_tree(&self, root: &str) -> Vec<(u64, String, String, String)> {
        self.items
            .lock()
            .iter()
            .filter(|t| !t.reverted && agent_runtime::run_in_tree(&t.run_id, root))
            .map(|t| (t.seq, t.author.clone(), t.tool.clone(), t.summary.clone()))
            .collect()
    }

    /// Undo the applied transactions of `root` and the runs below it, newest
    /// first, against `world` (the scene identified by `scene_key`).
    /// Transactions recorded against another scene are skipped and reported.
    pub fn revert_tree(&self, root: &str, world: &mut World, scene_key: usize) -> RevertOutcome {
        let mut items = self.items.lock();
        let mut outcome = RevertOutcome::default();
        for tx in items.iter_mut().rev() {
            if tx.reverted || !agent_runtime::run_in_tree(&tx.run_id, root) {
                continue;
            }
            if tx.scene_key != scene_key {
                outcome.failed.push(format!(
                    "#{} ({}) was made in a scene that is no longer open",
                    tx.seq, tx.tool
                ));
                continue;
            }
            match restore_before(world, &tx.before) {
                Ok(()) => {
                    tx.reverted = true;
                    outcome.reverted += 1;
                    outcome.objects += tx.touched.len();
                }
                Err(err) => outcome
                    .failed
                    .push(format!("#{} ({}): {err}", tx.seq, tx.tool)),
            }
        }
        outcome
    }
}
