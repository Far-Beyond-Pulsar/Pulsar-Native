//! Branch sandboxes: a run edits a private fork of the scene and its changes
//! come back as a diff, applied (or discarded) by the user like a pull
//! request.
//!
//! Two modes share the review flow (diff, apply, discard): the **copy** fork
//! described below, and the **overlay** ([`crate::overlay`]), which records
//! the run's validated operations over the live scene instead of copying it.
//!
//! # How the copy fork works, and what SceneDB offers
//!
//! SceneDB has no world fork or snapshot-for-merge: `World` is not `Clone`,
//! its replication `Snapshot` is a network resync format that needs a
//! per-field schema for every component, and its change journals carry no
//! previous values. What it does give is typed, per-object capture and
//! restore (the same one undo history uses). A fork is therefore built the
//! straightforward way: every object of the live scene is captured with
//! [`crate::snapshot`] and re-created in a fresh private `World`, keeping
//! stable ids and component instance ids. The sandboxed run then uses the
//! very same tools on that world, so reads see its own edits and nothing
//! needs an overlay.
//!
//! The diff is computed by comparing the fork's final state with the image
//! captured when it was created. Applying it replays added / removed /
//! changed objects into the live scene as **one** transaction (revertable
//! like any other agent edit), after checking that the live scene did not
//! change the same objects in the meantime.
//!
//! Costs and limits: forking is O(scene) in time and memory (every component
//! value is cloned), so very large scenes are refused ([`MAX_FORK_OBJECTS`]).
//! The fork world has no GPU mirror and no asset subsystem; it holds scene
//! data only, which is all the tools read or write.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use pulsar_scenedb::World;
use serde_json::{json, Value};

use crate::journal::Recorder;
use crate::overlay::Overlay;
use crate::settings::SandboxMode;
use crate::snapshot::{
    apply_snapshot, apply_snapshots, capture_all, instance_equal, instance_json, snapshots_equal,
    ObjectSnapshot, SceneIndex,
};

/// Largest scene a copy sandbox forks by default.
pub const MAX_FORK_OBJECTS: usize = crate::settings::DEFAULT_COPY_MAX_OBJECTS;

/// The objects of a world at one moment, parents first.
#[derive(Clone, Default)]
pub struct SceneImage {
    pub order: Vec<String>,
    pub objects: HashMap<String, ObjectSnapshot>,
}

impl SceneImage {
    pub fn capture(world: &World) -> Self {
        let snapshots = capture_all(world);
        Self {
            order: snapshots.iter().map(|s| s.id.clone()).collect(),
            objects: snapshots.into_iter().map(|s| (s.id.clone(), s)).collect(),
        }
    }

    pub fn get(&self, id: &str) -> Option<&ObjectSnapshot> {
        self.objects.get(id)
    }

    pub fn ordered(&self) -> impl Iterator<Item = &ObjectSnapshot> {
        self.order.iter().filter_map(|id| self.objects.get(id))
    }
}

// ── Fork ─────────────────────────────────────────────────────────────────────

/// Where a sandbox keeps the run's edits.
pub enum ForkState {
    /// A private copy of the whole scene.
    Copy(World),
    /// Recorded operations over the live scene; no copy.
    Overlay(Overlay),
}

/// A sandbox: a private copy of the scene (`copy` mode) or a recorded-ops
/// overlay (`overlay` mode), and the record of what its run did.
pub struct Fork {
    pub key: String,
    pub state: ForkState,
    /// `copy`: the scene as forked. `overlay`: the objects the run touched,
    /// as they were when first touched (set when the run finishes).
    pub base: SceneImage,
    /// One line per write the run made.
    pub ops: Vec<String>,
    /// The run is over; the diff is ready for review.
    pub finished: bool,
    pub diff: Option<SceneDiff>,
    /// `(title, rendered diff)` for the review list, set when it finishes.
    pub review: Option<(String, String)>,
}

impl Fork {
    pub fn mode(&self) -> SandboxMode {
        match self.state {
            ForkState::Copy(_) => SandboxMode::Copy,
            ForkState::Overlay(_) => SandboxMode::Overlay,
        }
    }

    /// An overlay sandbox: nothing is copied.
    pub fn overlay(key: &str) -> Self {
        Self {
            key: key.to_string(),
            state: ForkState::Overlay(Overlay::new()),
            base: SceneImage::default(),
            ops: Vec::new(),
            finished: false,
            diff: None,
            review: None,
        }
    }

    /// Copy `live` into a fresh world. `max_objects` caps the scene size
    /// (0 = unlimited); a larger scene is refused before anything is copied.
    pub fn create(key: &str, live: &World, max_objects: usize) -> Result<Self, String> {
        let count = SceneIndex::build(live).len();
        if max_objects != 0 && count > max_objects {
            return Err(format!(
                "The scene has {count} objects; a copy sandbox duplicates the whole scene and is limited \
                 to {max_objects} (copy_max_objects in .pulsar/agent_scene.json, 0 = unlimited). \
                 Use the overlay sandbox mode, which copies nothing, or edit the live scene directly.",
            ));
        }
        let base = SceneImage::capture(live);
        let mut world = World::new();
        let mut ids = HashMap::with_capacity(base.order.len());
        let snapshots: Vec<ObjectSnapshot> = base.ordered().cloned().collect();
        apply_snapshots(&mut world, &mut ids, &snapshots)
            .map_err(|e| format!("Could not fork the scene: {e}"))?;
        Ok(Self {
            key: key.to_string(),
            state: ForkState::Copy(world),
            base,
            ops: Vec::new(),
            finished: false,
            diff: None,
            review: None,
        })
    }

    /// Compare a copy fork's current state with the image it started from.
    /// (An overlay computes its diff against the live scene, see
    /// [`Overlay::finish`].)
    pub fn compute_diff(&self) -> SceneDiff {
        match &self.state {
            ForkState::Copy(world) => SceneDiff::compute(&self.base, &SceneImage::capture(world)),
            ForkState::Overlay(_) => SceneDiff::default(),
        }
    }
}

// ── Diff ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ChangedObject {
    pub before: ObjectSnapshot,
    pub after: ObjectSnapshot,
    /// Human-readable change lines.
    pub changes: Vec<String>,
}

#[derive(Clone, Default)]
pub struct SceneDiff {
    /// Parents first.
    pub added: Vec<ObjectSnapshot>,
    pub removed: Vec<ObjectSnapshot>,
    pub changed: Vec<ChangedObject>,
}

impl SceneDiff {
    pub fn compute(base: &SceneImage, now: &SceneImage) -> Self {
        let mut diff = SceneDiff::default();
        for snapshot in now.ordered() {
            match base.get(&snapshot.id) {
                None => diff.added.push(snapshot.clone()),
                Some(before) => {
                    if !snapshots_equal(before, snapshot) {
                        let changes = describe_changes(before, snapshot);
                        diff.changed.push(ChangedObject {
                            before: before.clone(),
                            after: snapshot.clone(),
                            changes,
                        });
                    }
                }
            }
        }
        for snapshot in base.ordered() {
            if now.get(&snapshot.id).is_none() {
                diff.removed.push(snapshot.clone());
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        (self.added.len(), self.removed.len(), self.changed.len())
    }

    pub fn headline(&self) -> String {
        let (a, r, c) = self.counts();
        format!("+{a} added, -{r} removed, ~{c} changed")
    }

    pub fn to_json(&self) -> Value {
        json!({
            "added": self.added.iter().map(|s| json!({
                "id": s.id, "name": s.name, "kind": crate::ops::kind_of(&s.object_type),
                "parent_id": s.parent, "position": s.transform.position,
                "components": s.components.iter().map(|c| c.meta.class_name.clone()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "removed": self.removed.iter().map(|s| json!({"id": s.id, "name": s.name})).collect::<Vec<_>>(),
            "changed": self.changed.iter().map(|c| json!({
                "id": c.after.id, "name": c.after.name, "changes": c.changes,
            })).collect::<Vec<_>>(),
        })
    }

    /// The diff as text for a report or the review panel.
    pub fn render_text(&self, key: &str, ops: &[String]) -> String {
        const LIMIT: usize = 40;
        let mut out = String::new();
        let _ = writeln!(out, "Scene sandbox diff, review id {key}: {}", self.headline());
        if self.is_empty() {
            out.push_str("No changes.\n");
            return out;
        }
        if !self.added.is_empty() {
            out.push_str("Added:\n");
            for s in self.added.iter().take(LIMIT) {
                let classes: Vec<&str> =
                    s.components.iter().map(|c| c.meta.class_name.as_str()).collect();
                let _ = writeln!(
                    out,
                    "  + {} \"{}\" ({}) at {}{}{}",
                    s.id,
                    s.name,
                    crate::ops::kind_of(&s.object_type),
                    fmt_vec(&s.transform.position),
                    s.parent
                        .as_ref()
                        .map(|p| format!(" under {p}"))
                        .unwrap_or_default(),
                    if classes.is_empty() {
                        String::new()
                    } else {
                        format!(", components: {}", classes.join(", "))
                    },
                );
            }
            more(&mut out, self.added.len(), LIMIT);
        }
        if !self.removed.is_empty() {
            out.push_str("Removed:\n");
            for s in self.removed.iter().take(LIMIT) {
                let _ = writeln!(out, "  - {} \"{}\"", s.id, s.name);
            }
            more(&mut out, self.removed.len(), LIMIT);
        }
        if !self.changed.is_empty() {
            out.push_str("Changed:\n");
            for c in self.changed.iter().take(LIMIT) {
                let _ = writeln!(
                    out,
                    "  ~ {} \"{}\": {}",
                    c.after.id,
                    c.after.name,
                    c.changes.join("; ")
                );
            }
            more(&mut out, self.changed.len(), LIMIT);
        }
        if !ops.is_empty() {
            let _ = writeln!(out, "The run made {} edit(s):", ops.len());
            for op in ops.iter().take(LIMIT) {
                let _ = writeln!(out, "  * {op}");
            }
            more(&mut out, ops.len(), LIMIT);
        }
        out.push_str("Status: pending user review (apply or discard); the live scene is unchanged.\n");
        out
    }
}

fn more(out: &mut String, total: usize, limit: usize) {
    if total > limit {
        let _ = writeln!(out, "  ... and {} more", total - limit);
    }
}

fn fmt_vec(v: &[f32; 3]) -> String {
    format!("[{}, {}, {}]", short(v[0]), short(v[1]), short(v[2]))
}

fn short(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    format!("{rounded}")
}

fn clip(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 48 {
        let head: String = text.chars().take(45).collect();
        format!("{head}...")
    } else {
        text
    }
}

fn json_changes(prefix: &str, before: &Value, after: &Value, out: &mut Vec<String>) {
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => json_changes(&path, x, y, out),
                    (x, y) => out.push(format!(
                        "{path}: {} -> {}",
                        x.map(clip).unwrap_or_else(|| "(none)".into()),
                        y.map(clip).unwrap_or_else(|| "(none)".into())
                    )),
                }
            }
        }
        _ if before != after => out.push(format!("{prefix}: {} -> {}", clip(before), clip(after))),
        _ => {}
    }
}

fn describe_changes(before: &ObjectSnapshot, after: &ObjectSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    if before.name != after.name {
        lines.push(format!("name \"{}\" -> \"{}\"", before.name, after.name));
    }
    if before.object_type != after.object_type {
        lines.push(format!(
            "kind {} -> {}",
            crate::ops::kind_of(&before.object_type),
            crate::ops::kind_of(&after.object_type)
        ));
    }
    if before.transform.position != after.transform.position {
        lines.push(format!(
            "position {} -> {}",
            fmt_vec(&before.transform.position),
            fmt_vec(&after.transform.position)
        ));
    }
    if before.transform.rotation != after.transform.rotation {
        lines.push(format!(
            "rotation {} -> {}",
            fmt_vec(&before.transform.rotation),
            fmt_vec(&after.transform.rotation)
        ));
    }
    if before.transform.scale != after.transform.scale {
        lines.push(format!(
            "scale {} -> {}",
            fmt_vec(&before.transform.scale),
            fmt_vec(&after.transform.scale)
        ));
    }
    if before.visibility.visible != after.visibility.visible {
        lines.push(format!("visible {} -> {}", before.visibility.visible, after.visibility.visible));
    }
    if before.visibility.locked != after.visibility.locked {
        lines.push(format!("locked {} -> {}", before.visibility.locked, after.visibility.locked));
    }
    if before.parent != after.parent {
        lines.push(format!(
            "parent {} -> {}",
            before.parent.as_deref().unwrap_or("(root)"),
            after.parent.as_deref().unwrap_or("(root)")
        ));
    }
    if before.props != after.props {
        lines.push("props changed".to_string());
    }

    let before_ids: HashSet<_> = before.components.iter().map(|c| c.meta.id).collect();
    let after_ids: HashSet<_> = after.components.iter().map(|c| c.meta.id).collect();
    for c in &after.components {
        if !before_ids.contains(&c.meta.id) {
            lines.push(format!("+ {}", c.meta.class_name));
        }
    }
    for c in &before.components {
        if !after_ids.contains(&c.meta.id) {
            lines.push(format!("- {}", c.meta.class_name));
        }
    }
    for old in &before.components {
        let Some(new) = after.components.iter().find(|c| c.meta.id == old.meta.id) else {
            continue;
        };
        if instance_equal(old, new) {
            continue;
        }
        if old.enabled != new.enabled {
            lines.push(format!("{} enabled {} -> {}", old.meta.class_name, old.enabled, new.enabled));
        }
        let mut fields = Vec::new();
        json_changes("", &instance_json(old), &instance_json(new), &mut fields);
        let extra = fields.len().saturating_sub(6);
        for field in fields.into_iter().take(6) {
            lines.push(format!("{}.{field}", old.meta.class_name));
        }
        if extra > 0 {
            lines.push(format!("{} ... {extra} more field(s)", old.meta.class_name));
        }
    }
    let order = |c: &[pulsar_world_registry::InstanceSnapshot], keep: &HashSet<_>| -> Vec<_> {
        c.iter().map(|x| x.meta.id).filter(|id| keep.contains(id)).collect()
    };
    let shared: HashSet<_> = before_ids.intersection(&after_ids).copied().collect();
    if order(&before.components, &shared) != order(&after.components, &shared) {
        lines.push("component order changed".to_string());
    }
    if lines.is_empty() {
        lines.push("components changed".to_string());
    }
    lines
}

// ── Apply ────────────────────────────────────────────────────────────────────

/// What an applied diff did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ApplyReport {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    /// Objects already in the diff's end state, left alone.
    pub already_applied: usize,
}

/// Where the live scene no longer matches what the fork started from.
pub fn find_conflicts(live: &World, base: &SceneImage, diff: &SceneDiff) -> Vec<String> {
    let index = SceneIndex::build(live);
    let removed_ids: HashSet<&str> = diff.removed.iter().map(|s| s.id.as_str()).collect();
    let added_ids: HashSet<&str> = diff.added.iter().map(|s| s.id.as_str()).collect();
    let mut conflicts = Vec::new();
    let live_snapshot = |id: &str| {
        index
            .entity(id)
            .and_then(|e| crate::snapshot::snapshot_object(live, &index, e))
    };

    for removed in &diff.removed {
        let Some(entity) = index.entity(&removed.id) else {
            continue;
        };
        if let (Some(now), Some(then)) = (live_snapshot(&removed.id), base.get(&removed.id)) {
            if !snapshots_equal(&now, then) {
                conflicts.push(format!(
                    "'{}' was removed in the sandbox but changed in the live scene since",
                    removed.id
                ));
            }
        }
        for child in index.children(Some(entity)) {
            if let Some(child_id) = index.id_of(*child) {
                if !removed_ids.contains(child_id) {
                    conflicts.push(format!(
                        "'{}' was removed in the sandbox but '{child_id}' was added under it in the live scene",
                        removed.id
                    ));
                }
            }
        }
    }
    for added in &diff.added {
        if index.entity(&added.id).is_some() {
            conflicts.push(format!(
                "'{}' was added in the sandbox but the live scene now has an object with that id",
                added.id
            ));
        }
        if let Some(parent) = &added.parent {
            if index.entity(parent).is_none() && !added_ids.contains(parent.as_str()) {
                conflicts.push(format!(
                    "'{}' was added under '{parent}', which no longer exists in the live scene",
                    added.id
                ));
            }
        }
    }
    for changed in &diff.changed {
        match live_snapshot(&changed.after.id) {
            None => conflicts.push(format!(
                "'{}' was changed in the sandbox but removed from the live scene since",
                changed.after.id
            )),
            Some(now) => {
                if !snapshots_equal(&now, &changed.before) && !snapshots_equal(&now, &changed.after) {
                    conflicts.push(format!(
                        "'{}' was changed in the sandbox and also in the live scene since",
                        changed.after.id
                    ));
                }
                if let Some(parent) = &changed.after.parent {
                    if index.entity(parent).is_none() && !added_ids.contains(parent.as_str()) {
                        conflicts.push(format!(
                            "'{}' was moved under '{parent}', which no longer exists in the live scene",
                            changed.after.id
                        ));
                    }
                }
            }
        }
    }
    conflicts
}

/// Replay `diff` into `live`: removals, then additions (parents first), then
/// changes. All writes are recorded in `rec`, so the caller commits them as
/// one transaction. With `force` the conflicts from [`find_conflicts`] are
/// ignored and the sandbox's state wins; otherwise any conflict refuses the
/// whole apply before anything is written.
pub fn apply_diff(
    live: &mut World,
    rec: &mut Recorder,
    base: &SceneImage,
    diff: &SceneDiff,
    force: bool,
) -> Result<ApplyReport, String> {
    if !force {
        let conflicts = find_conflicts(live, base, diff);
        if !conflicts.is_empty() {
            return Err(format!(
                "The live scene changed since the sandbox was created, so the diff was not applied: {}.",
                conflicts.join("; ")
            ));
        }
    }
    let mut report = ApplyReport::default();
    let index = SceneIndex::build(live);
    let mut ids: HashMap<String, pulsar_scenedb::Entity> = index
        .all_ordered()
        .into_iter()
        .filter_map(|e| index.id_of(e).map(|id| (id.to_string(), e)))
        .collect();

    // Removals: top-most removed objects take their subtree with them.
    let removed_ids: HashSet<&str> = diff.removed.iter().map(|s| s.id.as_str()).collect();
    for removed in &diff.removed {
        if removed.parent.as_deref().is_some_and(|p| removed_ids.contains(p)) {
            continue;
        }
        let Some(entity) = index.entity(&removed.id) else {
            continue;
        };
        rec.note_subtree(live, &index, &removed.id);
        report.removed += index.subtree(entity).len();
        for gone in index.subtree(entity) {
            if let Some(id) = index.id_of(gone) {
                ids.remove(id);
            }
        }
        pulsar_scene_model::SceneWorldExt::despawn_tree(live, entity);
    }

    for added in &diff.added {
        rec.note_new(&added.id);
        apply_snapshot(live, &mut ids, added)?;
        report.added += 1;
    }

    for changed in &diff.changed {
        let id = &changed.after.id;
        let Some(&entity) = ids.get(id) else {
            continue;
        };
        if let Some(now) = crate::snapshot::snapshot_object(live, &SceneIndex::build(live), entity) {
            if snapshots_equal(&now, &changed.after) {
                report.already_applied += 1;
                continue;
            }
        }
        let current = SceneIndex::build(live);
        rec.note_existing(live, &current, id);
        apply_snapshot(live, &mut ids, &changed.after)?;
        report.changed += 1;
    }
    Ok(report)
}
