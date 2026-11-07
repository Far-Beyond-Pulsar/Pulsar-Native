use engine_class_derive::engine_class;
use serde_json::Value;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
pub struct GeneralFoliageProps {
    #[property(category = "General")]
    pub enabled: bool,
    /// Instances per square metre at full density weight.
    #[property(min = 0.0, max = 2048.0, step = 1.0, category = "General")]
    pub density: f32,
    /// Slice in the density texture array driving where this type grows.
    #[property(min = 0.0, max = 255.0, step = 1.0, category = "General")]
    pub density_layer: u64,
    /// What a blade is: a smooth strip, or on voxel ground a stack of
    /// cubes (edge Width min) upright on one grid.
    #[serde(default)]
    #[property(category = "General")]
    pub kind: FoliageForm,
}

/// What a foliage type's blades are (`helio_pass_foliage_place::FoliageKind`).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    pulsar_reflection::Reflectable,
)]
pub enum FoliageForm {
    /// A procedural blade strip.
    #[default]
    Blade,
    /// A stack of cubes (voxel worlds' grass).
    Block,
}

impl Default for GeneralFoliageProps {
    fn default() -> Self {
        Self {
            enabled: true,
            density: 256.0,
            density_layer: 0,
            kind: FoliageForm::Blade,
        }
    }
}

impl GeneralFoliageProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("enabled").and_then(|v| v.as_bool()) {
            self.enabled = v;
        }
        if let Some(v) = obj.get("density").and_then(|v| v.as_f64()) {
            self.density = v as f32;
        }
        if let Some(v) = obj.get("density_layer").and_then(|v| v.as_u64()) {
            self.density_layer = v;
        }
    }
}
