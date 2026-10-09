use super::*;

impl LevelEditorPanel {
    pub(in crate::ui::panel) fn on_open_scene(
        &mut self,
        _: &OpenScene,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state_arc = self.shared_state.clone();
        let scene_db = { state_arc.read().scene.shared_scene() };
        let default_dir = state_arc
            .read()
            .scene
            .current_scene
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let dialog = rfd::AsyncFileDialog::new()
            .set_title("Open Scene")
            .add_filter("Level file", &["level", "json"])
            .set_directory(default_dir);
        cx.spawn(async move |this, cx| {
            if let Some(handle) = dialog.pick_file().await {
                let path = handle.path().to_path_buf();
                // Load into the existing shared SceneDb (renderer keeps its Arc).
                let result = {
                    let mut world = scene_db.write();
                    crate::scene_edit::level_io::load_from_file_with_editor_state(
                        &mut world.world,
                        &path,
                    )
                };
                cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        match result {
                            Ok((editor_state, world_settings)) => {
                                this.apply_editor_camera_state(editor_state.camera.as_ref());
                                let mut state = state_arc.write();
                                state.scene.current_scene = Some(path);
                                state.scene.world_settings = world_settings;
                                state.editor.terrain.foliage_sets = editor_state.foliage_sets;
                                state.scene.has_unsaved_changes = false;
                                // Deselect so properties panel clears stale data.
                                state.scene.select_object(None);
                            }
                            Err(e) => tracing::error!("Open scene failed: {}", e),
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    pub(in crate::ui::panel) fn on_new_scene(
        &mut self,
        _: &NewScene,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Warn if unsaved changes (TODO: modal dialog)
        // Clear the scene IN-PLACE so the renderer keeps its Arc<SceneDb>.
        let scene_db = { self.shared_state.read().scene.shared_scene() };
        let mut editor_state = crate::scene_edit::LevelEditorFileState::default();
        let mut world_settings = crate::world_settings_data::WorldSettingsData::default();
        {
            let mut world = scene_db.write();
            crate::scene_edit::objects::clear(&mut world.world);
        }

        // Load from the embedded default.level if available, otherwise start empty.
        if let Some(bytes) = engine_state::EngineContext::global()
            .and_then(|ctx| ctx.store.get_or_init::<Option<Vec<u8>>>().read().clone())
        {
            let tmp = std::env::temp_dir().join("pulsar_new_scene_seed.level");
            if engine_fs::virtual_fs::write_file(&tmp, &bytes).is_ok() {
                let load_result = {
                    let mut world = scene_db.write();
                    crate::scene_edit::level_io::load_from_file_with_editor_state(
                        &mut world.world,
                        &tmp,
                    )
                };
                match load_result {
                    Ok((loaded_editor, loaded_settings)) => {
                        editor_state = loaded_editor;
                        world_settings = loaded_settings;
                    }
                    Err(e) => {
                        tracing::warn!("New scene: could not load embedded default.level: {e}")
                    }
                }
            }
        }
        self.apply_editor_camera_state(editor_state.camera.as_ref());
        {
            let mut state = self.shared_state.write();
            state.scene.current_scene = None;
            state.scene.world_settings = world_settings;
            state.editor.terrain.foliage_sets = editor_state.foliage_sets;
            state.scene.has_unsaved_changes = false;
            // Deselect so properties panel clears stale data.
            state.scene.select_object(None);
        }
        cx.notify();
    }

    pub(in crate::ui::panel) fn on_focus_selected(
        &mut self,
        _: &FocusSelected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let camera = self.current_editor_camera_state();
        let framed = {
            let state = self.shared_state.read();
            let world = state.scene.world();
            engine_backend::scene::SceneWorldExt::selected_entity(&*world)
                .and_then(|entity| focus_camera(&world, entity, camera.as_ref()))
        };
        if let Some(framed) = framed {
            self.apply_editor_camera_state(Some(&framed));
        }
        cx.notify();
    }
}

/// A camera framing `entity`: over the ground of a voxel world, or a few
/// metres back from any other object along the current view.
fn focus_camera(
    world: &pulsar_scenedb::World,
    entity: pulsar_scenedb::Entity,
    camera: Option<&crate::scene_edit::LevelEditorCameraState>,
) -> Option<crate::scene_edit::LevelEditorCameraState> {
    use glam::DVec3;
    let eye = camera.map_or(DVec3::ZERO, |c| DVec3::from_array(c.position));
    let forward = camera.map_or(DVec3::NEG_Z, |c| {
        let (sy, cy) = f64::from(c.yaw).sin_cos();
        let (sp, cp) = f64::from(c.pitch).sin_cos();
        DVec3::new(sy * cp, sp, -cy * cp)
    });
    let terrain = engine_backend::scene::attachments::enabled_components_of::<
        helio_component::VoxelTerrainComponent,
    >(world, entity)
    .first()
    .map(|(instance, _)| *instance);
    let (position, forward) = if let Some(terrain) = terrain {
        match helio_component::voxel_world::terrain_world(world, terrain) {
            Ok(planet) => helio_component::voxel_world::frame_view(&planet, eye, forward, 30.0),
            Err(error) => {
                tracing::warn!("Focus: {error}");
                return None;
            }
        }
    } else {
        let object = crate::scene_edit::objects::entity_to_scene_object_data(world, entity);
        let target = DVec3::from_array(object.transform.position.map(f64::from));
        (target - forward * 8.0, forward)
    };
    Some(crate::scene_edit::LevelEditorCameraState {
        position: position.to_array(),
        yaw: forward.x.atan2(-forward.z) as f32,
        pitch: forward.y.clamp(-1.0, 1.0).asin() as f32,
    })
}
