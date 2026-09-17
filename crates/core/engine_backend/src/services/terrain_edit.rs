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
    CellWord, ContentHash, EditMode, EditOp, EditShape, MaterialId, NodeState, PlanetDefinition,
    PlanetId, PlanetIdParseError, SparseBrickTree, TerrainRuntimeError, TerrainSnapshot,
    LOD0_CELL_SIZE_METERS,
};
use pulsar_terrain::TerrainRuntimeHandle;

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
/// Only planets exist today. Milestone 3's flat voxel volumes add a
/// `Volume(..)` arm here, and `TerrainEditApi`'s surface is shaped so that
/// addition does not change any caller's control flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainTarget {
    Planet(PlanetId),
}

impl TerrainTarget {
    pub fn planet_id(self) -> PlanetId {
        match self {
            Self::Planet(id) => id,
        }
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
    /// Latest-wins: the full set of planets the scene currently defines, each
    /// keyed by the stable scene source that authored it. The render thread
    /// upserts them and drops any source that disappeared.
    pending_planets: Arc<Mutex<Option<Vec<(String, PlanetDefinition)>>>>,
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

    /// Take the scene's planet definitions, if the UI thread posted a new set.
    pub fn take_pending_planets(&self) -> Option<Vec<(String, PlanetDefinition)>> {
        self.pending_planets.lock().ok().and_then(|mut s| s.take())
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
}

impl TerrainEditApi {
    pub fn new(mailbox: TerrainEditMailbox) -> Self {
        Self { mailbox }
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

    /// Every planet currently registered with the runtime.
    pub fn planets(&self) -> Vec<PlanetDefinition> {
        self.with_runtime(|runtime| runtime.planet_definitions())
            .unwrap_or_default()
    }

    /// The definition backing a target, if it is still registered.
    pub fn planet_definition(&self, target: TerrainTarget) -> Option<PlanetDefinition> {
        let planet_id = target.planet_id();
        self.planets()
            .into_iter()
            .find(|definition| definition.planet_id == planet_id)
    }

    /// The planet a sculpt should default to when the user has not picked one.
    pub fn default_target(&self) -> Option<TerrainTarget> {
        self.planets()
            .first()
            .map(|definition| TerrainTarget::Planet(definition.planet_id))
    }

    /// Post the full set of planets the scene defines, each paired with the
    /// stable key of the scene component that authored it. Latest wins; source
    /// keys absent from `definitions` are retired on the render thread.
    pub fn sync_scene_planets(&self, definitions: Vec<(String, PlanetDefinition)>) {
        if let Ok(mut slot) = self.mailbox.pending_planets.lock() {
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

    // ── Hit testing ────────────────────────────────────────────────────

    /// Intersect a world-space ray with the active terrain.
    ///
    /// Two stages: an exact analytic intersection against the planet's
    /// canonical sphere, then a bounded march along the ray that snaps the hit
    /// onto sculpted geometry (raised or carved away from that sphere). The
    /// march costs at most [`REFINE_SAMPLES`] canonical cell evaluations, so
    /// this is cheap enough to run per pointer event.
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

        let mut best: Option<(f64, PlanetDefinition)> = None;
        for definition in self.planets() {
            let Some(distance) = intersect_planet(origin, direction, &definition) else {
                continue;
            };
            if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
                best = Some((distance, definition));
            }
        }
        let (distance, definition) = best?;
        Some(self.refine_hit(origin, direction, distance, &definition))
    }

    /// Snap an analytic sphere intersection onto the sculpted surface by
    /// marching the canonical density field around it.
    fn refine_hit(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        analytic_distance: f64,
        definition: &PlanetDefinition,
    ) -> TerrainHit {
        let planet_id = definition.planet_id;
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
            .with_runtime(|runtime| runtime.sample_cells(planet_id, &cells))
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
            (analytic_distance, meters_to_cell(point), definition.material)
        });

        let point = point_at(origin, direction, distance);
        let center_m = cell_to_meters(definition.center_cell);
        let normal = normalize([
            (point[0] - center_m[0]) as f32,
            (point[1] - center_m[1]) as f32,
            (point[2] - center_m[2]) as f32,
        ])
        .unwrap_or([0.0, 1.0, 0.0]);

        TerrainHit {
            target: TerrainTarget::Planet(planet_id),
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
        let planet_id = target.planet_id();
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
        self.with_runtime(|runtime| runtime.planet_snapshot(target.planet_id()))
            .flatten()
    }

    /// Restore a previously captured snapshot, rewinding the mutation tail.
    pub fn restore(
        &self,
        target: TerrainTarget,
        snapshot: TerrainSnapshot,
    ) -> Result<(), TerrainEditError> {
        self.with_runtime(|runtime| runtime.restore_planet_snapshot(target.planet_id(), snapshot))
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
        assert_eq!(mailbox.take_pending_planets().unwrap().len(), 1);
        assert!(mailbox.take_pending_planets().is_none());
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
