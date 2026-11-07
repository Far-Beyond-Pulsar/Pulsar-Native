//! A reflection capture's row as a SceneDB component, for standalone hosts (examples, tests):
//! it carries its contract row's exact fields (`same_layout!`), so SceneDB
//! mirrors it into the buffer the passes read. Engines derive the rows from
//! their own components instead. Behind the `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "reflection_captures")]
pub struct ReflectionCaptureComponent {
    #[gpu]
    pub position_radius: [f32; 4],
    #[gpu]
    pub extents_transition: [f32; 4],
    #[gpu]
    pub world_to_local: [[f32; 4]; 4],
    #[gpu]
    pub cubemap_index: i32,
    #[gpu]
    pub shape: u32,
    #[gpu]
    pub mobility: u32,
    #[gpu]
    pub brightness: f32,
}
same_layout!(ReflectionCaptureComponent, crate::ReflectionCaptureRow, crate::REFLECTION_CAPTURES, "reflection_captures"; position_radius, extents_transition, world_to_local, cubemap_index, shape, mobility, brightness);

impl From<crate::GpuReflectionCapture> for ReflectionCaptureComponent {
    fn from(value: crate::GpuReflectionCapture) -> Self {
        crate::ReflectionCaptureRow::from(value).into()
    }
}

impl From<ReflectionCaptureComponent> for crate::GpuReflectionCapture {
    fn from(value: ReflectionCaptureComponent) -> Self {
        crate::ReflectionCaptureRow::from(value).into()
    }
}
