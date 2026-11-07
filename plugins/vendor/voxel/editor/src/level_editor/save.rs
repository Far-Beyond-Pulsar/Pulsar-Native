//! What a level save does first for the voxel worlds (a save hook,
//! `ui_level_editor::extend::SaveHook`): fold old edits into their
//! journals' bases and keep the renderer's chunks of the edits with them,
//! both off the scene lock but for the moment each is put in.

/// Fold old voxel edits into their journals' bases, so the level saves
/// what they left rather than every brush (`VoxelEditJournal::compact`).
/// The bases are built off the scene lock; a journal that changed meanwhile
/// still takes its base when it starts with it.
pub fn compact_voxel_journals(scene: &engine_backend::scene::SharedScene) {
    use voxel_component::world as voxel_world;
    profiling::profile_scope!("level_editor::save::compact_voxel_edits");
    let pending = voxel_world::journals_to_compact(&scene.read().world);
    for (entity, recipe, edits) in pending {
        match voxel_world::compact_journal(recipe, &edits, voxel_world::LISTED_EDITS) {
            Ok(Some(base)) => {
                let mut scene = scene.write();
                let terrain = scene
                    .world
                    .get_mut::<voxel_component::VoxelTerrainComponent>(entity);
                if let Some(mut terrain) = terrain {
                    terrain.edits.compact(base);
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!("Voxel edits kept unfolded: {error}"),
        }
    }
}

/// How long a save waits for the renderer's voxel chunks (a few frames
/// when the viewport draws; none when it does not).
const VOXEL_CHUNKS_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Keep the drawn chunks of each voxel terrain's edits with its journal
/// (`VoxelEditJournal::keep_chunks`), so loading the level copies what the
/// edits made instead of generating it from the terrain and every edit.
/// Asked of the terrain's runtime (which the renderer drawing it answers)
/// only when a terrain with edits keeps none for them; kept only while the
/// journal is the one they were made for. Waited for and deflated here, off
/// the render thread and the scene lock.
pub fn keep_voxel_chunks(scene: &engine_backend::scene::SharedScene) {
    use voxel_component::VoxelTerrainComponent;
    use voxel_component::runtime::VoxelTerrainRuntime;
    profiling::profile_scope!("level_editor::save::keep_voxel_chunks");
    let wanted: Vec<_> = {
        let scene = scene.read();
        scene
            .world
            .query::<&VoxelTerrainComponent>()
            .filter(|(_, c)| !c.edits.is_empty() && c.edits.kept_chunks().is_none())
            .filter_map(|(entity, _)| {
                Some((entity, VoxelTerrainRuntime::of(&scene.world, entity)?))
            })
            .collect()
    };
    for (entity, runtime) in wanted {
        let Some((edits, chunks)) = runtime.kept_chunks(VOXEL_CHUNKS_WAIT) else {
            continue;
        };
        if chunks.is_empty() {
            continue;
        }
        let kept = voxel_component::data::VoxelKeptChunks::of(&edits, chunks.to_bytes().into());
        let mut scene = scene.write();
        let unchanged = scene
            .world
            .get::<VoxelTerrainComponent>(entity)
            .is_some_and(|terrain| terrain.edits == edits);
        if unchanged {
            if let Some(mut terrain) = scene.world.get_mut::<VoxelTerrainComponent>(entity) {
                terrain.edits.keep_chunks(kept);
            }
        }
    }
}
