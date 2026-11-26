//! Plugins the executable links, and the libraries of them the loader leaves alone.
//!
//! An editor built with `link-plugins` links every installed plugin: each root
//! registers through its `build` (`pulsar_plugins::installed!`), exactly as a game
//! links it. A plugin library of the same plugin in `plugins/editor` (a build of
//! the default editor left it there) would register the plugin a second time, so
//! the loader skips it, keyed by the plugin's descriptor id:
//!
//! 1. **Before loading.** A library is a plugin's if the install's lock
//!    (`plugins.lock.json`, written by `tools/plugin_builder`: `local:<plugin id>`)
//!    says so, or if its file is named after a linked plugin's root crate
//!    (`<root>.<ext>`, the name every plugin library is installed under). The OS
//!    loader never sees it: no code of it runs, and nothing of it stays mapped.
//! 2. **After loading.** A library whose exported descriptors name a linked
//!    plugin (it is named otherwise, or the lock is missing) has those plugins
//!    dropped from the import ([`without_linked`]); a library with nothing left
//!    is not installed at all (`PluginManagerError::AlreadyLinked`).
//!
//! A library of a plugin that is not linked (hub-installed, third party) matches
//! neither and loads as always, so does the whole default editor, which links
//! `render_bps` only.

use std::path::{Path, PathBuf};

use pulsar_install_layout::PluginLock;
use pulsar_plugins::library::{FfiLibrary, FfiPlugin};
use pulsar_plugins::{InstalledPlugin, PluginDescriptor};

/// A plugin this executable links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedPlugin {
    /// The descriptor's id (`pulsar.beta`).
    pub id: String,
    /// The root crate's package name: what the plugin's library is named after.
    pub root: String,
}

/// The plugins this executable links, by id: every root whose crate is in the
/// binary (`pulsar_plugins::installed!`), whatever libraries are loaded later.
/// The default editor links `render_bps`; `link-plugins` links every installed
/// plugin.
pub fn linked_plugins() -> Vec<LinkedPlugin> {
    let mut linked: Vec<LinkedPlugin> = pulsar_plugins::inventory::iter::<InstalledPlugin>
        .into_iter()
        .filter_map(|plugin| match PluginDescriptor::parse(plugin.descriptor) {
            Ok(descriptor) => Some(LinkedPlugin {
                id: descriptor.id,
                root: plugin.root.to_owned(),
            }),
            Err(error) => {
                tracing::error!("{}: a linked plugin's descriptor does not parse: {error}", plugin.root);
                None
            }
        })
        .collect();
    linked.sort_by(|a, b| a.id.cmp(&b.id));
    linked.dedup_by(|a, b| a.id == b.id);
    linked
}

/// Whether the plugin `id` is linked into this executable.
pub fn is_linked(id: &str) -> bool {
    linked_plugins().iter().any(|plugin| plugin.id == id)
}

/// A library the loader left alone because its plugin is linked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedLibrary {
    pub path: PathBuf,
    /// The id of the linked plugin it is a library of.
    pub plugin_id: String,
}

/// `paths` split into the libraries to load and those of linked plugins.
/// `lock` is the install's plugin lock (which plugin built which file).
pub fn split_linked(
    paths: Vec<PathBuf>,
    lock: &PluginLock,
    linked: &[LinkedPlugin],
) -> (Vec<PathBuf>, Vec<SkippedLibrary>) {
    let mut load = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        match linked_plugin_of(&path, lock, linked) {
            Some(plugin) => skipped.push(SkippedLibrary {
                path,
                plugin_id: plugin.id.clone(),
            }),
            None => load.push(path),
        }
    }
    (load, skipped)
}

/// The linked plugin the library at `path` is a build of, if any.
fn linked_plugin_of<'a>(
    path: &Path,
    lock: &PluginLock,
    linked: &'a [LinkedPlugin],
) -> Option<&'a LinkedPlugin> {
    let file = path.file_name()?.to_str()?;
    // The lock: the builder records `local:<plugin id>` for each library it installs.
    let by_lock = lock
        .plugins
        .iter()
        .filter(|record| record.file == file)
        .find_map(|record| record.repo_url.strip_prefix("local:"))
        .and_then(|id| linked.iter().find(|plugin| plugin.id == id));
    by_lock.or_else(|| {
        // The name: a plugin's library is `<root crate>.<ext>` (`lib` prefixed on Unix).
        let stem = path.file_stem()?.to_str()?;
        let stem = stem.strip_prefix("lib").filter(|rest| linked.iter().any(|p| p.root == *rest)).unwrap_or(stem);
        linked.iter().find(|plugin| plugin.root == stem)
    })
}

/// The install's plugin lock for the library directory `dir` (`<install>/plugins/editor`):
/// empty when `dir` is not an install's plugin directory or has no lock.
pub(crate) fn lock_of(dir: &Path) -> PluginLock {
    if dir.file_name().is_some_and(|name| name == "editor")
        && dir.parent().and_then(Path::file_name).is_some_and(|name| name == "plugins")
    {
        if let Some(install) = dir.parent().and_then(Path::parent) {
            return pulsar_install_layout::InstallLayout::new(install).load_plugin_lock();
        }
    }
    PluginLock::default()
}

/// `library` without the plugins this executable links, and the ids dropped.
///
/// # Safety
///
/// `library` is what a plugin library's export wrote: its arrays live for the
/// process (the library is never unloaded).
pub(crate) unsafe fn without_linked(
    library: &FfiLibrary,
    linked: &[LinkedPlugin],
) -> Result<(FfiLibrary, Vec<String>), String> {
    if library.len == 0 {
        return Ok((*library, Vec::new()));
    }
    // SAFETY: the caller's contract.
    let plugins = unsafe { std::slice::from_raw_parts(library.plugins, library.len) };
    let mut kept: Vec<FfiPlugin> = Vec::with_capacity(plugins.len());
    let mut dropped = Vec::new();
    for plugin in plugins {
        // SAFETY: the descriptor is UTF-8 that lives for the process (the caller's contract).
        let source = unsafe {
            std::str::from_utf8(std::slice::from_raw_parts(
                plugin.descriptor.ptr,
                plugin.descriptor.len,
            ))
        }
        .map_err(|error| format!("a plugin descriptor is not UTF-8: {error}"))?;
        let descriptor = PluginDescriptor::parse(source)?;
        if linked.iter().any(|plugin| plugin.id == descriptor.id) {
            dropped.push(descriptor.id);
        } else {
            kept.push(*plugin);
        }
    }
    if dropped.is_empty() {
        return Ok((*library, dropped));
    }
    // Lives for the process, like the library's own arrays.
    let kept: &'static [FfiPlugin] = Box::leak(kept.into_boxed_slice());
    Ok((
        FfiLibrary {
            abi: library.abi,
            plugins: kept.as_ptr(),
            len: kept.len(),
        },
        dropped,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_install_layout::PluginRecord;

    fn linked() -> Vec<LinkedPlugin> {
        vec![
            LinkedPlugin { id: "pulsar.alpha".into(), root: "alpha_plugin".into() },
            LinkedPlugin { id: "pulsar.beta".into(), root: "beta_plugin".into() },
        ]
    }

    fn record(file: &str, repo_url: &str) -> PluginRecord {
        PluginRecord {
            name: file.into(),
            repo_url: repo_url.into(),
            version: "0.1.0".into(),
            file: file.into(),
            sha256: String::new(),
            registry_url: String::new(),
            installed_at: String::new(),
            method: "source".into(),
        }
    }

    #[test]
    fn a_library_of_a_linked_plugin_is_skipped_by_its_id() {
        // A file whose name says nothing, recorded by the builder as the plugin's.
        let lock = PluginLock { plugins: vec![record("renamed.dll", "local:pulsar.alpha")] };
        let paths = vec![PathBuf::from("plugins/editor/renamed.dll")];
        let (load, skipped) = split_linked(paths, &lock, &linked());
        assert!(load.is_empty());
        assert_eq!(skipped[0].plugin_id, "pulsar.alpha");
    }

    #[test]
    fn a_library_named_after_a_linked_root_is_skipped_without_a_lock() {
        let paths = vec![
            PathBuf::from("plugins/editor/beta_plugin.dll"),
            PathBuf::from("plugins/editor/libbeta_plugin.so"),
        ];
        let (load, skipped) = split_linked(paths, &PluginLock::default(), &linked());
        assert!(load.is_empty());
        assert_eq!(skipped.len(), 2);
        assert!(skipped.iter().all(|library| library.plugin_id == "pulsar.beta"));
    }

    #[test]
    fn a_library_that_is_only_a_library_still_loads() {
        // Hub-installed: its record names a repository, and its file is its own.
        let lock = PluginLock {
            plugins: vec![record("thirdparty.dll", "https://example.com/thirdparty")],
        };
        let paths = vec![
            PathBuf::from("plugins/editor/thirdparty.dll"),
            PathBuf::from("plugins/editor/alpha_plugin.dll"),
            PathBuf::from("plugins/editor/other_plugin.dll"),
        ];
        let (load, skipped) = split_linked(paths, &lock, &linked());
        assert_eq!(
            load,
            vec![
                PathBuf::from("plugins/editor/thirdparty.dll"),
                PathBuf::from("plugins/editor/other_plugin.dll"),
            ]
        );
        assert_eq!(skipped.len(), 1);
    }

    #[test]
    fn a_lock_record_of_an_unlinked_plugin_does_not_skip_a_library_named_like_one() {
        // `local:pulsar.gamma` is not linked; its library is named like a linked root only by chance
        // of the name rule, so the name rule decides: the lock never makes a library of a linked
        // plugin load, and never skips a library of one that is not linked.
        let lock = PluginLock { plugins: vec![record("x.dll", "local:pulsar.gamma")] };
        let (load, skipped) =
            split_linked(vec![PathBuf::from("plugins/editor/x.dll")], &lock, &linked());
        assert_eq!(load.len(), 1);
        assert!(skipped.is_empty());
    }

    #[test]
    fn nothing_is_skipped_when_nothing_is_linked() {
        let paths = vec![PathBuf::from("plugins/editor/alpha_plugin.dll")];
        let (load, skipped) = split_linked(paths.clone(), &PluginLock::default(), &[]);
        assert_eq!(load, paths);
        assert!(skipped.is_empty());
    }

    /// A library's export of plugins with these descriptors, as `export_library!` writes it.
    fn export(ids: &[&'static str]) -> (Vec<FfiPlugin>, FfiLibrary) {
        use pulsar_plugins::library::FfiStr;
        let plugins: Vec<FfiPlugin> = ids
            .iter()
            .map(|source| FfiPlugin {
                descriptor: FfiStr { ptr: source.as_ptr(), len: source.len() },
                root: FfiStr { ptr: "r".as_ptr(), len: 1 },
                entries: std::ptr::null(),
                len: 0,
            })
            .collect();
        let library = FfiLibrary { abi: 1, plugins: plugins.as_ptr(), len: plugins.len() };
        (plugins, library)
    }

    const ALPHA: &str = "id = \"pulsar.alpha\"\nname = \"Alpha\"\nroot = \"alpha_plugin\"\n";
    const THIRD: &str = "id = \"vendor.third\"\nname = \"Third\"\nroot = \"third_plugin\"\n";

    #[test]
    fn a_linked_plugin_is_dropped_from_a_library_by_its_descriptor_id() {
        // Whatever the library is named: the descriptor it exports decides.
        let (_plugins, library) = export(&[ALPHA, THIRD]);
        // SAFETY: the arrays live as long as `_plugins`, and the descriptors are statics.
        let (filtered, dropped) = unsafe { without_linked(&library, &linked()) }.unwrap();
        assert_eq!(dropped, vec!["pulsar.alpha".to_string()]);
        assert_eq!(filtered.len, 1);
        // SAFETY: as above.
        let kept = unsafe { std::slice::from_raw_parts(filtered.plugins, filtered.len) };
        // SAFETY: as above.
        let descriptor = unsafe { std::slice::from_raw_parts(kept[0].descriptor.ptr, kept[0].descriptor.len) };
        assert_eq!(descriptor, THIRD.as_bytes());
    }

    #[test]
    fn a_library_of_plugins_nothing_links_is_imported_whole() {
        let (_plugins, library) = export(&[THIRD]);
        // SAFETY: as above.
        let (filtered, dropped) = unsafe { without_linked(&library, &linked()) }.unwrap();
        assert!(dropped.is_empty());
        assert_eq!(filtered.len, 1);
        assert_eq!(filtered.plugins, library.plugins);
    }

    #[test]
    fn the_lock_is_read_only_beside_an_install() {
        assert!(lock_of(Path::new("somewhere/else")).plugins.is_empty());
    }
}
