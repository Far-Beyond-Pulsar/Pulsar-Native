//! The loader and a plugin the executable links.
//!
//! An editor built with `link-plugins` registers every installed plugin itself, so
//! the library of one in `plugins/editor` (a default build's output) must not be
//! loaded too: the plugin would register twice. A library of a plugin that is not
//! linked (hub-installed, third party) loads as always.
//!
//! Uses the real fixture library (`plugin_loader_fixture`) through the real
//! `prepare_plugins_in_dir_except`, with a lock and an integrity manifest as
//! `tools/plugin_builder` writes them.

use plugin_manager::{LibraryOutcome, LinkedPlugin, PluginManager};
use pulsar_install_layout::{InstallLayout, PluginRecord};
use std::path::PathBuf;
use std::sync::Mutex;

/// The load report is process-wide: one load at a time.
static ONE_LOAD: Mutex<()> = Mutex::new(());

/// The compiled fixture plugin library in the test's dependency directory.
fn fixture_library() -> PathBuf {
    let deps = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let prefix = format!("{}plugin_loader_fixture", std::env::consts::DLL_PREFIX);
    let suffix = std::env::consts::DLL_SUFFIX;
    let exact = deps.join(format!("{prefix}{suffix}"));
    if exact.is_file() {
        return exact;
    }
    std::fs::read_dir(&deps)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(suffix))
        })
        .expect("the plugin loader fixture library")
}

fn record(file: &str, repo_url: &str) -> PluginRecord {
    PluginRecord {
        name: file.into(),
        repo_url: repo_url.into(),
        version: "0.1.0".into(),
        file: file.into(),
        sha256: String::new(),
        registry_url: String::new(),
        installed_at: "now".into(),
        method: "source".into(),
    }
}

#[test]
fn linked_wins_the_library_of_the_same_plugin_is_skipped_and_an_unknown_library_still_loads() {
    let _one = ONE_LOAD.lock().unwrap_or_else(|poison| poison.into_inner());
    let install = tempfile::tempdir().unwrap();
    let layout = InstallLayout::new(install.path());
    let dir = layout.plugins_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let ext = std::env::consts::DLL_EXTENSION;
    let source = fixture_library();

    // A default build left its library of the linked plugin here, named as the builder names it,
    // and another under a name that says nothing (the lock says whose it is).
    let named_after_root = format!("alpha_plugin.{ext}");
    let named_by_lock = format!("anything.{ext}");
    // A plugin that exists only as a library.
    let third_party = format!("third_party.{ext}");
    for file in [&named_after_root, &named_by_lock, &third_party] {
        std::fs::copy(&source, dir.join(file)).unwrap();
    }
    std::fs::write(
        layout.plugin_lock_path(),
        serde_json::to_string(&pulsar_install_layout::PluginLock {
            plugins: vec![
                record(&named_by_lock, "local:pulsar.alpha"),
                record(&third_party, "https://example.com/third_party"),
            ],
        })
        .unwrap(),
    )
    .unwrap();
    std::fs::write(layout.integrity_manifest_path(), r#"{"__allow_unlisted__":"true"}"#).unwrap();

    let linked = vec![LinkedPlugin { id: "pulsar.alpha".into(), root: "alpha_plugin".into() }];
    let prepared = PluginManager::prepare_plugins_in_dir_except(&dir, &linked);

    // Only the library of a plugin that is not linked was prepared (loaded, verified).
    let names: Vec<String> = prepared
        .iter()
        .map(|(path, _)| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec![third_party.clone()], "prepared: {names:?}");
    assert!(prepared[0].1.is_ok(), "the unknown library did not load: {:?}", prepared[0].1.as_ref().err());

    // And the report says which was which.
    let report = plugin_manager::load_report();
    let outcome = |file: &str| report.libraries.iter().find(|l| l.file == file).map(|l| l.outcome.clone());
    assert_eq!(outcome(&named_after_root), Some(LibraryOutcome::Linked { plugin_id: "pulsar.alpha".into() }));
    assert_eq!(outcome(&named_by_lock), Some(LibraryOutcome::Linked { plugin_id: "pulsar.alpha".into() }));
    assert_eq!(outcome(&third_party), Some(LibraryOutcome::Prepared));
}

#[test]
fn with_nothing_linked_every_library_is_prepared() {
    let _one = ONE_LOAD.lock().unwrap_or_else(|poison| poison.into_inner());
    let install = tempfile::tempdir().unwrap();
    let layout = InstallLayout::new(install.path());
    let dir = layout.plugins_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let file = format!("alpha_plugin.{}", std::env::consts::DLL_EXTENSION);
    std::fs::copy(fixture_library(), dir.join(&file)).unwrap();
    std::fs::write(layout.integrity_manifest_path(), r#"{"__allow_unlisted__":"true"}"#).unwrap();

    let prepared = PluginManager::prepare_plugins_in_dir_except(&dir, &[]);
    assert_eq!(prepared.len(), 1);
}
