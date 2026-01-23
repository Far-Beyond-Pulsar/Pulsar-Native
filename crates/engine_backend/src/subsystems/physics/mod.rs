
use rapier3d::prelude::*;
use bevy::prelude::Vec3;

pub struct PhysicsEngine {
    // Gravity as Vec3 for compatibility with PhysicsPipeline::step
    gravity: Vec3,
    integration_parameters: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,
    physics_hooks: (),
    event_handler: (),
    collider_set: ColliderSet,
    rigid_body_set: RigidBodySet,
    // Physics thread logic here
}

impl PhysicsEngine {
    pub fn new() -> Self {
        let rigid_body_set = RigidBodySet::new();
        let collider_set = ColliderSet::new();

        /* Create other structures necessary for the simulation. */
        let gravity = Vec3::new(0.0, -9.81, 0.0);
        let integration_parameters = IntegrationParameters::default();
        let physics_pipeline = PhysicsPipeline::new();
        let island_manager = IslandManager::new();
        let broad_phase = DefaultBroadPhase::new();
        let narrow_phase = NarrowPhase::new();
        let impulse_joint_set = ImpulseJointSet::new();
        let multibody_joint_set = MultibodyJointSet::new();
        let ccd_solver = CCDSolver::new();
        let physics_hooks = ();
        let event_handler = ();

        PhysicsEngine {
            gravity,
            integration_parameters,
            physics_pipeline,
            island_manager,
            broad_phase,
            narrow_phase,
            impulse_joint_set,
            multibody_joint_set,
            ccd_solver,
            physics_hooks,
            event_handler,
            collider_set,
            rigid_body_set,
        }
    }
    
    pub async fn start(&mut self) {
        
        let rigid_body = RigidBodyBuilder::dynamic().translation([0.0, 10.0, 0.0].into()).build();
        let ball_body_handle = self.rigid_body_set.insert(rigid_body);
        
        
        /* Create the ground. */
        let collider = ColliderBuilder::cuboid(100.0, 0.1, 100.0).build();
        self.collider_set.insert(collider);

        /* Create the bounding ball. */
        let collider = ColliderBuilder::ball(0.5).restitution(0.7).build();
        self.collider_set.insert_with_parent(collider, ball_body_handle, &mut self.rigid_body_set);

        loop {
            self.physics_pipeline.step(
                self.gravity,
                &self.integration_parameters,
                &mut self.island_manager,
                &mut self.broad_phase,
                &mut self.narrow_phase,
                &mut self.rigid_body_set,
                &mut self.collider_set,
                &mut self.impulse_joint_set,
                &mut self.multibody_joint_set,
                &mut self.ccd_solver,
                &self.physics_hooks,
                &self.event_handler
            );

            let ball_body = &self.rigid_body_set[ball_body_handle];

            tokio::time::sleep(std::time::Duration::from_millis(8)).await; // Approx ~60 FPS
        }
    }
}
