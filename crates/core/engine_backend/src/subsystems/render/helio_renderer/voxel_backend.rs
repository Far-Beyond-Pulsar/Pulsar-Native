//! Renderer registration for SceneDB voxel terrain sources.
//!
//! Source rows carry opaque renderer IDs and recipes. Each backend owns its
//! pass and translates only the rows it understands into a frame snapshot.

use std::sync::{Arc, Mutex};

use glam::{DVec3, Vec3};
use helio_default_graphs::VoxelPassFactory;
use helio_pass_voxel_planet::{
    engine::{PlanetFrame, PlanetPass, SharedPlanetFrame},
    Brush, BrushOp, BrushShape, Planet, PlanetRecipe,
};
use helio_voxel_data::VoxelDomain;

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

/// A backend edit becomes an update to its opaque source recipe. SceneDB owns
/// the component and persists this string with the level.
pub struct VoxelBrushCommit {
    pub id: VoxelEntryId,
    pub distance: f64,
    pub recipe: String,
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

/// Authored source recipe stored in the terrain's `generator_parameters`: the
/// landform and the ordered brush edits. SceneDB persists it with the level;
/// the backend rebuilds its planet from it (appending edits incrementally).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlanetSourceRecipe {
    /// Radius and landform. The component's `voxel_size` overrides the
    /// recipe's voxel size, and a nonzero component `seed` its landform seed.
    pub planet: PlanetRecipe,
    /// Ordered destruction and construction brushes.
    pub edits: Vec<Brush>,
}

impl PlanetSourceRecipe {
    pub fn from_json(json: &str) -> Result<Self, String> {
        if json.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(json).map_err(|error| format!("invalid voxel planet recipe: {error}"))
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    /// The planet recipe with the component's voxel size and seed applied.
    fn planet_recipe(&self, entry: &VoxelSceneEntry, seed: u64) -> PlanetRecipe {
        let mut recipe = self.planet.clone();
        recipe.voxel_size_m = entry.voxel_size;
        if seed != 0 {
            recipe.landform.seed = seed as u32;
        }
        recipe
    }
}

/// Built planet for one source revision.
struct CachedPlanet {
    id: VoxelEntryId,
    revision: u64,
    generator: VoxelGeneratorConfig,
    voxel_size: f64,
    /// Decoded recipe: a newer recipe that only appends edits reuses the planet.
    recipe: PlanetSourceRecipe,
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
        // The planet owns its acceleration layout; component chunk/LOD
        // metadata describes generic live payloads and does not apply.
        if entry.origin != [0.0; 3] {
            return Err("a voxel planet requires its world origin at the planet centre".into());
        }
        if !matches!(entry.domain, VoxelDomain::Unbounded { .. }) {
            return Err("a voxel planet requires an unbounded source domain".into());
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
            {
                return Ok(Arc::clone(&cached.planet));
            }
        }
        let recipe = PlanetSourceRecipe::from_json(&generator.parameters)?;
        let planet = match &self.cached {
            // A sculpt stroke appends brushes to an otherwise equal recipe.
            Some(cached)
                if cached.id == entry.id
                    && cached.voxel_size == entry.voxel_size
                    && cached.generator.seed == generator.seed
                    && cached.recipe.planet == recipe.planet
                    && recipe.edits.starts_with(&cached.recipe.edits) =>
            {
                let mut planet = (*cached.planet).clone();
                for brush in &recipe.edits[cached.recipe.edits.len()..] {
                    planet.apply(*brush)?;
                }
                planet
            }
            _ => {
                let mut planet = Planet::new(recipe.planet_recipe(entry, generator.seed))?;
                for brush in &recipe.edits {
                    planet.apply(*brush)?;
                }
                planet
            }
        };
        let planet = Arc::new(planet);
        self.cached = Some(CachedPlanet {
            id: entry.id,
            revision: entry.source_revision,
            generator,
            voxel_size: entry.voxel_size,
            recipe,
            planet: Arc::clone(&planet),
        });
        Ok(planet)
    }

    fn cached_planet(&self, entry: &VoxelSceneEntry) -> Option<&Arc<Planet>> {
        self.cached
            .as_ref()
            .filter(|c| c.id == entry.id && c.revision == entry.source_revision && entry.generator.as_ref() == Some(&c.generator))
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

    fn ambient_up(&self, _source: &VoxelSceneEntry, eye: DVec3) -> Option<DVec3> {
        eye.try_normalize()
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
        let mut recipe = PlanetSourceRecipe::from_json(&generator.parameters)?;
        let planet = match self.cached_planet(source) {
            Some(planet) => (**planet).clone(),
            None => {
                let mut planet = Planet::new(recipe.planet_recipe(source, generator.seed))?;
                for brush in &recipe.edits {
                    planet.apply(*brush)?;
                }
                planet
            }
        };
        // Rays are clipped to the planet shell, not to a draw distance or a
        // tool reach: orbital edits use the same exact cells.
        let Some(hit) = planet.raycast(origin, direction.normalize(), f64::INFINITY) else {
            return Ok(None);
        };
        let (cell, op) = if material == 0 { (hit.cell, BrushOp::Remove) } else { (hit.previous, BrushOp::Add) };
        let brush = Brush {
            center: planet.grid().cell_center(cell).to_array(),
            radius: f64::from(radius).max(planet.grid().voxel_size() * 0.5),
            shape: BrushShape::Sphere,
            op,
            material: if material == 0 { 0 } else { build_material(material) },
        };
        let mut check = planet;
        check.apply(brush)?;
        recipe.edits.push(brush);
        Ok(Some(VoxelBrushCommit {
            id: source.id,
            distance: hit.distance,
            recipe: recipe.to_json()?,
        }))
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
                return Err("the voxel planet does not consume live chunk payloads yet; edit through its recipe".into());
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
    fn exact_brush_edits_round_trip_through_the_terrain_recipe() {
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
        let replay = |json: &str| {
            let recipe = PlanetSourceRecipe::from_json(json).unwrap();
            let mut planet = Planet::new(recipe.planet.clone()).unwrap();
            for brush in &recipe.edits {
                planet.apply(*brush).unwrap();
            }
            planet
        };
        assert_eq!(replay(&commit.recipe).material(target), 0);
        // The same canonical cell is addressed from the ground, from orbit
        // and from far beyond the renderer's precision range.
        for distance in [2_000.0, 300_000.0, 1_000_000_000.0] {
            let remote = registry
                .edit_ray(&entries, eye - down * distance, down, 0.05, 0)
                .unwrap()
                .expect("remote terrain remains editable");
            assert_eq!(replay(&remote.recipe).material(target), 0);
            assert!(remote.distance > distance);
        }
        // Building fills the empty cell in front of the hit.
        let build = registry.edit_ray(&entries, eye, down, 0.05, 1).unwrap().unwrap();
        let built = PlanetSourceRecipe::from_json(&build.recipe).unwrap();
        assert_eq!(built.edits[0].op, BrushOp::Add);
        assert_eq!(built.edits[0].material, helio_pass_voxel_planet::field::material::COBBLE);

        assert!(super::super::renderer::apply_voxel_brush_commit(&mut scene, commit));
        let terrain = scene.get::<VoxelTerrainComponent>(entity).unwrap();
        assert_eq!(terrain.source_revision, 1);
        assert_eq!(replay(&terrain.generator_parameters).material(target), 0);

        // The backend extends its cached planet with the appended brush.
        let (entries, _) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        let mut backend = PlanetVoxelBackend::new();
        backend.publish_frame(&[&entries[0]], view(eye)).unwrap();
        assert_eq!(frame_planet(&backend).material(target), 0);
    }
}
