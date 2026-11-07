//! A billboard's row as a SceneDB component, for standalone hosts (examples, tests):
//! it carries its contract row's exact fields (`same_layout!`), so SceneDB
//! mirrors it into the buffer the passes read. Engines derive the rows from
//! their own components instead. Behind the `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

/// The authoritative SceneDB record consumed by the billboard pass.
///
/// The packed layout is intentional: the shader reads the three fields as one
/// 48-byte instance record. `World::insert`/`get_mut` update this record and
/// SceneDB owns the deferred GPU upload.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "billboard_instances")]
pub struct BillboardComponent {
    /// Where the billboard is, in the frame's space, as every position in a
    /// pass row: metres from the frame's world origin
    /// (`Renderer::world_origin`).
    #[gpu]
    pub world_pos: [f32; 4],
    #[gpu]
    pub scale_flags: [f32; 4],
    #[gpu]
    pub color: [f32; 4],
}
same_layout!(BillboardComponent, crate::BillboardRow, crate::BILLBOARD_INSTANCES, "billboard_instances"; world_pos, scale_flags, color);
