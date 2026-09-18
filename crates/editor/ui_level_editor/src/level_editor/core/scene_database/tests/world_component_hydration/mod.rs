use super::*;

    use helio_component::StaticMeshComponent;

    fn object(name: &str) -> SceneObjectData {
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type: ObjectType::Mesh(MeshType::Custom),
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        }
    }

mod component_misc;
/// Phase B4 (Pulsar-Native#555): proves `StaticMeshComponent` -- the first
/// component migrated onto `pulsar_world_registry`'s `World` bridge --
/// actually gets hydrated/removed through the real `SceneDatabase` wiring,
/// not just the synthetic fixture `pulsar_world_registry`'s own unit tests
/// use. Reaches into `db.store` directly (a private field) -- valid since
/// this module is a descendant of `scene_database`, not external code
/// working through the public API only.

mod hydration;
mod live_edit;