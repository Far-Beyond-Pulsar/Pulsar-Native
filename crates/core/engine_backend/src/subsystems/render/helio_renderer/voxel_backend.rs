//! Renderer registration for SceneDB voxel terrain sources.
//!
//! Source rows carry opaque renderer IDs and recipes. Each backend owns its
//! pass and translates only the rows it understands into a frame snapshot.

use std::sync::{Arc, Mutex, OnceLock};

use glam::{DVec3, Vec3};
use helio_default_graphs::VoxelPassFactory;
use helio_component::{voxel_world::planet_brush, VoxelWorldShape};
use helio_pass_voxel_planet::{
    engine::{PlanetFrame, PlanetPass, SharedPlanetFrame},
    terrain, Planet, PlanetRecipe, TerrainSource,
};
use helio_voxel_data::{VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape, VoxelEditJournal};

use crate::scene::voxel_frame::{VoxelEntryId, VoxelGeneratorConfig, VoxelSceneEntry};
use super::renderer::VoxelBrushRequest;

pub use helio_voxel_data::{VOXEL_TERRAIN_GENERATOR, VOXEL_TERRAIN_GENERATOR_VERSION, VOXEL_TERRAIN_RENDERER};

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
}

/// Stamps filling the gap between two consecutive samples of one stroke,
/// so a fast drag carves a continuous trench (or, with one-block cubes, a
/// continuous line of blocks) however few samples the frame rate allows.
/// Stamps are `spacing` apart (a fraction of the brush, never below a
/// voxel); the endpoints are not repeated. Samples of different brushes, or
/// farther apart than a plausible drag step (the pointer jumped to other
/// ground), start a new stroke segment instead.
pub fn stroke_fill(previous: &VoxelBrushEdit, next: &VoxelBrushEdit, voxel_size: f64) -> Vec<VoxelBrushEdit> {
    let same_brush = previous.op == next.op
        && previous.shape == next.shape
        && previous.material == next.material
        && (previous.radius - next.radius).abs() < 1e-9;
    let (a, b) = (DVec3::from_array(previous.center), DVec3::from_array(next.center));
    let distance = a.distance(b);
    let spacing = (next.radius * 0.6).max(voxel_size);
    if !same_brush || distance <= spacing || distance > (next.radius * 16.0).max(voxel_size * 32.0) {
        return Vec::new();
    }
    let steps = (distance / spacing).ceil() as usize;
    (1..steps)
        .map(|k| VoxelBrushEdit { center: a.lerp(b, k as f64 / steps as f64).to_array(), ..*next })
        .collect()
}

pub trait VoxelRenderBackend: Send {
    fn configure_appearance(&self, _renderer: &mut helio::Renderer, _source: &VoxelSceneEntry) -> Result<(), String> { Ok(()) }
    fn renderer_id(&self) -> &'static str;
    /// Choose temporal resolve for this backend at the current viewport size.
    fn temporal_quality(&self, _size: [u32; 2]) -> Option<helio_pass_tsr::TsrQuality> {
        None
    }
    /// Outdoor backends may use the sky pass's default atmosphere when the
    /// scene has no explicitly authored sky component.
    fn outdoor_sky(&self) -> bool {
        false
    }
    fn planetary_sky(&self, _source: &VoxelSceneEntry, _eye: DVec3, _sun: Option<[f32; 3]>) -> Option<helio_pass_sky::PlanetarySky> {
        None
    }
    /// Local vertical at `eye` for hemisphere ambient (a planet's radial).
    fn ambient_up(&self, _source: &VoxelSceneEntry, _eye: DVec3) -> Option<DVec3> {
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
    fn diagnostic_surface_point(&self, _source: &VoxelSceneEntry, _direction: DVec3,
        _clearance: f64) -> Option<DVec3> {
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
    fn edit_ray(
        &self,
        _source: &VoxelSceneEntry,
        _origin: DVec3,
        _direction: DVec3,
        _request: VoxelBrushRequest,
    ) -> Result<Option<VoxelBrushCommit>, String> {
        Ok(None)
    }
    /// Used only when a source leaves its renderer ID empty. Explicit IDs
    /// always win, and an ambiguous automatic match is reported to the host.
    fn supports(&self, _source: &VoxelSceneEntry) -> bool {
        false
    }
    fn pass_factory(&self) -> VoxelPassFactory;
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
            .map(|backend| backend.pass_factory())
            .collect()
    }

    pub fn uses_outdoor_sky(&self, entries: &[VoxelSceneEntry]) -> bool {
        let mut selected = self.backends.iter().filter(|backend| {
            entries.iter().any(|entry| {
                if entry.renderer_id.is_empty() {
                    backend.supports(entry)
                } else {
                    entry.renderer_id == backend.renderer_id()
                }
            })
        });
        selected.any(|backend| backend.outdoor_sky())
    }

    pub fn configure_appearance(&self, renderer: &mut helio::Renderer, entries: &[VoxelSceneEntry]) -> Vec<String> {
        let mut errors = Vec::new();
        for entry in entries.iter().filter(|entry| entry.visible) {
            for backend in &self.backends {
                if entry.renderer_id == backend.renderer_id() || (entry.renderer_id.is_empty() && backend.supports(entry)) {
                    if let Err(error) = backend.configure_appearance(renderer, entry) { errors.push(error); }
                }
            }
        }
        errors
    }

    /// Local vertical of the first visible source that defines one.
    pub fn planetary_sky(&self, entries: &[VoxelSceneEntry], eye: DVec3, sun: Option<[f32; 3]>) -> Option<helio_pass_sky::PlanetarySky> {
        // Hiding the terrain mesh does not remove its camera environment.
        entries.iter().find_map(|entry| {
            self.backends.iter().filter(|backend| {
                entry.renderer_id == backend.renderer_id() || (entry.renderer_id.is_empty() && backend.supports(entry))
            }).find_map(|backend| backend.planetary_sky(entry, eye, sun))
        })
    }

    /// Local vertical of the first visible source that defines one.
    pub fn ambient_up(&self, entries: &[VoxelSceneEntry], eye: DVec3) -> Option<DVec3> {
        entries.iter().filter(|entry| entry.visible).find_map(|entry| {
            self.backends
                .iter()
                .filter(|backend| {
                    entry.renderer_id == backend.renderer_id()
                        || (entry.renderer_id.is_empty() && backend.supports(entry))
                })
                .find_map(|backend| backend.ambient_up(entry, eye))
        })
    }

    /// [`VoxelRenderBackend::lift_out_of_ground`] of the first backend that
    /// has the eye inside its terrain.
    pub fn lift_out_of_ground(&self, eye: DVec3) -> Option<DVec3> {
        self.backends.iter().find_map(|backend| backend.lift_out_of_ground(eye))
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

    pub fn diagnostic_surface_point(&self, entries: &[VoxelSceneEntry], direction: DVec3,
        clearance: f64) -> Option<DVec3> {
        entries.iter().filter(|entry| entry.visible).find_map(|entry| {
            self.backends.iter().filter(|backend| {
                entry.renderer_id == backend.renderer_id()
                    || (entry.renderer_id.is_empty() && backend.supports(entry))
            }).find_map(|backend| backend.diagnostic_surface_point(entry, direction, clearance))
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

    pub fn edit_ray(
        &self,
        entries: &[VoxelSceneEntry],
        origin: DVec3,
        direction: DVec3,
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
                if let Some(commit) =
                    backend.edit_ray(entry, origin, direction, request)?
                {
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
        self.backends.iter().filter_map(|backend| backend.diagnostics(renderer)).collect()
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
fn build_planet(entry: &VoxelSceneEntry, generator: &VoxelGeneratorConfig) -> Result<Planet, String> {
    let mut planet = Planet::new(world_recipe(entry, generator))?;
    for edit in entry.edits.iter() {
        planet.apply(planet_brush(edit))?;
    }
    Ok(planet)
}

/// Built planet for one source revision.
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

/// Streamed destructible voxel terrain (`helio-pass-voxel-planet`): planets,
/// planes and infinite planes of any registered terrain generator, as a
/// GPU-driven clipmap of exact voxels from 0.1 m to 1 m rendered
/// camera-relative with traced sunlight.
pub struct PlanetVoxelBackend {
    frame: SharedPlanetFrame,
    cached: Option<CachedPlanet>,
}

impl PlanetVoxelBackend {
    pub fn new() -> Self {
        Self {
            frame: Arc::new(Mutex::new(None)),
            cached: None,
        }
    }

    fn clear(&mut self) -> Result<(), String> {
        self.cached = None;
        *self
            .frame
            .lock()
            .map_err(|_| "frame mailbox was poisoned")? = None;
        Ok(())
    }

    fn validate_source(entry: &VoxelSceneEntry) -> Result<&VoxelGeneratorConfig, String> {
        let generator = entry.generator.as_ref().ok_or("generator ID is required")?;
        if terrain::find(&generator.id, generator.version).is_none() {
            let known: Vec<_> = terrain::generators().into_iter().map(|g| format!("{} v{}", g.id, g.version)).collect();
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
            return Err("a voxel world is centred on the world origin; move the entity to (0, 0, 0)".into());
        }
        Ok(generator)
    }

    /// The planet for this source revision: cached, extended by appended
    /// edits, or rebuilt from the recipe.
    fn planet_for(&mut self, entry: &VoxelSceneEntry) -> Result<Arc<Planet>, String> {
        let generator = Self::validate_source(entry)?.clone();
        if let Some(cached) = &self.cached {
            if cached.id == entry.id
                && cached.revision == entry.source_revision
                && cached.generator == generator
                && cached.voxel_size == entry.voxel_size
                && cached.world == entry.world
            {
                return Ok(Arc::clone(&cached.planet));
            }
        }
        profiling::profile_scope!("voxel_world_update");
        let planet = match &self.cached {
            // A sculpt stroke appends brushes to an otherwise equal source.
            Some(cached)
                if cached.id == entry.id
                    && cached.voxel_size == entry.voxel_size
                    && cached.world == entry.world
                    && cached.generator == generator
                    && entry.edits.starts_with(&cached.edits) =>
            {
                let mut planet = (*cached.planet).clone();
                for edit in entry.edits.iter_from(cached.edits.len()) {
                    planet.apply(planet_brush(edit))?;
                }
                planet
            }
            _ => build_planet(entry, &generator)?,
        };
        let planet = Arc::new(planet);
        self.cached = Some(CachedPlanet {
            id: entry.id,
            revision: entry.source_revision,
            generator,
            voxel_size: entry.voxel_size,
            world: entry.world,
            edits: entry.edits.clone(),
            planet: Arc::clone(&planet),
        });
        Ok(planet)
    }

    fn cached_planet(&self, entry: &VoxelSceneEntry) -> Option<&Arc<Planet>> {
        self.cached
            .as_ref()
            .filter(|c| c.id == entry.id && c.revision == entry.source_revision && c.world == entry.world && c.edits == entry.edits && entry.generator.as_ref() == Some(&c.generator))
            .map(|c| &c.planet)
    }
}

impl Default for PlanetVoxelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl VoxelRenderBackend for PlanetVoxelBackend {
    fn configure_appearance(&self, renderer: &mut helio::Renderer, source: &VoxelSceneEntry) -> Result<(), String> {
        let appearance = if source.appearance_parameters.trim().is_empty() {
            helio_pass_voxel_planet::engine::TerrainAppearance::default()
        } else {
            serde_json::from_str(&source.appearance_parameters).map_err(|e| format!("invalid terrain appearance JSON: {e}"))?
        };
        let changed = renderer.find_pass_mut::<PlanetPass>()
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

    fn outdoor_sky(&self) -> bool {
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
        self.cached_planet(source).map(|planet| planet.ground_height(eye))
    }

    fn diagnostic_surface_point(&self, source: &VoxelSceneEntry, direction: DVec3,
        clearance: f64) -> Option<DVec3> {
        if !direction.is_finite() || direction.length_squared() == 0.0
            || !clearance.is_finite() || clearance < 0.0 { return None; }
        self.cached_planet(source).map(|planet| planet.surface_point(direction, clearance))
    }

    fn ambient_up(&self, source: &VoxelSceneEntry, eye: DVec3) -> Option<DVec3> {
        match source.world.shape {
            VoxelWorldShape::Sphere => eye.try_normalize(),
            VoxelWorldShape::Plane | VoxelWorldShape::InfinitePlane => Some(DVec3::Y),
        }
    }

    fn planetary_sky(&self, source: &VoxelSceneEntry, eye: DVec3, sun: Option<[f32; 3]>) -> Option<helio_pass_sky::PlanetarySky> {
        if source.world.shape != VoxelWorldShape::Sphere { return None; }
        let sun = sun.map(Vec3::from_array).and_then(Vec3::try_normalize)
            .unwrap_or(Vec3::new(0.35, 0.75, 0.45).normalize());
        Some(helio_pass_sky::PlanetarySky::earth_like(eye.to_array(), source.world.planet_radius, sun.to_array()))
    }

    fn camera_clip_range(&self, source: &VoxelSceneEntry, eye: DVec3) -> Option<(f32, f32)> {
        let far = (eye.length() + 40_000_000.0) as f32;
        let near = self
            .cached_planet(source)
            .map_or(0.05, |planet| (planet.air_clearance(eye) * 0.25).clamp(0.05, 50_000.0) as f32);
        Some((near, far))
    }

    fn edit_ray(
        &self,
        source: &VoxelSceneEntry,
        origin: DVec3,
        direction: DVec3,
        request: VoxelBrushRequest,
    ) -> Result<Option<VoxelBrushCommit>, String> {
        let generator = Self::validate_source(source)?;
        let built;
        let planet: &Planet = match self.cached_planet(source) {
            Some(planet) => planet,
            None => {
                built = build_planet(source, generator)?;
                &built
            }
        };
        // Rays are clipped to the planet shell, not to a draw distance or a
        // tool reach: orbital edits use the same exact cells.
        let Some(hit) = planet.raycast(origin, direction.normalize(), f64::INFINITY) else {
            return Ok(None);
        };
        let material = helio_pass_voxel_planet::terrain::material::ID & request.material;
        if request.op != VoxelBrushOp::Remove && !(1..helio_pass_voxel_planet::terrain::material::COUNT).contains(&material) {
            return Err(format!("material {} is not a solid terrain material", request.material));
        }
        // Building fills the empty block in front of the hit face.
        let cell = if request.op == VoxelBrushOp::Add { hit.previous } else { hit.cell };
        let grid = planet.grid();
        let (radius, shape) = if request.single_block {
            (grid.voxel_size() * 0.5, VoxelBrushShape::Cube)
        } else {
            (f64::from(request.radius).max(grid.voxel_size() * 0.5), request.shape)
        };
        let edit = VoxelBrushEdit {
            center: grid.cell_center(cell).to_array(),
            radius,
            shape,
            op: request.op,
            material: if request.op == VoxelBrushOp::Remove { 0 } else { material },
        };
        planet_brush(&edit).resolve(grid)?;
        Ok(Some(VoxelBrushCommit { id: source.id, distance: hit.distance, edit }))
    }

    fn supports(&self, source: &VoxelSceneEntry) -> bool {
        source
            .generator
            .as_ref()
            .is_some_and(|generator| terrain::find(&generator.id, generator.version).is_some())
    }

    fn pass_factory(&self) -> VoxelPassFactory {
        let frame = Arc::clone(&self.frame);
        Arc::new(move |_, _, _, _| {
            let mut settings = helio_pass_voxel_planet::engine::Settings::default();
            settings.primary_samples |= std::env::var("PULSAR_VOXEL_PRIMARY_SAMPLES").ok().is_some_and(|value| value == "1");
            Box::new(PlanetPass::with_settings(Arc::clone(&frame), settings))
        })
    }

    fn needs_frame(&self, renderer: &helio::Renderer) -> bool {
        renderer
            .find_pass::<PlanetPass>()
            .is_some_and(PlanetPass::needs_frame)
    }

    fn diagnostics(&self, renderer: &helio::Renderer) -> Option<String> {
        let pass = renderer.find_pass::<PlanetPass>()?;
        let s = pass.stats()?;
        let mut line = format!(
            "planet ready={} resident={} pending={} jobs={} budget={} us_per_job={:.3} failed={} overflow={} levels={} finest={} plan={:.2}ms upload={:.2}ms encode={:.2}ms windows={:.2}ms needs_frame={} free_pages={} free_units={} job_status={:?} visible_blocks={} visible_attempts={} visible_overflow={} visible_urgent_blocks={} queued_bytes={} queued_ops={} wanted_capacity={}",
            s.ready,
            s.resident_columns,
            s.pending_columns,
            s.jobs,
            s.job_budget,
            s.us_per_job,
            s.failed_jobs,
            s.overflow_columns,
            s.active_levels,
            s.finest_level,
            s.plan_cpu_ms,
            s.upload_cpu_ms,
            s.encode_cpu_ms,
            s.window_rebuild_ms,
            pass.needs_frame(),
            s.free_pages,
            s.free_pool_units,
            s.jobs_by_status,
            s.visible_request_blocks,
            s.visible_request_attempts,
            s.visible_request_overflow,
            s.visible_urgent_blocks,
            s.queued_delta_bytes,
            s.queued_delta_ops,
            s.wanted_key_capacity,
        );
        if s.primary_sampling_enabled {
            use std::fmt::Write;
            if let Some(sample) = s.sampled_primary.filter(|sample|
                sample.captured_at.elapsed() <= std::time::Duration::from_millis(500)) {
                let _ = write!(line,
                    " primary_sample=sparse_projected_estimates source_encoded_frame={} source_frame={} age_frames={} age_ms={:.2} sampled_rays={} sampled_terrain={} sampled_coarse_over2px={} sampled_coarse_over4px={} sampled_unresolved={} sample_stride={} sample_view={} sample_viewport={}x{} sample_projection_y={} sample_eye={:?} sample_forward={:?} sample_up={:?}",
                    sample.encoded_frame, sample.source_frame, sample.age_frames,
                    sample.captured_at.elapsed().as_secs_f64() * 1000.0,
                    sample.sampled_rays, sample.terrain_hits, sample.coarse_over_2px,
                    sample.coarse_over_4px, sample.unresolved, sample.stride, sample.view_id,
                    sample.viewport[0], sample.viewport[1], sample.projection_y,
                    sample.eye.to_array(), sample.forward.to_array(), sample.up.to_array());
            } else {
                line.push_str(" primary_sample=unavailable");
            }
        }
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
                return Err("the voxel planet does not consume live sample chunks yet; edit with brushes".into());
            }
            Ok(_) => {}
            Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.clear()?;
                return Err("voxel source payload store was poisoned".into());
            }
        }
        let planet = match self.planet_for(entry) {
            Ok(planet) => planet,
            Err(error) => {
                self.clear()?;
                return Err(error);
            }
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
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planetary_environment_uses_scaled_world_eye_and_scene_sun_even_if_terrain_is_hidden() {
        let mut scene = World::new();
        let entity = scene.spawn();
        scene.insert(entity, planet_terrain());
        let (mut entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());
        entries[0].visible = false;
        entries[0].world.planet_radius = 12_742_000.0;
        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(PlanetVoxelBackend::new())).unwrap();
        let eye = DVec3::X * 12_745_000.0;
        let sky = registry.planetary_sky(&entries, eye, Some([2.0, 0.0, 0.0])).unwrap();
        assert_eq!(sky.eye_m, eye.to_array());
        assert_eq!(sky.radius_m, 12_742_000.0);
        assert_eq!(sky.sun_direction, [1.0, 0.0, 0.0]);
        entries[0].world.shape = VoxelWorldShape::Plane;
        assert!(registry.planetary_sky(&entries, eye, None).is_none());
    }
    use crate::scene::Visibility;
    use helio_component::VoxelTerrainComponent;
    use helio_voxel_data::VoxelStoredPayload;
    use pulsar_scenedb::World;

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
        VoxelBrushRequest { op: VoxelBrushOp::Remove, shape: VoxelBrushShape::Sphere, radius, material: 0, single_block: false }
    }

    fn raise(radius: f32) -> VoxelBrushRequest {
        VoxelBrushRequest {
            op: VoxelBrushOp::Add,
            material: helio_pass_voxel_planet::terrain::material::COBBLE,
            ..dig(radius)
        }
    }

    fn frame_planet(backend: &PlanetVoxelBackend) -> Arc<Planet> {
        backend.frame.lock().unwrap().as_ref().unwrap().planet.clone()
    }

    #[test]
    fn native_flight_surface_is_unavailable_until_published_and_offsets_once() {
        let mut scene = World::new();
        let entity = scene.spawn();
        scene.insert(entity, planet_terrain());
        let (mut entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());
        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(PlanetVoxelBackend::new())).unwrap();
        assert!(registry.diagnostic_surface_point(&entries, DVec3::Y, 2.0).is_none());
        assert!(registry.publish_frame(&entries, view(DVec3::Y * 6_374_000.0)).is_empty());
        let ground = registry.diagnostic_surface_point(&entries, DVec3::Y, 0.0).unwrap();
        let lifted = registry.diagnostic_surface_point(&entries, DVec3::Y, 32.0).unwrap();
        assert!((lifted.distance(ground) - 32.0).abs() < 1e-7, "clearance must be applied only by the canonical query");
        assert!(registry.diagnostic_surface_point(&entries, DVec3::ZERO, 2.0).is_none());
        assert!(registry.diagnostic_surface_point(&entries, DVec3::Y, f64::NAN).is_none());
        entries[0].visible = false;
        assert!(registry.diagnostic_surface_point(&entries, DVec3::Y, 2.0).is_none());
    }

    #[test]
    fn renderer_selection_preserves_the_planet_snapshot_between_camera_frames() {
        let mut scene = World::new();
        let entity = scene.spawn();
        scene.insert(entity, planet_terrain());
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty(), "{errors:?}");
        let eye = DVec3::new(0.0, 6_371_000.0 + 3_000.0, 0.0);

        let mut backend = PlanetVoxelBackend::new();
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
        assert_eq!(first.grid().voxel_size(), 0.1, "old frame snapshots remain immutable");

        let orbit = eye.normalize() * (coarse.grid().radius() + 300_000.0);
        let (near, far) = backend.camera_clip_range(&revised, orbit).unwrap();
        assert!(near > 1_000.0 && far > orbit.length() as f32);

        let mut invalid = revised.clone();
        invalid.generator.as_mut().unwrap().parameters = "{".into();
        assert!(backend.publish_frame(&[&invalid], view(eye)).is_err());
        assert!(backend.frame.lock().unwrap().is_none(), "invalid recipes must not retain stale terrain");
        backend.publish_frame(&[&revised], view(eye)).unwrap();

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
        let entity = scene.spawn();
        let mut terrain = planet_terrain();
        terrain.renderer_id.clear();
        scene.insert(entity, terrain);
        let (entries, projection_errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(projection_errors.is_empty());

        let backend = PlanetVoxelBackend::new();
        let frame = Arc::clone(&backend.frame);
        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(backend)).unwrap();
        let eye = DVec3::new(0.0, 6_371_000.0 + 3_000.0, 0.0);
        assert!(registry.publish_frame(&entries, view(eye)).is_empty());
        assert!(frame.lock().unwrap().is_some());

        scene.insert(entity, Visibility { visible: false, locked: false });
        let (hidden, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());
        assert!(!hidden[0].visible);
        assert!(registry.uses_outdoor_sky(&hidden));
        assert!(registry.publish_frame(&hidden, view(eye)).is_empty());
        assert!(frame.lock().unwrap().is_none());
    }

    #[test]
    fn exact_brush_edits_round_trip_through_the_terrain_journal() {
        let mut scene = World::new();
        let entity = scene.spawn();
        scene.insert(entity, planet_terrain());
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());
        let original = Planet::new(PlanetRecipe::default()).unwrap();
        let eye = original.surface_point(DVec3::new(0.2, 1.0, 0.3), 3.0);
        let down = -eye.normalize();
        let target = original.raycast(eye, down, 100.0).unwrap().cell;
        assert_ne!(original.material(target), 0);

        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(PlanetVoxelBackend::new())).unwrap();
        let commit = registry.edit_ray(&entries, eye, down, dig(0.5)).unwrap().unwrap();
        assert_eq!(commit.id, entries[0].id);
        let replay = |edits: &VoxelEditJournal| {
            let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
            for edit in edits.iter() {
                planet.apply(planet_brush(edit)).unwrap();
            }
            planet
        };
        assert_eq!(replay(&[commit.edit].into_iter().collect()).material(target), 0);
        // The same canonical cell is addressed from the ground, from orbit
        // and from far beyond the renderer's precision range.
        for distance in [2_000.0, 300_000.0, 1_000_000_000.0] {
            let remote = registry
                .edit_ray(&entries, eye - down * distance, down, dig(0.05))
                .unwrap()
                .expect("remote terrain remains editable");
            assert_eq!(replay(&[remote.edit].into_iter().collect()).material(target), 0);
            assert!(remote.distance > distance);
        }
        // Building fills the empty cell in front of the hit.
        let build = registry.edit_ray(&entries, eye, down, raise(0.05)).unwrap().unwrap();
        assert_eq!(build.edit.op, VoxelBrushOp::Add);
        assert_eq!(build.edit.material, helio_pass_voxel_planet::terrain::material::COBBLE);
        // One block, painted: a unit cube on the hit block.
        let paint = VoxelBrushRequest { op: VoxelBrushOp::Paint, material: helio_pass_voxel_planet::terrain::material::SNOW, single_block: true, ..raise(3.0) };
        let painted = registry.edit_ray(&entries, eye, down, paint).unwrap().unwrap();
        assert_eq!((painted.edit.shape, painted.edit.radius), (VoxelBrushShape::Cube, 0.05));
        let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
        planet.apply(planet_brush(&painted.edit)).unwrap();
        assert_eq!(planet.material(target), helio_pass_voxel_planet::terrain::material::SNOW);
        let invalid = VoxelBrushRequest { material: 99, ..raise(1.0) };
        assert!(registry.edit_ray(&entries, eye, down, invalid).is_err());

        assert!(super::super::renderer::apply_voxel_brush_commit(&mut scene, commit));
        let terrain = scene.get::<VoxelTerrainComponent>(entity).unwrap();
        assert_eq!(terrain.source_revision, 1);
        assert_eq!(terrain.edits.len(), 1);
        assert_eq!(replay(&terrain.edits).material(target), 0);

        // The backend extends its cached planet with the appended brush.
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut backend = PlanetVoxelBackend::new();
        backend.publish_frame(&[&entries[0]], view(eye)).unwrap();
        assert_eq!(frame_planet(&backend).material(target), 0);
    }

    #[test]
    fn altitude_is_height_above_the_ground_below() {
        let mut scene = World::new();
        let entity = scene.spawn();
        scene.insert(entity, planet_terrain());
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(PlanetVoxelBackend::new())).unwrap();
        let planet = Planet::new(PlanetRecipe::default()).unwrap();
        let ground = planet.surface_point(DVec3::Y, 2.0);
        assert!(registry.publish_frame(&entries, view(ground)).is_empty());
        let near = registry.altitude(&entries, ground).unwrap();
        let orbit = registry.altitude(&entries, ground * 1.05).unwrap();
        assert!((near - 2.0).abs() < 0.2 && orbit > 250_000.0, "{near} {orbit}");
        // 3 km over lowland: below the highest possible terrain, where a
        // conservative clearance is ~0, the camera still moves at altitude speed.
        let high = planet.surface_point(DVec3::Y, 3000.0);
        let altitude = registry.altitude(&entries, high).unwrap();
        assert!((altitude - 3000.0).abs() < 1.0, "{altitude}");
    }

    #[test]
    fn an_eye_inside_the_ground_is_lifted_but_dug_air_is_kept() {
        let mut scene = World::new();
        let entity = scene.spawn();
        scene.insert(entity, planet_terrain());
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(PlanetVoxelBackend::new())).unwrap();
        let planet = Planet::new(PlanetRecipe::default()).unwrap();
        let surface = planet.surface_point(DVec3::Y, 0.0);
        assert!(registry.publish_frame(&entries, view(surface + DVec3::Y * 2.0)).is_empty());
        let buried = surface - DVec3::Y * 3.0;
        let lifted = registry.lift_out_of_ground(buried).expect("inside the ground");
        assert!((lifted.y - surface.y - 0.5).abs() < 0.2, "{} {}", lifted.y, surface.y);
        assert!(registry.lift_out_of_ground(surface + DVec3::Y * 2.0).is_none());

        // Dig a cave around the buried point: the camera may stay in it.
        let dig = VoxelBrushEdit { center: buried.to_array(), radius: 1.5, shape: VoxelBrushShape::Sphere, op: VoxelBrushOp::Remove, material: 0 };
        scene.get_mut::<VoxelTerrainComponent>(entity).unwrap().edits.push(dig);
        scene.get_mut::<VoxelTerrainComponent>(entity).unwrap().source_revision += 1;
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(registry.publish_frame(&entries, view(surface + DVec3::Y * 2.0)).is_empty());
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
        };
        // 3 m apart with 0.6 m spacing: stamps at 0.6 m steps between them.
        let fill = stroke_fill(&edit(0.0), &edit(3.0), 0.1);
        assert_eq!(fill.len(), 4);
        assert!(fill.windows(2).all(|w| (w[1].center[0] - w[0].center[0] - 0.6).abs() < 1e-9));
        assert!(stroke_fill(&edit(0.0), &edit(0.5), 0.1).is_empty(), "close samples need no fill");
        assert!(stroke_fill(&edit(0.0), &edit(40.0), 0.1).is_empty(), "a jump starts a new segment");
        let paint = VoxelBrushEdit { op: VoxelBrushOp::Paint, material: 5, ..edit(3.0) };
        assert!(stroke_fill(&edit(0.0), &paint, 0.1).is_empty(), "another brush starts a new segment");
        // One-block cubes: a stamp per voxel.
        let block = |x: f64| VoxelBrushEdit { shape: VoxelBrushShape::Cube, radius: 0.05, op: VoxelBrushOp::Add, material: 13, ..edit(x) };
        assert_eq!(stroke_fill(&block(0.0), &block(1.0), 0.1).len(), 9);
    }

    #[test]
    fn a_landform_component_configures_the_generator() {
        let mut scene = World::new();
        let entity = scene.spawn();
        let mut terrain = planet_terrain();
        terrain.seed = 99;
        scene.insert(entity, terrain);
        let mut landform = helio_component::VoxelLandformComponent::default();
        landform.snowline_m = 1_234.0;
        landform.mountain_km = 55.0;
        scene.insert(entity, landform);
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty(), "{errors:?}");
        let mut backend = PlanetVoxelBackend::new();
        backend.publish_frame(&[&entries[0]], view(DVec3::new(0.0, 6_371_000.0 + 3_000.0, 0.0))).unwrap();
        let recipe = frame_planet(&backend).recipe().clone();
        assert_eq!(recipe.terrain.generator, helio_pass_voxel_planet::landform::ID);
        assert_eq!(recipe.terrain.seed, 99);
        let settings: helio_pass_voxel_planet::landform::Landform = serde_json::from_str(&recipe.terrain.settings).unwrap();
        assert_eq!(settings.snowline_m, 1_234.0);
        assert_eq!(settings.mountain_km, 55.0);
    }

    #[test]
    fn shared_ids_name_the_registered_landform_generator() {
        assert_eq!(VOXEL_TERRAIN_GENERATOR, helio_pass_voxel_planet::landform::ID);
        assert_eq!(VOXEL_TERRAIN_GENERATOR_VERSION, helio_pass_voxel_planet::landform::VERSION);
    }

    #[test]
    fn a_flat_terrain_uses_its_settings_component() {
        let mut scene = World::new();
        let entity = scene.spawn();
        let mut terrain = VoxelTerrainComponent::plane(1_024.0);
        terrain.generator.id = helio_pass_voxel_planet::landform::FLAT_ID.into();
        terrain.generator.version = helio_pass_voxel_planet::landform::FLAT_VERSION;
        scene.insert(entity, terrain);
        let mut flat = helio_component::VoxelFlatTerrainComponent::default();
        flat.height = 12.0;
        flat.surface = helio_component::VoxelTerrainMaterial::Sand;
        scene.insert(entity, flat);
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty(), "{errors:?}");
        let mut backend = PlanetVoxelBackend::new();
        backend.publish_frame(&[&entries[0]], view(DVec3::new(0.0, 40.0, 0.0))).unwrap();
        let planet = frame_planet(&backend);
        let hit = planet.raycast(DVec3::new(3.0, 40.0, -5.0), -DVec3::Y, 100.0).unwrap();
        assert!((hit.distance - 28.0).abs() < 0.11, "{}", hit.distance);
        assert_eq!(planet.material(hit.cell), helio_pass_voxel_planet::terrain::material::SAND);
    }

    #[test]
    fn unknown_generators_are_rejected_with_the_registered_list() {
        let mut scene = World::new();
        let entity = scene.spawn();
        let mut terrain = planet_terrain();
        terrain.generator.id = "example.none".into();
        scene.insert(entity, terrain);
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let error = PlanetVoxelBackend::validate_source(&entries[0]).unwrap_err();
        assert!(error.contains("helio.landform") && error.contains("helio.flat"), "{error}");
    }

    #[test]
    fn plane_worlds_follow_the_component_shape_and_size() {
        for shape in [VoxelWorldShape::Plane, VoxelWorldShape::InfinitePlane] {
            let mut scene = World::new();
            let entity = scene.spawn();
            let mut terrain = planet_terrain();
            terrain.shape = shape;
            terrain.plane_size = 2_048.0;
            scene.insert(entity, terrain);
            let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
            assert!(errors.is_empty(), "{errors:?}");
            let mut registry = VoxelBackendRegistry::new();
            let backend = PlanetVoxelBackend::new();
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
            assert_eq!(registry.ambient_up(&entries, eye), Some(DVec3::Y));
            // Digging straight down removes the cell below the eye.
            let ground = planet.surface_point(DVec3::new(10.0, 0.0, -20.0), 3.0);
            let target = planet.raycast(ground, -DVec3::Y, 100.0).unwrap().cell;
            let commit = registry.edit_ray(&entries, ground, -DVec3::Y, dig(0.5)).unwrap().unwrap();
            assert!(super::super::renderer::apply_voxel_brush_commit(&mut scene, commit));
            let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
            assert!(registry.publish_frame(&entries, view(eye)).is_empty());
            assert_eq!(frame.lock().unwrap().as_ref().unwrap().planet.material(target), 0);
        }
    }
}
