//! What the linked binary actually registers: reflection classes, World
//! component registrations, runtime behaviors and SceneDB GPU-mirror schemas.

use std::collections::BTreeSet;

use pulsar_reflection::inventory;
use pulsar_reflection::REGISTRY;
use pulsar_scenedb::gpu::world_mirror::{
    GpuClearRegistration, GpuMirrorRegistration, VarLenReleaseRegistration,
};
use pulsar_scenedb::component::type_name;
use pulsar_scenedb::ComponentId;

/// One reflection class, as the editor's "Add component" menu lists it
/// (`REGISTRY.get_class_names()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedClass {
    pub name: String,
    pub category: Option<String>,
    /// Has a `#[register_world_component]` registration: adding it hydrates a
    /// typed `World` value. Without one, an attached instance exists only as
    /// a JSON attachment record.
    pub world_registered: bool,
    /// The class's own `World` type has `#[gpu]` columns that SceneDB mirrors.
    pub own_gpu_columns: bool,
}

/// One `#[derive(SceneStore)]` type with at least one `#[gpu]` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedSchema {
    /// Last path segment of the Rust type name.
    pub short: String,
    /// `std::any::type_name` as SceneDB recorded it.
    pub path: String,
    /// SceneDB zeroes its row on removal.
    pub clears_on_remove: bool,
    /// Releases variable-length pool allocations on removal.
    pub releases_var_len: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Linked {
    pub classes: Vec<LinkedClass>,
    /// World registrations whose class name has no reflection class.
    pub world_only: Vec<String>,
    pub schemas: Vec<LinkedSchema>,
}

pub fn short_type_name(path: &str) -> String {
    // Strip generics before taking the last segment.
    let base = path.split('<').next().unwrap_or(path);
    base.rsplit("::").next().unwrap_or(base).to_string()
}

fn ids<T: 'static>(id_of: impl Fn(&T) -> ComponentId) -> BTreeSet<ComponentId>
where
    T: inventory::Collect,
{
    inventory::iter::<T>.into_iter().map(id_of).collect()
}

pub fn collect() -> Linked {
    let world: BTreeSet<&'static str> =
        pulsar_world_registry::registered_world_component_classes().collect();
    let mirrored = ids::<GpuMirrorRegistration>(|r| (r.component_id)());
    let clears = ids::<GpuClearRegistration>(|r| (r.component_id)());
    let releases = ids::<VarLenReleaseRegistration>(|r| (r.component_id)());

    let mut names = REGISTRY.get_class_names();
    names.sort_unstable();
    names.dedup();
    let classes = names
        .iter()
        .map(|name| {
            let category = REGISTRY
                .get_categories()
                .into_iter()
                .find(|category| REGISTRY.get_class_names_by_category(category).contains(name))
                .map(str::to_string);
            let own_gpu_columns = pulsar_world_registry::component_id_for_class(name)
                .is_some_and(|id| mirrored.contains(&id));
            LinkedClass {
                name: name.to_string(),
                category,
                world_registered: world.contains(name),
                own_gpu_columns,
            }
        })
        .collect();

    let world_only = world
        .iter()
        .filter(|name| !names.contains(name))
        .map(|name| name.to_string())
        .collect();

    let mut schemas: Vec<LinkedSchema> = mirrored
        .iter()
        .map(|id| {
            let path = type_name(*id).to_string();
            LinkedSchema {
                short: short_type_name(&path),
                path,
                clears_on_remove: clears.contains(id),
                releases_var_len: releases.contains(id),
            }
        })
        .collect();
    schemas.sort_by(|a, b| a.path.cmp(&b.path));

    Linked {
        classes,
        world_only,
        schemas,
    }
}
