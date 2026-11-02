use engine_class_derive::engine_class;

use super::sub_props::{
    GeneralFoliageProps, InteractionFoliageProps, PlacementFoliageProps, RenderingFoliageProps,
    TuftFoliageProps, WindFoliageProps,
};

/// GPU-driven foliage (grass) attached to a scene object.
///
/// Registers a foliage type with the helio scene, grows it inside a square layer
/// centred on the owner object (or everywhere), pushes grass aside with a follower
/// interactor and sways in the level's global wind (or its own). Placement, culling and
/// LOD selection all happen on the GPU. On voxel ground a type may grow on chosen
/// terrain materials only, as tufts of blocky blades coloured by the turf (a meadow:
/// one component a type).
#[engine_class(category = "Rendering", default, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
#[category("Placement", category_color = "#6EC5FF")]
#[category("Tufts", category_color = "#9BD45A")]
#[category("Wind", category_color = "#7EE787")]
#[category("Interaction", category_color = "#A78BFA", default_collapsed = true)]
#[category("Rendering", category_color = "#FB7185", default_collapsed = true)]
pub struct FoliageComponent {
    #[sub_props]
    pub general: GeneralFoliageProps,
    #[sub_props]
    pub placement: PlacementFoliageProps,
    #[sub_props]
    #[serde(default)]
    pub tufts: TuftFoliageProps,
    #[sub_props]
    pub wind: WindFoliageProps,
    #[sub_props]
    pub interaction: InteractionFoliageProps,
    #[sub_props]
    pub rendering: RenderingFoliageProps,
}
