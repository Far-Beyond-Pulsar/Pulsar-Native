//! Terrain edit seam — the editor's only door into planetary voxel terrain.
//!
//! The level editor cannot reach `pulsar_terrain` on its own: the canonical
//! [`TerrainRuntimeHandle`] is created on the render thread, inside
//! `HelioInner::planet_terrain`. This module is the narrow, `Clone`-able seam
//! the design doc calls for (`level-editor-tool-modes.md` §5.4): authoring
//! front-ends hold a [`TerrainEditApi`], never a raw runtime handle, and every
//! terrain type they need is re-exported from here so no UI crate has to take
//! a direct `pulsar_terrain` dependency.
//!
//! ## Threading
//!
//! Two different mechanisms, deliberately:
//!
//! * **Terrain mutations and reads go straight through [`TerrainRuntimeHandle`].**
//!   That handle is `Clone` and internally an `Arc<Mutex<RuntimeState>>` — it
//!   is the runtime's own designed cross-thread surface, and every accessor
//!   takes the runtime's lock for its own duration only. Routing edits through
//!   a frame-boundary mailbox instead would buy nothing and would cost the
//!   caller the thing it actually needs synchronously: the allocated mutation
//!   sequence, and a yes/no answer for `Consumed` vs `PassThrough`.
//! * **Anything that touches the *render thread* goes through
//!   [`TerrainEditMailbox`]**, matching `HelioEditorMailbox`'s existing
//!   convention exactly (small `Arc<Mutex<..>>` / `Arc<AtomicBool>` slots,
//!   written by the UI thread, drained at the render-thread frame boundary,
//!   never a `gpu_engine.lock()`). That covers publishing the runtime handle
//!   outward, the brush cursor to draw, and the "terrain changed, advance the
//!   planet even though the camera is still" flag.
//!
//! No new ad-hoc locking is introduced by this module.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use thiserror::Error;

// The terrain vocabulary UI crates need. Re-exported rather than made a
// direct dependency of every front-end: the seam is the boundary, so the
// boundary owns the types that cross it. `ContentHash`/`NodeState`/
// `SparseBrickTree` are here because they are `TerrainSnapshot`'s own fields --
// holding a snapshot is meaningless without them.
pub use pulsar_terrain::{
    CellWord, ContentHash, EditMode, EditOp, EditShape, FlatTerrain, MaterialId, NodeState,
    PlanetDefinition, PlanetId, PlanetIdParseError, SparseBrickTree, TerrainBodyDefinition,
    TerrainRuntimeError, TerrainShape, TerrainSnapshot, VolumeDefinition, VolumeDefinitionError,
    VolumeId, LOD0_CELL_SIZE_METERS,
};
use pulsar_terrain::TerrainRuntimeHandle;
use std::collections::BTreeMap;

/// Cells sampled when refining an analytic sphere hit onto sculpted geometry.
///
/// Every sample replays the planet's edit tail (see
/// [`TerrainRuntimeHandle::sample_cells`]), so this is a hard budget, not a
/// target. It is spent as a symmetric march around the analytic intersection.
const REFINE_SAMPLES: usize = 64;

/// Half-width of the refinement march, in LOD0 cells (64 cells = 6.4 m).
///
/// Sculpting displaces the surface away from the generator's analytic sphere;
/// this is how far from that sphere a hit is still recognised. It bounds how
/// tall a raised feature can get before the brush stops climbing it.
const REFINE_HALF_SPAN_CELLS: i64 = 64;

/// Monotonic source for [`EditOp::stable_id`] uniqueness within a process.
static NEXT_STAMP_NONCE: AtomicU64 = AtomicU64::new(1);

// ── Geometry ───────────────────────────────────────────────────────────────

/// A world-space picking ray, in meters. Direction need not be normalized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray3 {
    pub origin: [f32; 3],
    pub direction: [f32; 3],
}

/// Which terrain body an edit addresses.
///
/// Both arms resolve to one body identity through [`Self::body_id`], which is
/// the whole point: every operation on this seam takes a `TerrainTarget` and
/// none of them branch on which arm it is. Only hit-testing geometry — ray
/// against a sphere or against a box — ever cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainTarget {
    Planet(PlanetId),
    Volume(VolumeId),
}

impl TerrainTarget {
    /// The identity the runtime registered this body under.
    pub fn body_id(self) -> PlanetId {
        match self {
            Self::Planet(id) => id,
            Self::Volume(id) => id.body_id(),
        }
    }

    /// The target addressing a registered body definition.
    pub fn of(definition: &TerrainBodyDefinition) -> Self {
        match definition {
            TerrainBodyDefinition::Planet(planet) => Self::Planet(planet.planet_id),
            TerrainBodyDefinition::Volume(volume) => Self::Volume(volume.volume_id),
        }
    }

    /// Hex form of the body identity, for display and for the editor domain's
    /// string-keyed target.
    pub fn to_hex(self) -> String {
        self.body_id().to_hex()
    }
}

/// Where a ray met terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainHit {
    pub target: TerrainTarget,
    /// Hit point in world meters.
    pub position_m: [f32; 3],
    /// Canonical LOD0 cell containing the hit point.
    pub cell: [i64; 3],
    /// Outward surface normal (the planet's radial direction at the hit).
    pub normal: [f32; 3],
    /// Material of the cell at the hit, or 0 when the hit is the analytic
    /// sphere rather than a refined solid cell.
    pub material: MaterialId,
    /// Distance from the ray origin, in meters.
    pub distance_m: f32,
}

/// A brush ring for the render thread to draw as transient debug geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushCursorRequest {
    pub center_m: [f32; 3],
    /// Ring plane normal — the terrain normal at the brush centre.
    pub normal: [f32; 3],
    pub radius_m: f32,
    pub color: [f32; 4],
}

// ── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum TerrainEditError {
    /// The render thread has not published a terrain runtime. Today this is
    /// the normal state in the level editor — see [`TerrainEditApi`]'s note on
    /// planet activation.
    #[error("no terrain runtime is active")]
    NoRuntime,
    #[error("planet {0:?} is not registered with the terrain runtime")]
    PlanetMissing(PlanetId),
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
    #[error("stored terrain edit log could not be decoded: {0}")]
    EditCodec(pulsar_terrain::EditError),
    /// Planet *creation* is authored by adding a `PlanetTerrainComponent` to a
    /// scene object, not by calling the edit seam. See
    /// [`TerrainEditApi::create_planet`].
    #[error("planets are created by scene components, not through the edit seam")]
    CreationUnsupported,
    #[error("invalid flat volume definition: {0}")]
    InvalidVolume(#[from] VolumeDefinitionError),
}

// ── Render-thread mailbox ──────────────────────────────────────────────────

/// Frame-boundary mailbox shared between the UI thread and the render thread.
///
/// Mirrors `HelioEditorMailbox`: every field is its own small `Arc`-ed slot,
/// so no path here can ever block on the render thread's per-frame
/// `gpu_engine` lock.
#[derive(Clone, Default)]
pub struct TerrainEditMailbox {
    /// Published by the render thread once it owns a terrain runtime; read by
    /// the UI thread on every edit. `None` until a planet exists.
    runtime: Arc<Mutex<Option<TerrainRuntimeHandle>>>,
    /// Latest-wins: the full set of terrain bodies the editor currently wants
    /// live, each keyed by the stable source that authored it. The render
    /// thread upserts them and drops any source that disappeared.
    pending_bodies: Arc<Mutex<Option<Vec<(String, TerrainBodyDefinition)>>>>,
    /// Latest-wins: the brush ring to draw, or `None` to draw nothing.
    brush_cursor: Arc<Mutex<Option<BrushCursorRequest>>>,
    /// Set after a mutation so the render thread advances planet streaming
    /// even when the camera is stationary (the idle path would otherwise skip
    /// the frame and the edit would not appear until the user moved).
    pending_advance: Arc<AtomicBool>,
}

impl TerrainEditMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Render-thread side ─────────────────────────────────────────────

    /// Publish (or retract) the runtime handle the UI thread should edit.
    pub fn publish_runtime(&self, handle: Option<TerrainRuntimeHandle>) {
        if let Ok(mut slot) = self.runtime.lock() {
            *slot = handle;
        }
    }

    /// Take the editor's terrain body set, if the UI thread posted a new one.
    pub fn take_pending_bodies(&self) -> Option<Vec<(String, TerrainBodyDefinition)>> {
        self.pending_bodies.lock().ok().and_then(|mut s| s.take())
    }

    /// The brush ring to draw this frame, if any.
    pub fn brush_cursor(&self) -> Option<BrushCursorRequest> {
        self.brush_cursor.lock().ok().and_then(|slot| *slot)
    }

    /// Consume the "terrain changed" flag.
    pub fn take_pending_advance(&self) -> bool {
        self.pending_advance.swap(false, Ordering::AcqRel)
    }

    /// Non-consuming read, for the renderer's idle check.
    pub fn wants_advance(&self) -> bool {
        self.pending_advance.load(Ordering::Acquire)
    }
}

// ── TerrainEditApi ─────────────────────────────────────────────────────────

/// The editor-facing terrain edit surface (design doc §5.4).
///
/// ## Planet activation, as of this milestone
///
/// `HelioInner::planet_terrain` is populated from the scene's
/// `PlanetTerrainComponent`s. The generic world-component dispatch that used
/// to drive that was removed by the in-flight SceneDB nativization work, so
/// this seam re-supplies it narrowly: the editor posts the planet definitions
/// it finds in its own scene database through [`Self::sync_scene_planets`],
/// and the render thread upserts them into the runtime. Nothing here depends
/// on the removed generic dispatch path.
#[derive(Clone, Default)]
pub struct TerrainEditApi {
    mailbox: TerrainEditMailbox,
    /// Flat volumes created through [`Self::create_volume`], keyed by source.
    ///
    /// A planet exists because a scene component says so, and the scene is
    /// re-collected on every revision. A volume has no scene component yet
    /// (see [`Self::create_volume`]), so the seam has to remember it — without
    /// this, the next scene sync would post a body set that does not contain
    /// the volume and the render thread would dutifully retire it.
    authored_volumes: Arc<Mutex<BTreeMap<String, VolumeDefinition>>>,
}

impl TerrainEditApi {
    pub fn new(mailbox: TerrainEditMailbox) -> Self {
        Self {
            mailbox,
            authored_volumes: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Whether a terrain runtime is live. `false` means there is no planet in
    /// the scene (or the renderer has not initialized yet), and every sculpt
    /// operation will no-op.
    pub fn is_active(&self) -> bool {
        self.with_runtime(|_| ()).is_some()
    }

    fn with_runtime<R>(&self, f: impl FnOnce(&TerrainRuntimeHandle) -> R) -> Option<R> {
        let guard = self.mailbox.runtime.lock().ok()?;
        guard.as_ref().map(f)
    }

    /// Every terrain body currently registered with the runtime — planets and
    /// flat volumes alike.
    ///
    /// This is what authoring code should read: it hit-tests, sculpts and
    /// saves whatever terrain the level has, and the shape only decides which
    /// analytic intersection runs inside this module.
    pub fn bodies(&self) -> Vec<TerrainBodyDefinition> {
        self.with_runtime(|runtime| runtime.body_definitions())
            .unwrap_or_default()
    }

    /// Every planet currently registered with the runtime.
    pub fn planets(&self) -> Vec<PlanetDefinition> {
        self.with_runtime(|runtime| runtime.planet_definitions())
            .unwrap_or_default()
    }

    /// The definition backing a target, if it is still registered.
    pub fn body_definition(&self, target: TerrainTarget) -> Option<TerrainBodyDefinition> {
        self.with_runtime(|runtime| runtime.body_definition(target.body_id()))
            .flatten()
    }

    /// The body a sculpt should default to when the user has not picked one.
    pub fn default_target(&self) -> Option<TerrainTarget> {
        self.bodies().first().map(TerrainTarget::of)
    }

    /// Post the full set of terrain bodies the scene defines, each paired with
    /// the stable key of the scene component that authored it. Latest wins;
    /// source keys absent from `definitions` are retired on the render thread.
    ///
    /// Volumes created through [`Self::create_volume`] are merged in here, so
    /// a scene revision cannot retire one the scene does not know about.
    pub fn sync_scene_planets(&self, definitions: Vec<(String, PlanetDefinition)>) {
        self.sync_scene_bodies(
            definitions
                .into_iter()
                .map(|(key, definition)| (key, TerrainBodyDefinition::Planet(definition)))
                .collect(),
        );
    }

    /// Post the scene's bodies, merged with every volume this session created.
    pub fn sync_scene_bodies(&self, mut definitions: Vec<(String, TerrainBodyDefinition)>) {
        if let Ok(authored) = self.authored_volumes.lock() {
            for (source_key, volume) in authored.iter() {
                if definitions.iter().any(|(key, _)| key == source_key) {
                    continue;
                }
                definitions.push((
                    source_key.clone(),
                    TerrainBodyDefinition::Volume(*volume),
                ));
            }
        }
        if let Ok(mut slot) = self.mailbox.pending_bodies.lock() {
            *slot = Some(definitions);
        }
    }

    /// Planet creation is *scene authoring*, not terrain editing: a planet
    /// exists because some scene object carries a `PlanetTerrainComponent`,
    /// and that component is what persists into the `.level` file. Creating
    /// one here would register a planet the scene does not know about, which
    /// would vanish on reload.
    ///
    /// Deliberately unimplemented for Milestone 2 (issue #711 explicitly
    /// allows this); the editor-side "create planet" affordance that would
    /// author the component is follow-up work.
    pub fn create_planet(&self, _definition: PlanetDefinition) -> Result<PlanetId, TerrainEditError> {
        Err(TerrainEditError::CreationUnsupported)
    }

    /// Create a flat voxel world and make it live.
    ///
    /// Unlike a planet, a volume has no scene component authoring it, so this
    /// *is* the creation door: the definition is remembered here and posted to
    /// the render thread with the next body set, which registers it in the
    /// terrain runtime exactly like a planet. From that moment the volume is
    /// an ordinary target — the same hit test, the same `EditOp`s, the same
    /// snapshots, the same sidecar.
    ///
    /// The definition is validated up front so a bad extent or a non-canonical
    /// cell size fails here, at the call site that can report it, rather than
    /// silently on the render thread a frame later.
    ///
    /// Persistence caveat: because there is no component, the volume's
    /// *definition* lives for the session only — the sculpt on it persists via
    /// the terrain sidecar, but reopening the level does not re-create the
    /// world. Authoring a scene component for volumes is follow-up work.
    pub fn create_volume(
        &self,
        definition: VolumeDefinition,
    ) -> Result<VolumeId, TerrainEditError> {
        definition.validate()?;
        let volume_id = definition.volume_id;
        let source_key = volume_source_key(volume_id);
        if let Ok(mut authored) = self.authored_volumes.lock() {
            authored.insert(source_key, definition);
        }
        // Push the merged set immediately: the caller expects the world to
        // exist now, not at the next scene revision.
        self.sync_scene_bodies(Vec::new());
        self.mark_dirty();
        Ok(volume_id)
    }

    /// Flat worlds created this session, in creation order.
    pub fn authored_volumes(&self) -> Vec<VolumeDefinition> {
        self.authored_volumes
            .lock()
            .map(|authored| authored.values().copied().collect())
            .unwrap_or_default()
    }

    /// Forget every volume this session created. Called when a different level
    /// is opened, so a flat world does not follow the user into it.
    pub fn clear_authored_volumes(&self) {
        if let Ok(mut authored) = self.authored_volumes.lock() {
            authored.clear();
        }
    }

    // ── Hit testing ────────────────────────────────────────────────────

    /// Intersect a world-space ray with the active terrain.
    ///
    /// Two stages: an exact analytic intersection against the body's canonical
    /// surface — a sphere for a planet, the solid box for a flat volume — then
    /// a bounded march along the ray that snaps the hit onto sculpted geometry
    /// (raised or carved away from that surface). The march costs at most
    /// [`REFINE_SAMPLES`] canonical cell evaluations, so this is cheap enough
    /// to run per pointer event.
    ///
    /// This is the one place shape genuinely matters, which is why the issue's
    /// acceptance bar allows it here and nowhere above.
    pub fn hit_terrain(&self, ray: Ray3) -> Option<TerrainHit> {
        let direction = normalize(ray.direction)?;
        let origin = [
            f64::from(ray.origin[0]),
            f64::from(ray.origin[1]),
            f64::from(ray.origin[2]),
        ];
        let direction = [
            f64::from(direction[0]),
            f64::from(direction[1]),
            f64::from(direction[2]),
        ];

        let mut best: Option<(AnalyticHit, TerrainBodyDefinition)> = None;
        for definition in self.bodies() {
            let Some(hit) = intersect_body(origin, direction, &definition) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(closest, _)| hit.distance_m < closest.distance_m)
            {
                best = Some((hit, definition));
            }
        }
        let (analytic, definition) = best?;
        Some(self.refine_hit(origin, direction, analytic, &definition))
    }

    /// Snap an analytic surface intersection onto the sculpted surface by
    /// marching the canonical density field around it.
    ///
    /// Shape-blind: the march reads the same density field for either body,
    /// and only the surface normal is recomputed per shape.
    fn refine_hit(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        analytic: AnalyticHit,
        definition: &TerrainBodyDefinition,
    ) -> TerrainHit {
        let body_id = definition.body_id();
        let analytic_distance = analytic.distance_m;
        let step_m = (2.0 * REFINE_HALF_SPAN_CELLS as f64 * LOD0_CELL_SIZE_METERS)
            / REFINE_SAMPLES as f64;
        let start_m = analytic_distance - REFINE_HALF_SPAN_CELLS as f64 * LOD0_CELL_SIZE_METERS;

        let distances: Vec<f64> = (0..REFINE_SAMPLES)
            .map(|index| start_m + step_m * index as f64)
            .filter(|distance| *distance > 0.0)
            .collect();
        let cells: Vec<[i64; 3]> = distances
            .iter()
            .map(|distance| meters_to_cell(point_at(origin, direction, *distance)))
            .collect();

        let refined = self
            .with_runtime(|runtime| runtime.sample_cells(body_id, &cells))
            .flatten()
            .and_then(|samples| {
                // First sample along the ray that is inside solid terrain.
                samples
                    .iter()
                    .position(|cell| cell.density() <= 0)
                    .map(|index| (distances[index], cells[index], samples[index].material()))
            });

        let (distance, cell, material) = refined.unwrap_or_else(|| {
            let point = point_at(origin, direction, analytic_distance);
            (
                analytic_distance,
                meters_to_cell(point),
                definition.material(),
            )
        });

        let point = point_at(origin, direction, distance);
        let normal = match definition {
            // A planet's outward direction is radial, recomputed at the
            // refined point so a sculpted slope still reads correctly.
            TerrainBodyDefinition::Planet(planet) => {
                let center_m = cell_to_meters(planet.center_cell);
                normalize([
                    (point[0] - center_m[0]) as f32,
                    (point[1] - center_m[1]) as f32,
                    (point[2] - center_m[2]) as f32,
                ])
                .unwrap_or([0.0, 1.0, 0.0])
            }
            // A box face's normal is constant, so the analytic one is exact.
            TerrainBodyDefinition::Volume(_) => analytic.normal,
        };

        TerrainHit {
            target: TerrainTarget::of(definition),
            position_m: [point[0] as f32, point[1] as f32, point[2] as f32],
            cell,
            normal,
            material,
            distance_m: distance as f32,
        }
    }

    // ── Mutation ───────────────────────────────────────────────────────

    /// Append one edit to a planet's canonical mutation log.
    ///
    /// `op`'s `sequence` and `stable_id` are *rewritten* here: the runtime
    /// rejects any operation that does not strictly advance the planet's
    /// sequence, and only the runtime knows the current value. Callers build
    /// the shape/mode/material and let the seam own ordering. The committed
    /// operation is returned so undo can record exactly what was applied.
    pub fn apply_edit(
        &self,
        target: TerrainTarget,
        op: EditOp,
    ) -> Result<EditOp, TerrainEditError> {
        let planet_id = target.body_id();
        let committed = self
            .with_runtime(|runtime| {
                let latest = runtime
                    .latest_sequence(planet_id)
                    .ok_or(TerrainEditError::PlanetMissing(planet_id))?;
                let sequence = latest.saturating_add(1);
                let nonce = NEXT_STAMP_NONCE.fetch_add(1, Ordering::Relaxed);
                let mut stable_id = [0_u8; 16];
                stable_id[..8].copy_from_slice(&sequence.to_le_bytes());
                stable_id[8..].copy_from_slice(&nonce.to_le_bytes());
                let committed = EditOp {
                    sequence,
                    stable_id,
                    ..op
                };
                runtime.append_edit(planet_id, committed)?;
                Ok::<EditOp, TerrainEditError>(committed)
            })
            .ok_or(TerrainEditError::NoRuntime)??;
        self.mark_dirty();
        Ok(committed)
    }

    /// Encode a planet's canonical mutation log for durable storage.
    ///
    /// The edit log *is* the authored terrain: a planet's surface is its
    /// deterministic generator with this log replayed on top, so persisting
    /// the log persists the sculpt exactly, at a fraction of a full page
    /// snapshot's size. Returns `None` when the planet is not registered.
    pub fn export_edits(&self, target: TerrainTarget) -> Option<Vec<u8>> {
        self.snapshot(target)
            .map(|snapshot| snapshot.edit_tail.encode())
    }

    /// Replay a previously exported mutation log onto a planet.
    ///
    /// Sequence numbers are reallocated by [`Self::apply_edit`], so a log
    /// exported from one session replays cleanly into a freshly generated
    /// planet in the next. Returns how many operations were applied.
    pub fn import_edits(
        &self,
        target: TerrainTarget,
        encoded: &[u8],
    ) -> Result<usize, TerrainEditError> {
        let log = pulsar_terrain::EditLog::decode(encoded).map_err(TerrainEditError::EditCodec)?;
        let mut applied = 0;
        for operation in log.operations() {
            self.apply_edit(target, *operation)?;
            applied += 1;
        }
        Ok(applied)
    }

    /// Capture a planet's canonical state, for use as an undo anchor.
    pub fn snapshot(&self, target: TerrainTarget) -> Option<TerrainSnapshot> {
        self.with_runtime(|runtime| runtime.planet_snapshot(target.body_id()))
            .flatten()
    }

    /// Restore a previously captured snapshot, rewinding the mutation tail.
    pub fn restore(
        &self,
        target: TerrainTarget,
        snapshot: TerrainSnapshot,
    ) -> Result<(), TerrainEditError> {
        self.with_runtime(|runtime| runtime.restore_planet_snapshot(target.body_id(), snapshot))
            .ok_or(TerrainEditError::NoRuntime)??;
        self.mark_dirty();
        Ok(())
    }

    /// Ask the render thread to advance planet streaming next frame even if
    /// the camera has not moved.
    pub fn mark_dirty(&self) {
        self.mailbox.pending_advance.store(true, Ordering::Release);
    }

    // ── Brush cursor ───────────────────────────────────────────────────

    /// Set (or clear) the brush ring the render thread draws each frame.
    pub fn set_brush_cursor(&self, cursor: Option<BrushCursorRequest>) {
        if let Ok(mut slot) = self.mailbox.brush_cursor.lock() {
            if *slot != cursor {
                *slot = cursor;
                // The ring is transient debug geometry rebuilt every frame, so
                // a change only becomes visible if a frame actually runs.
                self.mailbox.pending_advance.store(true, Ordering::Release);
            }
        }
    }
}

// ── Math helpers ───────────────────────────────────────────────────────────

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (length > f32::EPSILON).then(|| [v[0] / length, v[1] / length, v[2] / length])
}

fn point_at(origin: [f64; 3], direction: [f64; 3], distance: f64) -> [f64; 3] {
    [
        origin[0] + direction[0] * distance,
        origin[1] + direction[1] * distance,
        origin[2] + direction[2] * distance,
    ]
}

/// Canonical LOD0 cell containing a world-meter point.
pub fn meters_to_cell(point: [f64; 3]) -> [i64; 3] {
    [
        (point[0] / LOD0_CELL_SIZE_METERS).floor() as i64,
        (point[1] / LOD0_CELL_SIZE_METERS).floor() as i64,
        (point[2] / LOD0_CELL_SIZE_METERS).floor() as i64,
    ]
}

/// World-meter position of a canonical LOD0 cell's origin corner.
pub fn cell_to_meters(cell: [i64; 3]) -> [f64; 3] {
    [
        cell[0] as f64 * LOD0_CELL_SIZE_METERS,
        cell[1] as f64 * LOD0_CELL_SIZE_METERS,
        cell[2] as f64 * LOD0_CELL_SIZE_METERS,
    ]
}

/// Meters converted to a canonical cell count, rounded up and clamped to at
/// least one cell so a brush is never degenerate.
pub fn meters_to_radius_cells(meters: f32) -> u32 {
    let cells = (f64::from(meters.max(0.0)) / LOD0_CELL_SIZE_METERS).ceil();
    cells.clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Stable source key for a volume the seam authored.
fn volume_source_key(volume_id: VolumeId) -> String {
    format!("volume:{}", volume_id.to_hex())
}

/// The analytic (pre-refinement) intersection of a ray with a body's canonical
/// surface.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AnalyticHit {
    distance_m: f64,
    normal: [f32; 3],
}

/// Intersect a ray with a body's canonical surface, whatever its shape.
fn intersect_body(
    origin: [f64; 3],
    direction: [f64; 3],
    definition: &TerrainBodyDefinition,
) -> Option<AnalyticHit> {
    match definition {
        TerrainBodyDefinition::Planet(planet) => {
            let distance_m = intersect_planet(origin, direction, planet)?;
            let point = point_at(origin, direction, distance_m);
            let center = cell_to_meters(planet.center_cell);
            let normal = normalize([
                (point[0] - center[0]) as f32,
                (point[1] - center[1]) as f32,
                (point[2] - center[2]) as f32,
            ])
            .unwrap_or([0.0, 1.0, 0.0]);
            Some(AnalyticHit { distance_m, normal })
        }
        TerrainBodyDefinition::Volume(volume) => intersect_volume(origin, direction, volume),
    }
}

/// Smallest positive ray distance at which the ray meets a flat world's solid
/// region, plus the face normal there.
///
/// The solid region is the volume's box clipped to its ground plane, so this
/// is an ordinary slab test against that AABB. Looking down at a flat world
/// from above hits the top face and yields a `+Y` normal, which is what makes
/// a sculpt brush behave the way the user expects.
fn intersect_volume(
    origin: [f64; 3],
    direction: [f64; 3],
    definition: &VolumeDefinition,
) -> Option<AnalyticHit> {
    let (min_cell, max_cell) = definition.flat.cell_bounds();
    let min = cell_to_meters(min_cell);
    // The ground plane is the *top* of the origin cell, and the box's far
    // corner is the far side of its last cell, hence the one-cell extension.
    let max = cell_to_meters([
        max_cell[0] + 1,
        definition.flat.origin[1] + 1,
        max_cell[2] + 1,
    ]);
    if (0..3).any(|axis| max[axis] <= min[axis]) {
        return None;
    }

    let mut enter = f64::NEG_INFINITY;
    let mut exit = f64::INFINITY;
    // Faces are recorded as (axis, outward sign) so the hit carries a real
    // surface normal rather than a guess.
    let mut enter_face = (1_usize, 1.0_f64);
    let mut exit_face = (1_usize, 1.0_f64);
    for axis in 0..3 {
        if direction[axis].abs() < 1e-12 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let inverse = 1.0 / direction[axis];
        let near_at_min = direction[axis] > 0.0;
        let mut near = (min[axis] - origin[axis]) * inverse;
        let mut far = (max[axis] - origin[axis]) * inverse;
        if !near_at_min {
            std::mem::swap(&mut near, &mut far);
        }
        // Entering through the min face means the outward normal is -axis.
        let near_sign = if near_at_min { -1.0 } else { 1.0 };
        if near > enter {
            enter = near;
            enter_face = (axis, near_sign);
        }
        if far < exit {
            exit = far;
            exit_face = (axis, -near_sign);
        }
        if enter > exit {
            return None;
        }
    }

    // A ray starting inside the solid region still needs a surface to work
    // against, so fall through to the exit face exactly as the planet path
    // falls through to the far root.
    let (distance_m, (axis, sign)) = if enter > 0.0 {
        (enter, enter_face)
    } else if exit > 0.0 {
        (exit, exit_face)
    } else {
        return None;
    };

    let mut normal = [0.0_f32; 3];
    normal[axis] = sign as f32;
    Some(AnalyticHit { distance_m, normal })
}

/// Smallest positive ray distance at which the ray meets the planet's
/// canonical sphere, or `None` when it misses entirely.
///
/// When the ray origin is *inside* the planet the near root is behind the
/// camera; the far root is returned so sculpting from underground still has a
/// surface to work against.
fn intersect_planet(
    origin: [f64; 3],
    direction: [f64; 3],
    definition: &PlanetDefinition,
) -> Option<f64> {
    let center = cell_to_meters(definition.center_cell);
    let radius = definition.radius_cells as f64 * LOD0_CELL_SIZE_METERS;
    let offset = [
        origin[0] - center[0],
        origin[1] - center[1],
        origin[2] - center[2],
    ];
    // |offset + t*direction|^2 = radius^2, with `direction` normalized so the
    // quadratic's leading coefficient is 1.
    let half_b = offset[0] * direction[0] + offset[1] * direction[1] + offset[2] * direction[2];
    let c = offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]
        - radius * radius;
    let discriminant = half_b * half_b - c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let near = -half_b - root;
    let far = -half_b + root;
    [near, far]
        .into_iter()
        .find(|distance| *distance > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_planet() -> PlanetDefinition {
        PlanetDefinition {
            planet_id: PlanetId::from_stable_name("test"),
            center_cell: [0; 3],
            // 100 m radius.
            radius_cells: 1_000,
            material: 1,
            root_lod: 12,
            max_resident_pages: 64,
        }
    }

    #[test]
    fn a_ray_aimed_at_the_planet_centre_hits_the_near_surface() {
        let definition = test_planet();
        let distance =
            intersect_planet([0.0, 0.0, 300.0], [0.0, 0.0, -1.0], &definition).unwrap();
        assert!((distance - 200.0).abs() < 1e-6, "got {distance}");
    }

    #[test]
    fn a_ray_from_inside_the_planet_hits_the_far_surface() {
        let definition = test_planet();
        let distance = intersect_planet([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], &definition).unwrap();
        assert!((distance - 100.0).abs() < 1e-6, "got {distance}");
    }

    #[test]
    fn a_ray_pointing_away_from_the_planet_misses() {
        let definition = test_planet();
        assert!(intersect_planet([0.0, 0.0, 300.0], [0.0, 0.0, 1.0], &definition).is_none());
    }

    #[test]
    fn cell_and_meter_conversions_round_trip_at_cell_resolution() {
        assert_eq!(meters_to_cell([0.35, -0.05, 12.0]), [3, -1, 120]);
        assert_eq!(cell_to_meters([3, -1, 120]), [0.30000000000000004, -0.1, 12.0]);
    }

    #[test]
    fn a_brush_radius_is_never_degenerate() {
        assert_eq!(meters_to_radius_cells(0.0), 1);
        assert_eq!(meters_to_radius_cells(-5.0), 1);
        assert_eq!(meters_to_radius_cells(8.0), 80);
    }

    #[test]
    fn an_api_with_no_runtime_reports_inactive_and_refuses_edits() {
        let api = TerrainEditApi::default();
        assert!(!api.is_active());
        assert!(api.planets().is_empty());
        assert!(api.default_target().is_none());
        assert!(api
            .hit_terrain(Ray3 {
                origin: [0.0; 3],
                direction: [0.0, 0.0, -1.0],
            })
            .is_none());
        let error = api
            .apply_edit(
                TerrainTarget::Planet(PlanetId::from_stable_name("test")),
                EditOp {
                    sequence: 0,
                    stable_id: [0; 16],
                    shape: EditShape::Sphere {
                        center_cell: [0; 3],
                        radius_cells: 4,
                    },
                    mode: EditMode::Union,
                    material: 1,
                },
            )
            .unwrap_err();
        assert!(matches!(error, TerrainEditError::NoRuntime));
    }

    #[test]
    fn planet_creation_is_explicitly_out_of_scope_rather_than_silently_ignored() {
        let api = TerrainEditApi::default();
        assert!(matches!(
            api.create_planet(test_planet()).unwrap_err(),
            TerrainEditError::CreationUnsupported
        ));
    }

    #[test]
    fn the_mailbox_hands_the_render_thread_the_latest_planet_set_once() {
        let mailbox = TerrainEditMailbox::new();
        let api = TerrainEditApi::new(mailbox.clone());
        api.sync_scene_planets(vec![("earth:0".to_string(), test_planet())]);
        assert_eq!(mailbox.take_pending_bodies().unwrap().len(), 1);
        assert!(mailbox.take_pending_bodies().is_none());
    }

    fn test_volume() -> VolumeDefinition {
        VolumeDefinition {
            volume_id: VolumeId::from_stable_name("flat"),
            flat: FlatTerrain::centered_on([0, 0, 0]),
            material: 1,
            root_lod: 12,
            max_resident_pages: 4_096,
        }
    }

    #[test]
    fn creating_a_volume_posts_it_and_keeps_it_across_scene_syncs() {
        let mailbox = TerrainEditMailbox::new();
        let api = TerrainEditApi::new(mailbox.clone());
        let id = api.create_volume(test_volume()).unwrap();
        assert_eq!(id, VolumeId::from_stable_name("flat"));

        let posted = mailbox.take_pending_bodies().expect("creation posts at once");
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].1.shape(), TerrainShape::Volume);

        // A later scene sync carrying only planets must not retire the volume.
        api.sync_scene_planets(vec![("earth:0".to_string(), test_planet())]);
        let posted = mailbox.take_pending_bodies().unwrap();
        assert_eq!(posted.len(), 2);
        let mut shapes: Vec<_> = posted.iter().map(|(_, body)| body.shape()).collect();
        shapes.sort();
        assert_eq!(shapes, vec![TerrainShape::Planet, TerrainShape::Volume]);

        api.clear_authored_volumes();
        api.sync_scene_planets(vec![("earth:0".to_string(), test_planet())]);
        assert_eq!(mailbox.take_pending_bodies().unwrap().len(), 1);
    }

    #[test]
    fn an_invalid_volume_is_refused_at_the_call_site() {
        let api = TerrainEditApi::default();
        let mut definition = test_volume();
        definition.flat.extent = (0, 0, 0);
        assert!(matches!(
            api.create_volume(definition).unwrap_err(),
            TerrainEditError::InvalidVolume(_)
        ));
        assert!(api.authored_volumes().is_empty());
    }

    #[test]
    fn a_ray_from_above_hits_a_flat_worlds_ground_plane_with_an_upward_normal() {
        let definition = TerrainBodyDefinition::Volume(test_volume());
        let hit = intersect_body([0.0, 50.0, 0.0], [0.0, -1.0, 0.0], &definition).unwrap();
        // The ground plane is the top of cell y=0, i.e. 0.1 m up.
        assert!(
            (hit.distance_m - 49.9).abs() < 1e-6,
            "got {}",
            hit.distance_m
        );
        assert_eq!(hit.normal, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn a_ray_past_a_flat_worlds_edge_misses_it() {
        let definition = TerrainBodyDefinition::Volume(test_volume());
        // The default world is +/-102.4 m; look down well outside that.
        assert!(intersect_body([500.0, 50.0, 0.0], [0.0, -1.0, 0.0], &definition).is_none());
        // And a ray pointing away from it never meets it.
        assert!(intersect_body([0.0, 50.0, 0.0], [0.0, 1.0, 0.0], &definition).is_none());
    }

    #[test]
    fn a_ray_from_inside_a_flat_world_still_finds_a_surface() {
        let definition = TerrainBodyDefinition::Volume(test_volume());
        let hit = intersect_body([0.0, -10.0, 0.0], [0.0, 1.0, 0.0], &definition).unwrap();
        assert!((hit.distance_m - 10.1).abs() < 1e-6, "got {}", hit.distance_m);
        assert_eq!(hit.normal, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn a_target_resolves_to_one_body_identity_whatever_its_shape() {
        let volume_id = VolumeId::from_stable_name("flat");
        assert_eq!(
            TerrainTarget::Volume(volume_id).body_id(),
            volume_id.body_id()
        );
        assert_eq!(
            TerrainTarget::of(&TerrainBodyDefinition::Volume(test_volume())),
            TerrainTarget::Volume(volume_id)
        );
        assert_eq!(
            TerrainTarget::of(&TerrainBodyDefinition::Planet(test_planet())),
            TerrainTarget::Planet(test_planet().planet_id)
        );
    }

    #[test]
    fn a_brush_cursor_change_requests_a_frame_and_an_unchanged_one_does_not() {
        let mailbox = TerrainEditMailbox::new();
        let api = TerrainEditApi::new(mailbox.clone());
        let cursor = BrushCursorRequest {
            center_m: [1.0, 2.0, 3.0],
            normal: [0.0, 1.0, 0.0],
            radius_m: 8.0,
            color: [1.0; 4],
        };
        api.set_brush_cursor(Some(cursor));
        assert_eq!(mailbox.brush_cursor(), Some(cursor));
        assert!(mailbox.take_pending_advance());

        api.set_brush_cursor(Some(cursor));
        assert!(
            !mailbox.take_pending_advance(),
            "an identical cursor must not keep the renderer awake"
        );
    }
}
