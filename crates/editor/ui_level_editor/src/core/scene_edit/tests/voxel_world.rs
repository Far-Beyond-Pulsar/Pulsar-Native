//! Create Voxel Terrain: a ready-made world in one undoable step.

use engine_backend::scene::level_rules;
use engine_backend::scene::voxel_presets::VoxelWorldPreset;
use engine_backend::scene::SceneWorldExt;
use helio_component::VoxelTerrainComponent;

use crate::commands::{execute_command, SceneCommand};
use crate::scene_edit::sky;
use crate::state::LevelEditorState;

fn terrains(state: &LevelEditorState) -> Vec<VoxelTerrainComponent> {
    state
        .scene
        .world()
        .query::<&VoxelTerrainComponent>()
        .map(|(_, terrain)| terrain.clone())
        .collect()
}

#[test]
fn a_voxel_world_is_one_undo_step_that_selects_its_terrain_and_moves_the_camera() {
    let mut state = LevelEditorState::new();
    // A level with a sky already: the world's sky takes its place.
    assert!(sky::create_sky(&mut state).changed);
    let before = state.scene.world().children_of(None).len();

    let created = execute_command(
        &mut state,
        SceneCommand::AddVoxelWorld {
            preset: VoxelWorldPreset::Earth,
        },
    );
    assert!(created.changed, "{}", created.no_op_reason);
    let terrain = created.affected_ids[0].clone();
    assert_eq!(terrains(&state).len(), 1);
    assert!(terrains(&state)[0].editable, "sculpted at once");
    assert_eq!(state.scene.selected_object(), Some(terrain.clone()));
    let camera = state
        .editor
        .camera_request
        .clone()
        .expect("the camera over the ground");
    assert!(camera.position[1] > 6_371_000.0);
    {
        let world = state.scene.world();
        assert_eq!(level_rules::atmospheres(&world).len(), 1);
        assert_eq!(level_rules::winds(&world).len(), 1);
        assert_eq!(level_rules::directional_lights(&world).len(), 1);
    }

    assert!(state.scene.undo());
    assert!(terrains(&state).is_empty(), "one undo removes the world");
    assert_eq!(state.scene.world().children_of(None).len(), before);
    assert_eq!(
        level_rules::atmospheres(&state.scene.world()).len(),
        1,
        "the level's sky is back"
    );
    assert!(state.scene.redo());
    assert_eq!(terrains(&state).len(), 1);
    assert_eq!(level_rules::atmospheres(&state.scene.world()).len(), 1);
}
