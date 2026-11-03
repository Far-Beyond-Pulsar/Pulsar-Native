//! Post-process volumes' scene rows: the volume and camera post-process
//! components' rows placed into the rows the volume blend reads (a row
//! join, see `helio_scene_join::RowJoin`). A volume's bounds are its
//! owner-oriented box's world AABB; a camera's baseline is not placed.
use helio_scene_join::{placement::*, BufferKey, RowJoin, RowJoinKeys, RowTable};

/// `PostProcessVolumeSourceRow`: local size vec4, then the pass row after
/// its bounds (140 words).
pub const POST_PROCESS_VOLUME_SOURCE_ROW_BYTES: u64 = 144 * 4;
/// `CameraPostProcessSourceRow`: the `CameraPostProcessRow` pass row.
pub const CAMERA_POST_PROCESS_SOURCE_ROW_BYTES: u64 = 136 * 4;

pub const POST_PROCESS_VOLUMES_KEY: BufferKey = BufferKey::of("post_process_volumes");
pub const CAMERA_POST_PROCESS_KEY: BufferKey = BufferKey::of("camera_postprocess");

/// Where the post-process components' source rows live.
#[derive(Clone, Copy, Debug)]
pub struct PostRowSources {
    pub post_process_volumes: BufferKey,
    pub camera_post_process: BufferKey,
}

/// Post-process volumes' row join.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, sources: PostRowSources) -> RowJoin {
    RowJoin::new(
        device,
        "PostProcessRows",
        keys,
        vec![
            RowTable::new(
                "Post Rows Volumes",
                POST_PROCESS_VOLUMES_KEY,
                sources.post_process_volumes,
                POST_PROCESS_VOLUME_SOURCE_ROW_BYTES,
                148,
                OWNER_AABB | GATE_HIDDEN,
            ),
            RowTable::new(
                "Post Rows Camera",
                CAMERA_POST_PROCESS_KEY,
                sources.camera_post_process,
                CAMERA_POST_PROCESS_SOURCE_ROW_BYTES,
                136,
                0,
            ),
        ],
    )
}
