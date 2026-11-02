//! SceneDB World registration for voxel authoring components.
//!
//! Registration never invokes rendering or generation: the voxel backend
//! reads the terrain and layer instances when it projects the scene.

use engine_class_derive::{register_component_runtime, register_world_component};

use super::{
    TerrainEventsEventWriterExt as _, VoxelTerrainComponent,
    VoxelTerrainLayersComponent,
};

// Component callbacks borrow the authoritative terrain row from SceneDB.
// The pending event list is transient transport state populated only after a
// successful block edit; the EventHub writer queues delivery after the
// component borrow is released.
#[register_component_runtime(class = "VoxelTerrainComponent", enabled = enabled)]
impl VoxelTerrainComponent {
    /// Native Rust counterpart to Blueprint listeners. The generated
    /// adapter receives this event through the host-owned inbox on the next
    /// component phase, after Gamma delivery and outside the emitter borrow.
    #[bp_handler("block_broken")]
    fn on_block_broken(
        &mut self,
        _context: &mut pulsar_world_registry::ComponentContext<'_>,
        _block: super::BlockData,
    ) {
        // Terrain-specific Rust systems can add per-instance reactions here.
    }

    #[bp_handler("block_placed")]
    fn on_block_placed(
        &mut self,
        _context: &mut pulsar_world_registry::ComponentContext<'_>,
        _block: super::BlockData,
    ) {
        // Terrain-specific Rust systems can add per-instance reactions here.
    }

    #[bp_handler("block_material_changed")]
    fn on_block_material_changed(
        &mut self,
        _context: &mut pulsar_world_registry::ComponentContext<'_>,
        _change: super::BlockMaterialChange,
    ) {
        // Terrain-specific Rust systems can add per-instance reactions here.
    }

    fn tick(
        &mut self,
        context: &mut pulsar_world_registry::ComponentContext<'_>,
        _delta_seconds: f32,
    ) {
        macro_rules! flush_events {
            ($field:ident, $writer:ident, $event_name:literal) => {{
                let mut events = std::mem::take(&mut self.$field).into_iter();
                while let Some(event) = events.next() {
                    if let Err(error) = context.events.$writer(event.clone()) {
                        tracing::warn!(entity = ?context.entity, %error, event = $event_name, "could not queue terrain event; retaining it");
                        self.$field.push(event);
                        self.$field.extend(events);
                        break;
                    }
                }
            }};
        }
        flush_events!(pending_block_broken, block_broken, "block_broken");
        flush_events!(pending_block_placed, block_placed, "block_placed");
        flush_events!(
            pending_block_material_changed,
            block_material_changed,
            "block_material_changed"
        );
    }
}

#[register_world_component]
impl VoxelTerrainComponent {}

// Settings of the terrain on the same object, read when it is projected.
#[register_world_component]
impl VoxelTerrainLayersComponent {}
