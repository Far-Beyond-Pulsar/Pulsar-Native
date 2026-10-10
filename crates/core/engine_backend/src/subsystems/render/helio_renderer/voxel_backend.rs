//! Renderer registration for SceneDB voxel terrain sources.
//!
//! Source rows carry opaque renderer IDs and recipes. Each backend owns its
//! pass and translates only the rows it understands into a frame snapshot.

use std::sync::{Arc, Mutex, OnceLock};

use glam::{DVec3, Vec3};
use helio_component::{voxel_world::planet_brush, VoxelWorldShape};
use helio_default_graphs::VoxelPassFactory;
use helio_pass_voxel_planet::{
    engine::{PlanetFrame, PlanetPass, SharedPlanetFrame},
    terrain, Planet, PlanetRecipe, TerrainSource,
};
use helio_voxel_data::{VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape, VoxelEditJournal};

use super::renderer::{VoxelBrushRequest, VoxelBrushTool};
use crate::scene::voxel_frame::{VoxelEntryId, VoxelGeneratorConfig, VoxelSceneEntry};

pub use helio_voxel_data::{
    VOXEL_TERRAIN_GENERATOR, VOXEL_TERRAIN_GENERATOR_VERSION, VOXEL_TERRAIN_RENDERER,
};

/// Camera coordinates here are f64 so backend recipes can preserve a fine
/// world-space sample interval at planetary scale.
#[derive(Clone, Copy)]
pub struct VoxelView {
    pub position: [f64; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    pub tan_half_fov_y: f32,
    pub aspect: f32,
    pub far: f32,
    pub size: [u32; 2],
    /// Direction towards the scene's directional light, if it has one.
    pub sun: Option<[f32; 3]>,
}

/// A backend edit appends one brush to the terrain's journal. SceneDB owns
/// the component and persists the journal with the level.
#[derive(Clone, Debug)]
pub struct VoxelBrushCommit {
    pub id: VoxelEntryId,
    pub distance: f64,
    pub edit: VoxelBrushEdit,
    /// Edits of the same stamp after `edit` (flatten and smooth: the fill
    /// below the level after the carve above it).
    pub then: Vec<VoxelBrushEdit>,
    /// The ground height (radial, m) the stamp leveled to or hit: a
    /// flatten stroke keeps its first.
    pub level: f64,
}

/// The renderer's answer to a pick request: the first terrain hit drawn
/// under the requested view point, or `None` (sky, not loaded).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelPick {
    pub id: u64,
    pub hit: Option<helio_pass_voxel_planet::engine::PickHit>,
}

/// Where a brush ray hits the terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoxelRayHint {
    /// The renderer's hit for this ray: what the user aimed at.
    Drawn(helio_pass_voxel_planet::engine::PickHit),
    /// No renderer hit (scripts, nothing drawn yet): the exact terrain,
    /// walked this far.
    Reach(f64),
}

/// Without a renderer hit, an exact brush ray walks at most this far.
pub const UNPICKED_REACH_M: f64 = 500.0;

/// Stamps filling the gap between two consecutive samples of one stroke,
/// so a fast drag carves a continuous trench (or, with one-block cubes, a
/// continuous line of blocks) however few samples the frame rate allows.
/// Stamps are `spacing` apart (a fraction of the brush, never below a
/// voxel); the endpoints are not repeated. Samples of different brushes, or
/// farther apart than a plausible drag step (the pointer jumped to other
/// ground), start a new stroke segment instead.
pub fn stroke_fill(
    previous: &VoxelBrushEdit,
    next: &VoxelBrushEdit,
    voxel_size: f64,
) -> Vec<VoxelBrushEdit> {
    let same_brush = previous.op == next.op
        && previous.shape == next.shape
        && previous.material == next.material
        && (previous.radius - next.radius).abs() < 1e-9;
    let (a, b) = (
        DVec3::from_array(previous.center),
        DVec3::from_array(next.center),
    );
    let distance = a.distance(b);
    let spacing = (next.radius * 0.6).max(voxel_size);
    if !same_brush || distance <= spacing || distance > (next.radius * 16.0).max(voxel_size * 32.0)
    {
        return Vec::new();
    }
    let steps = (distance / spacing).ceil() as usize;
    (1..steps)
        .map(|k| VoxelBrushEdit {
            center: a.lerp(b, k as f64 / steps as f64).to_array(),
            ..*next
        })
        .collect()
}

pub trait VoxelRenderBackend: Send {
    fn configure_appearance(
        &self,
        _renderer: &mut helio::Renderer,
        _source: &VoxelSceneEntry,
    ) -> Result<(), String> {
        Ok(())
    }
    fn renderer_id(&self) -> &'static str;
    /// Choose temporal resolve for this backend at the current viewport size.
    fn temporal_quality(&self, _size: [u32; 2]) -> Option<helio_pass_tsr::TsrQuality> {
        None
    }
    /// Worlds whose coordinates outgrow f32 precision render camera-relative
    /// frames (world origin at the eye) when every scene source allows it.
    fn camera_relative_frames(&self) -> bool {
        false
    }
    /// Local vertical at `eye` (a planet's radial): the camera's up.
    fn local_up(&self, _source: &VoxelSceneEntry, _eye: DVec3) -> Option<DVec3> {
        None
    }
    /// Height of `eye` above the source's ground directly below it, if the
    /// backend knows it. The editor camera scales its speed with it, so moving
    /// from the ground to orbit takes seconds at any terrain height. (A
    /// conservative distance to all terrain would be ~0 anywhere below the
    /// highest possible mountain.)
    fn altitude(&self, _source: &VoxelSceneEntry, _eye: DVec3) -> Option<f64> {
        None
    }
    /// Canonical ground placement for the opt-in native flight diagnostic.
    /// The returned point already includes the requested clearance.
    fn diagnostic_surface_point(
        &self,
        _source: &VoxelSceneEntry,
        _direction: DVec3,
        _clearance: f64,
    ) -> Option<DVec3> {
        None
    }
    /// Where an editor camera at `eye` should be instead, if `eye` is inside
    /// solid voxels of the last published world: just above the ground.
    /// Air the user dug (tunnels, caves) is not solid, so the camera can fly
    /// into it.
    fn lift_out_of_ground(&self, _eye: DVec3) -> Option<DVec3> {
        None
    }
    /// Optional clipping range for a source. A backend may certify empty space
    /// around the eye to improve depth precision without clipping its terrain.
    fn camera_clip_range(&self, _source: &VoxelSceneEntry, _eye: DVec3) -> Option<(f32, f32)> {
        None
    }
    /// Ask for the terrain hit under view point `uv` (0..1 from the top
    /// left of the viewport); the answer arrives through `take_picks` a few
    /// frames later. `None`: this backend draws no terrain to pick.
    fn request_pick(&self, _uv: [f32; 2]) -> Option<u64> {
        None
    }
    fn take_picks(&self) -> Vec<VoxelPick> {
        Vec::new()
    }
    /// The brush edit where a ray hits the terrain ([`VoxelRayHint`]).
    fn edit_ray(
        &self,
        _source: &VoxelSceneEntry,
        _origin: DVec3,
        _direction: DVec3,
        _hint: VoxelRayHint,
        _request: VoxelBrushRequest,
    ) -> Result<Option<VoxelBrushCommit>, String> {
        Ok(None)
    }
    /// Used only when a source leaves its renderer ID empty. Explicit IDs
    /// always win, and an ambiguous automatic match is reported to the host.
    fn supports(&self, _source: &VoxelSceneEntry) -> bool {
        false
    }
    /// The backend's GBuffer pass, if it draws with one (a backend that
    /// uploads ordinary mesh rows has none).
    fn pass_factory(&self) -> Option<VoxelPassFactory>;
    fn publish_frame(
        &mut self,
        sources: &[&VoxelSceneEntry],
        view: VoxelView,
    ) -> Result<(), String>;
    /// True while an asynchronous pass needs more viewport frames to finish
    /// a complete replacement cut.
    fn needs_frame(&self, _renderer: &helio::Renderer) -> bool {
        false
    }
    /// One-line streaming/residency diagnostics for logs, if the backend has
    /// any (see `PULSAR_VOXEL_STATS`).
    fn diagnostics(&self, _renderer: &helio::Renderer) -> Option<String> {
        None
    }
}

pub struct VoxelBackendRegistry {
    backends: Vec<Box<dyn VoxelRenderBackend>>,
}

impl VoxelBackendRegistry {
    pub fn new() -> Self {
        Self {
            backends: Vec::new(),
        }
    }

    pub fn register(&mut self, backend: Box<dyn VoxelRenderBackend>) -> Result<(), String> {
        if self
            .backends
            .iter()
            .any(|existing| existing.renderer_id() == backend.renderer_id())
        {
            return Err(format!(
                "Voxel renderer '{}' is already registered",
                backend.renderer_id()
            ));
        }
        self.backends.push(backend);
        Ok(())
    }

    pub fn pass_factories(&self) -> Vec<VoxelPassFactory> {
        self.backends
            .iter()
            .filter_map(|backend| backend.pass_factory())
            .collect()
    }

    pub fn uses_camera_relative_frames(&self, entries: &[VoxelSceneEntry]) -> bool {
        let mut selected = self.backends.iter().filter(|backend| {
            entries.iter().any(|entry| {
                if entry.renderer_id.is_empty() {
                    backend.supports(entry)
                } else {
                    entry.renderer_id == backend.renderer_id()
                }
            })
        });
        selected.any(|backend| backend.camera_relative_frames())
    }

    pub fn configure_appearance(
        &self,
        renderer: &mut helio::Renderer,
        entries: &[VoxelSceneEntry],
    ) -> Vec<String> {
        let mut errors = Vec::new();
        for entry in entries.iter().filter(|entry| entry.visible) {
            for backend in &self.backends {
                if entry.renderer_id == backend.renderer_id()
                    || (entry.renderer_id.is_empty() && backend.supports(entry))
                {
                    if let Err(error) = backend.configure_appearance(renderer, entry) {
                        errors.push(error);
                    }
                }
            }
        }
        errors
    }

    /// Local vertical of the first visible source that defines one.
    pub fn local_up(&self, entries: &[VoxelSceneEntry], eye: DVec3) -> Option<DVec3> {
        entries
            .iter()
            .filter(|entry| entry.visible)
            .find_map(|entry| {
                self.backends
                    .iter()
                    .filter(|backend| {
                        entry.renderer_id == backend.renderer_id()
                            || (entry.renderer_id.is_empty() && backend.supports(entry))
                    })
                    .find_map(|backend| backend.local_up(entry, eye))
            })
    }

    /// [`VoxelRenderBackend::lift_out_of_ground`] of the first backend that
    /// has the eye inside its terrain.
    pub fn lift_out_of_ground(&self, eye: DVec3) -> Option<DVec3> {
        self.backends
            .iter()
            .find_map(|backend| backend.lift_out_of_ground(eye))
    }

    /// Smallest [`VoxelRenderBackend::altitude`] of the visible sources.
    pub fn altitude(&self, entries: &[VoxelSceneEntry], eye: DVec3) -> Option<f64> {
        profiling::profile_scope!("voxel_altitude");
        entries
            .iter()
            .filter(|entry| entry.visible)
            .filter_map(|entry| {
                self.backends
                    .iter()
                    .filter(|backend| {
                        entry.renderer_id == backend.renderer_id()
                            || (entry.renderer_id.is_empty() && backend.supports(entry))
                    })
                    .find_map(|backend| backend.altitude(entry, eye))
            })
            .reduce(f64::min)
    }

    pub fn diagnostic_surface_point(
        &self,
        entries: &[VoxelSceneEntry],
        direction: DVec3,
        clearance: f64,
    ) -> Option<DVec3> {
        entries
            .iter()
            .filter(|entry| entry.visible)
            .find_map(|entry| {
                self.backends
                    .iter()
                    .filter(|backend| {
                        entry.renderer_id == backend.renderer_id()
                            || (entry.renderer_id.is_empty() && backend.supports(entry))
                    })
                    .find_map(|backend| {
                        backend.diagnostic_surface_point(entry, direction, clearance)
                    })
            })
    }

    pub fn temporal_quality(
        &self,
        entries: &[VoxelSceneEntry],
        size: [u32; 2],
    ) -> Option<helio_pass_tsr::TsrQuality> {
        let mut quality: Option<helio_pass_tsr::TsrQuality> = None;
        for backend in &self.backends {
            let selected = entries.iter().any(|entry| {
                if entry.renderer_id.is_empty() {
                    backend.supports(entry)
                } else {
                    entry.renderer_id == backend.renderer_id()
                }
            });
            if selected {
                if let Some(candidate) = backend.temporal_quality(size) {
                    if quality
                        .is_none_or(|current| candidate.render_scale() > current.render_scale())
                    {
                        quality = Some(candidate);
                    }
                }
            }
        }
        quality
    }

    pub fn camera_clip_range(&self, entries: &[VoxelSceneEntry], eye: DVec3) -> Option<(f32, f32)> {
        let mut range: Option<(f32, f32)> = None;
        for entry in entries.iter().filter(|entry| entry.visible) {
            for backend in &self.backends {
                if entry.renderer_id == backend.renderer_id()
                    || (entry.renderer_id.is_empty() && backend.supports(entry))
                {
                    if let Some((near, far)) = backend.camera_clip_range(entry, eye) {
                        if near.is_finite() && far.is_finite() && near > 0.0 && far > near {
                            range = Some(
                                range.map_or((near, far), |old| (old.0.min(near), old.1.max(far))),
                            );
                        }
                    }
                }
            }
        }
        range
    }

    /// Ask the backends for the terrain hit under view point `uv`.
    pub fn request_pick(&self, uv: [f32; 2]) -> Option<u64> {
        self.backends
            .iter()
            .find_map(|backend| backend.request_pick(uv))
    }

    pub fn take_picks(&self) -> Vec<VoxelPick> {
        self.backends
            .iter()
            .flat_map(|backend| backend.take_picks())
            .collect()
    }

    pub fn edit_ray(
        &self,
        entries: &[VoxelSceneEntry],
        origin: DVec3,
        direction: DVec3,
        hint: VoxelRayHint,
        request: VoxelBrushRequest,
    ) -> Result<Option<VoxelBrushCommit>, String> {
        let mut closest: Option<VoxelBrushCommit> = None;
        for entry in entries
            .iter()
            .filter(|entry| entry.visible && entry.editable)
        {
            for backend in &self.backends {
                let matches = if entry.renderer_id.is_empty() {
                    backend.supports(entry)
                } else {
                    entry.renderer_id == backend.renderer_id()
                };
                if !matches {
                    continue;
                }
                if let Some(commit) = backend.edit_ray(entry, origin, direction, hint, request)? {
                    if closest
                        .as_ref()
                        .is_none_or(|old| commit.distance < old.distance)
                    {
                        closest = Some(commit);
                    }
                }
            }
        }
        Ok(closest)
    }

    /// [`VoxelRenderBackend::diagnostics`] of every backend that has some.
    pub fn diagnostics(&self, renderer: &helio::Renderer) -> Vec<String> {
        self.backends
            .iter()
            .filter_map(|backend| backend.diagnostics(renderer))
            .collect()
    }

    pub fn needs_frame(&self, renderer: &helio::Renderer) -> bool {
        self.backends
            .iter()
            .any(|backend| backend.needs_frame(renderer))
    }

    pub fn publish_frame(&mut self, entries: &[VoxelSceneEntry], view: VoxelView) -> Vec<String> {
        profiling::profile_scope!("voxel_publish_frame");
        let mut errors = Vec::new();
        let mut selected = vec![Vec::new(); self.backends.len()];
        for entry in entries.iter().filter(|entry| entry.visible) {
            let matches: Vec<_> = self
                .backends
                .iter()
                .enumerate()
                .filter_map(|(index, backend)| {
                    if (!entry.renderer_id.is_empty() && backend.renderer_id() == entry.renderer_id)
                        || (entry.renderer_id.is_empty() && backend.supports(entry))
                    {
                        Some(index)
                    } else {
                        None
                    }
                })
                .collect();
            match matches.as_slice() {
                [index] => selected[*index].push(entry),
                [] if entry.renderer_id.is_empty()
                    && entry.generator.is_none()
                    && entry.initial_cube.is_none() => {}
                [] => errors.push(format!(
                    "No compatible voxel renderer for source {:?} (requested '{}')",
                    entry.id, entry.renderer_id
                )),
                _ => errors.push(format!(
                    "Multiple voxel renderers match source {:?}; set renderer_id explicitly",
                    entry.id
                )),
            }
        }
        for (backend, sources) in self.backends.iter_mut().zip(selected) {
            if let Err(error) = backend.publish_frame(&sources, view) {
                errors.push(format!(
                    "Voxel renderer '{}': {error}",
                    backend.renderer_id()
                ));
            }
        }
        errors
    }
}

impl Default for VoxelBackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// The world recipe of an entry: its form, voxel size and terrain source.
fn world_recipe(entry: &VoxelSceneEntry, generator: &VoxelGeneratorConfig) -> PlanetRecipe {
    helio_component::voxel_world::world_recipe(
        entry.world.shape,
        entry.world.planet_radius,
        entry.world.plane_size,
        entry.voxel_size,
        TerrainSource {
            generator: generator.id.clone(),
            version: generator.version,
            seed: generator.seed,
            settings: generator.parameters.clone(),
        },
    )
}

/// Build the world of an entry: its generated terrain with every journal brush.
fn build_planet(
    entry: &VoxelSceneEntry,
    generator: &VoxelGeneratorConfig,
) -> Result<Planet, String> {
    helio_component::voxel_world::journal_planet(world_recipe(entry, generator), &entry.edits)
}

/// A flatten or smooth stamp at `hit`: the ground under the brush's square
/// footprint levels to a layer boundary, carved above it and filled below,
/// a box each as tall as the brush. Flatten levels to `request.level` (the
/// stroke's first ground, this hit's ground for the first stamp); smooth to
/// the average ground height around the hit, filling with the ground's own
/// material.
fn level_stamp(
    planet: &Planet,
    id: VoxelEntryId,
    hit: &helio_pass_voxel_planet::RayHit,
    request: VoxelBrushRequest,
    material: u32,
) -> Result<VoxelBrushCommit, String> {
    let grid = planet.grid();
    let voxel = grid.voxel_size();
    let centre = grid.cell_center(hit.cell);
    // The ground's height as a layer boundary (k: layers above the datum).
    let layer_of = |radial: f64| ((radial - grid.layer_radius(0.0)) / voxel).round();
    let hit_top = f64::from(hit.cell.k + 1);
    let k = match request.tool {
        VoxelBrushTool::Smooth => {
            let up = grid.up(centre);
            let side = up.any_orthonormal_vector();
            let ahead = up.cross(side);
            let r = f64::from(request.radius) * 0.7;
            let mut sum = 0.0;
            let mut n = 0.0;
            for (a, b) in [(0.0, 0.0), (1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (0.7, 0.7), (-0.7, 0.7), (0.7, -0.7), (-0.7, -0.7)] {
                let ground = planet.surface_point(centre + (side * a + ahead * b) * r, 0.0);
                sum += layer_of(grid.radial(ground));
                n += 1.0;
            }
            (sum / n).round()
        }
        _ => request.level.map_or(hit_top, layer_of),
    };
    let level = grid.layer_radius(k);
    // Whole cells tall, so the boxes meet exactly on the level.
    let cells = (f64::from(request.radius) / voxel).round().max(1.0);
    let half = cells * voxel / 2.0;
    let fill_material = match request.tool {
        VoxelBrushTool::Smooth => match planet.material(hit.cell) {
            0 => material,
            m => m,
        },
        _ => material,
    };
    let at = |radial: f64| grid.at_radial(centre, radial).to_array();
    let carve = VoxelBrushEdit {
        center: at(level + half),
        radius: f64::from(request.radius).max(voxel * 0.5),
        shape: VoxelBrushShape::Cube,
        op: VoxelBrushOp::Remove,
        material: 0,
        height: half,
    };
    let fill = VoxelBrushEdit { center: at(level - half), op: VoxelBrushOp::Add, material: fill_material, ..carve };
    planet_brush(&carve).resolve(grid)?;
    planet_brush(&fill).resolve(grid)?;
    Ok(VoxelBrushCommit { id, distance: hit.distance, edit: carve, then: vec![fill], level })
}

/// Built planet for one source revision.
#[derive(Clone)]
struct CachedPlanet {
    id: VoxelEntryId,
    revision: u64,
    generator: VoxelGeneratorConfig,
    voxel_size: f64,
    world: crate::scene::voxel_frame::VoxelWorldForm,
    /// Journal applied to `planet`: a newer journal that only appends
    /// brushes extends a copy of it.
    edits: VoxelEditJournal,
    planet: Arc<Planet>,
}

/// A world to build: the source and its generator, and the world it may
/// grow from.
struct WorldRequest {
    entry: VoxelSceneEntry,
    generator: VoxelGeneratorConfig,
    base: Option<CachedPlanet>,
}

/// A built world, or why the source was rejected.
struct WorldResult {
    id: VoxelEntryId,
    revision: u64,
    generator: VoxelGeneratorConfig,
    built: Result<CachedPlanet, String>,
}

/// Builds worlds off the render thread: a stroke's stamps extend a copy of
/// the shown world (sealing brushes into bricks, indexing large ones) while
/// frames keep drawing it. One build runs at a time; the source's latest
/// revision is requested when it lands.
struct WorldBuilder {
    requests: std::sync::mpsc::Sender<WorldRequest>,
    results: std::sync::mpsc::Receiver<WorldResult>,
    /// The source revision being built.
    building: Option<(VoxelEntryId, u64)>,
}

impl WorldBuilder {
    fn new() -> Self {
        let (requests, inbox) = std::sync::mpsc::channel::<WorldRequest>();
        let (outbox, results) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("voxel-world".into())
            .spawn(move || {
                while let Ok(request) = inbox.recv() {
                    profiling::profile_scope!("voxel_world_build");
                    let built = build_world(&request.entry, &request.generator, request.base.as_ref());
                    let result = WorldResult {
                        id: request.entry.id,
                        revision: request.entry.source_revision,
                        generator: request.generator,
                        built,
                    };
                    if outbox.send(result).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn the voxel world builder");
        Self { requests, results, building: None }
    }
}

/// The world for `entry`: `base` extended by appended brushes, or with its
/// latest brushes undone, or rebuilt from the recipe.
fn build_world(entry: &VoxelSceneEntry, generator: &VoxelGeneratorConfig, base: Option<&CachedPlanet>) -> Result<CachedPlanet, String> {
    let started = std::time::Instant::now();
    let base = base.filter(|c| c.id == entry.id && c.voxel_size == entry.voxel_size && c.world == entry.world && &c.generator == generator);
    let (planet, how) = match base {
        // A sculpt stroke appends brushes to an otherwise equal source.
        Some(base) if entry.edits.starts_with(&base.edits) => {
            let mut planet = (*base.planet).clone();
            entry.edits.iter_from(base.edits.len()).try_for_each(|edit| planet.apply(planet_brush(edit)).map(|_| ()))?;
            (planet, "extended")
        }
        // An undo removes the latest brushes: undone in the world when it
        // still can (`Edits::undoable`), not rebuilt from every edit.
        Some(base) if base.edits.starts_with(&entry.edits) && base.edits.len() - entry.edits.len() <= base.planet.edits().undoable() => {
            let mut planet = (*base.planet).clone();
            for _ in entry.edits.len()..base.edits.len() {
                planet.undo().expect("within the undoable brushes");
            }
            (planet, "undone")
        }
        _ => (build_planet(entry, generator)?, "rebuilt"),
    };
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms >= 20.0 {
        tracing::warn!("VOXEL_WORLD {how} in {ms:.1} ms ({} edits)", entry.edits.len());
    }
    Ok(CachedPlanet {
        id: entry.id,
        revision: entry.source_revision,
        generator: generator.clone(),
        voxel_size: entry.voxel_size,
        world: entry.world,
        edits: entry.edits.clone(),
        planet: Arc::new(planet),
    })
}

/// Streamed destructible voxel terrain (`helio-pass-voxel-planet`): planets,
/// planes and infinite planes of any registered terrain generator, as a
/// GPU-driven clipmap of exact voxels from 0.1 m to 1 m rendered
/// camera-relative with traced sunlight.
pub struct PlanetVoxelBackend {
    frame: SharedPlanetFrame,
    cached: Option<CachedPlanet>,
    /// Pick requests and answers shared with the pass through each frame.
    picks: helio_pass_voxel_planet::engine::SharedPicks,
    next_pick: std::sync::atomic::AtomicU64,
    /// The last source that failed to build and why: it is not rebuilt
    /// every frame, and the last good world stays on screen meanwhile.
    rejected: Option<(VoxelEntryId, u64, VoxelGeneratorConfig, String)>,
    builder: WorldBuilder,
    /// Wait for every world build (tests: each frame shows its revision).
    blocking: bool,
}

impl PlanetVoxelBackend {
    pub fn new() -> Self {
        Self {
            frame: Arc::new(Mutex::new(None)),
            cached: None,
            picks: Default::default(),
            next_pick: std::sync::atomic::AtomicU64::new(1),
            rejected: None,
            builder: WorldBuilder::new(),
            blocking: false,
        }
    }

    /// A backend whose frames wait for their world's build (tests).
    #[cfg(test)]
    fn blocking() -> Self {
        Self { blocking: true, ..Self::new() }
    }

    fn clear(&mut self) -> Result<(), String> {
        self.cached = None;
        self.rejected = None;
        *self
            .frame
            .lock()
            .map_err(|_| "frame mailbox was poisoned")? = None;
        Ok(())
    }

    fn validate_source(entry: &VoxelSceneEntry) -> Result<&VoxelGeneratorConfig, String> {
        let generator = entry.generator.as_ref().ok_or("generator ID is required")?;
        if terrain::find(&generator.id, generator.version).is_none() {
            let known: Vec<_> = terrain::generators()
                .into_iter()
                .map(|g| format!("{} v{}", g.id, g.version))
                .collect();
            return Err(format!(
                "unknown terrain generator '{}' version {}; registered: {}",
                generator.id,
                generator.version,
                known.join(", ")
            ));
        }
        if !entry.voxel_size.is_finite() || !(0.1 - 1e-9..=1.0 + 1e-9).contains(&entry.voxel_size) {
            return Err("this backend supports base voxels from 0.1 to 1.0 metres".into());
        }
        // The world owns its acceleration layout; component chunk/LOD
        // metadata describes generic live payloads and does not apply.
        if entry.origin != [0.0; 3] {
            return Err(
                "a voxel world is centred on the world origin; move the entity to (0, 0, 0)".into(),
            );
        }
        Ok(generator)
    }

    /// The world to draw for `entry`: the one built for its revision, or
    /// while that builds on the worker, the last one built for it (`None`
    /// before the first). Builds are requested here; a rejected revision
    /// is not rebuilt and reports its error.
    fn world_for(&mut self, entry: &VoxelSceneEntry) -> Result<Option<Arc<Planet>>, String> {
        let generator = Self::validate_source(entry)?.clone();
        let current = |c: &CachedPlanet| {
            c.id == entry.id && c.revision == entry.source_revision && c.generator == generator && c.voxel_size == entry.voxel_size && c.world == entry.world
        };
        loop {
            self.receive_worlds();
            if self.cached.as_ref().is_some_and(current) {
                break;
            }
            if let Some((id, revision, rejected, error)) = &self.rejected {
                if *id == entry.id && *revision == entry.source_revision && *rejected == generator {
                    return Err(error.clone());
                }
            }
            if self.builder.building.is_none() {
                let base = self.cached.clone();
                self.builder.building = Some((entry.id, entry.source_revision));
                self.builder
                    .requests
                    .send(WorldRequest { entry: entry.clone(), generator: generator.clone(), base })
                    .map_err(|_| "the voxel world builder stopped")?;
            }
            // Only the first world of a source (nothing to show yet) and
            // tests wait for a build.
            let first = self.cached.as_ref().is_none_or(|c| c.id != entry.id);
            if !(self.blocking || first) {
                break;
            }
            let Ok(result) = self.builder.results.recv() else {
                return Err("the voxel world builder stopped".into());
            };
            self.accept_world(result);
        }
        Ok(self.cached.as_ref().filter(|c| c.id == entry.id).map(|c| Arc::clone(&c.planet)))
    }

    /// Take the worlds the builder finished.
    fn receive_worlds(&mut self) {
        while let Ok(result) = self.builder.results.try_recv() {
            self.accept_world(result);
        }
    }

    fn accept_world(&mut self, result: WorldResult) {
        self.builder.building = None;
        match result.built {
            Ok(world) => {
                self.rejected = None;
                self.cached = Some(world);
            }
            Err(error) => self.rejected = Some((result.id, result.revision, result.generator, error)),
        }
    }

    /// The world a brush ray hits for `entry`: the cached one when the
    /// entry's journal only grew from it (stamps of this stroke not yet
    /// published), else `cached_planet`.
    fn shown_planet(&self, entry: &VoxelSceneEntry) -> Option<&Arc<Planet>> {
        self.cached
            .as_ref()
            .filter(|c| {
                c.id == entry.id
                    && c.world == entry.world
                    && entry.generator.as_ref() == Some(&c.generator)
                    && entry.edits.starts_with(&c.edits)
            })
            .map(|c| &c.planet)
            .or_else(|| self.cached_planet(entry))
    }

    /// The world the camera measures against for `entry`: its built world
    /// whatever brushes this frame adds or undoes. The renderer asks before
    /// `prepare` applies them; without this every edit frame had no ground,
    /// so the near plane jumped from the clearance to 5 cm and the fly
    /// speed to its default, and back the next frame.
    fn camera_planet(&self, entry: &VoxelSceneEntry) -> Option<&Arc<Planet>> {
        self.cached
            .as_ref()
            .filter(|c| {
                c.id == entry.id
                    && c.world == entry.world
                    && entry.generator.as_ref() == Some(&c.generator)
            })
            .map(|c| &c.planet)
            .or_else(|| self.cached_planet(entry))
    }

    /// The world on screen for `entry`: its current one, or while a change
    /// to it is rejected, its last good one.
    fn cached_planet(&self, entry: &VoxelSceneEntry) -> Option<&Arc<Planet>> {
        let rejected = self
            .rejected
            .as_ref()
            .is_some_and(|(id, revision, generator, _)| {
                *id == entry.id
                    && *revision == entry.source_revision
                    && entry.generator.as_ref() == Some(generator)
            });
        self.cached
            .as_ref()
            .filter(|c| {
                c.id == entry.id
                    && (rejected
                        || (c.revision == entry.source_revision
                            && c.world == entry.world
                            && c.edits == entry.edits
                            && entry.generator.as_ref() == Some(&c.generator)))
            })
            .map(|c| &c.planet)
    }
}

impl Default for PlanetVoxelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl VoxelRenderBackend for PlanetVoxelBackend {
    fn configure_appearance(
        &self,
        renderer: &mut helio::Renderer,
        source: &VoxelSceneEntry,
    ) -> Result<(), String> {
        // No appearance JSON: the terrain generator's own materials.
        let appearance = if source.appearance_parameters.trim().is_empty() {
            None
        } else {
            Some(
                serde_json::from_str(&source.appearance_parameters)
                    .map_err(|e| format!("invalid terrain appearance JSON: {e}"))?,
            )
        };
        let changed = renderer
            .find_pass_mut::<PlanetPass>()
            .is_some_and(|pass| pass.set_appearance(appearance));
        if changed {
            // The editor can go idle immediately after this frame. Old colour
            // history must not hide an inspector edit until the camera moves.
            if let Some(pass) = renderer.find_pass_mut::<helio_pass_tsr::TsrPass>() {
                pass.reset_history();
            }
        }
        Ok(())
    }
    fn renderer_id(&self) -> &'static str {
        VOXEL_TERRAIN_RENDERER
    }

    fn temporal_quality(&self, size: [u32; 2]) -> Option<helio_pass_tsr::TsrQuality> {
        let pixels = u64::from(size[0]) * u64::from(size[1]);
        Some(if pixels > 1_500_000 {
            helio_pass_tsr::TsrQuality::Quality
        } else {
            helio_pass_tsr::TsrQuality::Native
        })
    }

    fn camera_relative_frames(&self) -> bool {
        true
    }

    fn lift_out_of_ground(&self, eye: DVec3) -> Option<DVec3> {
        let planet = &self.cached.as_ref()?.planet;
        let grid = planet.grid();
        if !eye.is_finite() || grid.radial(eye) > planet.outer_radius() {
            return None;
        }
        let (cell, _) = grid.locate(eye);
        planet.solid(cell).then(|| planet.surface_point(eye, 0.5))
    }

    fn altitude(&self, source: &VoxelSceneEntry, eye: DVec3) -> Option<f64> {
        self.camera_planet(source)
            .map(|planet| planet.ground_height(eye))
    }

    fn diagnostic_surface_point(
        &self,
        source: &VoxelSceneEntry,
        direction: DVec3,
        clearance: f64,
    ) -> Option<DVec3> {
        if !direction.is_finite()
            || direction.length_squared() == 0.0
            || !clearance.is_finite()
            || clearance < 0.0
        {
            return None;
        }
        self.camera_planet(source)
            .map(|planet| planet.surface_point(direction, clearance))
    }

    fn local_up(&self, source: &VoxelSceneEntry, eye: DVec3) -> Option<DVec3> {
        match source.world.shape {
            VoxelWorldShape::Sphere => eye.try_normalize(),
            VoxelWorldShape::Plane | VoxelWorldShape::InfinitePlane => Some(DVec3::Y),
        }
    }

    fn camera_clip_range(&self, source: &VoxelSceneEntry, eye: DVec3) -> Option<(f32, f32)> {
        let far = (eye.length() + 40_000_000.0) as f32;
        let near = self.camera_planet(source).map_or(0.05, |planet| {
            (planet.air_clearance(eye) * 0.25).clamp(0.05, 50_000.0) as f32
        });
        Some((near, far))
    }

    fn request_pick(&self, uv: [f32; 2]) -> Option<u64> {
        self.cached.as_ref()?;
        let id = self
            .next_pick
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.picks
            .lock()
            .ok()?
            .requests
            .push(helio_pass_voxel_planet::engine::PickRequest { id, uv });
        Some(id)
    }

    fn take_picks(&self) -> Vec<VoxelPick> {
        let Ok(mut picks) = self.picks.lock() else {
            return Vec::new();
        };
        picks
            .results
            .drain(..)
            .map(|r| VoxelPick { id: r.id, hit: r.hit })
            .collect()
    }

    fn edit_ray(
        &self,
        source: &VoxelSceneEntry,
        origin: DVec3,
        direction: DVec3,
        hint: VoxelRayHint,
        request: VoxelBrushRequest,
    ) -> Result<Option<VoxelBrushCommit>, String> {
        Self::validate_source(source)?;
        // The world on screen: a stroke's earlier stamps may not be
        // published yet, so a world the journal only grew from is it.
        let Some(planet) = self.shown_planet(source) else {
            return Ok(None);
        };
        let d = direction.normalize();
        let hit = match hint {
            VoxelRayHint::Drawn(pick) => Some(planet.drawn_hit(origin, d, pick.distance, pick.cell, pick.level, pick.entered)),
            VoxelRayHint::Reach(reach) => planet.raycast(origin, d, reach),
        };
        let Some(hit) = hit else {
            return Ok(None);
        };
        let material = helio_pass_voxel_planet::terrain::material::ID & request.material;
        if request.op != VoxelBrushOp::Remove
            && !(1..helio_pass_voxel_planet::terrain::material::COUNT).contains(&material)
        {
            return Err(format!(
                "material {} is not a solid terrain material",
                request.material
            ));
        }
        // Building fills the empty block in front of the hit face.
        let cell = if request.op == VoxelBrushOp::Add {
            hit.previous
        } else {
            hit.cell
        };
        let grid = planet.grid();
        if request.tool != VoxelBrushTool::Stamp {
            return Ok(Some(level_stamp(planet, source.id, &hit, request, material)?));
        }
        let (radius, shape) = if request.single_block {
            (grid.voxel_size() * 0.5, VoxelBrushShape::Cube)
        } else {
            (
                f64::from(request.radius).max(grid.voxel_size() * 0.5),
                request.shape,
            )
        };
        let edit = VoxelBrushEdit {
            center: grid.cell_center(cell).to_array(),
            radius,
            shape,
            op: request.op,
            material: if request.op == VoxelBrushOp::Remove {
                0
            } else {
                material
            },
            height: Default::default(),
        };
        planet_brush(&edit).resolve(grid)?;
        Ok(Some(VoxelBrushCommit {
            id: source.id,
            distance: hit.distance,
            edit,
            then: Vec::new(),
            level: grid.layer_radius(f64::from(hit.cell.k + 1)),
        }))
    }

    fn supports(&self, source: &VoxelSceneEntry) -> bool {
        source
            .generator
            .as_ref()
            .is_some_and(|generator| terrain::find(&generator.id, generator.version).is_some())
    }

    fn pass_factory(&self) -> Option<VoxelPassFactory> {
        let frame = Arc::clone(&self.frame);
        Some(Arc::new(move |_, _, _, _| {
            Box::new(PlanetPass::new(Arc::clone(&frame)))
        }))
    }

    fn needs_frame(&self, renderer: &helio::Renderer) -> bool {
        self.builder.building.is_some()
            || renderer
                .find_pass::<PlanetPass>()
                .is_some_and(PlanetPass::needs_frame)
    }

    fn diagnostics(&self, renderer: &helio::Renderer) -> Option<String> {
        let pass = renderer.find_pass::<PlanetPass>()?;
        let s = pass.stats()?;
        let mut line = format!(
            "planet ready={} resident={} pending={} jobs={} units={:.0} budget={:.0} us_per_unit={:.3} failed={} scratch_retries={} clipped={} levels={} finest={} plan={:.2}ms upload={:.2}ms encode={:.2}ms windows={:.2}ms needs_frame={} evictions={} free_pages={}/{} free_units={} recycles={} lod_pressure={:.2} table_refused={} late_plans={} reranked={}",
            s.ready,
            s.resident_columns,
            s.pending_columns,
            s.jobs,
            s.units,
            s.unit_budget,
            s.us_per_unit,
            s.failed_jobs,
            s.scratch_retries,
            s.clipped_columns,
            s.active_levels,
            s.finest_level,
            s.plan_cpu_ms,
            s.upload_cpu_ms,
            s.encode_cpu_ms,
            s.window_rebuild_ms,
            pass.needs_frame(),
            s.evictions,
            s.free_pages,
            s.pool_pages,
            s.free_units,
            s.recycles,
            s.lod_pressure,
            s.table_refused,
            s.late_plans,
            s.reranked,
        );
        static GPU_STAGES: OnceLock<bool> = OnceLock::new();
        if *GPU_STAGES.get_or_init(|| std::env::var_os("PULSAR_VOXEL_GPU_STAGES").is_some()) {
            if let Some(active) = pass.renderer() {
                use std::fmt::Write;
                // Encode already consumes deferred timestamps for budgeting.
                // Read that cache; these stages belong to gpu_frame, not this eye.
                let profiler = active.profiler();
                let gpu_frame = profiler.and_then(|p| p.last_completed_frame());
                let encoded_frame = active.frame_number();
                let lag = gpu_frame.and_then(|frame| encoded_frame.checked_sub(frame));
                let stage_ms = |name: &str| {
                    profiler
                        .and_then(|p| p.get_last_timings().iter().find(|t| t.name == name))
                        .map(|t| t.duration_ns as f64 / 1.0e6)
                };
                let _ = write!(line,
                    " planet_gpu_frame={gpu_frame:?} planet_encoded_frame={encoded_frame} planet_gpu_lag={lag:?} planet_primary_ms={:?} planet_shade_ms={:?} planet_sunlight_ms={:?} planet_horizon_ms={:?} planet_residency_ms={:?} planet_gbuffer_ms={:?} planet_timestamps_supported={} planet_timestamp_drops={:?} planet_query_overflows={:?}",
                    stage_ms("planet_primary"),
                    stage_ms("planet_shade"),
                    stage_ms("planet_sunlight"),
                    stage_ms("planet_horizon"),
                    stage_ms("planet_residency"),
                    stage_ms("planet_gbuffer"),
                    profiler.is_some_and(|p| p.supported()),
                    profiler.map(|p| p.dropped_readbacks()),
                    profiler.map(|p| p.query_overflows()),
                );
            }
        }
        Some(line)
    }

    fn publish_frame(
        &mut self,
        sources: &[&VoxelSceneEntry],
        view: VoxelView,
    ) -> Result<(), String> {
        if sources.is_empty() {
            self.clear()?;
            return Ok(());
        }
        let [entry] = sources else {
            self.clear()?;
            return Err("the voxel planet renders one terrain source at a time".into());
        };
        // Generic live chunk payloads are not interpreted by this backend yet;
        // they must not be silently ignored.
        match entry.store.try_read() {
            Ok(state) if !state.1.is_empty() => {
                self.clear()?;
                return Err(
                    "the voxel planet does not consume live sample chunks yet; edit with brushes"
                        .into(),
                );
            }
            Ok(_) => {}
            Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.clear()?;
                return Err("voxel source payload store was poisoned".into());
            }
        }
        let (planet, error) = match self.world_for(entry) {
            Ok(Some(planet)) => (planet, None),
            Ok(None) => {
                *self.frame.lock().map_err(|_| "frame mailbox was poisoned")? = None;
                return Ok(());
            }
            // An invalid change (a layer stack mid-edit, a brush off the
            // grid) keeps this entity's last good world on screen; the
            // error says what to fix.
            Err(error) => match self.cached.as_ref().filter(|c| c.id == entry.id) {
                Some(cached) => (Arc::clone(&cached.planet), Some(error)),
                None => {
                    self.clear()?;
                    return Err(error);
                }
            },
        };
        let eye = DVec3::from_array(view.position);
        // Traced sunlight must match the scene's directional light, which
        // the deferred pass multiplies by the planet's visibility.
        let sun = view
            .sun
            .map(Vec3::from_array)
            .and_then(Vec3::try_normalize)
            .unwrap_or(Vec3::new(0.35, 0.75, 0.45).normalize());
        *self
            .frame
            .lock()
            .map_err(|_| "frame mailbox was poisoned")? = Some(PlanetFrame {
            eye,
            planet,
            sun,
            shadows: view.sun.is_some(),
            picks: Some(self.picks.clone()),
        });
        error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Visibility;
    use helio_component::VoxelTerrainComponent;
    use helio_voxel_data::VoxelStoredPayload;
    use pulsar_scenedb::{Entity, World};

    /// Attach `value` to `owner` as a component instance; returns it.
    fn attach<T: pulsar_reflection::EngineClass>(
        scene: &mut World,
        owner: Entity,
        value: T,
    ) -> Entity {
        pulsar_world_registry::attach_value(scene, owner, value).expect("attach test component")
    }

    fn planet_terrain() -> VoxelTerrainComponent {
        let mut terrain = VoxelTerrainComponent::default();
        terrain.shape = VoxelWorldShape::Sphere;
        terrain.renderer_id = VOXEL_TERRAIN_RENDERER.into();
        terrain.generator.id = VOXEL_TERRAIN_GENERATOR.into();
        terrain.generator.version = VOXEL_TERRAIN_GENERATOR_VERSION;
        terrain.generator_parameters = String::new();
        terrain.voxel_size = 0.1;
        // The world `PlanetRecipe::default()` describes.
        terrain.seed = TerrainSource::default().seed;
        terrain
    }

    fn view(eye: DVec3) -> VoxelView {
        VoxelView {
            position: eye.to_array(),
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, -1.0],
            forward: [0.0, -1.0, 0.0],
            tan_half_fov_y: 0.41421357,
            aspect: 16.0 / 9.0,
            far: 10_000.0,
            size: [1600, 900],
            sun: Some([0.3, 0.8, 0.5]),
        }
    }

    fn dig(radius: f32) -> VoxelBrushRequest {
        VoxelBrushRequest {
            op: VoxelBrushOp::Remove,
            shape: VoxelBrushShape::Sphere,
            radius,
            material: 0,
            single_block: false,
            level: Default::default(),
            tool: Default::default(),
        }
    }

    fn raise(radius: f32) -> VoxelBrushRequest {
        VoxelBrushRequest {
            op: VoxelBrushOp::Add,
            material: helio_pass_voxel_planet::terrain::material::COBBLE,
            ..dig(radius)
        }
    }

    fn frame_planet(backend: &PlanetVoxelBackend) -> Arc<Planet> {
        backend
            .frame
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .planet
            .clone()
    }

    #[test]
    fn native_flight_surface_is_unavailable_until_published_and_offsets_once() {
        let mut scene = World::new();
        let owner = scene.spawn();
        attach(&mut scene, owner, planet_terrain());
        let (mut entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());
        let mut registry = VoxelBackendRegistry::new();
        registry
            .register(Box::new(PlanetVoxelBackend::blocking()))
            .unwrap();
        assert!(registry
            .diagnostic_surface_point(&entries, DVec3::Y, 2.0)
            .is_none());
        assert!(registry
            .publish_frame(&entries, view(DVec3::Y * 6_374_000.0))
            .is_empty());
        let ground = registry
            .diagnostic_surface_point(&entries, DVec3::Y, 0.0)
            .unwrap();
        let lifted = registry
            .diagnostic_surface_point(&entries, DVec3::Y, 32.0)
            .unwrap();
        assert!(
            (lifted.distance(ground) - 32.0).abs() < 1e-7,
            "clearance must be applied only by the canonical query"
        );
        assert!(registry
            .diagnostic_surface_point(&entries, DVec3::ZERO, 2.0)
            .is_none());
        assert!(registry
            .diagnostic_surface_point(&entries, DVec3::Y, f64::NAN)
            .is_none());
        entries[0].visible = false;
        assert!(registry
            .diagnostic_surface_point(&entries, DVec3::Y, 2.0)
            .is_none());
    }

    #[test]
    fn renderer_selection_preserves_the_planet_snapshot_between_camera_frames() {
        let mut scene = World::new();
        let owner = scene.spawn();
        attach(&mut scene, owner, planet_terrain());
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty(), "{errors:?}");
        let eye = DVec3::new(0.0, 6_371_000.0 + 3_000.0, 0.0);

        let mut backend = PlanetVoxelBackend::blocking();
        backend.publish_frame(&[&entries[0]], view(eye)).unwrap();
        let first = frame_planet(&backend);
        backend.publish_frame(&[&entries[0]], view(eye)).unwrap();
        assert!(Arc::ptr_eq(&first, &frame_planet(&backend)));

        let mut revised = entries[0].clone();
        revised.source_revision += 1;
        revised.voxel_size = 1.0;
        backend.publish_frame(&[&revised], view(eye)).unwrap();
        let coarse = frame_planet(&backend);
        assert!(!Arc::ptr_eq(&first, &coarse));
        assert_eq!(coarse.grid().voxel_size(), 1.0);
        assert_eq!(
            first.grid().voxel_size(),
            0.1,
            "old frame snapshots remain immutable"
        );

        let orbit = eye.normalize() * (coarse.grid().radius() + 300_000.0);
        let (near, far) = backend.camera_clip_range(&revised, orbit).unwrap();
        assert!(near > 1_000.0 && far > orbit.length() as f32);

        // An invalid change (settings mid-edit) keeps the last good world on
        // screen and in the queries; the error says what to fix.
        let mut invalid = revised.clone();
        invalid.source_revision += 1;
        invalid.generator.as_mut().unwrap().parameters = "{".into();
        assert!(backend.publish_frame(&[&invalid], view(eye)).is_err());
        assert!(
            Arc::ptr_eq(&coarse, &frame_planet(&backend)),
            "the last good world stays"
        );
        assert!(backend.camera_clip_range(&invalid, orbit).is_some());
        assert!(
            backend.publish_frame(&[&invalid], view(eye)).is_err(),
            "still rejected, without a rebuild"
        );
        backend.publish_frame(&[&revised], view(eye)).unwrap();
        assert!(Arc::ptr_eq(&coarse, &frame_planet(&backend)));

        revised
            .store
            .write()
            .unwrap()
            .1
            .insert([0; 4], VoxelStoredPayload::raw_material(vec![1u8; 512]));
        assert!(backend.publish_frame(&[&revised], view(eye)).is_err());
        assert!(backend.frame.lock().unwrap().is_none());

        backend.publish_frame(&[], view(eye)).unwrap();
        assert!(backend.frame.lock().unwrap().is_none());
    }

    #[test]
    fn empty_renderer_id_selects_a_unique_compatible_backend() {
        let mut scene = World::new();
        let owner = scene.spawn();
        let mut terrain = planet_terrain();
        terrain.renderer_id.clear();
        attach(&mut scene, owner, terrain);
        let (entries, projection_errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(projection_errors.is_empty());

        let backend = PlanetVoxelBackend::blocking();
        let frame = Arc::clone(&backend.frame);
        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(backend)).unwrap();
        let eye = DVec3::new(0.0, 6_371_000.0 + 3_000.0, 0.0);
        assert!(registry.publish_frame(&entries, view(eye)).is_empty());
        assert!(frame.lock().unwrap().is_some());

        scene.insert(
            owner,
            Visibility {
                visible: false,
                locked: false,
            },
        );
        let (hidden, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());
        assert!(!hidden[0].visible);
        assert!(registry.uses_camera_relative_frames(&hidden));
        assert!(registry.publish_frame(&hidden, view(eye)).is_empty());
        assert!(frame.lock().unwrap().is_none());
    }

    #[test]
    fn exact_brush_edits_round_trip_through_the_terrain_journal() {
        let mut scene = World::new();
        let owner = scene.spawn();
        let entity = attach(&mut scene, owner, planet_terrain());
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());
        let original = Planet::new(PlanetRecipe::default()).unwrap();
        let eye = original.surface_point(DVec3::new(0.2, 1.0, 0.3), 3.0);
        let down = -eye.normalize();
        let target = original.raycast(eye, down, 100.0).unwrap().cell;
        assert_ne!(original.material(target), 0);

        let mut registry = VoxelBackendRegistry::new();
        registry
            .register(Box::new(PlanetVoxelBackend::blocking()))
            .unwrap();
        // Brushes hit the world on screen.
        assert!(registry
            .edit_ray(&entries, eye, down, VoxelRayHint::Reach(UNPICKED_REACH_M), dig(0.5))
            .unwrap()
            .is_none());
        assert!(registry.publish_frame(&entries, view(eye)).is_empty());
        let commit = registry
            .edit_ray(&entries, eye, down, VoxelRayHint::Reach(UNPICKED_REACH_M), dig(0.5))
            .unwrap()
            .unwrap();
        assert_eq!(commit.id, entries[0].id);
        let replay = |edits: &VoxelEditJournal| {
            let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
            for edit in edits.listed() {
                planet.apply(planet_brush(edit)).unwrap();
            }
            planet
        };
        assert_eq!(
            replay(&[commit.edit].into_iter().collect()).material(target),
            0
        );
        // The same canonical cell is addressed from the ground, from orbit
        // and from far beyond the renderer's precision range: a renderer hit
        // drawn at level 0 is the exact cell wherever the eye is.
        let exact = original.raycast(eye, down, 100.0).unwrap();
        for distance in [2_000.0, 300_000.0, 1_000_000_000.0] {
            let pick = helio_pass_voxel_planet::engine::PickHit {
                distance: distance + exact.distance,
                cell: target,
                level: 0,
                // Stepping down the radial axis.
                entered: Some(4),
            };
            let remote = registry
                .edit_ray(&entries, eye - down * distance, down, VoxelRayHint::Drawn(pick), dig(0.05))
                .unwrap()
                .unwrap_or_else(|| panic!("remote terrain remains editable from {distance} m"));
            assert_eq!(
                replay(&[remote.edit].into_iter().collect()).material(target),
                0
            );
            assert!(remote.distance > distance);
        }
        // Without one, the walk is bounded instead of crossing the planet.
        assert!(registry
            .edit_ray(&entries, eye - down * 2_000.0, down, VoxelRayHint::Reach(UNPICKED_REACH_M), dig(0.05))
            .unwrap()
            .is_none());
        // Building fills the empty cell in front of the hit.
        let build = registry
            .edit_ray(&entries, eye, down, VoxelRayHint::Reach(UNPICKED_REACH_M), raise(0.05))
            .unwrap()
            .unwrap();
        assert_eq!(build.edit.op, VoxelBrushOp::Add);
        assert_eq!(
            build.edit.material,
            helio_pass_voxel_planet::terrain::material::COBBLE
        );
        // One block, painted: a unit cube on the hit block.
        let paint = VoxelBrushRequest {
            op: VoxelBrushOp::Paint,
            material: helio_pass_voxel_planet::terrain::material::SNOW,
            single_block: true,
            ..raise(3.0)
        };
        let painted = registry
            .edit_ray(&entries, eye, down, VoxelRayHint::Reach(UNPICKED_REACH_M), paint)
            .unwrap()
            .unwrap();
        assert_eq!(
            (painted.edit.shape, painted.edit.radius),
            (VoxelBrushShape::Cube, 0.05)
        );
        let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
        planet.apply(planet_brush(&painted.edit)).unwrap();
        assert_eq!(
            planet.material(target),
            helio_pass_voxel_planet::terrain::material::SNOW
        );
        let invalid = VoxelBrushRequest {
            material: 99,
            ..raise(1.0)
        };
        assert!(registry
            .edit_ray(&entries, eye, down, VoxelRayHint::Reach(UNPICKED_REACH_M), invalid)
            .is_err());

        assert!(super::super::renderer::apply_voxel_brush_commit(
            &mut scene, commit
        ));
        let terrain = scene.get::<VoxelTerrainComponent>(entity).unwrap();
        assert_eq!(terrain.source_revision, 1);
        assert_eq!(terrain.edits.len(), 1);
        assert_eq!(replay(&terrain.edits).material(target), 0);

        // The backend extends its cached planet with the appended brush.
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut backend = PlanetVoxelBackend::blocking();
        backend.publish_frame(&[&entries[0]], view(eye)).unwrap();
        assert_eq!(frame_planet(&backend).material(target), 0);
    }

    #[test]
    fn altitude_is_height_above_the_ground_below() {
        let mut scene = World::new();
        let owner = scene.spawn();
        attach(&mut scene, owner, planet_terrain());
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut registry = VoxelBackendRegistry::new();
        registry
            .register(Box::new(PlanetVoxelBackend::blocking()))
            .unwrap();
        let planet = Planet::new(PlanetRecipe::default()).unwrap();
        let ground = planet.surface_point(DVec3::Y, 2.0);
        assert!(registry.publish_frame(&entries, view(ground)).is_empty());
        let near = registry.altitude(&entries, ground).unwrap();
        let orbit = registry.altitude(&entries, ground * 1.05).unwrap();
        assert!(
            (near - 2.0).abs() < 0.2 && orbit > 250_000.0,
            "{near} {orbit}"
        );
        // 3 km over lowland: below the highest possible terrain, where a
        // conservative clearance is ~0, the camera still moves at altitude speed.
        let high = planet.surface_point(DVec3::Y, 3000.0);
        let altitude = registry.altitude(&entries, high).unwrap();
        assert!((altitude - 3000.0).abs() < 1.0, "{altitude}");
    }

    #[test]
    fn camera_ground_holds_on_frames_whose_edits_are_not_built_yet() {
        let mut scene = World::new();
        let owner = scene.spawn();
        let entity = attach(&mut scene, owner, planet_terrain());
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut registry = VoxelBackendRegistry::new();
        registry
            .register(Box::new(PlanetVoxelBackend::blocking()))
            .unwrap();
        let planet = Planet::new(PlanetRecipe::default()).unwrap();
        let eye = planet.surface_point(DVec3::Y, 800.0);
        assert!(registry.publish_frame(&entries, view(eye)).is_empty());
        let altitude = registry.altitude(&entries, eye).unwrap();
        let clip = registry.camera_clip_range(&entries, eye).unwrap();

        // The renderer asks before `prepare` builds the frame's new brush:
        // the camera keeps the built ground instead of losing it.
        {
            let mut terrain = scene.get_mut::<VoxelTerrainComponent>(entity).unwrap();
            terrain.edits.push(VoxelBrushEdit {
                center: planet.surface_point(DVec3::X, 0.0).to_array(),
                radius: 4.0,
                shape: VoxelBrushShape::Sphere,
                op: VoxelBrushOp::Remove,
                material: 0,
                height: Default::default(),
            });
            terrain.source_revision += 1;
        }
        let (grown, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert_eq!(registry.altitude(&grown, eye), Some(altitude));
        assert_eq!(registry.camera_clip_range(&grown, eye), Some(clip));
        assert!(clip.0 > 1.0, "the near plane follows the clearance: {clip:?}");
    }

    #[test]
    fn an_eye_inside_the_ground_is_lifted_but_dug_air_is_kept() {
        let mut scene = World::new();
        let owner = scene.spawn();
        let entity = attach(&mut scene, owner, planet_terrain());
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut registry = VoxelBackendRegistry::new();
        registry
            .register(Box::new(PlanetVoxelBackend::blocking()))
            .unwrap();
        let planet = Planet::new(PlanetRecipe::default()).unwrap();
        let surface = planet.surface_point(DVec3::Y, 0.0);
        assert!(registry
            .publish_frame(&entries, view(surface + DVec3::Y * 2.0))
            .is_empty());
        let buried = surface - DVec3::Y * 3.0;
        let lifted = registry
            .lift_out_of_ground(buried)
            .expect("inside the ground");
        assert!(
            (lifted.y - surface.y - 0.5).abs() < 0.2,
            "{} {}",
            lifted.y,
            surface.y
        );
        assert!(registry
            .lift_out_of_ground(surface + DVec3::Y * 2.0)
            .is_none());

        // Dig a cave around the buried point: the camera may stay in it.
        let dig = VoxelBrushEdit {
            center: buried.to_array(),
            radius: 1.5,
            shape: VoxelBrushShape::Sphere,
            op: VoxelBrushOp::Remove,
            material: 0,
            height: Default::default(),
        };
        scene
            .get_mut::<VoxelTerrainComponent>(entity)
            .unwrap()
            .edits
            .push(dig);
        scene
            .get_mut::<VoxelTerrainComponent>(entity)
            .unwrap()
            .source_revision += 1;
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(registry
            .publish_frame(&entries, view(surface + DVec3::Y * 2.0))
            .is_empty());
        assert!(registry.lift_out_of_ground(buried).is_none());
    }

    #[test]
    fn strokes_fill_gaps_between_samples_and_break_on_jumps() {
        let edit = |x: f64| VoxelBrushEdit {
            center: [x, 0.0, 0.0],
            radius: 1.0,
            shape: VoxelBrushShape::Sphere,
            op: VoxelBrushOp::Remove,
            material: 0,
            height: Default::default(),
        };
        // 3 m apart with 0.6 m spacing: stamps at 0.6 m steps between them.
        let fill = stroke_fill(&edit(0.0), &edit(3.0), 0.1);
        assert_eq!(fill.len(), 4);
        assert!(fill
            .windows(2)
            .all(|w| (w[1].center[0] - w[0].center[0] - 0.6).abs() < 1e-9));
        assert!(
            stroke_fill(&edit(0.0), &edit(0.5), 0.1).is_empty(),
            "close samples need no fill"
        );
        assert!(
            stroke_fill(&edit(0.0), &edit(40.0), 0.1).is_empty(),
            "a jump starts a new segment"
        );
        let paint = VoxelBrushEdit {
            op: VoxelBrushOp::Paint,
            material: 5,
            ..edit(3.0)
        };
        assert!(
            stroke_fill(&edit(0.0), &paint, 0.1).is_empty(),
            "another brush starts a new segment"
        );
        // One-block cubes: a stamp per voxel.
        let block = |x: f64| VoxelBrushEdit {
            shape: VoxelBrushShape::Cube,
            radius: 0.05,
            op: VoxelBrushOp::Add,
            material: 13,
            ..edit(x)
        };
        assert_eq!(stroke_fill(&block(0.0), &block(1.0), 0.1).len(), 9);
    }

    #[test]
    fn a_layers_component_configures_the_generator() {
        let mut scene = World::new();
        let owner = scene.spawn();
        let mut terrain = planet_terrain();
        terrain.seed = 99;
        attach(&mut scene, owner, terrain);
        let mut layers = helio_component::VoxelTerrainLayersComponent::default();
        layers.stack.snowline_m = 1_234.0;
        layers.stack.layers[2].scale_km = 55.0;
        attach(&mut scene, owner, layers);
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty(), "{errors:?}");
        let mut backend = PlanetVoxelBackend::blocking();
        backend
            .publish_frame(
                &[&entries[0]],
                view(DVec3::new(0.0, 6_371_000.0 + 3_000.0, 0.0)),
            )
            .unwrap();
        let recipe = frame_planet(&backend).recipe().clone();
        assert_eq!(
            recipe.terrain.generator,
            helio_pass_voxel_planet::layers::ID
        );
        assert_eq!(recipe.terrain.seed, 99);
        let settings: helio_pass_voxel_planet::layers::TerrainLayers =
            serde_json::from_str(&recipe.terrain.settings).unwrap();
        assert_eq!(settings.snowline_m, 1_234.0);
        assert_eq!(
            settings.layers[2].kind,
            helio_pass_voxel_planet::layers::LayerKind::Mountains
        );
        assert_eq!(settings.layers[2].scale_km, 55.0);
    }

    #[test]
    fn shared_ids_name_the_registered_terrain_generator() {
        assert_eq!(VOXEL_TERRAIN_GENERATOR, helio_pass_voxel_planet::layers::ID);
        assert_eq!(
            VOXEL_TERRAIN_GENERATOR_VERSION,
            helio_pass_voxel_planet::layers::VERSION
        );
    }

    #[test]
    fn a_flat_terrain_uses_its_settings_component() {
        let mut scene = World::new();
        let owner = scene.spawn();
        attach(&mut scene, owner, VoxelTerrainComponent::plane(1_024.0));
        let mut flat = helio_component::VoxelTerrainLayersComponent::flat(12.0);
        flat.stack.surface = helio_component::VoxelTerrainMaterial::Sand;
        attach(&mut scene, owner, flat);
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty(), "{errors:?}");
        let mut backend = PlanetVoxelBackend::blocking();
        backend
            .publish_frame(&[&entries[0]], view(DVec3::new(0.0, 40.0, 0.0)))
            .unwrap();
        let planet = frame_planet(&backend);
        let hit = planet
            .raycast(DVec3::new(3.0, 40.0, -5.0), -DVec3::Y, 100.0)
            .unwrap();
        assert!((hit.distance - 28.0).abs() < 0.11, "{}", hit.distance);
        assert_eq!(
            planet.material(hit.cell),
            helio_pass_voxel_planet::terrain::material::SAND
        );
    }

    #[test]
    fn unknown_generators_are_rejected_with_the_registered_list() {
        let mut scene = World::new();
        let owner = scene.spawn();
        let mut terrain = planet_terrain();
        terrain.generator.id = "example.none".into();
        attach(&mut scene, owner, terrain);
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let error = PlanetVoxelBackend::validate_source(&entries[0]).unwrap_err();
        assert!(error.contains("helio.terrain"), "{error}");
    }

    #[test]
    fn plane_worlds_follow_the_component_shape_and_size() {
        for shape in [VoxelWorldShape::Plane, VoxelWorldShape::InfinitePlane] {
            let mut scene = World::new();
            let owner = scene.spawn();
            let mut terrain = planet_terrain();
            terrain.shape = shape;
            terrain.plane_size = 2_048.0;
            attach(&mut scene, owner, terrain);
            let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
            assert!(errors.is_empty(), "{errors:?}");
            let mut registry = VoxelBackendRegistry::new();
            let backend = PlanetVoxelBackend::blocking();
            let frame = Arc::clone(&backend.frame);
            registry.register(Box::new(backend)).unwrap();
            let eye = DVec3::new(10.0, 400.0, -20.0);
            assert!(registry.publish_frame(&entries, view(eye)).is_empty());
            let planet = frame.lock().unwrap().as_ref().unwrap().planet.clone();
            assert!(planet.grid().is_plane());
            if shape == VoxelWorldShape::Plane {
                let edge = f64::from(planet.grid().cells()) * planet.grid().voxel_size();
                assert!((edge - 2_048.0).abs() < 200.0, "{edge}");
            }
            assert_eq!(registry.local_up(&entries, eye), Some(DVec3::Y));
            // Digging straight down removes the cell below the eye.
            let ground = planet.surface_point(DVec3::new(10.0, 0.0, -20.0), 3.0);
            let target = planet.raycast(ground, -DVec3::Y, 100.0).unwrap().cell;
            let commit = registry
                .edit_ray(&entries, ground, -DVec3::Y, VoxelRayHint::Reach(UNPICKED_REACH_M), dig(0.5))
                .unwrap()
                .unwrap();
            assert!(super::super::renderer::apply_voxel_brush_commit(
                &mut scene, commit
            ));
            let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
            assert!(registry.publish_frame(&entries, view(eye)).is_empty());
            assert_eq!(
                frame
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .planet
                    .material(target),
                0
            );
        }
    }
}
