//! Voxel sculpt tool settings. They persist across tool switches.

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::VoxelBrushRequest;
use helio_voxel_data::{VoxelBrushOp, VoxelBrushShape};

/// What a click does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoxelSculptMode {
    Dig,
    Build,
    Paint,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelSculptDomain {
    pub mode: VoxelSculptMode,
    pub shape: VoxelBrushShape,
    /// Radius (half size for a cube) in metres.
    pub radius_m: f32,
    /// Terrain material id used to build and paint.
    pub material: u32,
    /// Edit exactly one block per click.
    pub single_block: bool,
}

impl Default for VoxelSculptDomain {
    fn default() -> Self {
        Self { mode: VoxelSculptMode::Dig, shape: VoxelBrushShape::Sphere, radius_m: 1.5, material: 15, single_block: false }
    }
}

pub const MIN_RADIUS_M: f32 = 0.1;
pub const MAX_RADIUS_M: f32 = 32.0;
/// Solid terrain material ids (see `helio_component::voxel_world::material`).
pub const MATERIALS: std::ops::RangeInclusive<u32> = 1..=15;

impl VoxelSculptDomain {
    /// The stroke request; Shift swaps digging and building.
    pub fn request(&self, shift: bool) -> VoxelBrushRequest {
        let mode = match (self.mode, shift) {
            (VoxelSculptMode::Dig, true) => VoxelSculptMode::Build,
            (VoxelSculptMode::Build, true) => VoxelSculptMode::Dig,
            (mode, _) => mode,
        };
        VoxelBrushRequest {
            op: match mode {
                VoxelSculptMode::Dig => VoxelBrushOp::Remove,
                VoxelSculptMode::Build => VoxelBrushOp::Add,
                VoxelSculptMode::Paint => VoxelBrushOp::Paint,
            },
            shape: self.shape,
            radius: self.radius_m,
            material: self.material,
            single_block: self.single_block,
        }
    }

    pub fn set_radius(&mut self, radius_m: f32) {
        self.radius_m = radius_m.clamp(MIN_RADIUS_M, MAX_RADIUS_M);
    }

    pub fn set_material(&mut self, material: u32) {
        self.material = material.clamp(*MATERIALS.start(), *MATERIALS.end());
    }

    /// Name of the selected material.
    pub fn material_name(&self) -> &'static str {
        helio_component::voxel_world::material::NAMES
            .get(self.material.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("?")
    }
}

/// One sculpt stroke (pointer down to up), recorded as a single undo step.
///
/// Brush samples are applied by the render thread, so the stroke is closed
/// a moment after the pointer is released, once the last samples landed.
#[derive(Clone)]
pub struct VoxelStroke {
    ids: Vec<crate::level_editor::scene_edit::ObjectId>,
    before: crate::level_editor::scene_edit::history::SceneHistorySnapshot,
    revision: u64,
    ended_at: Option<std::time::Instant>,
}

/// Grace period after release before a stroke becomes an undo step.
const STROKE_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// Every voxel terrain object and the sum of their source revisions.
fn terrains(world: &pulsar_scenedb::World) -> (Vec<crate::level_editor::scene_edit::ObjectId>, u64) {
    let mut ids = Vec::new();
    let mut revision = 0u64;
    for (entity, terrain) in world.query::<&helio_component::VoxelTerrainComponent>() {
        if let Some(id) = world.stable_id_of(entity) {
            ids.push(id.to_string());
            revision = revision.wrapping_add(terrain.source_revision);
        }
    }
    (ids, revision)
}

impl VoxelStroke {
    /// Start a stroke at pointer down (closing a previous one first).
    pub fn begin(state: &mut crate::level_editor::state::LevelEditorState) {
        Self::finish(state, true);
        let world = state.scene.world();
        let (ids, revision) = terrains(&world);
        let before = crate::level_editor::scene_edit::history::capture_history_subset(&world, &ids);
        drop(world);
        state.editor.voxel_stroke = Some(Self { ids, before, revision, ended_at: None });
    }

    /// Mark the stroke released.
    pub fn end(state: &mut crate::level_editor::state::LevelEditorState) {
        if let Some(stroke) = &mut state.editor.voxel_stroke {
            stroke.ended_at.get_or_insert_with(std::time::Instant::now);
        }
    }

    /// Commit a released stroke as one undo step once it settled (or now,
    /// with `force`). Returns whether a step was recorded.
    pub fn finish(state: &mut crate::level_editor::state::LevelEditorState, force: bool) -> bool {
        let settled = state.editor.voxel_stroke.as_ref().is_some_and(|stroke| {
            force || stroke.ended_at.is_some_and(|at| at.elapsed() >= STROKE_SETTLE)
        });
        if !settled {
            return false;
        }
        let stroke = state.editor.voxel_stroke.take().expect("checked above");
        let world = state.scene.world();
        let (_, revision) = terrains(&world);
        if revision == stroke.revision {
            return false;
        }
        let after = crate::level_editor::scene_edit::history::capture_history_subset(&world, &stroke.ids);
        drop(world);
        state.scene.commit_undo_checkpoint(stroke.before, after);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_swaps_dig_and_build_and_settings_are_clamped() {
        let mut sculpt = VoxelSculptDomain::default();
        assert_eq!(sculpt.request(false).op, VoxelBrushOp::Remove);
        assert_eq!(sculpt.request(true).op, VoxelBrushOp::Add);
        sculpt.mode = VoxelSculptMode::Paint;
        assert_eq!(sculpt.request(true).op, VoxelBrushOp::Paint);
        sculpt.set_radius(1000.0);
        sculpt.set_material(0);
        assert_eq!((sculpt.radius_m, sculpt.material), (MAX_RADIUS_M, 1));
        assert_eq!(sculpt.material_name(), "Grass");
        sculpt.set_material(15);
        assert_eq!(sculpt.material_name(), "Cobble");
    }

    #[test]
    fn a_sculpt_stroke_is_one_undo_step() {
        use crate::level_editor::scene_edit::{components, objects, ObjectType, SceneObjectData, Transform};
        use helio_component::VoxelTerrainComponent;

        let mut state = crate::level_editor::state::LevelEditorState::new();
        let id = {
            let mut world = state.scene.world_mut();
            let id = objects::add_object(
                &mut world,
                SceneObjectData {
                    id: String::new(),
                    name: "Terrain".into(),
                    object_type: ObjectType::Empty,
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                None,
            );
            components::add_component(
                &mut world,
                &id,
                "VoxelTerrainComponent".into(),
                serde_json::to_value(VoxelTerrainComponent::plane(512.0)).unwrap(),
            );
            id
        };
        let edits = |state: &crate::level_editor::state::LevelEditorState| {
            let world = state.scene.world();
            world.get::<VoxelTerrainComponent>(world.entity_for(&id).unwrap()).unwrap().edits.len()
        };

        VoxelStroke::begin(&mut state);
        // The render thread commits two brush samples during the drag.
        for x in [1.0, 2.0] {
            let mut world = state.scene.world_mut();
            let entity = world.entity_for(&id).unwrap();
            let mut terrain = world.get_mut::<VoxelTerrainComponent>(entity).unwrap();
            terrain.edits.push(helio_voxel_data::VoxelBrushEdit {
                center: [x, 0.0, 0.0],
                radius: 1.0,
                shape: VoxelBrushShape::Sphere,
                op: VoxelBrushOp::Remove,
                material: 0,
            });
            terrain.source_revision += 1;
        }
        VoxelStroke::end(&mut state);
        assert!(!VoxelStroke::finish(&mut state, false), "the last samples may still be in flight");
        assert!(VoxelStroke::finish(&mut state, true));
        assert_eq!(edits(&state), 2);

        assert!(state.scene.undo());
        assert_eq!(edits(&state), 0, "one undo removes the whole stroke");
        assert!(state.scene.redo());
        assert_eq!(edits(&state), 2);

        // A click that changes nothing records nothing.
        VoxelStroke::begin(&mut state);
        VoxelStroke::end(&mut state);
        assert!(!VoxelStroke::finish(&mut state, true));
    }
}
