//! The materials' Helio feature (`pulsar.materials`): the material texture
//! table.
//!
//! Every frame, before any pass runs, the table is refreshed from the
//! scene's texture store (SceneDB owns texture residency) and published as
//! [`helio_mats::MATERIAL_TEXTURES`]: one view per texture-store slot (a
//! white texture where a slot is empty), the material sampler and the
//! per-material texture metadata (`helio_mats::MaterialTextureTable`).
//! Every kind's drawers bind it through the materials api
//! (`MaterialBindingConfig`), so a material's texture index means the same
//! texture to a mesh, the voxel terrain, a decal or a sprite.
//!
//! How others do it: Bevy's material plugin prepares its bindless material
//! slabs in the render world before the graph runs; Unreal's materials bind
//! their textures through uniform buffers the renderer refreshes per frame.
//! Here the table is the materials plugin's frame input (a
//! [`helio_core::FrameInputPublisher`]): the renderer facade knows no
//! material, and without the plugin a kind's drawer binds the stand-in
//! (`helio_mats::MaterialStandIn`: default grey PBR).

use std::sync::{Arc, RwLock};

use helio_core::{FrameInputContext, FrameInputPublisher, ResourceRegistry};
use helio_feature_api::{Feature, FeatureRegistrar, NodeDescriptor};
use helio_frame_api::{host_kind, HOST_INPUTS};
use helio_mats::{MaterialSampler, MaterialTextureTable, MATERIAL_TEXTURES};
use pulsar_scenedb::gpu::{GpuMirrorHandle, TextureStore};

/// The materials' feature: the texture table ([`SceneMaterialTextures`]).
pub struct MaterialsFeature;

impl Feature for MaterialsFeature {
    fn id(&self) -> &'static str {
        "pulsar.materials"
    }

    fn register(&self, registrar: &mut FeatureRegistrar<'_>) {
        // The host input's node (the kind `helio_frame_api::host_input`
        // gives it): it declares the input and supplies it.
        registrar.node(
            NodeDescriptor::new(
                &host_kind(MATERIAL_TEXTURES),
                &format!("Host::{MATERIAL_TEXTURES}"),
                "Host",
                "The material texture table: the scene's textures, their sampler and metadata.",
                |build| {
                    build.graph.declare_external_input(MATERIAL_TEXTURES);
                    let table = SceneMaterialTextures::new(
                        build.env.device,
                        build.env.queue,
                        build.env.setting::<GpuMirrorHandle>(),
                        build.env.setting::<MaterialSampler>(),
                    );
                    build.graph.add_frame_input_publisher(Box::new(table));
                    Ok(())
                },
            )
            .with_data_only()
            .with_generated_pins()
            .at(HOST_INPUTS, 0),
        );
    }
}

/// The material texture table kept in step with the scene's texture store:
/// each store slot's texture in the table's slot of the same index (views
/// cached by GPU texture identity, so a frame without changes binds nothing
/// new).
pub struct SceneMaterialTextures {
    store: Option<Arc<RwLock<TextureStore>>>,
    features: wgpu::Features,
    table: MaterialTextureTable,
}

impl SceneMaterialTextures {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: Option<&GpuMirrorHandle>,
        sampler: Option<&MaterialSampler>,
    ) -> Self {
        Self {
            store: scene.and_then(GpuMirrorHandle::texture_store),
            features: device.features(),
            table: MaterialTextureTable::new(device, queue, sampler),
        }
    }
}

impl FrameInputPublisher for SceneMaterialTextures {
    fn name(&self) -> &'static str {
        "Materials: texture table"
    }

    fn update(&mut self, _ctx: &FrameInputContext<'_>) -> helio_core::Result<()> {
        let Some(store) = self.store.clone() else {
            return Ok(());
        };
        let guard = {
            // Shared with SceneDB's texture streaming/upload side, which takes
            // the write half; a long wait here is streaming holding the store.
            helio_core::cpu_scope!("Materials: wait SceneDB texture store lock");
            store.read().map_err(|_| {
                helio_core::Error::InvalidPassConfig("SceneDB texture store lock poisoned".into())
            })?
        };
        if guard.slot_count() as usize > self.table.capacity() {
            return Err(helio_core::Error::InvalidPassConfig(
                "SceneDB texture slots exceed the device material binding capacity".into(),
            ));
        }
        for slot in 0..self.table.capacity() {
            self.table
                .set(self.features, slot, guard.texture(slot as u32))
                .map_err(|message| {
                    helio_core::Error::InvalidPassConfig(format!("SceneDB {message}"))
                })?;
        }
        Ok(())
    }

    fn publish<'a>(&'a self, frame: &mut ResourceRegistry<'a>) {
        frame.write(
            helio_mats::material_textures(),
            self.table.bindings(),
            "Materials",
        );
    }
}
