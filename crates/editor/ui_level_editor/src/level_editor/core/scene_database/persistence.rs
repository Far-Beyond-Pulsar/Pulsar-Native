//! Level-file persistence (save/load round-trips) and the internal
//! `WorldSceneStore` <-> `SceneObjectData`/component conversion helpers.

use super::*;

impl SceneDatabase {
    // ── Persistence ────────────────────────────────────────────────────────

    /// Serialize the scene to a JSON level file.
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        self.save_to_file_with_editor_camera(path, None, None)
    }

    /// Serialize the scene to a JSON level file, optionally persisting editor
    /// camera state and any authored voxel terrain.
    ///
    /// `terrain` is the level editor's terrain edit seam. Voxel data does not
    /// live in the scene database, so it cannot ride along in the `LevelFile`
    /// the way objects and components do; it is flushed to a sidecar beside
    /// the level instead (see [`crate::level_editor::core::terrain_sidecar`]).
    /// Passing `None` — as headless and test callers do — writes the level
    /// exactly as before.
    pub fn save_to_file_with_editor_camera<P: AsRef<Path>>(
        &self,
        path: P,
        editor_camera: Option<LevelEditorCameraState>,
        terrain: Option<&engine_backend::services::terrain_edit::TerrainEditApi>,
    ) -> Result<(), String> {
        if let Some(parent_dir) = path.as_ref().parent() {
            virtual_fs::create_dir_all(parent_dir)
                .map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        let objects = self.get_all_objects();
        let components = objects
            .iter()
            .map(|obj| (obj.id.clone(), self.get_components(&obj.id)))
            .collect::<HashMap<_, _>>();
        let now = chrono::Utc::now().to_rfc3339();
        // Read the existing file once: its editor camera is preserved when
        // no fresh camera state was supplied, and its #650 blueprint-binding
        // section always rides along (the editor cannot author it yet, but a
        // re-save must never destroy it).
        let existing_file = virtual_fs::read_file(path.as_ref())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|json: String| serde_json::from_str::<LevelFile>(&json).ok());
        let preserved_editor = if editor_camera.is_none() {
            existing_file.as_ref().and_then(|file| file.editor.clone())
        } else {
            None
        };
        let preserved_bindings = existing_file
            .map(|file| file.blueprint_bindings)
            .unwrap_or_default();
        let level_file = LevelFile {
            version: "2.1".into(),
            objects,
            components,
            blueprint_bindings: preserved_bindings,
            metadata: LevelMetadata {
                created: now.clone(),
                modified: now,
                editor_version: env!("CARGO_PKG_VERSION").into(),
            },
            editor: editor_camera
                .map(|camera| LevelEditorFileState {
                    camera: Some(camera),
                })
                .or(preserved_editor),
        };
        let json = serde_json::to_string_pretty(&level_file)
            .map_err(|e| format!("Failed to serialize: {e}"))?;
        virtual_fs::write_file(path.as_ref(), json.as_bytes())
            .map_err(|e| format!("Failed to write file: {e}"))?;

        // Flush voxel terrain after the level itself is on disk: a terrain
        // sidecar without its level is meaningless, so the level is the
        // thing that must land first.
        if let Some(terrain) = terrain {
            crate::level_editor::core::terrain_sidecar::save(path.as_ref(), terrain)?;
        }

        tracing::info!("Scene saved to: {}", path.as_ref().display());
        Ok(())
    }

    /// Load a scene from a JSON level file (replaces the current scene).
    pub fn load_from_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        self.load_from_file_with_editor_camera(path).map(|_| ())
    }

    /// Load a scene from a JSON level file and return any persisted editor camera state.
    pub fn load_from_file_with_editor_camera<P: AsRef<Path>>(
        &self,
        path: P,
    ) -> Result<Option<LevelEditorCameraState>, String> {
        let bytes = virtual_fs::read_file(path.as_ref())
            .map_err(|e| format!("Failed to read file: {e}"))?;
        let json = String::from_utf8(bytes).map_err(|e| format!("File is not valid UTF-8: {e}"))?;
        let level_file: LevelFile =
            serde_json::from_str(&json).map_err(|e| format!("Failed to parse JSON: {e}"))?;
        if !level_file.version.starts_with("2.") && !level_file.version.starts_with("1.") {
            return Err(format!(
                "Unsupported scene version: {}. Expected 1.x or 2.x",
                level_file.version
            ));
        }
        self.clear();
        // Objects are stored in DFS order so parents are always inserted first.
        let has_persisted_components = !level_file.components.is_empty();
        for obj in level_file.objects {
            let parent = obj.parent.clone();
            self.add_object(obj, parent);
        }

        // When present, persisted components are authoritative and replace defaults.
        if has_persisted_components {
            for (object_id, components) in level_file.components {
                while !self.get_components(&object_id).is_empty() {
                    self.remove_component(&object_id, 0);
                }

                for component in components {
                    self.add_component_instance(&object_id, component);
                }
            }
        }

        tracing::info!(
            "Scene loaded from: {} (version: {})",
            path.as_ref().display(),
            level_file.version
        );
        Ok(level_file.editor.and_then(|editor| editor.camera))
    }

    // ── Internal conversion helpers ──────────────────────────────────────

    /// Build a `SceneObjectData` for `entity` directly off `WorldSceneStore` --
    /// transform/name/visibility/object_type/render_props plus the derived
    /// `parent`/`children`/`scene_path` fields. Does NOT merge live
    /// `component_store` component props on top (see [`Self::merge_component_props`]
    /// -- callers that need that call it separately afterward, matching the
    /// pre-B1 code's exact read paths: `get_object`/`get_all_objects` merge,
    /// `get_root_objects`/`get_selected_object` deliberately don't).
    pub(super) fn entity_to_scene_object_data(store: &WorldSceneStore, entity: Entity) -> SceneObjectData {
        let stable_id = store.stable_id_of(entity).unwrap_or_default().to_string();
        let transform = store.transform(entity).unwrap_or_default();
        let visibility = store.visibility(entity).unwrap_or_default();
        let render_props = store.render_props(&stable_id).unwrap_or_default();
        let parent = store
            .parent_of(entity)
            .and_then(|p| store.stable_id_of(p))
            .map(str::to_string);
        let children = store
            .children_of(Some(entity))
            .iter()
            .filter_map(|&child| store.stable_id_of(child).map(str::to_string))
            .collect();

        SceneObjectData {
            id: stable_id,
            name: store.name(entity).unwrap_or_default().to_string(),
            object_type: store.object_type(entity).unwrap_or(ObjectType::Empty),
            transform: Transform {
                position: transform.position,
                rotation: transform.rotation,
                scale: transform.scale,
            },
            visible: visibility.visible,
            locked: visibility.locked,
            parent,
            children,
            scene_path: Self::compute_scene_path(store, entity),
            props: render_props.props,
            component_instances: render_props.component_instances,
        }
    }

    /// Name-joined path from the root to `entity`, matching the pre-B1
    /// `SceneDb::update_subtree_path` format exactly (`"Parent/Child"`,
    /// recomputed on every read rather than cached -- see
    /// `WorldSceneStore::ObjectSnapshot`'s doc for why it isn't stored data).
    pub(super) fn compute_scene_path(store: &WorldSceneStore, entity: Entity) -> String {
        let mut parts = vec![store.name(entity).unwrap_or_default().to_string()];
        let mut current = store.parent_of(entity);
        while let Some(parent) = current {
            parts.push(store.name(parent).unwrap_or_default().to_string());
            current = store.parent_of(parent);
        }
        parts.reverse();
        parts.join("/")
    }

    pub(super) fn collect_dfs(
        store: &WorldSceneStore,
        parent: Option<Entity>,
        out: &mut Vec<SceneObjectData>,
    ) {
        for &entity in store.children_of(parent) {
            out.push(Self::entity_to_scene_object_data(store, entity));
            Self::collect_dfs(store, Some(entity), out);
        }
    }

    pub(super) fn merge_component_props(&self, object_id: &str, props: &mut HashMap<String, Value>) {
        let components = self.get_components(&object_id.to_string());
        for component in components.into_iter().filter(|component| component.enabled) {
            if apply_scene_props_for_class(&component.class_name, props, Some(&component.data)) {
                continue;
            }

            if let Value::Object(map) = component.data {
                for (k, v) in map {
                    props.insert(k, v);
                }
            }
        }
    }

    pub(super) fn sync_registered_component_props_to_scene_db(&self, object_id: &str) {
        let mut components = self.component_store.get_components(object_id);
        let mut store = self.store.write();
        let Some(entity) = store.entity_for(object_id) else {
            return;
        };
        for class_name in pulsar_world_registry::registered_world_component_classes() {
            let component = components
                .iter_mut()
                .find(|component| component.enabled && component.class_name == class_name);
            if let Some(component) = component {
                // Null is the attachment marker for a live typed value. Only
                // explicit edits or newly promoted instances carry input JSON.
                if component.data != attachment_data(&component.data) {
                    if let Err(error) = pulsar_world_registry::hydrate_world_component_for_class(
                        class_name,
                        store.world_mut(),
                        entity,
                        &component.data,
                    ) {
                        tracing::error!(
                            "World hydration failed for {class_name} on '{object_id}': {error}"
                        );
                        continue;
                    }
                }
                if pulsar_world_registry::get_world_component_as_engine_class(
                    class_name,
                    store.world(),
                    entity,
                )
                .is_some()
                {
                    component.data = attachment_data(&component.data);
                }
            } else {
                pulsar_world_registry::remove_world_component_for_class(
                    class_name,
                    store.world_mut(),
                    entity,
                );
            }
        }
        store.world_mut().insert(
            entity,
            engine_backend::scene::ComponentAttachments(components.clone()),
        );
        // Legacy props are a disposable serialization projection, never the
        // input to a typed component during an unrelated object edit.
        let mut projected_classes = HashSet::new();
        for component in &mut components {
            if component.enabled && projected_classes.insert(component.class_name.clone()) {
                if let Some(live) = pulsar_world_registry::get_world_component_as_engine_class(
                    &component.class_name,
                    store.world(),
                    entity,
                ) {
                    if let Ok(data) = live.to_json() {
                        component.data = overlay_live_data(&component.data, data);
                    }
                }
            }
        }
        store.update_render_props(object_id, |render_props| {
            for class_name in registered_scene_props_classes() {
                let data = components
                    .iter()
                    .find(|c| c.class_name == class_name && c.enabled)
                    .map(|c| &c.data);
                apply_scene_props_for_class(class_name, &mut render_props.props, data);
            }
            render_props.component_instances = Some(Value::Array(components.iter().enumerate()
                .filter(|(_, component)| component.enabled)
                .map(|(index, component)| serde_json::json!({
                    "index": index, "class_name": component.class_name, "data": component.data
                })).collect()));
        });
    }
    pub(super) fn collect_descendant_ids(store: &WorldSceneStore, entity: Entity, out: &mut Vec<ObjectId>) {
        for &child in store.children_of(Some(entity)) {
            if let Some(id) = store.stable_id_of(child) {
                out.push(id.to_string());
            }
            Self::collect_descendant_ids(store, child, out);
        }
    }
}