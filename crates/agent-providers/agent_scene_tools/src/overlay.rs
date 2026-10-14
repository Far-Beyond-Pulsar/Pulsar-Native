//! The overlay sandbox: a run's writes are recorded as validated operations
//! ([`WriteOp`]) against a read-through view of the live scene; nothing is
//! copied.
//!
//! # How it works
//!
//! The overlay owns the ordered list of ops the run made so far and, for each
//! object the run touched, the state it had when the run first touched it
//! (the *base*, `None` for an object the run created).
//!
//! * A **read** by the run materialises the view: under the live scene's
//!   write lock the pending ops are replayed on the live world, the read
//!   runs, and the replay is rolled back with the same journal recorder the
//!   live transactions use. The run therefore sees live state plus its own
//!   pending ops, and a live edit made by the user in between shows through.
//!   With no pending ops a read is a plain live read.
//! * A **write** replays the pending ops, runs the new op (validating it
//!   exactly as a live write would, including the level rules), keeps it on
//!   success, and rolls everything back. A rejected op leaves no trace.
//! * The **diff** replays once more, snapshots only the touched objects, and
//!   compares them with their base. Applying it is the ordinary
//!   [`crate::sandbox::apply_diff`], with its conflict check.
//!
//! Everything is O(ops and touched objects); nothing is O(scene) except what
//! every scene tool already does (indexing the world by stable id).
//!
//! Cost to know about: the replay happens in the live world, under its write
//! lock and rolled back before the lock is released, so other readers never
//! see it, but the world's change journals do record the apply and the
//! rollback. Use `copy` mode when that matters more than copying the scene.

use std::collections::HashSet;

use anyhow::{anyhow, Result};
use pulsar_scenedb::World;
use serde_json::Value;

use crate::journal::Recorder;
use crate::sandbox::SceneImage;
use crate::snapshot::{snapshot_object, ObjectSnapshot, SceneIndex};
use crate::source::SceneSource;
use crate::write_op::WriteOp;

#[derive(Default)]
pub struct Overlay {
    pending: Vec<WriteOp>,
    /// State of each touched object when first touched, in first-touch order.
    base: Vec<(String, Option<ObjectSnapshot>)>,
    seen: HashSet<String>,
}

impl Overlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Apply the pending ops to `world`. Returns what they touched (for the
    /// rollback) and a note per op that no longer applies to the live scene
    /// (the user deleted what it edited, say); such ops are skipped.
    fn replay(&self, world: &mut World, rules: &dyn SceneSource) -> (Recorder, Vec<String>) {
        let mut all = Recorder::new();
        let mut skipped = Vec::new();
        for op in &self.pending {
            let mut op = op.clone();
            let mut rec = Recorder::new();
            match op.exec(world, &mut rec, rules) {
                Ok(_) => all.absorb(rec),
                Err(err) => {
                    if let Err(re) = rec.rollback(world) {
                        tracing::error!("[scene_tools] overlay replay rollback failed: {re}");
                    }
                    skipped.push(format!("{}: {err}", op.tool()));
                }
            }
        }
        (all, skipped)
    }

    fn undo(world: &mut World, all: &Recorder) -> Result<()> {
        all.rollback(world)
            .map_err(|e| anyhow!("The overlay could not restore the live scene: {e}"))
    }

    /// Run `op` as the next op of the overlay. On success the op is kept and
    /// its result and summary returned; the live world is unchanged either
    /// way.
    pub fn write(
        &mut self,
        world: &mut World,
        rules: &dyn SceneSource,
        mut op: WriteOp,
    ) -> Result<(Value, String)> {
        let (mut all, _) = self.replay(world, rules);
        let mut rec = Recorder::new();
        let outcome = op.exec(world, &mut rec, rules);
        match outcome {
            Ok(done) => {
                for (id, snapshot) in rec.entries() {
                    if self.seen.insert(id.clone()) {
                        self.base.push((id.clone(), snapshot.clone()));
                    }
                }
                all.absorb(rec);
                self.pending.push(op);
                Self::undo(world, &all)?;
                Ok(done)
            }
            Err(err) => {
                if let Err(re) = rec.rollback(world) {
                    tracing::error!("[scene_tools] overlay write rollback failed: {re}");
                }
                Self::undo(world, &all)?;
                Err(err)
            }
        }
    }

    /// Run a read over live state plus the pending ops.
    pub fn view<R>(
        &self,
        world: &mut World,
        rules: &dyn SceneSource,
        f: impl FnOnce(&World) -> Result<R>,
    ) -> Result<R> {
        let (all, _) = self.replay(world, rules);
        let result = f(world);
        Self::undo(world, &all)?;
        result
    }

    /// The base and the end state of every touched object, plus the ops that
    /// no longer apply.
    pub fn finish(
        &self,
        world: &mut World,
        rules: &dyn SceneSource,
    ) -> Result<(SceneImage, SceneImage, Vec<String>)> {
        let (all, skipped) = self.replay(world, rules);
        let index = SceneIndex::build(world);
        let now: Vec<ObjectSnapshot> = self
            .base
            .iter()
            .filter_map(|(id, _)| index.entity(id).and_then(|e| snapshot_object(world, &index, e)))
            .collect();
        Self::undo(world, &all)?;
        let base: Vec<ObjectSnapshot> = self.base.iter().filter_map(|(_, s)| s.clone()).collect();
        Ok((image_of(base), image_of(now), skipped))
    }
}

/// An image of `snapshots` with parents before children.
fn image_of(snapshots: Vec<ObjectSnapshot>) -> SceneImage {
    let by_id: std::collections::HashMap<&str, &ObjectSnapshot> =
        snapshots.iter().map(|s| (s.id.as_str(), s)).collect();
    let depth = |snapshot: &ObjectSnapshot| {
        let mut depth = 0usize;
        let mut parent = snapshot.parent.as_deref();
        while let Some(p) = parent.and_then(|p| by_id.get(p)) {
            depth += 1;
            parent = p.parent.as_deref();
            if depth > snapshots.len() {
                break;
            }
        }
        depth
    };
    let mut keyed: Vec<(usize, usize)> =
        snapshots.iter().enumerate().map(|(i, s)| (depth(s), i)).collect();
    keyed.sort();
    let order: Vec<String> = keyed.iter().map(|&(_, i)| snapshots[i].id.clone()).collect();
    SceneImage {
        order,
        objects: snapshots.into_iter().map(|s| (s.id.clone(), s)).collect(),
    }
}
