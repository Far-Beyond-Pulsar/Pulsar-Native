//! Per-stroke undo history for voxel terrain edits.
//!
//! This is deliberately **not** `SceneDomain`'s undo stack. That one stores
//! `SceneHistorySnapshot`s, which capture the scene database — objects,
//! hierarchy, components. Voxel terrain does not live there at all; it lives
//! in the `pulsar_terrain` runtime behind
//! [`TerrainEditApi`](engine_backend::services::terrain_edit::TerrainEditApi).
//! Undoing a sculpt stroke by restoring a scene snapshot would revert the
//! wrong thing and leave the voxels untouched (design doc §5.5).
//!
//! # The undo unit is a stroke, not a stamp
//!
//! A drag emits many `EditOp` stamps. Undoing them one at a time would be
//! useless to a user. So one stroke — pointer-down through pointer-up — is one
//! history entry: the planet's canonical state is captured once when the
//! stroke opens, and the stamps the stroke committed are recorded as the
//! forward delta.
//!
//! # Why snapshot + op list, rather than two snapshots
//!
//! The terrain mutation log is append-only and replay-deterministic, so
//! "before" plus "the ops applied" fully determines "after". Storing the op
//! list instead of a second [`TerrainSnapshot`] makes redo nearly free and
//! roughly halves what each history entry costs.

use engine_backend::services::terrain_edit::{
    EditOp, TerrainEditApi, TerrainEditError, TerrainSnapshot, TerrainTarget,
};

/// Strokes retained per direction.
///
/// Much smaller than `SceneDomain`'s `MAX_UNDO_HISTORY` of 100 on purpose:
/// each entry here owns a whole-planet [`TerrainSnapshot`] (hierarchy plus
/// mutation tail), which is orders of magnitude larger than a scene snapshot.
/// Bounding the count is what keeps a long sculpting session from growing
/// without limit.
pub const MAX_TERRAIN_UNDO_HISTORY: usize = 16;

// ── Stroke record ──────────────────────────────────────────────────────────

/// One completed sculpt stroke: where it happened, the canonical state it
/// started from, and the stamps it committed.
#[derive(Clone, Debug)]
pub struct TerrainStrokeRecord {
    pub target: TerrainTarget,
    /// Canonical planet state captured when the stroke opened.
    pub before: TerrainSnapshot,
    /// Stamps committed during the stroke, in the order the runtime accepted
    /// them. Replaying these onto `before` reproduces the post-stroke state.
    pub ops: Vec<EditOp>,
}

impl TerrainStrokeRecord {
    pub fn stamp_count(&self) -> usize {
        self.ops.len()
    }
}

/// A stroke that is still open — accumulating stamps until pointer-up.
#[derive(Clone, Debug)]
pub struct OpenStroke {
    pub target: TerrainTarget,
    before: TerrainSnapshot,
    ops: Vec<EditOp>,
}

impl OpenStroke {
    pub fn record(&mut self, op: EditOp) {
        self.ops.push(op);
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

// ── Domain ─────────────────────────────────────────────────────────────────

/// Bounded per-stroke terrain undo/redo history.
#[derive(Clone, Debug, Default)]
pub struct TerrainUndoDomain {
    open: Option<OpenStroke>,
    undo_stack: Vec<TerrainStrokeRecord>,
    redo_stack: Vec<TerrainStrokeRecord>,
}

impl TerrainUndoDomain {
    /// Open a stroke, anchoring it to the planet's current canonical state.
    ///
    /// Returns `false` when no snapshot could be captured (no runtime, or the
    /// planet is not registered); the caller must then not treat the stroke as
    /// started. A stroke already open is closed first, so a dropped
    /// pointer-up can never strand one.
    pub fn begin_stroke(&mut self, api: &TerrainEditApi, target: TerrainTarget) -> bool {
        if self.open.is_some() {
            self.abort_stroke();
        }
        let Some(before) = api.snapshot(target) else {
            return false;
        };
        self.open = Some(OpenStroke {
            target,
            before,
            ops: Vec::new(),
        });
        true
    }

    pub fn is_stroke_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn open_stroke_mut(&mut self) -> Option<&mut OpenStroke> {
        self.open.as_mut()
    }

    /// Record a committed stamp against the open stroke. A stamp applied with
    /// no stroke open is dropped from history rather than silently becoming
    /// its own entry — that would reintroduce per-stamp undo.
    pub fn record_stamp(&mut self, op: EditOp) {
        if let Some(open) = self.open.as_mut() {
            open.record(op);
        }
    }

    /// Close the open stroke and commit it to history.
    ///
    /// A stroke that committed no stamps (click on empty space, or every stamp
    /// coalesced away) is discarded rather than pushed, so undo never has to
    /// step through no-op entries. Returns the committed record, if any.
    pub fn end_stroke(&mut self) -> Option<&TerrainStrokeRecord> {
        let open = self.open.take()?;
        if open.is_empty() {
            return None;
        }
        self.undo_stack.push(TerrainStrokeRecord {
            target: open.target,
            before: open.before,
            ops: open.ops,
        });
        if self.undo_stack.len() > MAX_TERRAIN_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        self.undo_stack.last()
    }

    /// Drop the open stroke without committing it. Used when the tool mode is
    /// left mid-drag, so switching back to Level Edit cannot strand a stroke.
    pub fn abort_stroke(&mut self) -> Option<OpenStroke> {
        self.open.take()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    /// Revert the most recent stroke by restoring its "before" snapshot.
    pub fn undo(&mut self, api: &TerrainEditApi) -> Result<bool, TerrainEditError> {
        let Some(record) = self.undo_stack.pop() else {
            return Ok(false);
        };
        match api.restore(record.target, record.before.clone()) {
            Ok(()) => {
                self.redo_stack.push(record);
                Ok(true)
            }
            Err(error) => {
                // Restore failed: put the entry back so history is not
                // silently lost, exactly as `SceneDomain::undo` does.
                self.undo_stack.push(record);
                Err(error)
            }
        }
    }

    /// Reapply the most recently undone stroke by replaying its stamps.
    pub fn redo(&mut self, api: &TerrainEditApi) -> Result<bool, TerrainEditError> {
        let Some(record) = self.redo_stack.pop() else {
            return Ok(false);
        };
        let mut replayed = Vec::with_capacity(record.ops.len());
        for op in &record.ops {
            match api.apply_edit(record.target, *op) {
                // `apply_edit` reallocates sequence and stable id, so keep
                // what the runtime actually committed -- a later undo/redo
                // round trip must replay the accepted ops, not the originals.
                Ok(committed) => replayed.push(committed),
                Err(error) => {
                    self.redo_stack.push(record);
                    return Err(error);
                }
            }
        }
        self.undo_stack.push(TerrainStrokeRecord {
            ops: replayed,
            ..record
        });
        Ok(true)
    }

    /// Whether any terrain edit has been made that a save should flush.
    pub fn has_unsaved_edits(&self) -> bool {
        self.open.is_some() || !self.undo_stack.is_empty()
    }

    /// The planets touched by anything still in history, for the save path.
    pub fn dirty_targets(&self) -> Vec<TerrainTarget> {
        let mut targets: Vec<TerrainTarget> = self
            .open
            .iter()
            .map(|open| open.target)
            .chain(self.undo_stack.iter().map(|record| record.target))
            .collect();
        targets.dedup();
        targets
    }

    /// Forget all history. Called when a different level is loaded, since the
    /// snapshots belong to planets that no longer exist.
    pub fn clear(&mut self) {
        self.open = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::services::terrain_edit::{EditMode, EditShape, PlanetId};

    fn target() -> TerrainTarget {
        TerrainTarget::Planet(PlanetId::from_stable_name("test"))
    }

    fn op(sequence: u64) -> EditOp {
        EditOp {
            sequence,
            stable_id: [sequence as u8; 16],
            shape: EditShape::Sphere {
                center_cell: [0; 3],
                radius_cells: 8,
            },
            mode: EditMode::Union,
            material: 1,
        }
    }

    #[test]
    fn a_stroke_cannot_open_without_a_terrain_runtime() {
        let api = TerrainEditApi::default();
        let mut undo = TerrainUndoDomain::default();
        assert!(!undo.begin_stroke(&api, target()));
        assert!(!undo.is_stroke_open());
    }

    #[test]
    fn stamps_applied_with_no_open_stroke_do_not_become_history_entries() {
        let mut undo = TerrainUndoDomain::default();
        undo.record_stamp(op(1));
        assert!(undo.end_stroke().is_none());
        assert!(!undo.can_undo());
    }

    #[test]
    fn one_stroke_is_one_history_entry_regardless_of_stamp_count() {
        let mut undo = TerrainUndoDomain::default();
        undo.open = Some(OpenStroke {
            target: target(),
            before: dummy_snapshot(),
            ops: Vec::new(),
        });
        for sequence in 1..=25 {
            undo.record_stamp(op(sequence));
        }
        let committed = undo.end_stroke().expect("a stroke with stamps commits");
        assert_eq!(committed.stamp_count(), 25);
        assert_eq!(undo.undo_depth(), 1);
    }

    #[test]
    fn an_empty_stroke_is_discarded_rather_than_padding_history() {
        let mut undo = TerrainUndoDomain::default();
        undo.open = Some(OpenStroke {
            target: target(),
            before: dummy_snapshot(),
            ops: Vec::new(),
        });
        assert!(undo.end_stroke().is_none());
        assert!(!undo.can_undo());
    }

    #[test]
    fn history_is_bounded_and_drops_the_oldest_stroke_first() {
        let mut undo = TerrainUndoDomain::default();
        for index in 0..(MAX_TERRAIN_UNDO_HISTORY + 5) {
            undo.open = Some(OpenStroke {
                target: target(),
                before: dummy_snapshot(),
                ops: Vec::new(),
            });
            undo.record_stamp(op(index as u64 + 1));
            undo.end_stroke();
        }
        assert_eq!(undo.undo_depth(), MAX_TERRAIN_UNDO_HISTORY);
        assert_eq!(
            undo.undo_stack[0].ops[0].sequence,
            6,
            "the five oldest strokes should have been evicted"
        );
    }

    #[test]
    fn leaving_the_mode_mid_drag_cannot_strand_an_open_stroke() {
        let mut undo = TerrainUndoDomain::default();
        undo.open = Some(OpenStroke {
            target: target(),
            before: dummy_snapshot(),
            ops: Vec::new(),
        });
        undo.record_stamp(op(1));
        assert!(undo.abort_stroke().is_some());
        assert!(!undo.is_stroke_open());
        assert!(!undo.can_undo(), "an aborted stroke must not enter history");
    }

    #[test]
    fn opening_a_second_stroke_closes_the_first_instead_of_nesting() {
        let api = TerrainEditApi::default();
        let mut undo = TerrainUndoDomain::default();
        undo.open = Some(OpenStroke {
            target: target(),
            before: dummy_snapshot(),
            ops: vec![op(1)],
        });
        // `begin_stroke` fails without a runtime, but must still have closed
        // the stale stroke on its way out.
        assert!(!undo.begin_stroke(&api, target()));
        assert!(!undo.is_stroke_open());
    }

    #[test]
    fn clearing_history_forgets_open_and_committed_strokes_alike() {
        let mut undo = TerrainUndoDomain::default();
        undo.open = Some(OpenStroke {
            target: target(),
            before: dummy_snapshot(),
            ops: vec![op(1)],
        });
        undo.end_stroke();
        assert!(undo.has_unsaved_edits());
        undo.clear();
        assert!(!undo.has_unsaved_edits());
        assert!(undo.dirty_targets().is_empty());
    }

    /// A structurally valid but empty snapshot. These tests exercise history
    /// bookkeeping, which never inspects the snapshot's contents — only
    /// `TerrainEditApi::restore` does, and that needs a live runtime.
    fn dummy_snapshot() -> TerrainSnapshot {
        use engine_backend::services::terrain_edit::{ContentHash, NodeState, SparseBrickTree};

        let generator_hash = ContentHash::of(b"terrain-undo-test");
        TerrainSnapshot {
            planet_id: PlanetId::from_stable_name("test"),
            generator_hash,
            hierarchy: SparseBrickTree::centered(12, NodeState::Procedural(generator_hash))
                .expect("a centered root at lod 12 is valid"),
            edit_tail: Default::default(),
            override_tail: Default::default(),
            compacted_pages: Vec::new(),
        }
    }
}
