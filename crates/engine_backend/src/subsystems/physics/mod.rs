use rapier3d::prelude::*;
use std::sync::{Arc, Mutex};
use crate::subsystems::framework::{Subsystem, SubsystemContext, SubsystemError, SubsystemId, subsystem_ids};

pub struct PhysicsEngine {
    // Gravity as rapier3d::math::Vector (which is an alias for Vec3)
    gravity: rapier3d::math::Vector,
    integration_parameters: IntegrationParameters,
    physics_pipeline: Arc<Mutex<PhysicsPipeline>>,
    island_manager: Arc<Mutex<IslandManager>>,
    broad_phase: Arc<Mutex<DefaultBroadPhase>>,
    narrow_phase: Arc<Mutex<NarrowPhase>>,
    impulse_joint_set: Arc<Mutex<ImpulseJointSet>>,
    multibody_joint_set: Arc<Mutex<MultibodyJointSet>>,
    ccd_solver: Arc<Mutex<CCDSolver>>,
    physics_hooks: (),
    event_handler: (),
    collider_set: Arc<Mutex<ColliderSet>>,
    rigid_body_set: Arc<Mutex<RigidBodySet>>,

    // Task handle for cleanup
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl PhysicsEngine {
    pub fn new() -> Self {
        let rigid_body_set = RigidBodySet::new();
        let collider_set = ColliderSet::new();

        /* Create other structures necessary for the simulation. */
        let gravity = rapier3d::math::Vector::new(0.0, -9.81, 0.0);
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
            physics_pipeline: Arc::new(Mutex::new(physics_pipeline)),
            island_manager: Arc::new(Mutex::new(island_manager)),
            broad_phase: Arc::new(Mutex::new(broad_phase)),
            narrow_phase: Arc::new(Mutex::new(narrow_phase)),
            impulse_joint_set: Arc::new(Mutex::new(impulse_joint_set)),
            multibody_joint_set: Arc::new(Mutex::new(multibody_joint_set)),
            ccd_solver: Arc::new(Mutex::new(ccd_solver)),
            physics_hooks,
            event_handler,
            collider_set: Arc::new(Mutex::new(collider_set)),
            rigid_body_set: Arc::new(Mutex::new(rigid_body_set)),
            task_handle: None,
        }
    }
}

impl Subsystem for PhysicsEngine {
    fn id(&self) -> SubsystemId {
        subsystem_ids::PHYSICS
    }

    fn dependencies(&self) -> Vec<SubsystemId> {
        vec![] // Physics has no dependencies
    }

    fn init(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError> {
        profiling::profile_scope!("Subsystem::Physics::Init");

        tracing::debug!("Initializing physics engine with Rapier3D");

        // Setup test scene
        {
            let mut rigid_body_set = self.rigid_body_set.lock().unwrap();
            let mut collider_set = self.collider_set.lock().unwrap();

            let rigid_body = RigidBodyBuilder::dynamic()
                .translation([0.0, 10.0, 0.0].into())
                .build();
            let ball_body_handle = rigid_body_set.insert(rigid_body);

            /* Create the ground. */
            let collider = ColliderBuilder::cuboid(100.0, 0.1, 100.0).build();
            collider_set.insert(collider);

            /* Create the bounding ball. */
            let collider = ColliderBuilder::ball(0.5).restitution(0.7).build();
            collider_set.insert_with_parent(collider, ball_body_handle, &mut rigid_body_set);
        }

        // Clone Arc pointers for the async task
        let gravity = self.gravity;
        let integration_parameters = self.integration_parameters.clone();
        let physics_pipeline = self.physics_pipeline.clone();
        let island_manager = self.island_manager.clone();
        let broad_phase = self.broad_phase.clone();
        let narrow_phase = self.narrow_phase.clone();
        let impulse_joint_set = self.impulse_joint_set.clone();
        let multibody_joint_set = self.multibody_joint_set.clone();
        let ccd_solver = self.ccd_solver.clone();
        let rigid_body_set = self.rigid_body_set.clone();
        let collider_set = self.collider_set.clone();

        // Spawn physics loop on the provided runtime
        let handle = context.runtime.spawn(async move {
            profiling::set_thread_name("Physics Thread");

            loop {
                profiling::profile_scope!("Physics::Step");

                // Lock all components and perform physics step
                {
                    let mut pipeline = physics_pipeline.lock().unwrap();
                    let mut islands = island_manager.lock().unwrap();
                    let mut broad = broad_phase.lock().unwrap();
                    let mut narrow = narrow_phase.lock().unwrap();
                    let mut bodies = rigid_body_set.lock().unwrap();
                    let mut colliders = collider_set.lock().unwrap();
                    let mut impulse_joints = impulse_joint_set.lock().unwrap();
                    let mut multibody_joints = multibody_joint_set.lock().unwrap();
                    let mut ccd = ccd_solver.lock().unwrap();

                    pipeline.step(
                        gravity,
                        &integration_parameters,
                        &mut islands,
                        &mut broad,
                        &mut narrow,
                        &mut bodies,
                        &mut colliders,
                        &mut impulse_joints,
                        &mut multibody_joints,
                        &mut ccd,
                        &(),
                        &()
                    );
                }

                tokio::time::sleep(std::time::Duration::from_millis(8)).await; // Approx ~120 FPS
            }
        });

        self.task_handle = Some(handle);
        tracing::info!("✓ Physics engine initialized and running");

        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), SubsystemError> {
        profiling::profile_scope!("Subsystem::Physics::Shutdown");

        tracing::debug!("Shutting down physics engine");

        if let Some(handle) = self.task_handle.take() {
            handle.abort();
            tracing::info!("✓ Physics task stopped");
        }

        Ok(())
    }
}
