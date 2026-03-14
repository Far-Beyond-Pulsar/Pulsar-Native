use rapier3d::prelude::*;
use std::sync::{Arc, Mutex};
use crate::subsystems::framework::{Subsystem, SubsystemContext, SubsystemError, SubsystemId, subsystem_ids};
use crate::services::PhysicsQueryService;

/// All physics-engine-owned pipeline state consolidated into one lock.
///
/// `collider_set` and `rigid_body_set` are kept separate because they are
/// shared with `PhysicsQueryService`.
pub struct PhysicsWorld {
    pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,
}

impl PhysicsWorld {
    fn new() -> Self {
        Self {
            pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
        }
    }

    /// Advance the simulation by one step.
    fn step(
        &mut self,
        gravity: &rapier3d::math::Vector,
        integration_parameters: &IntegrationParameters,
        bodies: &mut RigidBodySet,
        colliders: &mut ColliderSet,
    ) {
        self.pipeline.step(
            gravity,
            integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            bodies,
            colliders,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            &(),
            &(),
        );
    }
}

pub struct PhysicsEngine {
    gravity: rapier3d::math::Vector,
    integration_parameters: IntegrationParameters,
    /// All pipeline-owned state behind a single lock.
    world: Arc<Mutex<PhysicsWorld>>,
    collider_set: Arc<Mutex<ColliderSet>>,
    rigid_body_set: Arc<Mutex<RigidBodySet>>,

    // Query service for raycasting
    pub query_service: Arc<PhysicsQueryService>,

    // Task handle for cleanup
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl PhysicsEngine {
    pub fn new() -> Self {
        let rigid_body_set = RigidBodySet::new();
        let collider_set = ColliderSet::new();

        let gravity = rapier3d::math::Vector::new(0.0, -9.81, 0.0);
        let integration_parameters = IntegrationParameters::default();

        let collider_set_arc = Arc::new(Mutex::new(collider_set));
        let rigid_body_set_arc = Arc::new(Mutex::new(rigid_body_set));

        let query_service = Arc::new(PhysicsQueryService::new(
            collider_set_arc.clone(),
            rigid_body_set_arc.clone(),
        ));

        PhysicsEngine {
            gravity,
            integration_parameters,
            world: Arc::new(Mutex::new(PhysicsWorld::new())),
            collider_set: collider_set_arc,
            rigid_body_set: rigid_body_set_arc,
            query_service,
            task_handle: None,
        }
    }

    /// Get a reference to the query service for raycasting
    pub fn query_service(&self) -> Arc<PhysicsQueryService> {
        self.query_service.clone()
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
            let mut rigid_body_set = self.rigid_body_set.lock().unwrap_or_else(|e| e.into_inner());
            let mut collider_set = self.collider_set.lock().unwrap_or_else(|e| e.into_inner());

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
        let world = self.world.clone();
        let rigid_body_set = self.rigid_body_set.clone();
        let collider_set = self.collider_set.clone();

        // Spawn physics loop on the provided runtime
        let handle = context.runtime.spawn(async move {
            profiling::set_thread_name("Physics Thread");

            loop {
                profiling::profile_scope!("Physics::Step");

                {
                    let mut w = world.lock().unwrap_or_else(|e| e.into_inner());
                    let mut bodies = rigid_body_set.lock().unwrap_or_else(|e| e.into_inner());
                    let mut colliders = collider_set.lock().unwrap_or_else(|e| e.into_inner());
                    w.step(&gravity, &integration_parameters, &mut bodies, &mut colliders);
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
