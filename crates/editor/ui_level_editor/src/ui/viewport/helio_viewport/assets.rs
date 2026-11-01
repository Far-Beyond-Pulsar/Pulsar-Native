use super::*;

impl HelioViewport {
    pub(super) fn supports_asset_placement(payload: &AssetPayload) -> bool {
        !payload.is_source_model()
            && (matches!(
                payload.kind,
                AssetKind::Mesh | AssetKind::Scene | AssetKind::Blueprint
            ) || component_for_asset(&payload.kind).is_some())
    }

    pub(super) fn update_active_placement(
        &mut self,
        payload: &AssetPayload,
        norm_x: f32,
        norm_y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::supports_asset_placement(payload) {
            return;
        }
        let path = PathBuf::from(&payload.engine_path);
        if !path.exists()
            || (payload.kind == AssetKind::Blueprint && !path.join("graph_save.json").exists())
        {
            return;
        }
        // Remember every pointer sample before querying the renderer. The
        // render thread owns the same mutex and a missed try_lock must never
        // turn into a stale release position.
        self.pending_placement = Some((payload.clone(), (norm_x, norm_y)));
        if let Some((_, _, cursor)) = &mut self.active_placement {
            *cursor = (norm_x, norm_y);
        }
        let start_monitor = !self.placement_monitor_running;
        self.placement_monitor_running = true;
        let ignore_id = self.active_placement.as_ref().map(|(id, _, _)| id.as_str());
        let position = match self.gpu_engine.try_lock() {
            Ok(engine) => engine.editor_placement_position(norm_x, norm_y, ignore_id),
            Err(_) if !cx.has_active_drag() => {
                // Mouse-up is the commit boundary. Wait for the renderer's
                // short frame critical section so the final release sample
                // cannot be lost to lock contention.
                let engine = self
                    .gpu_engine
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                engine.editor_placement_position(norm_x, norm_y, ignore_id)
            }
            Err(_) => None,
        };
        let Some(position) = position else {
            self.placement_retry_needed = true;
            if start_monitor {
                self.start_placement_monitor(window, cx);
            }
            return;
        };
        self.placement_retry_needed = false;
        let asset_path = path.to_string_lossy().replace('\\', "/");
        let mut state = self.shared_state.write();
        let id = if let Some((id, _, cursor)) = &mut self.active_placement {
            *cursor = (norm_x, norm_y);
            id.clone()
        } else {
            let before =
                crate::scene_edit::history::capture_history_subset(&state.scene.world(), &[]);
            let mut world = state.scene.world_mut();
            let id = if payload.kind == AssetKind::Blueprint {
                crate::scene_edit::classes::instantiate_class_dir(
                    &mut world,
                    &path,
                    &Transform {
                        position,
                        ..Transform::default()
                    },
                    None,
                )
                .ok()
                .and_then(|ids| ids.into_iter().next())
                .unwrap_or_default()
            } else {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Imported Asset")
                    .to_string();
                let data = SceneObjectData {
                    id: String::new(),
                    name,
                    object_type: ObjectType::Mesh(MeshType::Custom),
                    transform: Transform {
                        position,
                        ..Transform::default()
                    },
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    props: std::collections::HashMap::new(),
                    scene_path: path.display().to_string(),
                    component_instances: None,
                };
                crate::scene_edit::objects::add_object(&mut world, data, None)
            };
            if id.is_empty() {
                return;
            }
            if payload.kind != AssetKind::Blueprint {
                if let Some(registration) = component_for_asset(&payload.kind)
                    .filter(|registration| REGISTRY.has_class(registration.class_name))
                {
                    crate::scene_edit::components::add_component_value_enabled(
                        &mut world,
                        &id,
                        registration.class_name,
                        Some((registration.value_for)(&asset_path)),
                        true,
                    );
                }
            }
            drop(world);
            self.active_placement = Some((id.clone(), before, (norm_x, norm_y)));
            id
        };
        crate::scene_edit::objects::set_transform(
            &mut state.scene.world_mut(),
            &id,
            Some(position),
            None,
            None,
        );
        state.scene.bump_revision(false);
        drop(state);
        if start_monitor {
            self.start_placement_monitor(window, cx);
        }
    }

    fn start_placement_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().clone();
        cx.spawn_in(window, async move |_, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let active = this
                    .update_in(cx, |viewport, window, cx| {
                        let active = cx.has_active_drag();
                        if active && viewport.placement_retry_needed {
                            if let Some((payload, (x, y))) = viewport.pending_placement.clone() {
                                viewport.update_active_placement(&payload, x, y, window, cx);
                            }
                        }
                        active
                    })
                    .unwrap_or(false);
                if !active {
                    // Drop dispatch follows shortly after GPUI clears the
                    // active drag. Let its callback commit before cancelling.
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(120))
                        .await;
                    let active = this
                        .update_in(cx, |viewport, _window, cx| {
                            if cx.has_active_drag() {
                                true
                            } else {
                                viewport.cancel_active_placement();
                                false
                            }
                        })
                        .unwrap_or(false);
                    if !active {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    pub(super) fn finish_active_placement(&mut self) -> bool {
        let Some((id, before, (norm_x, norm_y))) = self.active_placement.take() else {
            return false;
        };
        self.placement_monitor_running = false;
        self.pending_placement = None;
        self.placement_retry_needed = false;
        let mut state = self.shared_state.write();
        let engine = self
            .gpu_engine
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let release_position = engine.editor_placement_position(norm_x, norm_y, Some(&id));
        drop(engine);
        if let Some(position) = release_position {
            crate::scene_edit::objects::set_transform(
                &mut state.scene.world_mut(),
                &id,
                Some(position),
                None,
                None,
            );
        }
        let after =
            crate::scene_edit::history::capture_history_subset(&state.scene.world(), &[id.clone()]);
        state
            .scene
            .commit_labeled_checkpoint(before, after, Some("LevelEditor.Undo.Add"));
        state.scene.bump_revision(true);
        let _ = execute_command(&mut state, SceneCommand::SelectObject { id: Some(id) });
        true
    }

    fn cancel_active_placement(&mut self) {
        self.placement_monitor_running = false;
        self.pending_placement = None;
        self.placement_retry_needed = false;
        let Some((id, _, _)) = self.active_placement.take() else {
            return;
        };
        let mut state = self.shared_state.write();
        if crate::scene_edit::objects::remove_object(&mut state.scene.world_mut(), &id) {
            state.scene.bump_revision(false);
        }
    }

    /// A source model (FBX, …) was dropped on the viewport: nothing is placed,
    /// and the user is told why.
    pub(super) fn reject_source_model_drop(
        &mut self,
        payload: &AssetPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.push_notification(
            Notification::info("Import it first").message(format!(
                "{} is a source model. Import it, then place the imported mesh.",
                payload.name
            )),
            cx,
        );
    }

    /// Handle an asset being dropped on the viewport
    pub(super) fn handle_asset_drop(
        &mut self,
        payload: &AssetPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // MouseUp can be newer than the final MouseMove. The drop callback
        // still has the precise release position even though GPUI has already
        // cleared its active drag payload.
        if Self::supports_asset_placement(payload) {
            if let Some(bounds) = self.placement_bounds {
                let mouse = window.mouse_position();
                let width: f32 = bounds.size.width.into();
                let height: f32 = bounds.size.height.into();
                if width > 0.0 && height > 0.0 {
                    let norm_x = f32::from(mouse.x - bounds.origin.x) / width;
                    let norm_y = f32::from(mouse.y - bounds.origin.y) / height;
                    if (0.0..=1.0).contains(&norm_x) && (0.0..=1.0).contains(&norm_y) {
                        self.update_active_placement(payload, norm_x, norm_y, window, cx);
                    }
                }
            }
        }
        if self.finish_active_placement() {
            window.push_notification(
                Notification::success(t!("Notification.Title.AddedToScene").to_string())
                    .message(t!("Notification.Message.Placed", name => &payload.name).to_string()),
                cx,
            );
            return;
        }
        if Self::supports_asset_placement(payload) {
            window.push_notification(
                Notification::error(t!("Notification.Title.PlacementFailed").to_string())
                    .message("Move the asset over the viewport before releasing it."),
                cx,
            );
            return;
        }
        let path = PathBuf::from(&payload.engine_path);
        let name = payload.name.clone();
        let kind = payload.kind.clone();

        tracing::info!("Asset dropped on viewport: {} ({:?})", name, kind);

        window.push_notification(
            Notification::info(t!("Notification.Title.AddingToScene").to_string())
                .message(t!("Notification.Message.Placing", name => &name).to_string()),
            cx,
        );

        let result = Self::import_asset(path, kind, self.shared_state.clone());
        match result {
            Ok(()) => {
                window.push_notification(
                    Notification::success(t!("Notification.Title.AddedToScene").to_string())
                        .message(t!("Notification.Message.Placed", name => &name).to_string()),
                    cx,
                );
            }
            Err(e) => {
                tracing::error!("Failed to place {}: {}", name, e);
                window.push_notification(
                    Notification::error(t!("Notification.Title.PlacementFailed").to_string())
                        .message(
                            t!(
                                "Notification.Message.FailedToPlace",
                                name => &name,
                                error => e.to_string()
                            )
                            .to_string(),
                        ),
                    cx,
                );
            }
        }
    }

    /// Insert the dropped asset into the scene through the scene edit functions.
    ///
    /// All assets — meshes, FBX files, blueprints — are inserted as SceneObjectData
    /// entries with the appropriate component instances.  The renderer's sync_scene()
    /// loop handles the actual GPU work every frame; nothing writes directly to Helio
    /// from this path.
    fn import_asset(
        path: PathBuf,
        kind: AssetKind,
        shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()).into());
        }

        match kind {
            AssetKind::Mesh | AssetKind::Scene => {
                let asset_path = path.to_string_lossy().replace('\\', "/");
                let imported_name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Imported Asset")
                    .to_string();

                let mut state = shared_state.write();
                let mesh_object = SceneObjectData {
                    id: String::new(),
                    name: imported_name,
                    object_type: ObjectType::Mesh(MeshType::Custom),
                    transform: Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    props: std::collections::HashMap::new(),
                    scene_path: path.display().to_string(),
                    component_instances: None,
                };

                // The object and its component are one undo step.
                let components = component_for_asset(&kind)
                    .filter(|registration| REGISTRY.has_class(registration.class_name))
                    .map(|registration| TypedComponent {
                        class_name: registration.class_name.to_string(),
                        enabled: true,
                        value: (registration.value_for)(&asset_path),
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                let has_component = !components.is_empty();
                let add_result = execute_command(
                    &mut state,
                    SceneCommand::AddObjectWithComponents {
                        data: mesh_object,
                        parent_id: None,
                        components,
                    },
                );

                if let (Some(id), true) = (add_result.affected_ids.first(), has_component) {
                    let _ = execute_command(
                        &mut state,
                        SceneCommand::SelectObject {
                            id: Some(id.clone()),
                        },
                    );
                }
            }
            AssetKind::Blueprint => {
                if !path.is_dir() {
                    return Err(
                        format!("Blueprint path is not a directory: {}", path.display()).into(),
                    );
                }
                if !path.join("graph_save.json").exists() {
                    return Err(format!(
                        "Not a valid blueprint class (missing graph_save.json): {}",
                        path.display()
                    )
                    .into());
                }

                // A placed class is a real instance (#921): the root with a
                // `ClassInstance` referencing the class GUID, every prefab
                // component, and child objects where one entity can't hold
                // them. Undoable as one command.
                let mut state = shared_state.write();
                let result = execute_command(
                    &mut state,
                    SceneCommand::InstantiateClass {
                        class_dir: path.to_path_buf(),
                        transform: Transform::default(),
                        parent_id: None,
                    },
                );
                let Some(id) = result.affected_ids.first().cloned() else {
                    return Err(format!(
                        "Could not place blueprint class {}: {}",
                        path.display(),
                        result.no_op_reason
                    )
                    .into());
                };
                let _ = execute_command(&mut state, SceneCommand::SelectObject { id: Some(id) });
            }
            _ => {
                return Err(format!("Unsupported asset type: {:?}", kind).into());
            }
        }

        Ok(())
    }
}
