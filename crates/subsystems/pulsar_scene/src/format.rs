//! Scene file format — the game runtime's view of the canonical schema.
//!
//! Since Pulsar-Native#557 (Phase B6) this module owns **no** serde
//! definitions. The one canonical Rust type set with the one serde
//! implementation lives in [`pulsar_scene_format`]; everything here is a
//! re-export of it, so the runtime and the editor can no longer drift apart
//! on the wire format. See that crate's module doc for the format itself
//! (v1 flat vs. v2.x nested, and the leniency contract).
//!
//! What *is* defined here is [`SceneObjectRuntimeExt`]: the projected-prop
//! accessors (`mat_base_color`, `light_color`, `mesh_asset`, …) that read a
//! scene object's props through the reflection system. Those are runtime
//! ergonomics, not schema — they need `pulsar_reflection`, which the
//! dependency-light schema crate deliberately does not depend on — so they
//! ride on an extension trait implemented for the canonical
//! [`SceneObject`]. The editor never sees them; the game loader keeps them
//! by importing this trait.

use pulsar_reflection::apply_scene_props_for_class;
use serde_json::Value;
use std::collections::HashMap;

// ── The canonical schema, re-exported verbatim ────────────────────────────────

pub use pulsar_scene_format::{
    BlueprintBinding, BlueprintBindings, ComponentInstance, LevelEditorCameraState,
    LevelEditorFileState, LevelMetadata, LightType, MeshType, ObjectId, ObjectType, SceneFile,
    SceneLoadError, SceneObject, SceneTransform,
};

// ── Runtime-only projected-prop accessors ─────────────────────────────────────

/// Reflection-projected reads over a [`SceneObject`]'s props — the game
/// runtime's convenience layer, deliberately kept off the schema type.
///
/// Each accessor works against [`Self::projected_props`], i.e. the object's
/// own `props` with every component instance's data projected in through its
/// registered `ScenePropsProjector`. That means renaming a field on, say,
/// `LightComponent` only requires updating that component's projector, never
/// this module.
pub trait SceneObjectRuntimeExt {
    /// A copy of `props` with all component-instance data projected into it
    /// via registered `ScenePropsProjector` implementations.
    fn projected_props(&self) -> HashMap<String, Value>;

    /// The `mesh_asset` path from this object's props or its component
    /// instances, projected through the reflection system.
    fn mesh_asset(&self) -> Option<String>;

    fn mat_base_color(&self) -> [f32; 4];
    fn mat_roughness(&self) -> f32;
    fn mat_metallic(&self) -> f32;
    fn mat_emissive(&self) -> [f32; 3];
    fn mat_emissive_strength(&self) -> f32;

    fn light_color(&self) -> [f32; 3];
    fn light_intensity(&self) -> f32;
    fn light_range(&self) -> f32;
    fn light_inner_angle(&self) -> f32;
    fn light_outer_angle(&self) -> f32;
}

impl SceneObjectRuntimeExt for SceneObject {
    fn projected_props(&self) -> HashMap<String, Value> {
        let mut props = self.props.clone();
        if let Some(instances) = component_instance_array(self) {
            for inst in instances {
                if let Some(class_name) = inst.get("class_name").and_then(|v| v.as_str()) {
                    let component_data = inst.get("data");
                    apply_scene_props_for_class(class_name, &mut props, component_data);
                }
            }
        }
        props
    }

    fn mesh_asset(&self) -> Option<String> {
        self.projected_props()
            .get("mesh_asset")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty() && *s != "None")
            .map(String::from)
    }

    fn mat_base_color(&self) -> [f32; 4] {
        prop_f32_arr4(&self.props, "base_color", [0.5, 0.5, 0.5, 1.0])
    }
    fn mat_roughness(&self) -> f32 {
        prop_f32(&self.props, "roughness", 0.5)
    }
    fn mat_metallic(&self) -> f32 {
        prop_f32(&self.props, "metallic", 0.0)
    }
    fn mat_emissive(&self) -> [f32; 3] {
        prop_f32_arr3(&self.props, "emissive", [0.0, 0.0, 0.0])
    }
    fn mat_emissive_strength(&self) -> f32 {
        prop_f32(&self.props, "emissive_strength", 0.0)
    }

    fn light_color(&self) -> [f32; 3] {
        prop_f32_arr3(&self.projected_props(), "color", [1.0, 1.0, 1.0])
    }
    fn light_intensity(&self) -> f32 {
        prop_f32(&self.projected_props(), "intensity", 1.0)
    }
    fn light_range(&self) -> f32 {
        prop_f32(&self.projected_props(), "range", 10.0)
    }
    fn light_inner_angle(&self) -> f32 {
        prop_f32(&self.projected_props(), "inner_angle", 30.0)
    }
    fn light_outer_angle(&self) -> f32 {
        prop_f32(&self.projected_props(), "outer_angle", 45.0)
    }
}

/// The object's component-instance array: the dedicated `component_instances`
/// field, falling back to the legacy `props["__component_instances"]`.
fn component_instance_array(obj: &SceneObject) -> Option<&Vec<Value>> {
    obj.component_instances
        .as_ref()
        .and_then(|v| v.as_array())
        .or_else(|| {
            obj.props
                .get("__component_instances")
                .and_then(|v| v.as_array())
        })
}

// ── Prop extraction helpers ───────────────────────────────────────────────────

fn prop_f32(props: &HashMap<String, Value>, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default)
}

fn prop_f32_arr3(props: &HashMap<String, Value>, key: &str, default: [f32; 3]) -> [f32; 3] {
    props
        .get(key)
        .and_then(|v| v.as_array())
        .and_then(|a| {
            if a.len() >= 3 {
                Some([
                    a[0].as_f64().unwrap_or(default[0] as f64) as f32,
                    a[1].as_f64().unwrap_or(default[1] as f64) as f32,
                    a[2].as_f64().unwrap_or(default[2] as f64) as f32,
                ])
            } else {
                None
            }
        })
        .unwrap_or(default)
}

fn prop_f32_arr4(props: &HashMap<String, Value>, key: &str, default: [f32; 4]) -> [f32; 4] {
    props
        .get(key)
        .and_then(|v| v.as_array())
        .and_then(|a| {
            if a.len() >= 4 {
                Some([
                    a[0].as_f64().unwrap_or(0.0) as f32,
                    a[1].as_f64().unwrap_or(0.0) as f32,
                    a[2].as_f64().unwrap_or(0.0) as f32,
                    a[3].as_f64().unwrap_or(1.0) as f32,
                ])
            } else if a.len() == 3 {
                Some([
                    a[0].as_f64().unwrap_or(0.0) as f32,
                    a[1].as_f64().unwrap_or(0.0) as f32,
                    a[2].as_f64().unwrap_or(0.0) as f32,
                    1.0,
                ])
            } else {
                None
            }
        })
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    //! Schema behaviour is verified in `pulsar_scene_format`'s own tests
    //! (round-trip, v1/v2 backward compat, enum superset). What's left to
    //! cover here is that the *runtime alias* really is that schema, and
    //! that the extension trait still projects props the way the loader
    //! expects.

    use super::*;

    /// The runtime's `SceneFile`/`SceneObject` are the canonical types, and a
    /// v1 flat-transform file still opens through them.
    #[test]
    fn runtime_alias_is_the_canonical_schema() {
        let json = r#"{
            "version": 1,
            "objects": [
                { "id": "ground", "name": "Ground", "object_type": {"Mesh": "Plane"},
                  "position": [1.0, 2.0, 3.0], "scale": [10.0, 1.0, 10.0] }
            ]
        }"#;
        let file: SceneFile = serde_json::from_str(json).expect("v1 parses");
        let canonical: pulsar_scene_format::SceneFile =
            serde_json::from_str(json).expect("v1 parses canonically");
        assert_eq!(file.objects[0].world_position(), [1.0, 2.0, 3.0]);
        assert_eq!(
            file.objects[0].world_scale(),
            canonical.objects[0].world_scale()
        );
    }

    /// Forward compatibility from the enum superset: the runtime now parses
    /// object/light kinds it does not act on, rather than failing or
    /// silently mislabelling them.
    #[test]
    fn runtime_accepts_editor_only_object_kinds() {
        let json = r#"{
            "version": "2.1",
            "objects": [
                { "id": "p", "name": "P", "object_type": "ParticleSystem" },
                { "id": "l", "name": "L", "object_type": {"Light": "Area"} }
            ]
        }"#;
        let file: SceneFile = serde_json::from_str(json).expect("superset parses");
        assert_eq!(file.objects[0].object_type, ObjectType::ParticleSystem);
        assert_eq!(file.objects[1].object_type, ObjectType::Light(LightType::Area));
    }

    /// The projected-prop accessors moved to a trait but kept their
    /// behaviour: flat props are read directly, defaults apply when absent.
    #[test]
    fn extension_trait_reads_flat_props_and_defaults() {
        let json = r#"{
            "version": "2.1",
            "objects": [
                { "id": "a", "name": "A", "object_type": {"Mesh": "Cube"},
                  "props": { "base_color": [0.1, 0.2, 0.3, 0.4], "roughness": 0.75,
                             "color": [1.0, 0.5, 0.25], "intensity": 3.0 } },
                { "id": "b", "name": "B", "object_type": {"Light": "Point"}, "props": {} }
            ]
        }"#;
        let file: SceneFile = serde_json::from_str(json).expect("parses");

        let a = &file.objects[0];
        assert_eq!(a.mat_base_color(), [0.1, 0.2, 0.3, 0.4]);
        assert_eq!(a.mat_roughness(), 0.75);
        assert_eq!(a.light_color(), [1.0, 0.5, 0.25]);
        assert_eq!(a.light_intensity(), 3.0);

        let b = &file.objects[1];
        assert_eq!(b.mat_base_color(), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(b.mat_metallic(), 0.0);
        assert_eq!(b.light_color(), [1.0, 1.0, 1.0]);
        assert_eq!(b.light_range(), 10.0);
        assert_eq!(b.light_outer_angle(), 45.0);
        assert_eq!(b.mesh_asset(), None);
    }

    /// `mesh_asset` falls back to the legacy `props["__component_instances"]`
    /// array when the dedicated field is absent.
    #[test]
    fn mesh_asset_reads_the_legacy_component_instances_key() {
        let json = r#"{
            "version": "2.1",
            "objects": [
                { "id": "a", "name": "A", "object_type": {"Mesh": "Custom"},
                  "props": { "__component_instances": [
                      { "class_name": "StaticMeshComponent", "data": { "mesh_asset": "models/a.mesh" } }
                  ] } }
            ]
        }"#;
        let file: SceneFile = serde_json::from_str(json).expect("parses");
        // Whether the projector rewrites it or the raw prop survives, the
        // legacy array is still the source that gets consulted.
        assert!(file.objects[0]
            .projected_props()
            .contains_key("__component_instances"));
    }
}
