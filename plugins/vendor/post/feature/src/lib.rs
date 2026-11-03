//! Post-process volumes: the camera's post-process baseline and the volumes
//! around it (SceneDB rows), blended on the GPU into the frame's post
//! settings (`helio_pass_postprocess::POST_SETTINGS`), which the tone map and
//! fog read. Without this feature the tone map uses its defaults.

mod components;
pub mod feature;
pub mod scene_rows;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub use scene_rows::PostRowSources;
mod volume_blend;

pub use components::{
    CameraPostProcessRow, PostProcessVolumeRow, CAMERA_POSTPROCESS, POST_PROCESS_VOLUMES,
};
pub use feature::PostVolumesFeature;
pub use volume_blend::{shader as volume_blend_shader, PostProcessVolumeBlendPass};

#[cfg(test)]
mod abi_tests;
