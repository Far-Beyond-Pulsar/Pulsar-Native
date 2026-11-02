//! Authored mesh LOD selection for a scene object.

use super::MeshAssetPath;
use engine_class_derive::{engine_class, register_world_component};
use pulsar_reflection::{pulsar_type, ReflectError, ReflectResult};

/// Selects among authored mesh assets according to camera distance.
///
/// This is independent of the renderer's geometric LOD mechanism. For
/// example, Helio may still choose meshlet detail inside the selected mesh.
#[engine_class(
    category = "Rendering",
    default,
    clone,
    debug,
    serialize,
    deserialize,
    scene_store
)]
pub struct LODComponent {
    /// Ordered from highest to lowest detail. Each level becomes eligible at
    /// its distance threshold; thresholds should increase with the level index.
    #[property]
    pub lod_levels: Vec<LODLevel>,

    /// Discrete level offset. Positive values prefer coarser authored levels;
    /// negative values prefer finer ones.
    #[property(min = -2.0, max = 2.0, step = 0.1)]
    pub lod_bias: f32,

    /// GPU source rows derived from the authored levels. Asset identities
    /// here do not become strong SceneDB asset references.
    #[gpu(buffer = "lod_component_levels")]
    #[serde(skip)]
    pub gpu_levels: Vec<LODLevelGpu>,
}

impl LODComponent {
    /// The authored level eligible at `camera_distance`, before checking
    /// whether that mesh is resident. `None` leaves the object's ordinary
    /// `StaticMeshComponent` as the renderer fallback.
    pub fn desired_level(&self, camera_distance: f32) -> Option<usize> {
        select_lod_level(&self.lod_levels, self.lod_bias, camera_distance)
    }
}

/// Resolve authored levels by monotonically increasing distance threshold.
/// Positive bias lowers thresholds (preferring coarser levels earlier); a
/// negative bias raises them. Level order is authoritative if serialized
/// thresholds are accidentally out of order.
pub fn select_lod_level(levels: &[LODLevel], bias: f32, camera_distance: f32) -> Option<usize> {
    let bias = if bias.is_finite() {
        bias.clamp(-16.0, 16.0)
    } else {
        0.0
    };
    let threshold_scale = 2.0_f32.powf(-bias);
    let distance = if camera_distance.is_nan() {
        0.0
    } else {
        camera_distance.max(0.0)
    };
    let mut previous_threshold = 0.0_f32;
    let mut selected = None;
    for (index, level) in levels.iter().enumerate() {
        let authored_threshold = if level.distance_threshold.is_finite() {
            level.distance_threshold.max(0.0)
        } else {
            f32::MAX
        };
        previous_threshold = previous_threshold.max(authored_threshold);
        let threshold = previous_threshold * threshold_scale;
        if distance >= threshold {
            selected = Some(index);
        }
    }
    selected
}

/// Compact per-level source data consumed by the render graph. Asset identity
/// is data here, not an `AssetId` handle, so authoring all levels does not load
/// and pin every mesh.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LODLevelGpu {
    pub distance_threshold: f32,
    /// Repeated per level so one variable-length source table carries its
    /// complete selection policy without a second entity-keyed join.
    pub lod_bias: f32,
    pub mesh_asset: [u32; 4],
}

// SAFETY: `repr(C)`, all fields are `Pod`, and the 24-byte layout has no
// implicit padding.
unsafe impl pulsar_scenedb::page::Pod for LODLevelGpu {}

impl LODLevelGpu {
    fn from_level(level: &LODLevel, lod_bias: f32) -> Self {
        let id = super::mesh_asset_id(&level.mesh_asset);
        let bytes = id.0.to_le_bytes();
        Self {
            distance_threshold: if level.distance_threshold.is_finite() {
                level.distance_threshold.max(0.0)
            } else {
                f32::MAX
            },
            lod_bias,
            mesh_asset: std::array::from_fn(|word| {
                let offset = word * 4;
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{refresh_gpu_levels, select_lod_level, LODComponent, LODLevel};
    use crate::components::MeshAssetPath;

    fn levels(thresholds: &[f32]) -> Vec<LODLevel> {
        thresholds
            .iter()
            .map(|distance_threshold| LODLevel {
                distance_threshold: *distance_threshold,
                mesh_asset: MeshAssetPath::default(),
            })
            .collect()
    }

    #[test]
    fn authored_lod_uses_distance_thresholds_and_bias() {
        let levels = levels(&[0.0, 100.0, 300.0]);
        assert_eq!(select_lod_level(&levels, 0.0, 0.0), Some(0));
        assert_eq!(select_lod_level(&levels, 0.0, 99.9), Some(0));
        assert_eq!(select_lod_level(&levels, 0.0, 100.0), Some(1));
        assert_eq!(select_lod_level(&levels, 0.0, 300.0), Some(2));
        assert_eq!(select_lod_level(&levels, 1.0, 50.0), Some(1));
        assert_eq!(select_lod_level(&levels, -1.0, 200.0), Some(1));
    }

    #[test]
    fn authored_lod_handles_empty_invalid_and_unsorted_inputs() {
        assert_eq!(select_lod_level(&[], 0.0, 100.0), None);
        let levels = levels(&[0.0, 300.0, 100.0]);
        assert_eq!(select_lod_level(&levels, 0.0, 200.0), Some(0));
        assert_eq!(select_lod_level(&levels, 0.0, 300.0), Some(2));
        assert_eq!(select_lod_level(&levels, f32::NAN, 0.0), Some(0));
    }

    #[test]
    fn source_rows_refresh_for_policy_and_level_edits() {
        let mut component = LODComponent {
            lod_bias: 1.5,
            lod_levels: levels(&[0.0, 125.0]),
            ..Default::default()
        };

        refresh_gpu_levels(&mut component);
        assert_eq!(component.gpu_levels.len(), 2);
        assert_eq!(component.gpu_levels[1].distance_threshold, 125.0);
        assert_eq!(component.gpu_levels[1].lod_bias, 1.5);

        component.lod_bias = f32::INFINITY;
        refresh_gpu_levels(&mut component);
        assert_eq!(component.gpu_levels[1].lod_bias, 0.0);
    }
}

/// One authored mesh representation and the distance where it becomes
/// eligible.
#[engine_class(no_register, default, clone, debug, serialize, deserialize)]
pub struct LODLevel {
    /// Camera distance at or beyond which this level may be selected.
    #[property(min = 0.0, max = 10000.0, step = 1.0)]
    pub distance_threshold: f32,

    /// Mesh asset to draw at this level. Asset loading and residency are
    /// driven by active render demand, not by this authored reference alone.
    #[property]
    #[serde(default, alias = "mesh_path")]
    pub mesh_asset: MeshAssetPath,
}

fn serialize_lod_level_json(value: &LODLevel) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_lod_level_json(value: serde_json::Value) -> ReflectResult<LODLevel> {
    serde_json::from_value(value).map_err(|e| ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_type(
    serialize_json_with = serialize_lod_level_json,
    deserialize_json_with = deserialize_lod_level_json
)]
pub type RegisteredLodLevel = LODLevel;

fn decode_lod_component(data: &serde_json::Value) -> Result<LODComponent, String> {
    let mut component: LODComponent = pulsar_world_registry::decode_json(data)?;
    refresh_gpu_levels(&mut component);
    Ok(component)
}

fn lod_property_written(component: &mut LODComponent, property: Option<&str>) {
    if matches!(property, None | Some("lod_levels" | "lod_bias")) {
        refresh_gpu_levels(component);
    }
}

fn refresh_gpu_levels(component: &mut LODComponent) {
    let bias = if component.lod_bias.is_finite() {
        component.lod_bias.clamp(-16.0, 16.0)
    } else {
        0.0
    };
    component.gpu_levels = component
        .lod_levels
        .iter()
        .map(|level| LODLevelGpu::from_level(level, bias))
        .collect();
}

#[register_world_component(decode = decode_lod_component, property_written = lod_property_written)]
impl LODComponent {}
