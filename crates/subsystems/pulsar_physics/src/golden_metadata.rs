//! #645 golden registry-snapshot guard (unit-test module: see the linkage
//! note below).
//!
//! The reflected surface of every class this crate registers
//! (`RigidbodyComponent`, `PhysicsComponent`) is diffed against a checked-in
//! file, so metadata regressions -- renamed parameters, flipped
//! `method_type` purity tags, lost categories or display names -- fail CI
//! instead of silently degrading Blueprint discovery.
//!
//! ## Why a `cfg(test)` module and not `tests/`
//!
//! Inventory registration statics are extracted from an rlib only when the
//! linking binary references their containing object files; an integration-
//! test binary that touches nothing pulls in a linker-GC-arbitrary SUBSET
//! of registrations (observed: classes survived, method registrations did
//! not). A unit test recompiles THIS whole crate into its test binary, so
//! every registration is present deterministically.
//!
//! Regenerating deliberately (metadata CHANGED on purpose):
//!
//! ```text
//! PULSAR_UPDATE_SNAPSHOT=1 cargo test -p pulsar_physics golden_metadata
//! git add the reviewed snapshot diff && commit it with the change
//! ```
//!
//! The snapshot is produced by `pulsar_world_registry::metadata_snapshot_json`
//! (fully sorted, stable across runs); `find_overloaded_methods` enforces
//! the overload-policy half of #645.

use pulsar_world_registry::{find_overloaded_methods, metadata_snapshot_json};

const SNAPSHOT_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/expected_registry_snapshot.json"
);

#[test]
fn registry_metadata_matches_the_golden_snapshot() {
    let actual = metadata_snapshot_json();

    if std::env::var("PULSAR_UPDATE_SNAPSHOT").is_ok() {
        let pretty = serde_json::to_string_pretty(&actual).expect("snapshot serializes") + "\n";
        std::fs::write(SNAPSHOT_PATH, pretty).expect("writes snapshot file");
        eprintln!("golden snapshot regenerated at {SNAPSHOT_PATH}");
    }

    let raw =
        std::fs::read_to_string(SNAPSHOT_PATH).expect("checked-in snapshot exists under tests/");
    let expected: serde_json::Value = serde_json::from_str(&raw).expect("snapshot parses");
    assert_eq!(
        actual, expected,
        "reflection metadata drifted from the golden snapshot (#645); \
         fix the regression or regenerate with PULSAR_UPDATE_SNAPSHOT=1 and review the diff"
    );
}

/// #645 overload policy at link time: no class may carry two methods under
/// one name (name-keyed dispatch would shadow one of them).
#[test]
fn no_class_has_overloaded_methods() {
    assert_eq!(
        find_overloaded_methods(),
        Vec::new(),
        "overloaded reflected methods found -- rename them; dispatch is name-keyed"
    );
}
