//! Pulsar scene loader — canonical implementation shared by game runtime and editor engine.
//!
//! ## Design
//!
//! Component dispatch goes through the **inventory registration system**:
//!
//! 1. Each component crate (e.g. `pulsar_rendering`) submits a
//!    `RuntimeBehaviorRegistration` via `inventory::submit!` in its
//!    `#[register_runtime_behavior]` proc-macro expansion.
//! 2. The loader creates a [`SceneObjectContext`] that implements
//!    [`ComponentRuntimeContext`] and owns all renderer state needed to
//!    materialise lights and meshes.
//! 3. `apply_runtime_behavior_for_class` iterates the inventory and calls the
//!    matching component's `sync_component` — which parses its own fields and
//!    calls `context.upsert_light` / `context.upsert_mesh`.
//!
//! The loader **never touches component field values**.  All parsing, defaults,
//! and unit conversions live inside the component's `sync_component`.  Adding a
//! new field to `LightComponent` automatically works here with zero loader edits.
//!
//! ## Linker note
//!
//! `pulsar_rendering` types are re-exported from `pulsar_scene::rendering` to
//! create a live code reference.  Without it the linker can silently drop
//! `pulsar_rendering`'s `#[used]` inventory statics.

use std::collections::HashMap;
use std::path::Path;

use glam::{EulerRot, Mat4, Quat, Vec3};
use helio::Renderer;
use serde_json::Value;

use pulsar_reflection::{
    apply_runtime_behavior_for_class, ComponentRuntimeContext, LiveKeySet, RuntimeComponentOwner,
    Subsystems,
};
use pulsar_rendering::subsystems::{MeshCache, SceneObjectCache};

use crate::format::{SceneFile, SceneLoadError};

// ── Force pulsar_rendering into the binary ────────────────────────────────────
// Re-exporting these types creates a live symbol reference that prevents the
// linker from dropping pulsar_rendering's #[used] inventory statics.
// (ComponentRuntimeContext dispatch only works if those statics are linked in.)
pub use pulsar_rendering::LightComponent as _ForceLink_LightComponent;
pub use pulsar_rendering::PlanetTerrainComponent as _ForceLink_PlanetTerrainComponent;
pub use pulsar_rendering::ScriptComponent as _ForceLink_ScriptComponent;
pub use pulsar_rendering::StaticMeshComponent as _ForceLink_StaticMeshComponent;

// ── SceneLoader ───────────────────────────────────────────────────────────────

pub struct SceneLoader;

impl SceneLoader {
    pub fn load_file(
        path: &Path,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<(), SceneLoadError> {
        let scene = SceneFile::load(path)?;
        Self::load_scene(&scene, project_root, renderer)
    }

    pub fn load_scene(
        scene: &SceneFile,
        project_root: &Path,
        renderer: &mut Renderer,
    ) -> Result<(), SceneLoadError> {
        Self::load_objects(&scene.objects, project_root, renderer);
        Ok(())
    }

    /// Core loader — dispatches every scene object through the component system.
    ///
    /// Each object gets a [`SceneObjectContext`] implementing
    /// [`ComponentRuntimeContext`].  `apply_runtime_behavior_for_class` calls the
    /// matching component's `sync_component`, which owns all parsing and renderer
    /// interaction.  The loader never touches component field values.
    ///
    /// V1 objects (no `__component_instances`) have synthetic component data
    /// constructed from their flat props and dispatched through the same path.
    pub fn load_objects(
        objects: &[crate::format::SceneObject],
        project_root: &Path,
        mut renderer: &mut Renderer,
    ) {
        tracing::info!(total = objects.len(), "Loading scene objects");

        for obj in objects {
            if !obj.visible {
                continue;
            }
            tracing::debug!(id = obj.id, name = obj.name, "Scene object");

            let owner = RuntimeComponentOwner {
                scene_object_id: &obj.id,
                position: obj.world_position(),
                rotation: obj.world_rotation(),
                scale: obj.world_scale(),
                props: &obj.props,
            };

            let instances =
                component_instances_from_props(&obj.props, obj.component_instances.as_ref());
            {
                let mut subsystems = Subsystems::new();
                subsystems.register_ref::<Renderer>(renderer);
                subsystems.register(MeshCache::new());
                subsystems.register(SceneObjectCache::new());
                subsystems.register(LiveKeySet::new());
                let mut ctx = SceneObjectContext {
                    obj_id: &obj.id,
                    project_root,
                    renderer,
                    subsystems,
                };
                for (idx, class_name, data) in &instances {
                    let handled =
                        apply_runtime_behavior_for_class(class_name, &owner, *idx, data, &mut ctx);
                    if !handled {
                        tracing::debug!(
                            class = class_name,
                            id = obj.id,
                            "No runtime behavior (skipped)"
                        );
                    }
                }
                renderer = ctx.renderer;
            }
        }
        tracing::info!(objects = objects.len(), "Scene loaded");
    }
}

// ── SceneObjectContext — ComponentRuntimeContext impl ─────────────────────────

struct SceneObjectContext<'r, 'p> {
    obj_id: &'p str,
    project_root: &'p Path,
    subsystems: Subsystems,
    renderer: &'r mut Renderer,
}

impl ComponentRuntimeContext for SceneObjectContext<'_, '_> {
    fn subsystems_mut(&mut self) -> &mut Subsystems {
        &mut self.subsystems
    }

    fn project_root(&self) -> &std::path::Path {
        self.project_root
    }

    fn report_error(&mut self, message: String) {
        tracing::warn!(id = self.obj_id, "{message}");
    }
}

// ── Shared public API (called by engine_backend too) ─────────────────────────

/// Extract `(index, class_name, data)` from a component-instances value.
///
/// Prefers the explicit `component_instances` parameter (the modern path).
/// Falls back to `props["__component_instances"]` for backward compatibility
/// with v1/v2 scene files that embed this data inside the props map.
pub fn component_instances_from_props(
    props: &HashMap<String, Value>,
    component_instances: Option<&Value>,
) -> Vec<(usize, String, Value)> {
    let arr = component_instances.and_then(|v| v.as_array()).or_else(|| {
        props
            .get("__component_instances")
            .and_then(|v| v.as_array())
    });
    let Some(arr) = arr else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .filter_map(|(fi, entry)| {
            let o = entry.as_object()?;
            let idx = o
                .get("index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(fi);
            let cls = o
                .get("class_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)?;
            let dat = o.get("data").cloned().unwrap_or(Value::Null);
            Some((idx, cls, dat))
        })
        .collect()
}

/// Build transform from position / rotation (degrees YXZ) / scale.
/// Identical to engine's `build_transform`.
pub fn build_transform_parts(position: [f32; 3], rotation: [f32; 3], scale: [f32; 3]) -> Mat4 {
    let q = Quat::from_euler(
        EulerRot::YXZ,
        rotation[1].to_radians(),
        rotation[0].to_radians(),
        rotation[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(Vec3::from_array(scale), q, Vec3::from_array(position))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::ComponentRuntimeContext;
    use std::path::{Path, PathBuf};

    struct LinkageContext {
        project_root: PathBuf,
        subsystems: Subsystems,
        errors: Vec<String>,
    }

    impl ComponentRuntimeContext for LinkageContext {
        fn subsystems_mut(&mut self) -> &mut Subsystems {
            &mut self.subsystems
        }

        fn project_root(&self) -> &Path {
            &self.project_root
        }

        fn report_error(&mut self, message: String) {
            self.errors.push(message);
        }
    }

    #[test]
    fn shared_scene_loader_links_planet_terrain_runtime_behavior() {
        let props = HashMap::new();
        let owner = RuntimeComponentOwner {
            scene_object_id: "earth",
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            props: &props,
        };
        let mut context = LinkageContext {
            project_root: PathBuf::from("."),
            subsystems: Subsystems::new(),
            errors: Vec::new(),
        };

        assert!(apply_runtime_behavior_for_class(
            pulsar_rendering::PLANET_TERRAIN_CLASS_NAME,
            &owner,
            0,
            &Value::Null,
            &mut context,
        ));
        assert_eq!(context.errors.len(), 1);
        assert!(context.errors[0].contains("is invalid"));
    }
}
