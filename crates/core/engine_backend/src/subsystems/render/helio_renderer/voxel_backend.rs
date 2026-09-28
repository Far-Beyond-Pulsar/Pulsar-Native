//! Renderer registration for SceneDB voxel terrain sources.
//!
//! Source rows carry opaque renderer IDs and recipes. Each backend owns its
//! pass and translates only the rows it understands into a frame snapshot.

use std::sync::{Arc, Mutex};

use glam::{DVec3, Vec3};
use helio_default_graphs::VoxelPassFactory;
use helio_component::VoxelWorldShape;
use helio_pass_voxel_planet::{
    engine::{PlanetFrame, PlanetPass, SharedPlanetFrame},
    field::Landform,
    grid::Shape,
    Brush, BrushOp, BrushShape, Planet, PlanetRecipe,
};
use helio_voxel_data::{VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape};

use crate::scene::voxel_frame::{VoxelEntryId, VoxelGeneratorConfig, VoxelSceneEntry};

pub const VOXEL_PLANET_RENDERER_ID: &str = "helio.voxel-planet";
pub const VOXEL_PLANET_GENERATOR_ID: &str = "helio.voxel-planet.default";
pub const VOXEL_PLANET_GENERATOR_VERSION: u32 = 1;

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
pub struct VoxelBrushCommit {
    pub id: VoxelEntryId,
    pub distance: f64,
    pub edit: VoxelBrushEdit,
}

pub trait VoxelRenderBackend: Send {
    fn renderer_id(&self) -> &'static str;
    /// Choose temporal resolve for this backend at the current viewport size.
    fn temporal_quality(&self, _size: [u32; 2]) -> Option<helio_pass_tsr::TsrQuality> {
        None
    }
    /// This backend renders in camera-local coordinates while retaining the
    /// precise world-space eye in `VoxelView`.
    fn camera_relative(&self) -> bool {
        false
    }
    /// Outdoor backends may use the sky pass's default atmosphere when the
    /// scene has no explicitly authored sky component.
    fn outdoor_sky(&self) -> bool {
        false
    }
    /// Local vertical at `eye` for hemisphere ambient (a planet's radial).
    fn ambient_up(&self, _source: &VoxelSceneEntry, _eye: DVec3) -> Option<DVec3> {
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
        _radius: f32,
        _material: u32,
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

    pub fn frame_environment(&self, entries: &[VoxelSceneEntry]) -> (bool, bool) {
        let selected = self.backends.iter().filter(|backend| {
            entries.iter().any(|entry| {
                if entry.renderer_id.is_empty() {
                    backend.supports(entry)
                } else {
                    entry.renderer_id == backend.renderer_id()
                }
            })
        });
        selected.fold((false, false), |(relative, sky), backend| {
            (
                relative || backend.camera_relative(),
                sky || backend.outdoor_sky(),
            )
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
        radius: f32,
        material: u32,
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
                    backend.edit_ray(entry, origin, direction, radius, material)?
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

    pub fn needs_frame(&self, renderer: &helio::Renderer) -> bool {
        self.backends
            .iter()
            .any(|backend| backend.needs_frame(renderer))
    }

    pub fn publish_frame(&mut self, entries: &[VoxelSceneEntry], view: VoxelView) -> Vec<String> {
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

/// The generator's landform from its parameters: the entity's
/// `VoxelLandformComponent` (serialized by the scene projection) or a raw
/// JSON string; empty means the defaults. A nonzero component `seed`
/// replaces the landform seed.
pub fn planet_landform(parameters: &str, seed: u64) -> Result<Landform, String> {
    let mut landform: Landform = if parameters.trim().is_empty() {
        Landform::default()
    } else {
        serde_json::from_str(parameters).map_err(|error| format!("invalid voxel planet landform: {error}"))?
    };
    if seed != 0 {
        landform.seed = seed as u32;
    }
    Ok(landform)
}

/// The world recipe: the component's shape, size and voxel size with a landform.
fn world_recipe(entry: &VoxelSceneEntry, landform: Landform) -> PlanetRecipe {
    PlanetRecipe {
        shape: match entry.world.shape {
            VoxelWorldShape::Sphere => Shape::Sphere,
            VoxelWorldShape::Plane => Shape::Plane,
            VoxelWorldShape::InfinitePlane => Shape::InfinitePlane,
        },
        radius_m: entry.world.planet_radius,
        plane_size_m: entry.world.plane_size,
        voxel_size_m: entry.voxel_size,
        landform,
        ..PlanetRecipe::default()
    }
}

/// A generic journal brush as a planet brush.
fn planet_brush(edit: &VoxelBrushEdit) -> Brush {
    Brush {
        center: edit.center,
        radius: edit.radius,
        shape: match edit.shape {
            VoxelBrushShape::Sphere => BrushShape::Sphere,
            VoxelBrushShape::Cube => BrushShape::Cube,
        },
        op: match edit.op {
            VoxelBrushOp::Remove => BrushOp::Remove,
            VoxelBrushOp::Add => BrushOp::Add,
            VoxelBrushOp::Paint => BrushOp::Paint,
        },
        material: edit.material,
    }
}

/// Build the world of an entry: its landform with every journal brush.
fn build_planet(entry: &VoxelSceneEntry, generator: &VoxelGeneratorConfig) -> Result<Planet, String> {
    let mut planet = Planet::new(world_recipe(entry, planet_landform(&generator.parameters, generator.seed)?))?;
    for edit in &entry.edits {
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
    edits: Vec<VoxelBrushEdit>,
    planet: Arc<Planet>,
}

/// Destructible voxel planet (`helio-pass-voxel-planet`): a GPU-driven clipmap
/// of exact voxels from 0.1 m to 1 m, rendered camera-relative with traced
/// sunlight.
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
        if generator.id != VOXEL_PLANET_GENERATOR_ID || generator.version != VOXEL_PLANET_GENERATOR_VERSION {
            return Err(format!(
                "this backend requires generator '{VOXEL_PLANET_GENERATOR_ID}' version {VOXEL_PLANET_GENERATOR_VERSION}"
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
                for edit in &entry.edits[cached.edits.len()..] {
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

/// Tool material 1 builds with cobblestone; other palette indices pass
/// through when the planet knows them.
fn build_material(material: u32) -> u32 {
    if material == 1 || material >= helio_pass_voxel_planet::field::material::COUNT {
        helio_pass_voxel_planet::field::material::COBBLE
    } else {
        material
    }
}

impl VoxelRenderBackend for PlanetVoxelBackend {
    fn renderer_id(&self) -> &'static str {
        VOXEL_PLANET_RENDERER_ID
    }

    fn temporal_quality(&self, size: [u32; 2]) -> Option<helio_pass_tsr::TsrQuality> {
        let pixels = u64::from(size[0]) * u64::from(size[1]);
        Some(if pixels > 1_500_000 {
            helio_pass_tsr::TsrQuality::Quality
        } else {
            helio_pass_tsr::TsrQuality::Native
        })
    }

    fn camera_relative(&self) -> bool {
        true
    }

    fn outdoor_sky(&self) -> bool {
        true
    }

    fn ambient_up(&self, source: &VoxelSceneEntry, eye: DVec3) -> Option<DVec3> {
        match source.world.shape {
            VoxelWorldShape::Sphere => eye.try_normalize(),
            VoxelWorldShape::Plane | VoxelWorldShape::InfinitePlane => Some(DVec3::Y),
        }
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
        radius: f32,
        material: u32,
    ) -> Result<Option<VoxelBrushCommit>, String> {
        let generator = Self::validate_source(source)?;
        let planet = match self.cached_planet(source) {
            Some(planet) => (**planet).clone(),
            None => build_planet(source, generator)?,
        };
        // Rays are clipped to the planet shell, not to a draw distance or a
        // tool reach: orbital edits use the same exact cells.
        let Some(hit) = planet.raycast(origin, direction.normalize(), f64::INFINITY) else {
            return Ok(None);
        };
        let (cell, op) = if material == 0 { (hit.cell, VoxelBrushOp::Remove) } else { (hit.previous, VoxelBrushOp::Add) };
        let edit = VoxelBrushEdit {
            center: planet.grid().cell_center(cell).to_array(),
            radius: f64::from(radius).max(planet.grid().voxel_size() * 0.5),
            shape: VoxelBrushShape::Sphere,
            op,
            material: if material == 0 { 0 } else { build_material(material) },
        };
        let mut check = planet;
        check.apply(planet_brush(&edit))?;
        Ok(Some(VoxelBrushCommit { id: source.id, distance: hit.distance, edit }))
    }

    fn supports(&self, source: &VoxelSceneEntry) -> bool {
        source
            .generator
            .as_ref()
            .is_some_and(|generator| generator.id == VOXEL_PLANET_GENERATOR_ID)
    }

    fn pass_factory(&self) -> VoxelPassFactory {
        let frame = Arc::clone(&self.frame);
        Arc::new(move |_, _, _, _| Box::new(PlanetPass::new(Arc::clone(&frame))))
    }

    fn needs_frame(&self, renderer: &helio::Renderer) -> bool {
        renderer
            .find_pass::<PlanetPass>()
            .is_some_and(PlanetPass::needs_frame)
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
    use crate::scene::Visibility;
    use helio_component::VoxelTerrainComponent;
    use helio_voxel_data::VoxelStoredPayload;
    use pulsar_scenedb::World;

    fn planet_terrain() -> VoxelTerrainComponent {
        let mut terrain = VoxelTerrainComponent::default();
        terrain.shape = VoxelWorldShape::Sphere;
        terrain.renderer_id = VOXEL_PLANET_RENDERER_ID.into();
        terrain.generator_id = VOXEL_PLANET_GENERATOR_ID.into();
        terrain.generator_version = VOXEL_PLANET_GENERATOR_VERSION;
        terrain.generator_parameters = String::new();
        terrain.voxel_size = 0.1;
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

    fn frame_planet(backend: &PlanetVoxelBackend) -> Arc<Planet> {
        backend.frame.lock().unwrap().as_ref().unwrap().planet.clone()
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
        assert_eq!(registry.frame_environment(&hidden), (true, true));
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
        let commit = registry.edit_ray(&entries, eye, down, 0.5, 0).unwrap().unwrap();
        assert_eq!(commit.id, entries[0].id);
        let replay = |edits: &[VoxelBrushEdit]| {
            let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
            for edit in edits {
                planet.apply(planet_brush(edit)).unwrap();
            }
            planet
        };
        assert_eq!(replay(&[commit.edit]).material(target), 0);
        // The same canonical cell is addressed from the ground, from orbit
        // and from far beyond the renderer's precision range.
        for distance in [2_000.0, 300_000.0, 1_000_000_000.0] {
            let remote = registry
                .edit_ray(&entries, eye - down * distance, down, 0.05, 0)
                .unwrap()
                .expect("remote terrain remains editable");
            assert_eq!(replay(&[remote.edit]).material(target), 0);
            assert!(remote.distance > distance);
        }
        // Building fills the empty cell in front of the hit.
        let build = registry.edit_ray(&entries, eye, down, 0.05, 1).unwrap().unwrap();
        assert_eq!(build.edit.op, VoxelBrushOp::Add);
        assert_eq!(build.edit.material, helio_pass_voxel_planet::field::material::COBBLE);

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
        assert_eq!(recipe.landform.snowline_m, 1_234.0);
        assert_eq!(recipe.landform.mountain_km, 55.0);
        assert_eq!(recipe.landform.seed, 99);
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
            let commit = registry.edit_ray(&entries, ground, -DVec3::Y, 0.5, 0).unwrap().unwrap();
            assert!(super::super::renderer::apply_voxel_brush_commit(&mut scene, commit));
            let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
            assert!(registry.publish_frame(&entries, view(eye)).is_empty());
            assert_eq!(frame.lock().unwrap().as_ref().unwrap().planet.material(target), 0);
        }
    }
}
