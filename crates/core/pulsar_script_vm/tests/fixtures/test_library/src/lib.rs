//! Natives for `pulsar_script_vm`'s library loading tests. The behavior of
//! `fixture::version` is chosen at build time by `PULSAR_FIXTURE_VERSION`,
//! so a test can build two versions of this library and swap them.

use pulsar_script_vm::{LibraryRegistrar, NativeFn};

fn register(registrar: &mut LibraryRegistrar) {
    let version: i64 = option_env!("PULSAR_FIXTURE_VERSION").and_then(|v| v.parse().ok()).unwrap_or(1);
    registrar.add(NativeFn::builder("fixture::double").pure().build(|x: i64| x * 2));
    registrar.add(NativeFn::builder("fixture::version").pure().build(move || version));
    registrar.add(NativeFn::builder("fixture::greet").pure().build(|name: String| format!("hello {name}")));
}

pulsar_script_vm::native_library!(register);
