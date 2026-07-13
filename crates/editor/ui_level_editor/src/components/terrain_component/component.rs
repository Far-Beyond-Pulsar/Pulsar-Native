use engine_class_derive::engine_class;

use super::sub_props::{
    GeneralTerrainProps, MaterialTerrainProps, RenderingTerrainProps, TransformTerrainProps,
};

#[engine_class(category = "Rendering", default, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#4ADE80")]
#[category("Transform", category_color = "#A78BFA")]
#[category("Material", category_color = "#F97316")]
#[category("Rendering", category_color = "#22D3EE", default_collapsed = true)]
pub struct TerrainComponent {
    #[sub_props]
    pub general: GeneralTerrainProps,
    #[sub_props]
    pub transform: TransformTerrainProps,
    #[sub_props]
    pub material: MaterialTerrainProps,
    #[sub_props]
    pub rendering: RenderingTerrainProps,
}
