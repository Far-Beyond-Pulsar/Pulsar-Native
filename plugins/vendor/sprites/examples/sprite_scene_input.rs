//! SceneDB-backed input and sprite authoring adapter for the standalone 2D demos.
//!
//! Sprite rows are authored in SceneDB and mirrored into the sprite_instances
//! GPU row buffer. Helio receives only the type-erased projection; it does not
//! own the persistent sprite state.

use crate::common::SpriteComponent;
use bytemuck::Zeroable;
use helio_core::{GpuCameraUniforms, SceneBufferProjection, SceneInput};
use helio_pass_sprite_batch::{SpriteHandle, SpriteInstance};
use pulsar_scenedb::{Entity, SceneDb};
use std::collections::HashMap;
use std::sync::Arc;

pub struct SceneInputAdapter {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub frame_count: u64,
    camera_buf: wgpu::Buffer,
    camera_data: GpuCameraUniforms,
    scene_db: SceneDb,
    buffers: SceneBufferProjection,
    sprite_entities: HashMap<SpriteHandle, Entity>,
}

impl SceneInputAdapter {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let camera_data = GpuCameraUniforms::zeroed();
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Demo Camera Buffer"),
            size: std::mem::size_of::<GpuCameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut scene_db = SceneDb::new();
        let ctx = pulsar_scenedb::gpu::EngineGpuContext::new(device.clone(), queue.clone());
        let mut gpu_store = pulsar_scenedb::gpu::SceneGpuStore::new(
            &ctx,
            pulsar_scenedb::gpu::SceneGpuConfig {
                classes: Vec::new(),
                tombstone_headroom: 0,
                max_cells_metadata: 0,
            },
        );
        // Keep the SceneDB-backed sprite storage growable while retaining the demo's established initial pool size.
        SpriteComponent::register_gpu_columns_growable(&mut gpu_store, 4096, ctx.device());
        let gpu_store = Arc::new(gpu_store);
        scene_db
            .world
            .attach_gpu_mirror(pulsar_scenedb::gpu::GpuMirrorHandle::new(
                gpu_store,
                queue.clone(),
            ));
        scenedb_inspector_agent::install_world(&mut scene_db.world);
        Self {
            device,
            queue,
            frame_count: 0,
            camera_buf,
            camera_data,
            scene_db,
            buffers: SceneBufferProjection::empty(),
            sprite_entities: HashMap::new(),
        }
    }

    pub fn insert_sprite(&mut self, instance: SpriteInstance) -> SpriteHandle {
        let entity = self.scene_db.world.spawn();
        self.scene_db.world.insert(
            entity,
            SpriteComponent::from(helio_pass_sprite_batch::SpriteRow::from(instance)),
        );
        let handle = SpriteHandle::from_index(entity.index());
        self.sprite_entities.insert(handle, entity);
        handle
    }

    pub fn update_sprite(&mut self, handle: SpriteHandle, instance: SpriteInstance) {
        if let Some(&entity) = self.sprite_entities.get(&handle) {
            self.scene_db.world.insert(
                entity,
                SpriteComponent::from(helio_pass_sprite_batch::SpriteRow::from(instance)),
            );
        }
    }

    pub fn remove_sprite(&mut self, handle: SpriteHandle) {
        if let Some(entity) = self.sprite_entities.remove(&handle) {
            self.scene_db.world.despawn(entity);
        }
    }

    /// Flush authored rows and refresh the type-erased projection for the next
    /// graph execution. This is the frame boundary between SceneDB and Helio.
    pub fn sync(&mut self) {
        let sprite_buffer_bytes = self
            .scene_db
            .world
            .gpu_mirror()
            .and_then(|mirror| {
                mirror
                    .store()
                    .resolve_buffer(pulsar_scenedb::gpu::BufferKey::of("sprite_instances"))
            })
            .map(|(buffer, _)| buffer.size())
            .unwrap_or(0);
        if self.frame_count == 0 {
            println!(
                "[sprite_scene_input] SceneDB sprite_instances before flush: {} bytes, {} rows",
                sprite_buffer_bytes,
                self.sprite_entities.len()
            );
        }
        let _ = self.scene_db.world.flush_gpu_mirror(&self.queue);
        self.scene_db.world.publish_inspector_snapshot();
        self.buffers = self
            .scene_db
            .world
            .gpu_mirror()
            .map(|mirror| SceneBufferProjection::from_store_all(mirror.store()))
            .unwrap_or_default();
        if self.frame_count == 0 {
            println!(
                "[sprite_scene_input] after flush: projection has sprite_instances={}, bytes={}",
                self.buffers
                    .contains(helio_core::BufferKey::of("sprite_instances")),
                self.scene_db
                    .world
                    .gpu_mirror()
                    .and_then(|mirror| mirror
                        .store()
                        .resolve_buffer(helio_core::BufferKey::of("sprite_instances")))
                    .map(|(buffer, _)| buffer.size())
                    .unwrap_or(0)
            );
        }
        self.frame_count = self.frame_count.wrapping_add(1);
    }
}

impl SceneInput for SceneInputAdapter {
    fn world_origin(&self) -> glam::DVec3 {
        glam::DVec3::ZERO
    }
    fn device(&self) -> &Arc<wgpu::Device> {
        &self.device
    }
    fn queue(&self) -> &Arc<wgpu::Queue> {
        &self.queue
    }
    fn frame_count(&self) -> u64 {
        self.frame_count
    }
    fn camera(&self) -> &wgpu::Buffer {
        &self.camera_buf
    }
    fn camera_data(&self) -> &GpuCameraUniforms {
        &self.camera_data
    }
    fn camera_generation(&self) -> u64 {
        0
    }
    fn scene_buffers(&self) -> &SceneBufferProjection {
        &self.buffers
    }
}
