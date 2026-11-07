//! Adding a ready-made voxel world (`voxel_component::presets`): its
//! terrain, sky, sun, haze, ground cover, wind and look, taking over the
//! level's own. One undo step (a plugin command,
//! `ui_level_editor::extend::ExtensionCommand`); selects the terrain and
//! moves the camera over its ground.

use pulsar_scenedb::World;
use ui_level_editor::LevelEditorState;
use ui_level_editor::commands::CommandResult;
use ui_level_editor::extend::ExtensionCommand;
use ui_level_editor::scene_edit::LevelEditorCameraState;
use voxel_component::presets::VoxelWorldPreset;

pub struct AddVoxelWorld {
    pub preset: VoxelWorldPreset,
}

impl ExtensionCommand for AddVoxelWorld {
    fn label(&self) -> String {
        rust_i18n::t!("LevelEditor.Undo.AddVoxelWorld").to_string()
    }

    fn scope(&self, world: &World) -> Vec<String> {
        voxel_component::presets::objects_changed_by(world)
    }

    fn apply(self: Box<Self>, state: &mut LevelEditorState) -> CommandResult {
        let setup =
            voxel_component::presets::add_voxel_world(&mut state.scene.world_mut(), self.preset);
        let setup = match setup {
            Ok(setup) => setup,
            Err(error) => {
                tracing::error!("Could not add the voxel world: {error}");
                return CommandResult::noop("The voxel world could not be added");
            }
        };
        state.scene.select_object(Some(setup.terrain.clone()));
        state.scene.pending_selection_sync = true;
        state.editor.camera_request = setup.camera.map(|camera| LevelEditorCameraState {
            position: camera.position,
            yaw: camera.yaw,
            pitch: camera.pitch,
        });
        state.scene.bump_revision(true);
        let mut ids = vec![setup.terrain];
        for id in setup.added.into_iter().chain(setup.changed) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        CommandResult::ok(ids)
    }

    fn describe(&self) -> String {
        format!("AddVoxelWorld({:?})", self.preset)
    }
}
