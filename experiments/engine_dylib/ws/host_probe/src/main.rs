use sdk_dylib as _;
use sdk_extra as _;
#[path = "../../probe/probe.rs"]
mod probe;
use probe::Probe;

fn main() {
    let path = std::env::args().nth(1).expect("usage: host_probe <plugin dll>");
    println!("agent_chat_core (lives in 2nd dylib) ProviderKind TypeId = {:?}", std::any::TypeId::of::<agent_chat_core::ProviderKind>());
    let host = probe::probe_here();
    println!("host: linked_descriptor before load = {:?}", pulsar_plugins::library::linked_descriptor().map(|d| d.id));
    let lib = unsafe { libloading::Library::new(&path) }.expect("load plugin");
    println!("loaded {path}");
    let mut plug = std::mem::MaybeUninit::<Probe>::uninit();
    let plug = unsafe {
        let f: libloading::Symbol<extern "C" fn(*mut Probe)> = lib.get(b"pulsar_spike_probe").expect("probe symbol");
        f(plug.as_mut_ptr());
        plug.assume_init()
    };
    println!("host: linked_descriptor after load  = {:?}", pulsar_plugins::library::linked_descriptor().map(|d| d.id));
    macro_rules! cmp { ($($f:ident),*) => { $( println!("{:<26} same={}  host={:?} plugin={:?}", stringify!($f), host.$f == plug.$f, host.$f, plug.$f); )* } }
    cmp!(gpui_window, gpui_app, ui_theme, language_servers, entity_language_servers);
    cmp!(gpui_runtime_addr, gpui_module_slot, engine_settings_addr, helio_task_host, helio_handle);
    // loader entry points
    for sym in ["_plugin_create", "_plugin_version", "_plugin_manifest", "_plugin_attach_helio", "_plugin_attach_world_runtime", "_plugin_attach_parking_lot", "_plugin_attach_event_bus", "_plugin_attach_profiler", "_plugin_attach_settings", "_pulsar_plugins_export"] {
        let found = unsafe { lib.get::<*const ()>(sym.as_bytes()) }.is_ok();
        println!("export {sym}: {found}");
    }
    // Run the plugin's real export: its root's `build` registers into THIS process's registries.
    unsafe {
        let exp: libloading::Symbol<pulsar_plugins::library::ExportFn> = lib.get(b"_pulsar_plugins_export").unwrap();
        let mut out = std::mem::MaybeUninit::<pulsar_plugins::library::FfiLibrary>::uninit();
        let ok = exp(out.as_mut_ptr());
        if ok {
            let l = out.assume_init();
            println!("_pulsar_plugins_export ok: abi={} plugins={}", l.abi, l.len);
            for i in 0..l.len {
                let p = &*l.plugins.add(i);
                let cats: Vec<String> = (0..p.len).map(|j| { let e = &*p.entries.add(j); String::from_utf8_lossy(std::slice::from_raw_parts(e.category.ptr, e.category.len)).into_owned() }).collect();
                println!("  plugin root registered {} entries: {:?}", p.len, cats);
            }
        } else { println!("_pulsar_plugins_export returned false (panicked inside)"); }
    }
    let all_same = host.gpui_window == plug.gpui_window && host.entity_language_servers == plug.entity_language_servers
        && host.gpui_runtime_addr == plug.gpui_runtime_addr && host.engine_settings_addr == plug.engine_settings_addr
        && host.helio_handle == plug.helio_handle && host.language_servers == plug.language_servers;
    println!("ALL SHARED: {all_same}");
    std::mem::forget(lib);
}
