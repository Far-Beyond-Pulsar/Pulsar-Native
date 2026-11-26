//! What the editor links is what `plugins/installed` installs.
//!
//! Without `all` the editor links `render_bps` and loads every other plugin as a
//! library. With `all` (the editor's `link-plugins`) it links the whole installed
//! set with its editor registrations, so the plugins the editor registers are
//! exactly those under `plugins/vendor` that describe themselves, the set
//! `tools/plugin_builder` builds as libraries plus `render_bps`.
//!
//! `cargo test -p linked_plugins` and `cargo test -p linked_plugins --features all`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use pulsar_plugins::PluginDescriptor;

// Linked, so the plugins' root crates install their descriptors in this binary.
use linked_plugins as _;

/// Every plugin id under `plugins/vendor` (a `pulsar-plugin.toml` each).
fn vendor_ids() -> BTreeSet<String> {
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vendor");
    let mut ids = BTreeSet::new();
    for entry in std::fs::read_dir(&vendor).expect("plugins/vendor") {
        let file = entry.unwrap().path().join(pulsar_plugins::DESCRIPTOR_FILE);
        if let Ok(source) = std::fs::read_to_string(&file) {
            let descriptor = PluginDescriptor::parse(&source)
                .unwrap_or_else(|error| panic!("{}: {error}", file.display()));
            ids.insert(descriptor.id);
        }
    }
    ids
}

fn linked_ids() -> BTreeSet<String> {
    plugin_manager::linked_plugins()
        .into_iter()
        .map(|plugin| plugin.id)
        .collect()
}

#[cfg(feature = "all")]
#[test]
fn every_installed_plugin_is_linked() {
    let vendor = vendor_ids();
    let linked = linked_ids();
    assert!(vendor.len() > 20, "found only {} plugins under plugins/vendor", vendor.len());
    assert_eq!(
        linked, vendor,
        "the linked set differs from the installed set (plugins/installed, generated from the descriptors)"
    );
    // The same set the installed crate installs: nothing linked is outside it, and the
    // loader and the catalog agree on what is linked.
    let installed: BTreeSet<String> = pulsar_plugins::installed()
        .iter()
        .map(|descriptor| descriptor.id.clone())
        .collect();
    assert_eq!(linked, installed);
    assert!(linked_plugins::ALL);
}

#[cfg(not(feature = "all"))]
#[test]
fn only_render_bps_is_linked_by_default() {
    assert_eq!(
        linked_ids().into_iter().collect::<Vec<_>>(),
        vec!["pulsar.render_graph".to_string()]
    );
    assert!(!linked_plugins::ALL);
}
