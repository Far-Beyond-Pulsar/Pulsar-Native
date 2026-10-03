//! Quasar's SceneDB bridge: resolves audio-relevant entities through the
//! capture volume listener and exports only handles/ranges into Helio's shared
//! mesh buffers. Triangle data is never copied into a second audio scene.
use helio_component::components::{AudioSpeakerComponent, StaticMeshComponent};
use pulsar_scenedb::{Entity, SceneDb, VolumeListenerId, gpu::VarLenHandle};

use super::Transform;

#[derive(Clone, Copy, Debug)]
pub struct AcousticMeshRef {
    pub entity: Entity,
    pub vertex_range: VarLenHandle,
    pub index_range: VarLenHandle,
    pub world_from_local: [[f32; 4]; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct AcousticSpeaker {
    pub entity: Entity,
    pub channel: u32,
    pub gain: f32,
    pub directivity: f32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
}

#[derive(Debug, Default)]
pub struct AcousticSceneRefs {
    /// Shared-buffer ranges, not copied vertex/index arrays.
    pub meshes: Vec<AcousticMeshRef>,
    pub speakers: Vec<AcousticSpeaker>,
}

/// Collect the current dynamic acoustic scene. Broad-phase membership comes
/// from SceneDB's spatial listener; only those entities' mesh components and
/// transforms are read. Speakers are collected separately because their
/// positions commonly lie outside the capture volume.
pub fn resolve_acoustic_scene_refs(scene: &SceneDb, volume: VolumeListenerId) -> AcousticSceneRefs {
    let mut out = AcousticSceneRefs::default();
    let Some(mirror) = scene.world.gpu_mirror() else {
        return out;
    };
    let store = mirror.store();
    for entity in scene.world.volume_entities(volume) {
        let (Some(_mesh), Some(transform)) = (
            scene.world.get::<StaticMeshComponent>(entity),
            scene.world.get::<Transform>(entity),
        ) else {
            continue;
        };
        let (Some(vertex_range), Some(index_range)) = (
            StaticMeshComponent::vertices_gpu_handle(store, entity.index())
                .filter(|r| r.count != 0),
            StaticMeshComponent::indices_gpu_handle(store, entity.index()).filter(|r| r.count != 0),
        ) else {
            continue;
        };
        let world = glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::from_array(transform.scale),
            glam::Quat::from_euler(
                glam::EulerRot::YXZ,
                transform.rotation[1].to_radians(),
                transform.rotation[0].to_radians(),
                transform.rotation[2].to_radians(),
            ),
            glam::Vec3::from_array(transform.position),
        );
        out.meshes.push(AcousticMeshRef {
            entity,
            vertex_range,
            index_range,
            world_from_local: world.to_cols_array_2d(),
        });
    }
    for (entity, speaker) in scene.world.query::<&AudioSpeakerComponent>() {
        if speaker.enabled == 0 {
            continue;
        }
        let Some(transform) = scene.world.get::<Transform>(entity) else {
            continue;
        };
        out.speakers.push(AcousticSpeaker {
            entity,
            channel: speaker.channel,
            gain: speaker.gain,
            directivity: speaker.directivity,
            position: transform.position,
            rotation: transform.rotation,
        });
    }
    out
}
