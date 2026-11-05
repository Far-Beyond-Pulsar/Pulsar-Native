//! A sculpt stroke's undo step: the edits it appended to each terrain's
//! edit journal (a plugin undo step, `ui_level_editor::extend::ExtensionUndo`).
//! Retaining only that appended range avoids cloning the terrain's complete
//! component (and its accumulated edit history) on every stroke.

use ui_level_editor::extend::ExtensionUndo;
use ui_level_editor::scene_edit::ObjectId;
use ui_level_editor::state::SceneDomain;

#[derive(Clone, Debug)]
pub struct VoxelEditJournal {
    pub(crate) entries: Vec<VoxelEditJournalEntry>,
}

#[derive(Clone, Debug)]
pub(crate) struct VoxelEditJournalEntry {
    /// The terrain's object, for scoping the history entry.
    pub(crate) id: ObjectId,
    /// The terrain component instance itself (Pulsar-Native#1035).
    pub(crate) instance: engine_backend::scene::attachments::ComponentInstanceId,
    pub(crate) before_len: usize,
    pub(crate) before_revision: u64,
    pub(crate) edits: Vec<voxel_component::data::VoxelBrushEdit>,
    pub(crate) after_revision: u64,
}

impl VoxelEditJournal {
    /// The terrains' objects.
    pub fn ids(&self) -> Vec<ObjectId> {
        self.entries.iter().map(|entry| entry.id.clone()).collect()
    }

    /// Pop the stroke's edits (`redo`: append them again), through the
    /// terrain component's edit journal.
    fn apply(&self, scene: &mut SceneDomain, redo: bool) -> bool {
        let mut world = scene.world_mut();
        for entry in &self.entries {
            let Some(entity) =
                engine_backend::scene::attachments::instance_by_id(&world, entry.instance)
            else {
                return false;
            };
            let Some(mut terrain) = world.get_mut::<voxel_component::VoxelTerrainComponent>(entity)
            else {
                return false;
            };
            if redo {
                if terrain.edits.len() != entry.before_len {
                    return false;
                }
                terrain.edits.extend(entry.edits.iter().cloned());
                terrain.source_revision = entry.after_revision;
            } else {
                // Edits folded into the journal's base (a save) are no
                // longer undoable.
                if terrain.edits.len() < entry.before_len + entry.edits.len()
                    || terrain.edits.base_len() > entry.before_len
                {
                    return false;
                }
                while terrain.edits.len() > entry.before_len {
                    terrain.edits.pop();
                }
                terrain.source_revision = entry.before_revision;
            }
        }
        true
    }
}

impl ExtensionUndo for VoxelEditJournal {
    fn label(&self) -> String {
        rust_i18n::t!("LevelEditor.Undo.Sculpt").to_string()
    }
    fn undo(&mut self, scene: &mut SceneDomain) -> bool {
        self.apply(scene, false)
    }
    fn redo(&mut self, scene: &mut SceneDomain) -> bool {
        self.apply(scene, true)
    }
}
