//! Voxel sculpt tool settings. They persist across tool switches.

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{VoxelBrushRequest, VoxelBrushTool};
use helio_voxel_data::{VoxelBrushOp, VoxelBrushShape};

/// What a click does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoxelSculptMode {
    Dig,
    Build,
    Paint,
    /// Level the ground to where the stroke started.
    Flatten,
    /// Ease the ground to its local average height.
    Smooth,
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
    /// Material palette favorites, retained across mode switches in this editor.
    favorite_materials: u32,
}

impl Default for VoxelSculptDomain {
    fn default() -> Self {
        Self {
            mode: VoxelSculptMode::Dig,
            shape: VoxelBrushShape::Sphere,
            radius_m: 1.5,
            material: 15,
            single_block: false,
            favorite_materials: 0,
        }
    }
}

pub const MIN_RADIUS_M: f32 = 0.1;
/// Up to 2 km (a 4 km footprint): brushes larger than a few metres stay
/// analytic shapes in the engine, so their cost is the columns they cover
/// regenerating, which streaming spreads over frames.
pub const MAX_RADIUS_M: f32 = 2_000.0;
/// Solid terrain material ids (see `helio_component::voxel_world::material`):
/// every built-in material, shared by all terrain programs.
pub const MATERIALS: std::ops::RangeInclusive<u32> = 1..=helio_component::voxel_world::material::COUNT - 1;

impl VoxelSculptDomain {
    pub fn is_favorite(&self, material: u32) -> bool {
        MATERIALS.contains(&material) && self.favorite_materials & (1u32 << material) != 0
    }

    pub fn toggle_favorite(&mut self, material: u32) {
        if MATERIALS.contains(&material) {
            self.favorite_materials ^= 1u32 << material;
        }
    }

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
                VoxelSculptMode::Build | VoxelSculptMode::Flatten | VoxelSculptMode::Smooth => VoxelBrushOp::Add,
                VoxelSculptMode::Paint => VoxelBrushOp::Paint,
            },
            shape: self.shape,
            radius: self.radius_m,
            material: self.material,
            single_block: self.single_block,
            tool: match mode {
                VoxelSculptMode::Flatten => VoxelBrushTool::Flatten,
                VoxelSculptMode::Smooth => VoxelBrushTool::Smooth,
                _ => VoxelBrushTool::Stamp,
            },
            level: None,
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
    before: Vec<VoxelStrokeTerrain>,
    revision: u64,
    ended_at: Option<std::time::Instant>,
}

#[derive(Clone)]
struct VoxelStrokeTerrain {
    id: crate::scene_edit::ObjectId,
    instance: engine_backend::scene::attachments::ComponentInstanceId,
    before_len: usize,
    before_revision: u64,
}

/// Grace period after release before a stroke becomes an undo step.
const STROKE_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// Every attached voxel terrain instance and the sum of their source
/// revisions.
fn terrains(world: &pulsar_scenedb::World) -> (Vec<VoxelStrokeTerrain>, u64) {
    use engine_backend::scene::attachments;
    let mut ids = Vec::new();
    let mut revision = 0u64;
    for (instance, terrain) in world.query::<&helio_component::VoxelTerrainComponent>() {
        let id = attachments::owner_of(world, instance).and_then(|owner| world.stable_id_of(owner));
        if let (Some(id), Some(meta)) = (id, attachments::meta(world, instance)) {
            ids.push(VoxelStrokeTerrain {
                id: id.to_string(),
                instance: meta.id,
                before_len: terrain.edits.len(),
                before_revision: terrain.source_revision,
            });
            revision = revision.wrapping_add(terrain.source_revision);
        }
    }
    (ids, revision)
}

impl VoxelStroke {
    /// Start a stroke at pointer down (closing a previous one first).
    pub fn begin(state: &mut crate::state::LevelEditorState) {
        Self::finish(state, true);
        let world = state.scene.world();
        let (ids, revision) = terrains(&world);
        drop(world);
        state.editor.voxel_stroke = Some(Self {
            before: ids,
            revision,
            ended_at: None,
        });
    }

    /// Mark the stroke released.
    pub fn end(state: &mut crate::state::LevelEditorState) {
        if let Some(stroke) = &mut state.editor.voxel_stroke {
            stroke.ended_at.get_or_insert_with(std::time::Instant::now);
        }
    }

    /// Commit a released stroke as one undo step once it settled (or now,
    /// with `force`). Returns whether a step was recorded.
    pub fn finish(state: &mut crate::state::LevelEditorState, force: bool) -> bool {
        let settled = state.editor.voxel_stroke.as_ref().is_some_and(|stroke| {
            force
                || stroke
                    .ended_at
                    .is_some_and(|at| at.elapsed() >= STROKE_SETTLE)
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
        let mut entries = Vec::new();
        for terrain_before in stroke.before {
            let Some(terrain) =
                engine_backend::scene::attachments::instance_by_id(&world, terrain_before.instance)
                    .and_then(|instance| {
                        world.get::<helio_component::VoxelTerrainComponent>(instance)
                    })
            else {
                continue;
            };
            // A save folded the stroke's start into the base: not undoable.
            if terrain.edits.base_len() > terrain_before.before_len {
                continue;
            }
            entries.push(crate::scene_edit::history::VoxelEditJournalEntry {
                id: terrain_before.id,
                instance: terrain_before.instance,
                before_len: terrain_before.before_len,
                before_revision: terrain_before.before_revision,
                edits: terrain
                    .edits
                    .iter_from(terrain_before.before_len)
                    .copied()
                    .collect(),
                after_revision: terrain.source_revision,
            });
        }
        drop(world);
        if entries.is_empty() {
            return false;
        }
        state
            .scene
            .commit_voxel_journal(crate::scene_edit::history::VoxelEditJournal { entries });
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
        sculpt.mode = VoxelSculptMode::Flatten;
        assert_eq!(sculpt.request(false).tool, VoxelBrushTool::Flatten);
        sculpt.mode = VoxelSculptMode::Smooth;
        assert_eq!(sculpt.request(true).tool, VoxelBrushTool::Smooth, "Shift swaps only dig and build");
        sculpt.mode = VoxelSculptMode::Paint;
        sculpt.set_radius(10_000.0);
        sculpt.set_material(0);
        assert_eq!((sculpt.radius_m, sculpt.material), (MAX_RADIUS_M, 1));
        assert_eq!(sculpt.material_name(), "Grass");
        sculpt.set_material(15);
        assert_eq!(sculpt.material_name(), "Cobble");
    }

    #[test]
    fn a_sculpt_stroke_is_one_undo_step() {
        use crate::scene_edit::{components, objects, ObjectType, SceneObjectData, Transform};
        use helio_component::VoxelTerrainComponent;

        let mut state = crate::state::LevelEditorState::new();
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
        let edits = |state: &crate::state::LevelEditorState| {
            let world = state.scene.world();
            let terrain = engine_backend::scene::attachments::enabled_components_of::<
                VoxelTerrainComponent,
            >(&world, world.entity_for(&id).unwrap())[0]
                .0;
            world
                .get::<VoxelTerrainComponent>(terrain)
                .unwrap()
                .edits
                .len()
        };

        VoxelStroke::begin(&mut state);
        // The render thread commits two brush samples during the drag.
        for x in [1.0, 2.0] {
            let mut world = state.scene.world_mut();
            let owner = world.entity_for(&id).unwrap();
            let entity = engine_backend::scene::attachments::enabled_components_of::<
                VoxelTerrainComponent,
            >(&world, owner)[0]
                .0;
            let mut terrain = world.get_mut::<VoxelTerrainComponent>(entity).unwrap();
            terrain.edits.push(helio_voxel_data::VoxelBrushEdit {
                center: [x, 0.0, 0.0],
                radius: 1.0,
                shape: VoxelBrushShape::Sphere,
                op: VoxelBrushOp::Remove,
                material: 0,
                height: Default::default(),
            });
            terrain.source_revision += 1;
        }
        VoxelStroke::end(&mut state);
        assert!(
            !VoxelStroke::finish(&mut state, false),
            "the last samples may still be in flight"
        );
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
