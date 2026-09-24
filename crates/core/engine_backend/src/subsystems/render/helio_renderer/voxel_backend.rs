//! Renderer registration for SceneDB voxel terrain sources.
//!
//! Source rows carry opaque renderer IDs and recipes. Each backend owns its
//! pass and translates only the rows it understands into a frame snapshot.

use std::sync::{Arc, Mutex};

use glam::{DVec3, Vec3};
use helio_default_graphs::VoxelPassFactory;
use helio_pass_tiny_voxel::{
    engine::{EngineVoxelFrame, LazyEngineVoxelPass, SharedVoxelFrame},
    world::{render_origin, GENERATOR_REVISION},
    Params, World as TinyWorld,
};
use helio_voxel_data::VoxelDomain;

use crate::scene::voxel_frame::{VoxelEntryId, VoxelGeneratorConfig, VoxelSceneEntry};

pub const TINY_VOXEL_RENDERER_ID: &str = "helio.tiny-voxel";
pub const TINY_VOXEL_GENERATOR_ID: &str = "helio.tiny-voxel.default";

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
}

pub trait VoxelRenderBackend: Send {
    fn renderer_id(&self) -> &'static str;
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

    pub fn needs_frame(&self, renderer: &helio::Renderer) -> bool {
        self.backends
            .iter()
            .any(|backend| backend.needs_frame(renderer))
    }

    pub fn publish_frame(&mut self, entries: &[VoxelSceneEntry], view: VoxelView) -> Vec<String> {
        let mut errors = Vec::new();
        let mut selected = vec![Vec::new(); self.backends.len()];
        for entry in entries {
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

pub struct TinyVoxelBackend {
    frame: SharedVoxelFrame,
    cached_recipe: Option<(VoxelEntryId, u64, VoxelGeneratorConfig, Arc<TinyWorld>)>,
}

impl TinyVoxelBackend {
    pub fn new() -> Self {
        Self {
            frame: Arc::new(Mutex::new(None)),
            cached_recipe: None,
        }
    }

    fn clear(&mut self) -> Result<(), String> {
        self.cached_recipe = None;
        *self
            .frame
            .lock()
            .map_err(|_| "frame mailbox was poisoned")? = None;
        Ok(())
    }

    fn validate_source(entry: &VoxelSceneEntry) -> Result<&VoxelGeneratorConfig, String> {
        let generator = entry.generator.as_ref().ok_or("generator ID is required")?;
        if generator.id != TINY_VOXEL_GENERATOR_ID
            || generator.version != GENERATOR_REVISION
            || generator.seed != 0
        {
            return Err(
                "this backend requires its matching generator ID, revision, and zero seed".into(),
            );
        }
        if (entry.voxel_size - 0.1).abs() > 1.0e-9
            || entry.chunk_edge_voxels != 32
            || entry.lod_scale != 2
            || entry.origin != [0.0; 3]
        {
            return Err("this backend requires 0.1 m voxels, 32-cell chunks, LOD scale 2, and world origin at the planet center".into());
        }
        if !matches!(entry.domain, VoxelDomain::Unbounded { .. }) {
            return Err("this planet backend requires an unbounded source domain".into());
        }
        Ok(generator)
    }

    fn params(view: VoxelView) -> Params {
        let eye = DVec3::from_array(view.position);
        let origin = render_origin(eye);
        let fraction = std::array::from_fn(|axis| {
            if axis < 3 {
                (eye[axis] / 0.1 - f64::from(origin[axis])) as f32
            } else {
                0.0
            }
        });
        let radial = eye.normalize_or_zero().as_vec3();
        let light = Vec3::new(0.4, 0.8, 0.3).normalize();
        Params {
            origin: [origin[0], origin[1], origin[2], 0],
            fraction,
            radial: [radial.x, radial.y, radial.z, 0.0],
            right: [view.right[0], view.right[1], view.right[2], view.aspect],
            up: [view.up[0], view.up[1], view.up[2], view.tan_half_fov_y],
            forward: [view.forward[0], view.forward[1], view.forward[2], 0.0],
            screen: [view.size[0] as f32, view.size[1] as f32, 0.0, 0.0],
            lighting: [light.x, light.y, light.z, 0.0],
            settings: [view.far, 0.0, 1.0, 0.0],
        }
    }
}

impl Default for TinyVoxelBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl VoxelRenderBackend for TinyVoxelBackend {
    fn renderer_id(&self) -> &'static str {
        TINY_VOXEL_RENDERER_ID
    }

    fn supports(&self, source: &VoxelSceneEntry) -> bool {
        source
            .generator
            .as_ref()
            .is_some_and(|generator| generator.id == TINY_VOXEL_GENERATOR_ID)
    }

    fn pass_factory(&self) -> VoxelPassFactory {
        let frame = Arc::clone(&self.frame);
        Arc::new(move |_, _, _, _| Box::new(LazyEngineVoxelPass::new(Arc::clone(&frame))))
    }

    fn needs_frame(&self, renderer: &helio::Renderer) -> bool {
        renderer
            .find_pass::<LazyEngineVoxelPass>()
            .is_some_and(LazyEngineVoxelPass::needs_frame)
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
            return Err("multiple terrains selected the single-world tiny voxel backend".into());
        };
        let generator = match Self::validate_source(entry) {
            Ok(generator) => generator.clone(),
            Err(error) => {
                self.clear()?;
                return Err(error);
            }
        };
        // This pass rebuilds bricks from its recipe. Other formats in the
        // component's live chunk store must not be silently ignored.
        match entry.store.try_read() {
            Ok(state) if !state.1.is_empty() => {
                self.clear()?;
                return Err("this backend does not consume live chunk payloads; supply edits in its versioned generator recipe".into());
            }
            Ok(_) => {}
            Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                self.clear()?;
                return Err("voxel source payload store was poisoned".into());
            }
        }
        let world = match &self.cached_recipe {
            Some((entity, revision, cached, world))
                if *entity == entry.id
                    && *revision == entry.source_revision
                    && *cached == generator =>
            {
                Arc::clone(world)
            }
            _ => {
                let world = Arc::new(TinyWorld::from_recipe_json(&generator.parameters)?);
                self.cached_recipe = Some((
                    entry.id,
                    entry.source_revision,
                    generator,
                    Arc::clone(&world),
                ));
                world
            }
        };
        *self
            .frame
            .lock()
            .map_err(|_| "frame mailbox was poisoned")? = Some(EngineVoxelFrame {
            params: Self::params(view),
            world,
            raytraced_sun: false,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_component::VoxelTerrainComponent;
    use helio_pass_tiny_voxel::world::RADIUS;
    use helio_voxel_data::VoxelStoredPayload;
    use pulsar_scenedb::World;

    fn view() -> VoxelView {
        VoxelView {
            position: [0.0, RADIUS + 3.0, 0.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, -1.0],
            forward: [0.0, -1.0, 0.0],
            tan_half_fov_y: 0.41421357,
            aspect: 16.0 / 9.0,
            far: 10_000.0,
            size: [1600, 900],
        }
    }

    #[test]
    fn renderer_selection_preserves_the_world_snapshot_between_camera_frames() {
        let mut scene = World::new();
        let entity = scene.spawn();
        let mut terrain = VoxelTerrainComponent::default();
        terrain.renderer_id = TINY_VOXEL_RENDERER_ID.into();
        terrain.generator_id = TINY_VOXEL_GENERATOR_ID.into();
        terrain.generator_version = GENERATOR_REVISION;
        terrain.voxel_size = 0.1;
        terrain.chunk_edge_voxels = 32;
        terrain.lod_scale = 2;
        scene.insert(entity, terrain);
        let (entries, errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(errors.is_empty());

        let mut backend = TinyVoxelBackend::new();
        backend.publish_frame(&[&entries[0]], view()).unwrap();
        let first = backend
            .frame
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .world
            .clone();
        backend.publish_frame(&[&entries[0]], view()).unwrap();
        let second = backend
            .frame
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .world
            .clone();
        assert!(Arc::ptr_eq(&first, &second));

        let mut revised = entries[0].clone();
        revised.source_revision += 1;
        backend.publish_frame(&[&revised], view()).unwrap();
        let third = backend
            .frame
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .world
            .clone();
        assert!(!Arc::ptr_eq(&first, &third));

        revised
            .store
            .write()
            .unwrap()
            .1
            .insert([0; 4], VoxelStoredPayload::raw_material(vec![1u8; 512]));
        assert!(backend.publish_frame(&[&revised], view()).is_err());
        assert!(backend.frame.lock().unwrap().is_none());

        backend.publish_frame(&[], view()).unwrap();
        assert!(backend.frame.lock().unwrap().is_none());
    }

    #[test]
    fn empty_renderer_id_selects_a_unique_compatible_backend() {
        let mut scene = World::new();
        let entity = scene.spawn();
        let mut terrain = VoxelTerrainComponent::default();
        terrain.generator_id = TINY_VOXEL_GENERATOR_ID.into();
        terrain.generator_version = GENERATOR_REVISION;
        terrain.voxel_size = 0.1;
        terrain.chunk_edge_voxels = 32;
        scene.insert(entity, terrain);
        let (entries, projection_errors) = crate::scene::voxel_frame::project_voxel_entries(&scene);
        assert!(projection_errors.is_empty());

        let backend = TinyVoxelBackend::new();
        let frame = Arc::clone(&backend.frame);
        let mut registry = VoxelBackendRegistry::new();
        registry.register(Box::new(backend)).unwrap();
        assert!(registry.publish_frame(&entries, view()).is_empty());
        assert!(frame.lock().unwrap().is_some());
    }
}
