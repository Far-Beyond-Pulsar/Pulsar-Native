//! `world_plugin_host <plugin library>`: load the plugin and print, one
//! `key=value` per line, what its component registration reached in this
//! process. Exits non-zero only when something it relies on is missing.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use pulsar_world_dylib as _;

/// The editor installs its own global allocator (`TrackingAllocator`); the
/// host does too and reports which allocations reach it. With the standard
/// library linked dynamically, as a Rust dylib requires, code compiled into
/// `libstd` and the dylib allocates through `libstd`'s default allocator.
struct HostAllocator;

static HOST_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged.
unsafe impl GlobalAlloc for HostAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        HOST_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: HostAllocator = HostAllocator;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: world_plugin_host <plugin library>");
    // SAFETY: a fixture library built for this test; permanently loaded, as
    // the editor's plugin manager does.
    let library = unsafe { libloading::Library::new(&path) }
        .unwrap_or_else(|error| panic!("loading {path}: {error}"));
    let call_bool = |name: &str| unsafe {
        library
            .get::<extern "C" fn() -> bool>(name.as_bytes())
            .unwrap()()
    };
    let call_u64 = |name: &str| unsafe {
        library
            .get::<extern "C" fn() -> u64>(name.as_bytes())
            .unwrap()()
    };

    println!(
        "plugin_sees_class={}",
        call_bool("plugin_registry_has_widget")
    );
    let host_sees_class = pulsar_world_registry::registered_world_component_classes()
        .any(|class| class == "PluginWidget");
    println!("host_sees_class={host_sees_class}");
    let plugin_id = call_u64("plugin_widget_component_id");
    let host_id =
        pulsar_world_registry::component_id_for_class("PluginWidget").map(|id| id.0 as u64);
    println!("same_component_id={}", host_id == Some(plugin_id));

    if host_sees_class {
        // A live component of the host's World, through the registry alone.
        use pulsar_reflection::EngineClass as _;
        let mut world = pulsar_scenedb::World::new();
        let entity = world.spawn();
        let value = pulsar_world_registry::new_world_component_value("PluginWidget")
            .expect("the class has a factory");
        pulsar_world_registry::insert_world_component_value(
            "PluginWidget",
            &mut world,
            entity,
            value,
        )
        .expect("insert");
        let mut cursor = world.open_change_cursor_id(
            pulsar_world_registry::component_id_for_class("PluginWidget").unwrap(),
        );
        pulsar_world_registry::set_world_component_property(
            "PluginWidget",
            &mut world,
            entity,
            "charge",
            Box::new(7.5_f32),
        )
        .unwrap_or_else(|_| panic!("property write refused"));
        let live = pulsar_world_registry::get_world_component_as_engine_class(
            "PluginWidget",
            &world,
            entity,
        )
        .expect("live value");
        let charge = live
            .get_properties()
            .into_iter()
            .find(|property| property.name == "charge")
            .and_then(|property| (property.getter)(live).downcast_ref::<f32>().copied());
        println!("live_charge={charge:?}");
        let mut changes = Vec::new();
        let _ = world.read_changes(&mut cursor, &mut changes);
        println!("journal_changes={}", changes.len());
    }
    // Which allocations reach the host's allocator: one made by library code
    // (`format!` runs in `libstd`), and one made by a generic instantiated in
    // this binary (`Box<HostOnly>`).
    struct HostOnly(#[allow(dead_code)] [u8; 48]);
    let reached = |allocate: &dyn Fn()| {
        let before = HOST_ALLOCATIONS.load(Ordering::Relaxed);
        allocate();
        HOST_ALLOCATIONS.load(Ordering::Relaxed) > before
    };
    println!(
        "library_allocation_reaches_host_allocator={}",
        reached(&|| drop(std::hint::black_box(format!(
            "{}",
            std::hint::black_box(4096)
        )))),
    );
    println!(
        "host_generic_allocation_reaches_host_allocator={}",
        reached(&|| drop(std::hint::black_box(Box::new(HostOnly([7; 48]))))),
    );
    // Plugins are never unloaded.
    std::mem::forget(library);
}
