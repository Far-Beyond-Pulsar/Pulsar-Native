//! `world_plugin_host <plugin library> [attach | attach-bad-abi]`: load the
//! plugin, attach it to this process's world runtime when asked (or offer it
//! a runtime of another ABI), and print, one `key=value` per line, what its
//! component registration reached in this process.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hash::{Hash, Hasher};

/// The editor installs its own global allocator (`TrackingAllocator`); the
/// host does too, so the plugin's allocator differs from the host's.
struct HostAllocator;

// SAFETY: forwards every call to the system allocator unchanged.
unsafe impl GlobalAlloc for HostAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: HostAllocator = HostAllocator;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: world_plugin_host <plugin library> [attach | attach-bad-abi]");
    let mode = args.next().unwrap_or_default();
    // SAFETY: a fixture library built for this test; permanently loaded, as
    // the editor's plugin manager does.
    let library = unsafe { libloading::Library::new(&path) }
        .unwrap_or_else(|error| panic!("loading {path}: {error}"));
    let symbol = |name: &str| unsafe {
        library
            .get::<*const ()>(name.as_bytes())
            .map(|s| *s)
            .unwrap()
    };
    let call_bool =
        |name: &str| unsafe { std::mem::transmute::<_, extern "C" fn() -> bool>(symbol(name))() };
    let call_u64 =
        |name: &str| unsafe { std::mem::transmute::<_, extern "C" fn() -> u64>(symbol(name))() };
    // The plugin manager's call: the library's exported attach entry point.
    let attach = |runtime: *const pulsar_world_registry::runtime::WorldRuntimes| unsafe {
        std::mem::transmute::<_, pulsar_world_registry::runtime::AttachFn>(symbol(
            pulsar_world_registry::runtime::ATTACH_SYMBOL,
        ))(runtime.cast())
    };

    match mode.as_str() {
        "attach" => println!(
            "attached={}",
            attach(pulsar_world_registry::runtime::host())
        ),
        "attach-bad-abi" => {
            // The host's runtime as a library of another world-runtime ABI
            // would describe it.
            let mut other = *pulsar_world_registry::runtime::host();
            other.abi ^= 1;
            println!("attached={}", attach(Box::leak(Box::new(other))));
        }
        _ => {}
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::any::TypeId::of::<pulsar_scenedb::World>().hash(&mut hasher);
    println!(
        "same_engine_type_ids={}",
        hasher.finish() == call_u64("plugin_engine_type_hash")
    );
    println!(
        "plugin_sees_class={}",
        call_bool("plugin_registry_has_widget")
    );
    let host_sees_class = pulsar_world_registry::registered_world_component_classes()
        .any(|class| class == "PluginWidget");
    println!("host_sees_class={host_sees_class}");
    println!(
        "host_reflects_class={}",
        pulsar_reflection::REGISTRY.has_class("PluginGadget")
    );
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
    if host_sees_class {
        // Placed on an object, saved as level records, loaded into a fresh
        // World: the level file's path, through the plugin's own codec.
        use pulsar_reflection::EngineClass as _;
        use pulsar_world_registry::pulsar_scene_model::{
            attachments::NewInstance, ComponentInstance,
        };
        let mut world = pulsar_scenedb::World::new();
        let owner = world.spawn();
        pulsar_world_registry::attach_component(
            &mut world,
            owner,
            NewInstance::new("PluginWidget"),
            pulsar_world_registry::ComponentPayload::Json(serde_json::json!({ "charge": 3.25 })),
        )
        .expect("place");
        let saved = serde_json::to_string(&pulsar_world_registry::component_records(&world, owner))
            .expect("save");
        let records: Vec<ComponentInstance> = serde_json::from_str(&saved).expect("read");
        let mut loaded = pulsar_scenedb::World::new();
        let owner = loaded.spawn();
        let attached =
            pulsar_world_registry::attach_records(&mut loaded, owner, &records).expect("load");
        let reloaded = pulsar_world_registry::instance_engine_class(&loaded, attached[0])
            .expect("reloaded value");
        let charge = reloaded
            .get_properties()
            .into_iter()
            .find(|property| property.name == "charge")
            .and_then(|property| (property.getter)(reloaded).downcast_ref::<f32>().copied());
        println!("reloaded_charge={charge:?}");
    }
    // Plugins are never unloaded.
    std::mem::forget(library);
}
