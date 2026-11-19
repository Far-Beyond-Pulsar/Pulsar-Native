//! Events a component offers because of the assets its properties name.
//!
//! A property typed `AssetRef<K>` already says what it points at, so the
//! **asset kind**, not the component class, owns the answer to "what events
//! does an instance of this asset raise". A plugin registers an
//! [`AssetEventSource`] for its kind; a scripting editor (the Blueprint
//! palette, the compiler, the cook) asks [`component_events`] for the events
//! of a class's components and gets, per component and per asset-reference
//! property, the source's catalog. The editor names no class, no property and
//! no kind: reflection says which properties are asset references
//! (`pulsar_reflection::asset_kind_of`), the registry says who answers.
//!
//! # Performance
//!
//! * [`asset_slots`]: one reflection walk per class, kept until the plugin set
//!   changes.
//! * [`component_events`]: a property read by name from the prefab's JSON per
//!   asset slot; no full deserialization.
//! * [`EventCache`]: one `Arc<ComponentEventCatalog>` per
//!   `(project, kind, asset)`. A hit is a hash lookup and an `Arc` clone: no
//!   file or index access. File changes are applied lazily from the
//!   `engine_fs` change feed when the cache is next asked (no thread, no
//!   polling, no TTL), by the exact files a source read and the extensions it
//!   asked to watch, so a stale catalog cannot be served.
//!
//! How others do it: Unreal finds each module's assets through its asset
//! registry; Unity's Animation
//! Events are method names. Ours is the asset kind's own catalog, reached by
//! the property's type.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

use super::component_events::ComponentEventCatalog;

/// What a source sees while it reads an asset: the project, the file index and
/// a reader that records which files the catalog depends on.
pub struct SourceCx<'a> {
    root: &'a Path,
    index: Option<Arc<engine_fs::ProjectFileIndex>>,
    files: Vec<PathBuf>,
    extensions: Vec<&'static str>,
}

impl<'a> SourceCx<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            index: engine_fs::open_project_file_index(root).ok().flatten(),
            files: Vec::new(),
            extensions: Vec::new(),
        }
    }

    pub fn root(&self) -> &Path {
        self.root
    }

    /// The file an asset reference names: an asset id through the index, or a
    /// project-relative path as written (a property that names its asset by
    /// path, see [`AssetPathSlot`]).
    pub fn resolve(&self, kind: &str, reference: &str) -> Option<PathBuf> {
        let reference = reference.trim();
        if reference.contains(['/', '\\', '.']) {
            let path = self.root.join(reference.replace('\\', "/"));
            return path.is_file().then_some(path);
        }
        self.path_of(kind, reference)
    }

    /// The project file that carries asset `id` of `kind`, if the index knows
    /// one (and only one).
    pub fn path_of(&self, kind: &str, id: &str) -> Option<PathBuf> {
        self.index
            .as_ref()?
            .path_for_asset_id_kind(kind, id)
            .ok()
            .flatten()
    }

    /// Read a project file through the virtual file system (so remote and
    /// shared projects work) and remember it: the catalog is dropped when the
    /// file changes.
    pub fn read_to_string(&mut self, path: &Path) -> Result<String, String> {
        self.depend_on_file(path);
        engine_fs::virtual_fs::read_to_string(path).map_err(|error| error.to_string())
    }

    /// The catalog depends on this file even if it was not read through
    /// [`read_to_string`](Self::read_to_string).
    pub fn depend_on_file(&mut self, path: &Path) {
        if !self.files.iter().any(|known| known == path) {
            self.files.push(path.to_path_buf());
        }
    }

    /// The catalog depends on every project file with this extension (for a
    /// source that walks assets through a reader of its own). Coarser than
    /// [`depend_on_file`](Self::depend_on_file); prefer the exact files.
    pub fn watch_extension(&mut self, extension: &'static str) {
        if !self.extensions.contains(&extension) {
            self.extensions.push(extension);
        }
    }
}

/// The catalog of events an asset of one kind raises. Registered once by the
/// plugin that owns the kind.
pub struct AssetEventSource {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// `AssetKind::KIND` of the assets this source reads.
    pub kind: &'static str,
    /// The events of the asset with this id in the project `cx` was opened on.
    /// Pure: the same project state gives the same catalog.
    pub declare: fn(&mut SourceCx<'_>, asset: &str) -> ComponentEventCatalog,
    /// The title of a saved entry-point node for one of this source's events,
    /// by the event's name (`montage::<id>.hit`), so a graph loaded without its
    /// assets still reads well. `None` for an event this source does not name.
    pub node_title: Option<fn(event: &str) -> Option<String>>,
}

impl AssetEventSource {
    /// A source for the assets of kind `K`. The kind comes from the type, so
    /// a renamed or mistyped kind string cannot be registered.
    pub fn for_kind<K: pulsar_reflection::AssetKind>(
        origin: &'static str,
        declare: fn(&mut SourceCx<'_>, &str) -> ComponentEventCatalog,
    ) -> Self {
        Self {
            origin,
            kind: K::KIND,
            declare,
            node_title: None,
        }
    }
}

impl AssetEventSource {
    /// Name this source's saved nodes.
    pub fn with_node_title(mut self, node_title: fn(&str) -> Option<String>) -> Self {
        self.node_title = Some(node_title);
        self
    }
}

impl pulsar_plugins::Contribution for AssetEventSource {
    const CATEGORY: &'static str = "script.asset_event_source";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.kind.to_string()
    }
}

pulsar_plugins::registration!(AssetEventSource);

// ---- the cache -------------------------------------------------------------

type Key = (PathBuf, &'static str, String);

struct Entry {
    catalog: Arc<ComponentEventCatalog>,
    /// Normalised absolute paths the catalog was built from.
    files: HashSet<String>,
    extensions: Vec<&'static str>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Key, Entry>,
    /// Catalogs from a component's own values, by class and saved data.
    dynamic: HashMap<(String, String), Arc<ComponentEventCatalog>>,
    feed: Option<tokio::sync::broadcast::Receiver<engine_fs::events::FsEvent>>,
    hits: u64,
    misses: u64,
}

/// Catalogs by `(project, kind, asset)`, dropped when a file they were built
/// from changes.
pub struct EventCache {
    inner: Mutex<Inner>,
}

/// What the cache has served.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
}

fn normal(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

impl EventCache {
    /// An empty cache that listens to the `engine_fs` change feed.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                feed: Some(engine_fs::events::subscribe()),
                ..Inner::default()
            }),
        }
    }

    /// The process-wide cache.
    pub fn global() -> &'static EventCache {
        static CACHE: OnceLock<EventCache> = OnceLock::new();
        CACHE.get_or_init(EventCache::new)
    }

    /// The catalog of asset `id` through `source`, built at most once until a
    /// file it depends on changes.
    pub fn catalog(
        &self,
        source: &AssetEventSource,
        root: &Path,
        id: &str,
    ) -> Arc<ComponentEventCatalog> {
        let key: Key = (root.to_path_buf(), source.kind, id.to_owned());
        {
            let mut inner = lock(&self.inner);
            Self::drain(&mut inner, root);
            if let Some(entry) = inner.entries.get(&key) {
                let catalog = entry.catalog.clone();
                inner.hits += 1;
                return catalog;
            }
            inner.misses += 1;
        }
        // The source runs unlocked: it reads files, and may be slow.
        let mut cx = SourceCx::new(root);
        let catalog = Arc::new((source.declare)(&mut cx, id));
        let entry = Entry {
            catalog: catalog.clone(),
            files: cx.files.iter().map(|file| normal(file)).collect(),
            extensions: cx.extensions,
        };
        let mut inner = lock(&self.inner);
        // A change that arrived while the source ran may have made this stale.
        Self::drain(&mut inner, root);
        inner.entries.insert(key, entry);
        catalog
    }

    /// The catalog a component's own values add (`#[bp_event_source]`), built
    /// once per distinct saved data. Values do not depend on files, so only the
    /// size of the cache is bounded.
    pub fn dynamic_catalog(
        &self,
        class_name: &str,
        data: &Value,
        build: impl FnOnce() -> ComponentEventCatalog,
    ) -> Arc<ComponentEventCatalog> {
        let key = (class_name.to_owned(), data.to_string());
        let mut inner = lock(&self.inner);
        if let Some(found) = inner.dynamic.get(&key) {
            let found = found.clone();
            inner.hits += 1;
            return found;
        }
        inner.misses += 1;
        drop(inner);
        let catalog = Arc::new(build());
        let mut inner = lock(&self.inner);
        if inner.dynamic.len() >= 512 {
            inner.dynamic.clear();
        }
        inner.dynamic.insert(key, catalog.clone());
        catalog
    }

    /// Drop every catalog built from `path` (or, for a watched extension, from
    /// any file with it). The change feed does this by itself; this is for a
    /// caller that knows better, and for tests.
    pub fn invalidate_path(&self, root: &Path, path: &Path) {
        let mut inner = lock(&self.inner);
        Self::evict(&mut inner, root, path);
    }

    /// Drop everything.
    pub fn clear(&self) {
        let mut inner = lock(&self.inner);
        inner.entries.clear();
        inner.dynamic.clear();
    }

    pub fn stats(&self) -> CacheStats {
        let inner = lock(&self.inner);
        CacheStats {
            hits: inner.hits,
            misses: inner.misses,
            entries: inner.entries.len(),
        }
    }

    /// Apply the file changes that arrived since the last call.
    fn drain(inner: &mut Inner, root: &Path) {
        use tokio::sync::broadcast::error::TryRecvError;
        loop {
            let Some(feed) = inner.feed.as_mut() else {
                return;
            };
            match feed.try_recv() {
                Ok(event) => Self::evict(inner, root, &event.path),
                Err(TryRecvError::Empty) => return,
                // Changes were dropped: nothing can be trusted.
                Err(TryRecvError::Lagged(_)) => inner.entries.clear(),
                Err(TryRecvError::Closed) => {
                    inner.feed = Some(engine_fs::events::subscribe());
                    inner.entries.clear();
                    return;
                }
            }
        }
    }

    fn evict(inner: &mut Inner, root: &Path, path: &Path) {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        let wanted = normal(&absolute);
        let extension = absolute
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        inner.entries.retain(|_, entry| {
            let by_file = entry.files.contains(&wanted);
            let by_extension = extension
                .as_deref()
                .is_some_and(|e| entry.extensions.iter().any(|w| w.eq_ignore_ascii_case(e)));
            !(by_file || by_extension)
        });
    }
}

impl Default for EventCache {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---- reflection: which properties are asset references ---------------------

/// One property of a component class that references assets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetSlot {
    pub property: &'static str,
    pub kind: &'static str,
}

/// The asset-reference properties of `class_name`, from reflection: built once
/// per class and kept until the plugin set changes.
pub fn asset_slots(class_name: &str) -> Arc<[AssetSlot]> {
    static SLOTS: Mutex<Option<(u64, HashMap<String, Arc<[AssetSlot]>>)>> = Mutex::new(None);
    let generation = pulsar_plugins::generation();
    let mut guard = lock(&SLOTS);
    let (seen, classes) = guard.get_or_insert_with(|| (generation, HashMap::new()));
    if *seen != generation {
        *seen = generation;
        classes.clear();
    }
    if let Some(slots) = classes.get(class_name) {
        return slots.clone();
    }
    let slots: Arc<[AssetSlot]> = pulsar_reflection::REGISTRY
        .create_instance(class_name)
        .map(|instance| {
            instance
                .get_properties()
                .iter()
                .filter(|property| {
                    pulsar_reflection::property_declares_events(class_name, property.name)
                })
                .filter_map(|property| {
                    Some(AssetSlot {
                        property: property.name,
                        kind: pulsar_reflection::asset_kind_of(property.type_info)?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut slots: Vec<AssetSlot> = slots.to_vec();
    slots.extend(
        pulsar_plugins::registered::<AssetPathSlot>()
            .filter(|slot| slot.class_name == class_name)
            .map(|slot| AssetSlot {
                property: slot.property,
                kind: slot.kind,
            }),
    );
    let slots: Arc<[AssetSlot]> = slots.into();
    classes.insert(class_name.to_owned(), slots.clone());
    slots
}

// ---- the walker --------------------------------------------------------------

/// A component as a prefab stores it (`pulsar_class::PrefabComponent`).
#[derive(Clone, Copy, Debug)]
pub struct ComponentData<'a> {
    pub slot_id: &'a str,
    pub class_name: &'a str,
    pub enabled: bool,
    pub data: &'a Value,
}

/// An asset a component's property names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AssetUse {
    pub slot_id: String,
    pub class_name: String,
    pub property: &'static str,
    pub kind: &'static str,
    pub asset_id: String,
}

/// The assets named by `data` through `slots`: a property read by name, one
/// level of array. Unset (`null`), empty and non-string values name nothing.
pub fn referenced_assets(
    slot_id: &str,
    class_name: &str,
    slots: &[AssetSlot],
    data: &Value,
) -> Vec<AssetUse> {
    let mut found = Vec::new();
    let mut push = |slot: &AssetSlot, value: &Value| {
        if let Some(id) = value.as_str().map(str::trim).filter(|id| !id.is_empty()) {
            let used = AssetUse {
                slot_id: slot_id.to_owned(),
                class_name: class_name.to_owned(),
                property: slot.property,
                kind: slot.kind,
                asset_id: id.to_owned(),
            };
            if !found.contains(&used) {
                found.push(used);
            }
        }
    };
    for slot in slots {
        match data.get(slot.property) {
            Some(Value::Array(items)) => items.iter().for_each(|item| push(slot, item)),
            Some(value) => push(slot, value),
            None => {}
        }
    }
    found
}

/// The events one component gets from one asset.
#[derive(Clone, Debug)]
pub struct AssetEvents {
    pub used: AssetUse,
    pub catalog: Arc<ComponentEventCatalog>,
}

/// The events the enabled components in `components` raise because of the
/// assets they name, in component order then property order. Assets of a kind
/// nobody registered a source for are skipped (they cost nothing).
pub fn component_events(root: &Path, components: &[ComponentData<'_>]) -> Vec<AssetEvents> {
    component_events_with(EventCache::global(), root, components)
}

/// [`component_events`] through a given cache.
pub fn component_events_with(
    cache: &EventCache,
    root: &Path,
    components: &[ComponentData<'_>],
) -> Vec<AssetEvents> {
    let mut out = Vec::new();
    for component in components.iter().filter(|c| c.enabled) {
        let slots = asset_slots(component.class_name);
        if !slots.is_empty() {
            for used in referenced_assets(
                component.slot_id,
                component.class_name,
                &slots,
                component.data,
            ) {
                let Some(source) = pulsar_plugins::registered::<AssetEventSource>()
                    .find(|source| source.kind == used.kind)
                else {
                    continue;
                };
                let catalog = cache.catalog(source, root, &used.asset_id);
                out.push(AssetEvents { used, catalog });
            }
        }
        // Events the component's own values add (`#[bp_event_source]`).
        for source in pulsar_world_registry::component_event_sources(component.class_name) {
            let catalog = cache.dynamic_catalog(component.class_name, component.data, || {
                dynamic_catalog((source.events)(component.data), component.class_name)
            });
            if catalog.events.is_empty() {
                continue;
            }
            out.push(AssetEvents {
                used: AssetUse {
                    slot_id: component.slot_id.to_owned(),
                    class_name: component.class_name.to_owned(),
                    property: "",
                    kind: "",
                    asset_id: String::new(),
                },
                catalog,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn calls(asset: &str) -> usize {
        lock(&CALLS).iter().filter(|a| a.as_str() == asset).count()
    }

    fn declare(cx: &mut SourceCx<'_>, asset: &str) -> ComponentEventCatalog {
        lock(&CALLS).push(asset.to_owned());
        cx.depend_on_file(&cx.root().join("maps").join(format!("{asset}.inputmap")));
        ComponentEventCatalog::default()
    }

    fn watching(cx: &mut SourceCx<'_>, asset: &str) -> ComponentEventCatalog {
        lock(&CALLS).push(asset.to_owned());
        cx.watch_extension("animgraph");
        ComponentEventCatalog::default()
    }

    fn source(declare: fn(&mut SourceCx<'_>, &str) -> ComponentEventCatalog) -> AssetEventSource {
        AssetEventSource::for_kind::<pulsar_reflection::InputMapAssetKind>("test", declare)
    }

    fn root() -> PathBuf {
        std::env::temp_dir().join("asset_event_sources_no_such_project")
    }

    #[test]
    fn the_kind_comes_from_the_type() {
        assert_eq!(source(declare).kind, "inputmap");
    }

    #[test]
    fn referenced_assets_read_properties_by_name_and_skip_unset() {
        let slots = [
            AssetSlot {
                property: "mapping",
                kind: "inputmap",
            },
            AssetSlot {
                property: "extra",
                kind: "inputmap",
            },
        ];
        let data = json!({
            "mapping": "11111111-1111-4111-8111-111111111111",
            "extra": ["22222222-2222-4222-8222-222222222222", "", null,
                      "22222222-2222-4222-8222-222222222222"],
            "other": "not-a-slot"
        });
        let used = referenced_assets("slot", "C", &slots, &data);
        assert_eq!(
            used.iter().map(|u| u.asset_id.as_str()).collect::<Vec<_>>(),
            [
                "11111111-1111-4111-8111-111111111111",
                "22222222-2222-4222-8222-222222222222"
            ],
            "in property order, de-duplicated, unset skipped"
        );
        assert!(referenced_assets("s", "C", &slots, &json!({ "mapping": null })).is_empty());
        assert!(referenced_assets("s", "C", &slots, &json!({})).is_empty());
    }

    #[test]
    fn a_catalog_is_built_once_then_served_from_the_cache() {
        let cache = EventCache::new();
        let source = source(declare);

        let a = cache.catalog(&source, &root(), "once");
        let b = cache.catalog(&source, &root(), "once");
        assert!(Arc::ptr_eq(&a, &b), "the same Arc");
        assert_eq!(calls("once"), 1, "built once");
        cache.catalog(&source, &root(), "once-two");
        assert_eq!(
            cache.stats(),
            CacheStats {
                hits: 1,
                misses: 2,
                entries: 2
            }
        );
    }

    #[test]
    fn changing_a_file_the_source_read_drops_only_its_catalogs() {
        let cache = EventCache::new();
        let source = source(declare);
        let one = cache.catalog(&source, &root(), "one");
        let two = cache.catalog(&source, &root(), "two");
        cache.invalidate_path(&root(), &root().join("maps").join("one.inputmap"));
        let one_again = cache.catalog(&source, &root(), "one");
        let two_again = cache.catalog(&source, &root(), "two");
        assert!(!Arc::ptr_eq(&one, &one_again), "rebuilt");
        assert!(Arc::ptr_eq(&two, &two_again), "untouched");
    }

    #[test]
    fn a_relative_path_is_taken_from_the_project_root() {
        let cache = EventCache::new();
        let source = source(declare);
        let one = cache.catalog(&source, &root(), "one");
        cache.invalidate_path(&root(), Path::new("maps/one.inputmap"));
        assert!(!Arc::ptr_eq(&one, &cache.catalog(&source, &root(), "one")));
    }

    #[test]
    fn a_watched_extension_drops_every_catalog_that_asked_for_it() {
        let cache = EventCache::new();
        let source = source(watching);
        let a = cache.catalog(&source, &root(), "a");
        cache.invalidate_path(&root(), &root().join("x").join("Other.ANIMGRAPH"));
        assert!(!Arc::ptr_eq(&a, &cache.catalog(&source, &root(), "a")));
        let b = cache.catalog(&source, &root(), "b");
        cache.invalidate_path(&root(), &root().join("x").join("unrelated.txt"));
        assert!(Arc::ptr_eq(&b, &cache.catalog(&source, &root(), "b")));
    }

    #[test]
    fn the_same_asset_in_two_projects_is_cached_separately() {
        let cache = EventCache::new();
        let source = source(declare);
        let a = cache.catalog(&source, &root(), "one");
        let b = cache.catalog(&source, &root().join("other"), "one");
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn the_change_feed_evicts_without_anyone_asking() {
        let cache = EventCache::new();
        let source = source(declare);
        let one = cache.catalog(&source, &root(), "feed");
        engine_fs::events::emit(
            root().join("maps").join("feed.inputmap"),
            engine_fs::events::FsChangeKind::Modified,
        );
        assert!(
            !Arc::ptr_eq(&one, &cache.catalog(&source, &root(), "feed")),
            "the next lookup sees the change"
        );
    }
}

/// The title a source gives a saved asset-owned event node, if any source
/// names it.
pub fn node_title(event: &str) -> Option<String> {
    pulsar_plugins::registered::<AssetEventSource>()
        .find_map(|source| source.node_title.and_then(|title| title(event)))
}

// ---- path slots: properties that name an asset by path ---------------------

/// A `String` property that names an asset by project-relative path rather
/// than by a typed `AssetRef`. Reflection cannot see what such a string
/// points at, so the owning plugin says so. It is the bridge for a property
/// that predates typed references and cannot change its saved form; a new
/// property should be an `AssetRef<K>`, which needs no registration.
pub struct AssetPathSlot {
    pub origin: &'static str,
    pub class_name: &'static str,
    pub property: &'static str,
    /// `AssetKind::KIND` of the asset the path names.
    pub kind: &'static str,
}

impl AssetPathSlot {
    /// The slot of `class_name.property`, a path to an asset of kind `K`.
    pub fn for_kind<K: pulsar_reflection::AssetKind>(
        origin: &'static str,
        class_name: &'static str,
        property: &'static str,
    ) -> Self {
        Self {
            origin,
            class_name,
            property,
            kind: K::KIND,
        }
    }
}

impl pulsar_plugins::Contribution for AssetPathSlot {
    const CATEGORY: &'static str = "script.asset_path_slot";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        format!("{}.{}", self.class_name, self.property)
    }
}

pulsar_plugins::registration!(AssetPathSlot);

/// A component's value-dependent events as a catalog the script editor draws.
fn dynamic_catalog(
    events: Vec<pulsar_world_registry::DynamicEvent>,
    class_name: &str,
) -> ComponentEventCatalog {
    use super::component_events::{CatalogEvent, EventView};
    ComponentEventCatalog {
        events: events
            .into_iter()
            .map(|event| CatalogEvent {
                view: EventView {
                    group: event.group,
                    description: format!("Runs when this {class_name} raises {}.", event.label),
                    documentation: format!(
                        "`{}` is offered because of this {class_name}'s current values, and \
                         is delivered on the component's actor channel.",
                        event.descriptor.name
                    ),
                    properties: Vec::new(),
                },
                label: event.label,
                descriptor: event.descriptor,
            })
            .collect(),
        ..ComponentEventCatalog::default()
    }
}
