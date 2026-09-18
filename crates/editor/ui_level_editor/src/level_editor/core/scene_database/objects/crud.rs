use super::super::*;

impl SceneDatabase {
    // ── Object CRUD ───────────────────────────────────────────────────────
    //
    // WorldSceneStore is the single source of truth. sync_scene() in the
    // renderer reconciles Helio state every frame — no immediate
    // write-through needed.

    /// Add an object. Returns the assigned `ObjectId`.
    ///
    /// Blueprint objects always receive a `ScriptComponent` in `component_store`
    /// pointing at their blueprint directory. `sync_registered_component_props_to_scene_db`
    /// rebuilds `__component_instances` from `component_store`, so the component
    /// must live there — setting it only in `props` would be immediately overwritten.
    pub fn add_object(&self, obj: SceneObjectData, parent: Option<ObjectId>) -> ObjectId {
        // Reject caller-supplied identity/parent errors before touching the
        // metadata projection. An add must never turn a duplicate or stale
        // reference into a different object while the caller keeps using the
        // original ID.
        {
            let store = self.store.read();
            if !obj.id.is_empty() && store.entity_for(&obj.id).is_some() {
                tracing::error!(id = %obj.id, "SceneDatabase rejected duplicate object ID");
                return String::new();
            }
            if let Some(parent_id) = parent.as_deref() {
                if store.entity_for(parent_id).is_none() {
                    tracing::error!(parent = %parent_id, "SceneDatabase rejected missing parent");
                    return String::new();
                }
            }
        }

        // v2 scene objects may carry component instances inline. Preserve
        // those instances in the metadata store before the normal hydration
        // pass; otherwise the empty metadata store overwrites the inline list
        // and World-registered components (notably StaticMeshComponent) never
        // reach the live World.
        let mut inline_components = obj
            .component_instances
            .as_ref()
            .and_then(serde_json::Value::as_array)
            .map(|instances| {
                instances
                    .iter()
                    .filter_map(|instance| {
                        let object = instance.as_object()?;
                        Some(ComponentInstance {
                            class_name: object.get("class_name")?.as_str()?.to_string(),
                            enabled: object
                                .get("enabled")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(true),
                            data: object
                                .get("data")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // A few older scene files projected StaticMeshComponent's asset path
        // into `props` without emitting a component-instances entry. Treat
        // that as a load-time compatibility form so those scenes also get a
        // typed World component; current files still use the normal path.
        if inline_components.is_empty() {
            if let Some(mesh_asset) = obj
                .props
                .get("mesh_asset")
                .and_then(serde_json::Value::as_str)
                .filter(|path| !path.trim().is_empty())
            {
                inline_components.push(ComponentInstance {
                    class_name: "StaticMeshComponent".to_string(),
                    enabled: true,
                    data: static_mesh_component_json(mesh_asset),
                });
            }
        }
        let blueprint_script_path = if obj.object_type == ObjectType::Blueprint {
            Some(find_script_path(
                &obj.props,
                obj.component_instances.as_ref(),
            ))
        } else {
            None
        };

        let object_id = {
            let mut store = self.store.write();
            let parent_entity = parent.as_deref().and_then(|p| store.entity_for(p));
            let requested_id = if obj.id.is_empty() {
                None
            } else {
                Some(obj.id.clone())
            };
            let entity = match store.spawn(requested_id, obj.name.clone(), parent_entity) {
                Ok(entity) => entity,
                Err(WorldSceneStoreError::DuplicateId(dup)) => {
                    tracing::warn!(
                        "SceneDatabase::add_object: id '{dup}' already exists, auto-assigning a new one"
                    );
                    store
                        .spawn(None, obj.name.clone(), parent_entity)
                        .expect("auto-assigned stable id cannot collide")
                }
                Err(err) => {
                    tracing::warn!("SceneDatabase::add_object: {err}, auto-assigning a new id");
                    store
                        .spawn(None, obj.name.clone(), parent_entity)
                        .expect("auto-assigned stable id cannot collide")
                }
            };
            store.set_transform(
                entity,
                WorldTransform {
                    position: obj.transform.position,
                    rotation: obj.transform.rotation,
                    scale: obj.transform.scale,
                },
            );
            store.set_visibility(
                entity,
                WorldVisibility {
                    visible: obj.visible,
                    locked: obj.locked,
                },
            );
            store.set_object_type(entity, obj.object_type);
            let id = store.stable_id_of(entity).unwrap_or_default().to_string();
            store.update_render_props(&id, |render_props| {
                render_props.props = obj.props;
                render_props.component_instances = obj.component_instances;
            });
            id
        };

        for component in inline_components {
            self.attach_component_instance(&object_id, component, false);
        }

        if let Some(script_path) = blueprint_script_path {
            let already_has = self
                .component_store
                .get_components(&object_id)
                .iter()
                .any(|c| c.class_name == "ScriptComponent");

            if !already_has {
                self.attach_component_instance(
                    &object_id,
                    ComponentInstance {
                        class_name: "ScriptComponent".to_string(),
                        enabled: true,
                        data: serde_json::json!({ "script_asset": script_path }),
                    },
                    false,
                );
            }
        }

        self.sync_registered_component_props_to_scene_db(&object_id);
        object_id
    }

    /// Remove an object and all of its descendants. Returns `true` if found.
    pub fn remove_object(&self, id: &ObjectId) -> bool {
        let ids_to_clear = {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(id) else {
                return false;
            };
            let mut ids_to_clear = vec![id.clone()];
            Self::collect_descendant_ids(&store, entity, &mut ids_to_clear);
            store.despawn(entity);
            ids_to_clear
        };
        for object_id in ids_to_clear {
            self.component_store.clear_components(&object_id);
        }
        true
    }

    /// Write updated transform, name, visibility, and component data back to an existing object.
    pub fn update_object(&self, obj: SceneObjectData) -> bool {
        let id = obj.id.clone();
        {
            let mut store = self.store.write();
            let Some(entity) = store.entity_for(&id) else {
                return false;
            };
            store.set_transform(
                entity,
                WorldTransform {
                    position: obj.transform.position,
                    rotation: obj.transform.rotation,
                    scale: obj.transform.scale,
                },
            );
            store.set_name(entity, obj.name);
            store.set_visibility(
                entity,
                WorldVisibility {
                    visible: obj.visible,
                    locked: obj.locked,
                },
            );
            store.update_render_props(&id, |render_props| render_props.props = obj.props);
        }
        self.sync_registered_component_props_to_scene_db(&id);
        true
    }
}
