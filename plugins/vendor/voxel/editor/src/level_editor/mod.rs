//! Voxel worlds in the level editor, registered through its extension
//! points (`ui_level_editor::extend`):
//! - the **Voxel Sculpt** tool mode ([`sculpt`]): strokes through the
//!   terrain component's `sculpt` method on the render thread, each stroke
//!   one undo step ([`journal`]) once its last samples landed (an
//!   interaction the editor polls and finishes before undo);
//! - the **Terrain** tool mode ([`terrain`]): a ready-made world in one
//!   click ([`add_world`], one undoable command) and the level's terrains;
//! - what a level save does first ([`save`]): fold old edits into the
//!   journals' bases, keep the renderer's chunks of the edits;
//! - choosing a terrain's generator attaches its settings
//!   (`voxel_component::world::ensure_generator_settings`, the component's
//!   method);
//! - framing a voxel world over its ground.

pub mod add_world;
pub mod journal;
pub mod save;
pub mod sculpt;
pub mod state;
pub mod terrain;

use ui_level_editor::ToolModeId;
use ui_level_editor::extend::{
    CameraFraming, Interaction, PropertyFollowUp, SaveHook, ToolModeRegistration,
};

/// The Voxel Sculpt mode's id.
pub const SCULPT_MODE: ToolModeId = ToolModeId("voxel_sculpt");
/// The Terrain mode's id.
pub const TERRAIN_MODE: ToolModeId = ToolModeId("terrain");

inventory::submit! {
    ToolModeRegistration {
        origin: pulsar_plugins::origin!(),
        order: 20,
        make: || Box::new(sculpt::VoxelSculptMode),
    }
}

inventory::submit! {
    ToolModeRegistration {
        origin: pulsar_plugins::origin!(),
        order: 25,
        make: || Box::new(terrain::TerrainMode),
    }
}

inventory::submit! {
    Interaction {
        origin: pulsar_plugins::origin!(),
        in_progress: |state| state::in_progress(state).is_some(),
        finish: |state, force| {
            state::VoxelStroke::finish(state, force);
        },
    }
}

inventory::submit! {
    SaveHook {
        origin: pulsar_plugins::origin!(),
        before_save: |scene| {
            save::compact_voxel_journals(scene);
            save::keep_voxel_chunks(scene);
        },
    }
}

inventory::submit! {
    PropertyFollowUp {
        origin: pulsar_plugins::origin!(),
        class_name: "VoxelTerrainComponent",
        property: "generator",
        follow: |world, owner, instance| {
            if let Err(error) =
                voxel_component::world::ensure_generator_settings(world, owner, instance)
            {
                tracing::warn!("{error}");
            }
        },
    }
}

inventory::submit! {
    CameraFraming {
        origin: pulsar_plugins::origin!(),
        frame: |world, object, eye, forward| {
            let terrain = engine_backend::scene::attachments::enabled_components_of::<
                voxel_component::VoxelTerrainComponent,
            >(world, object)
            .first()
            .map(|(instance, _)| *instance)?;
            Some(
                voxel_component::world::terrain_world(world, terrain)
                    .map(|planet| voxel_component::world::frame_view(&planet, eye, forward, 30.0))
                    .map_err(|error| error.to_string()),
            )
        },
    }
}
