use super::*;

impl HelioViewport {

    /// Handle an asset being dropped on the viewport
    pub(super) fn handle_asset_drop(
        &mut self,
        payload: &AssetPayload,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    /// Insert the dropped asset into the scene via the central SceneDatabase API.
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

                let add_result = execute_command(
                    &mut state,
                    SceneCommand::AddObject {
                        data: mesh_object,
                        parent_id: None,
                    },
                );

                if let Some(id) = add_result.affected_ids.first() {
                    if let Some((class_name, data_field)) = component_class_for_asset(&kind) {
                        if REGISTRY.has_class(class_name) {
                            crate::level_editor::scene_edit::components::add_component(&mut state.scene.world_mut(), 
                                id,
                                class_name.to_string(),
                                serde_json::json!({ data_field: asset_path }),
                            );
                            let _ = execute_command(
                                &mut state,
                                SceneCommand::SelectObject {
                                    id: Some(id.clone()),
                                },
                            );
                        }
                    }
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
