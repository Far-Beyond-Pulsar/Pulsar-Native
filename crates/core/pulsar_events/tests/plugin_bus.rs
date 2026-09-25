//! A plugin compiled as a separate library shares the host's buses through
//! Gamma's FFI (Pulsar-Native#930, #924).
//!
//! `tests/fixtures/plugin` is built here with its own cargo invocation and
//! target directory, so it contains its own copy of `pulsar_events` and
//! Gamma: its statics (host bus) and `TypeId`s are its own. It is attached
//! the way the editor's plugin loader attaches plugins
//! (`_plugin_attach_event_bus` with `host::export_host_bus()`).

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use libloading::Library;
use pulsar_events::builtin::Hit;
use pulsar_events::gamma::ffi::RawBus;
use pulsar_events::gamma::{Channel, DynValue, SubscribeOptions};
use pulsar_events::{AssetKind, AssetUpdated, EventHub, FlushPoint, host};

fn build_plugin() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugin/Cargo.toml");
    let target_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pulsar-events-test-plugin");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target_dir)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("failed to run cargo for the test plugin");
    assert!(status.success(), "building the test plugin failed");
    target_dir.join("release").join(libloading::library_filename("pulsar_events_test_plugin"))
}

fn plugin() -> &'static Mutex<Library> {
    static PLUGIN: OnceLock<Mutex<Library>> = OnceLock::new();
    PLUGIN.get_or_init(|| {
        let path = build_plugin();
        // SAFETY: our own test library; no load-time side effects.
        let lib = unsafe { Library::new(&path) }.unwrap_or_else(|e| panic!("loading {}: {e}", path.display()));
        // Attach it to this process's host bus, as the plugin manager does.
        let attach: libloading::Symbol<host::AttachFn> =
            unsafe { lib.get(host::ATTACH_SYMBOL.as_bytes()) }.expect("the attach entry point");
        let status = unsafe { attach(host::export_host_bus().expect("the host exports its bus")) };
        assert_eq!(status, host::ATTACH_OK);
        Mutex::new(lib)
    })
}

fn call<R>(lib: &Library, name: &str) -> R {
    unsafe { lib.get::<extern "C" fn() -> R>(name.as_bytes()).unwrap()() }
}

#[test]
fn asset_updates_cross_the_library_boundary_both_ways() {
    let lib = plugin().lock().unwrap();
    assert_eq!(call::<u32>(&lib, "fixture_is_attached"), 1, "the plugin uses the host's bus");

    // Plugin -> host.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = Arc::clone(&seen);
    let _sub = pulsar_events::subscribe_asset_updates(None, move |e| s.lock().unwrap().push(e.clone()));
    call::<()>(&lib, "fixture_publish_asset");
    assert_eq!(
        *seen.lock().unwrap(),
        vec![AssetUpdated::new(AssetKind::Blueprint).with_id("plugin-class")]
    );

    // Host -> plugin.
    call::<()>(&lib, "fixture_watch_assets");
    let before = call::<u64>(&lib, "fixture_asset_updates_seen");
    pulsar_events::publish_asset_updated(AssetUpdated::new(AssetKind::Blueprint).with_id("x"));
    pulsar_events::publish_asset_updated(AssetUpdated::new(AssetKind::Mesh));
    assert_eq!(call::<u64>(&lib, "fixture_asset_updates_seen"), before + 1, "filtered by kind in the plugin");
}

#[test]
fn plugin_events_reach_host_subscribers_through_the_engine_hub() {
    let lib = plugin().lock().unwrap();
    let hub = EventHub::new();
    hub.set_tap(true, 16);
    let attach: libloading::Symbol<unsafe extern "C" fn(RawBus) -> u32> =
        unsafe { lib.get(b"fixture_attach_hub") }.unwrap();
    assert_eq!(unsafe { attach(hub.export_raw()) }, 0);
    assert!(hub.descriptor_by_name("Plugin.Scored").is_some(), "the plugin registered its event");

    let hits = Arc::new(Mutex::new(Vec::new()));
    let h = Arc::clone(&hits);
    let _mine = hub.bus().subscribe_with(SubscribeOptions::channel(Channel::Entity(7)), move |e: &Hit| {
        h.lock().unwrap().push(*e);
    });
    let others = Arc::new(AtomicU64::new(0));
    let o = Arc::clone(&others);
    let _other = hub.bus().subscribe_with(SubscribeOptions::channel(Channel::Entity(8)), move |_: &Hit| {
        o.fetch_add(1, Ordering::SeqCst);
    });
    let scored = Arc::new(Mutex::new(Vec::new()));
    let sc = Arc::clone(&scored);
    let id = hub.descriptor_by_name("Plugin.Scored").unwrap().id;
    let _scored = hub.bus().subscribe_dyn(id, SubscribeOptions::default(), move |e| {
        sc.lock().unwrap().push(e.fields.clone());
    });

    let publish_hit: libloading::Symbol<extern "C" fn(u64, u64, f64) -> u32> =
        unsafe { lib.get(b"fixture_publish_hit") }.unwrap();
    let publish_scored: libloading::Symbol<extern "C" fn(u64, i64) -> u32> =
        unsafe { lib.get(b"fixture_publish_scored") }.unwrap();
    assert_eq!(publish_hit(7, 99, 2.5), 0);
    assert_eq!(publish_scored(7, 10), 0);
    assert_ne!(call::<u32>(&lib, "fixture_publish_bad"), 0, "the host checks the descriptor");
    assert!(hits.lock().unwrap().is_empty(), "deferred until the flush");

    hub.flush(FlushPoint::AfterPhysics);
    assert_eq!(*hits.lock().unwrap(), vec![Hit { entity: 7, other: 99, impulse: 2.5 }]);
    assert_eq!(others.load(Ordering::SeqCst), 0, "entity 8 heard nothing");
    assert_eq!(*scored.lock().unwrap(), vec![vec![DynValue::U64(7), DynValue::I64(10)]]);

    call::<()>(&lib, "fixture_shutdown");
    assert_eq!(hub.queued_len(), 0);
}
