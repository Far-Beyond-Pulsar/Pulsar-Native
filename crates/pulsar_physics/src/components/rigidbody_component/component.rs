use engine_class_derive::engine_class;

use super::sub_props::{
    AdvancedRigidbodyProps, ConstraintsRigidbodyProps, DampingRigidbodyProps,
    ForcesRigidbodyProps, GeneralRigidbodyProps, VelocityRigidbodyProps,
};

#[engine_class(category = "Physics", default, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
#[category("Velocity", category_color = "#3B82F6")]
#[category("Damping", category_color = "#8B5CF6")]
#[category("Forces", category_color = "#F59E0B")]
#[category("Constraints", category_color = "#EF4444")]
#[category("Advanced", category_color = "#9CA3AF", default_collapsed = true)]
pub struct RigidbodyComponent {
    #[sub_props]
    pub general: GeneralRigidbodyProps,
    #[sub_props]
    pub velocity: VelocityRigidbodyProps,
    #[sub_props]
    pub damping: DampingRigidbodyProps,
    #[sub_props]
    pub forces: ForcesRigidbodyProps,
    #[sub_props]
    pub constraints: ConstraintsRigidbodyProps,
    #[sub_props]
    pub advanced: AdvancedRigidbodyProps,
}
