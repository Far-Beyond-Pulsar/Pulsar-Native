use engine_class_derive::engine_class;

use super::sub_props::{
    AdvancedPhysicsProps, CollisionPhysicsProps, GeneralPhysicsProps, MaterialPhysicsProps,
    SimulationPhysicsProps,
};

#[engine_class(category = "Physics", default, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
#[category("Collision", category_color = "#FF6B6B")]
#[category("Physics Material", category_color = "#4ECDC4")]
#[category("Simulation", category_color = "#A78BFA", default_collapsed = true)]
#[category("Advanced", category_color = "#9CA3AF", default_collapsed = true)]
pub struct PhysicsComponent {
    #[sub_props]
    pub general: GeneralPhysicsProps,
    #[sub_props]
    pub collision: CollisionPhysicsProps,
    #[sub_props]
    pub material: MaterialPhysicsProps,
    #[sub_props]
    pub simulation: SimulationPhysicsProps,
    #[sub_props]
    pub advanced: AdvancedPhysicsProps,
}
