//! The module store: where a class's compiled script module lives.
//!
//! Compiling is a build step, so its output goes to the project's build
//! output, never beside the source (a class in a read-only folder, such as a
//! plugin's examples, compiles like any other):
//!
//! ```text
//! <project>/target/pulsar/scripts/<class id>/
//!   module.json     the compiled module (engine script module, JSON)
//!   language        the id of the language that wrote it
//!   source.json     which source it was compiled from (`ModuleSource`)
//! ```
//!
//! A module is keyed by its class's id, the class's stable identity
//! (`class.json`), so a renamed or moved class keeps its module, and
//! `source.json` says where the source is and the hash of what was compiled:
//! a module whose source changed since is stale ([`ModuleStore::is_current`]).
//!
//! A packaged game has no store: the cook writes each module into content as
//! `Pulsar/modules/<class id>.pvm` ([`cooked_module_path`]), and
//! [`module_file`] finds either.
//!
//! Projects from before the store kept modules in `<class>/events/.build/`;
//! [`migrate_legacy_modules`] moves them in once.

use std::path::{Path, PathBuf};

use engine_fs::virtual_fs;
use serde::{Deserialize, Serialize};

use crate::id::ClassId;
use crate::registry::ClassEntry;

/// The store's folder, project-relative.
pub const STORE_DIR: &str = "target/pulsar/scripts";
/// A compiled module in the editor's JSON encoding.
pub const MODULE_JSON_FILE: &str = "module.json";
/// A compiled module in the binary encoding packaged games ship.
pub const MODULE_BINARY_FILE: &str = "module.pvm";
/// The language that wrote a module.
pub const LANGUAGE_FILE: &str = "language";
/// Which source a module was compiled from.
pub const SOURCE_FILE: &str = "source.json";
/// Packaged content's folder of cooked modules, content-relative.
pub const COOKED_MODULES_DIR: &str = "Pulsar/modules";

/// Which source a stored module was compiled from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleSource {
    pub class_id: String,
    pub class_name: String,
    /// The class folder, project-relative, `/`-separated.
    pub source_dir: String,
    pub language: String,
    /// XXH3-128 (hex) of the source files the language compiled, in the
    /// language's order ([`source_hash`]).
    pub source_hash: String,
}

/// Hash of a class's source files, for [`ModuleSource::source_hash`]. A
/// missing file hashes as empty, so adding it changes the hash.
pub fn source_hash(files: &[PathBuf]) -> String {
    let mut hasher = twox_hash::XxHash3_128::new();
    for file in files {
        let bytes = virtual_fs::read_file(file).unwrap_or_default();
        hasher.write(&(bytes.len() as u64).to_le_bytes());
        hasher.write(&bytes);
    }
    format!("{:032x}", hasher.finish_128())
}

/// A project's module store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleStore {
    project_root: PathBuf,
    root: PathBuf,
}

impl ModuleStore {
    pub fn of_project(project_root: &Path) -> Self {
        Self {
            project_root: project_root.to_path_buf(),
            root: STORE_DIR
                .split('/')
                .fold(project_root.to_path_buf(), |p, part| p.join(part)),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of class `id`'s module.
    pub fn dir(&self, id: &ClassId) -> PathBuf {
        self.root.join(folder_name(id))
    }

    /// Where class `id`'s compiled module is (or would be).
    pub fn module_path(&self, id: &ClassId) -> PathBuf {
        self.dir(id).join(MODULE_JSON_FILE)
    }

    pub fn has_module(&self, id: &ClassId) -> bool {
        virtual_fs::is_file(&self.module_path(id))
    }

    /// The language that wrote class `id`'s module, if it has one.
    pub fn language(&self, id: &ClassId) -> Option<String> {
        virtual_fs::read_to_string(&self.dir(id).join(LANGUAGE_FILE))
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    }

    /// Which source class `id`'s module was compiled from.
    pub fn source(&self, id: &ClassId) -> Option<ModuleSource> {
        let text = virtual_fs::read_to_string(&self.dir(id).join(SOURCE_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Whether class `id` has a module compiled from source whose hash is
    /// `hash`.
    pub fn is_current(&self, id: &ClassId, hash: &str) -> bool {
        self.has_module(id) && self.source(id).is_some_and(|s| s.source_hash == hash)
    }

    /// Store `module_json` as `entry`'s module, written by `language` from
    /// source whose hash is `source_hash`. The module is written last, so a
    /// reader never sees a module with another source's record.
    pub fn write(
        &self,
        entry: &ClassEntry,
        language: &str,
        source_hash: String,
        module_json: &[u8],
    ) -> Result<PathBuf, String> {
        let dir = self.dir(&entry.id);
        virtual_fs::create_dir_all(&dir).map_err(|e| format!("{}: {e:#}", dir.display()))?;
        let source = ModuleSource {
            class_id: entry.id.as_str().to_owned(),
            class_name: entry.name.clone(),
            source_dir: self.relative(&entry.dir),
            language: language.to_owned(),
            source_hash,
        };
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.join(name);
            virtual_fs::write_file_atomically(&path, bytes)
                .map_err(|e| format!("{}: {e:#}", path.display()))
        };
        write(LANGUAGE_FILE, language.as_bytes())?;
        write(
            SOURCE_FILE,
            serde_json::to_string_pretty(&source)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )?;
        write(MODULE_JSON_FILE, module_json)?;
        Ok(self.module_path(&entry.id))
    }

    /// Remove class `id`'s module (its source failed to compile: no module
    /// from an older source is left to run).
    pub fn remove(&self, id: &ClassId) {
        let dir = self.dir(id);
        if virtual_fs::is_dir(&dir) {
            if let Err(error) = virtual_fs::delete_path(&dir) {
                tracing::warn!(dir = %dir.display(), "Could not remove a stale module: {error:#}");
            }
        }
    }

    fn relative(&self, dir: &Path) -> String {
        let rel = dir.strip_prefix(&self.project_root).unwrap_or(dir);
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl ModuleStore {
    /// The class whose folder is `dir`, and its project's store: what a
    /// language compiling that folder writes to. `None` when `dir` is in no
    /// project.
    pub fn for_class_dir(dir: &Path) -> Option<(ClassEntry, Self)> {
        let root = crate::registry::project_root_of_class_dir(dir)?;
        let entry = ClassEntry {
            id: crate::id::ensure_class_id(dir),
            name: crate::registry::class_name_of_dir(dir),
            dir: dir.to_path_buf(),
        };
        Some((entry, Self::of_project(&root)))
    }
}

/// A class id as a folder name. Ids are UUIDs; anything else (an id from
/// before, with characters a file system refuses) is hashed.
fn folder_name(id: &ClassId) -> String {
    let text = id.as_str();
    let safe = !text.is_empty()
        && text.len() <= 64
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if safe {
        text.to_owned()
    } else {
        format!(
            "id-{:032x}",
            twox_hash::XxHash3_128::oneshot(text.as_bytes())
        )
    }
}

/// Where the class whose folder is `dir` has its compiled module in its
/// project's store (which may not hold one). `None` when `dir` is in no
/// project.
pub fn class_module_path(dir: &Path) -> Option<PathBuf> {
    ModuleStore::for_class_dir(dir).map(|(entry, store)| store.module_path(&entry.id))
}

/// Where the cook puts class `id`'s module in packaged content,
/// content-relative.
pub fn cooked_module_path(id: &ClassId) -> String {
    format!("{COOKED_MODULES_DIR}/{}.{}", folder_name(id), "pvm")
}

/// Where `entry`'s compiled module is for content rooted at `content_root`:
/// the cooked one when the content has it (a packaged game), else the
/// project's store. Either may be missing. Checked through the virtual
/// filesystem, so a pak counts.
pub fn module_file(content_root: &Path, entry: &ClassEntry) -> PathBuf {
    let cooked = cooked_module_path(&entry.id)
        .split('/')
        .fold(content_root.to_path_buf(), |p, part| p.join(part));
    if virtual_fs::is_file(&cooked) {
        cooked
    } else {
        ModuleStore::of_project(content_root).module_path(&entry.id)
    }
}

/// Move modules of `registry`'s classes from their pre-store place
/// (`<class>/events/.build/`) into the project's store, and remove the old
/// folder. A class whose folder is read-only keeps it (nothing can be
/// removed), and its old module is not used. Returns how many moved.
pub fn migrate_legacy_modules(project_root: &Path, registry: &crate::ClassRegistry) -> usize {
    let store = ModuleStore::of_project(project_root);
    let mut moved = 0;
    for entry in registry.entries() {
        let legacy = entry.dir.join("events").join(".build");
        if !virtual_fs::is_dir(&legacy) {
            continue;
        }
        // A folder with only an older VM's output (`bytecode.json`) has
        // nothing to move; it is removed below all the same.
        let module = legacy.join(MODULE_JSON_FILE);
        if virtual_fs::is_file(&module) && !store.has_module(&entry.id) {
            let Ok(bytes) = virtual_fs::read_file(&module) else {
                continue;
            };
            // The language file says who wrote it; a module from before it was
            // written belongs to the one language whose marker the class has.
            let language = virtual_fs::read_to_string(&legacy.join(LANGUAGE_FILE))
                .ok()
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .or_else(|| match crate::language::language_of(&entry.dir) {
                    Ok(Some(language)) => Some(language.id.to_owned()),
                    _ => None,
                });
            let Some(language) = language else {
                // No language owns it (its plugin is disabled, or the class is
                // two languages'): the old folder stays until one does.
                tracing::debug!(class = %entry.name, "Old module kept: no language to attribute it to");
                continue;
            };
            // Unknown source: an empty hash, so the next build compiles it.
            if let Err(error) = store.write(entry, &language, String::new(), &bytes) {
                tracing::warn!(class = %entry.name, "Module not moved to the store: {error}");
                continue;
            }
            moved += 1;
        }
        if let Err(error) = virtual_fs::delete_path(&legacy) {
            tracing::debug!(class = %entry.name, "Old module folder kept: {error:#}");
        }
    }
    if moved > 0 {
        tracing::info!(moved, store = %store.root().display(), "Moved compiled modules into the module store");
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(root: &Path, name: &str, id: &str) -> ClassEntry {
        ClassEntry {
            id: ClassId::from(id),
            name: name.into(),
            dir: root.join(format!("{name}.class")),
        }
    }

    #[test]
    fn modules_are_stored_by_class_id_with_their_source() {
        let project = tempfile::tempdir().unwrap();
        let store = ModuleStore::of_project(project.path());
        let door = entry(
            project.path(),
            "Door",
            "0b9d6c3e-1d7a-4c2e-9a51-3f0c8d2b7e10",
        );
        std::fs::create_dir_all(&door.dir).unwrap();
        std::fs::write(door.dir.join("graph_save.json"), "{}").unwrap();
        let hash = source_hash(&[door.dir.join("graph_save.json")]);

        let path = store
            .write(&door, "blueprint", hash.clone(), b"{\"m\":1}")
            .unwrap();
        assert_eq!(
            path,
            project
                .path()
                .join("target/pulsar/scripts/0b9d6c3e-1d7a-4c2e-9a51-3f0c8d2b7e10/module.json")
        );
        assert_eq!(store.language(&door.id).as_deref(), Some("blueprint"));
        let source = store.source(&door.id).unwrap();
        assert_eq!(source.source_dir, "Door.class");
        assert_eq!(source.class_name, "Door");
        assert!(store.is_current(&door.id, &hash));

        std::fs::write(door.dir.join("graph_save.json"), "{\"changed\":1}").unwrap();
        let changed = source_hash(&[door.dir.join("graph_save.json")]);
        assert!(
            !store.is_current(&door.id, &changed),
            "a changed source makes it stale"
        );

        store.remove(&door.id);
        assert!(!store.has_module(&door.id));
    }

    #[test]
    fn ids_a_file_system_refuses_get_a_safe_folder() {
        assert_eq!(folder_name(&ClassId::from("abc-123")), "abc-123");
        let odd = folder_name(&ClassId::from("name:HelloWorld"));
        assert!(odd.starts_with("id-") && !odd.contains(':'));
        assert_eq!(
            cooked_module_path(&ClassId::from("abc")),
            "Pulsar/modules/abc.pvm"
        );
    }

    #[test]
    fn the_cooked_module_is_found_before_the_store() {
        let root = tempfile::tempdir().unwrap();
        let door = entry(root.path(), "Door", "abc");
        assert_eq!(
            module_file(root.path(), &door),
            root.path().join("target/pulsar/scripts/abc/module.json")
        );
        let cooked = root.path().join("Pulsar/modules/abc.pvm");
        std::fs::create_dir_all(cooked.parent().unwrap()).unwrap();
        std::fs::write(&cooked, b"pvm").unwrap();
        assert_eq!(module_file(root.path(), &door), cooked);
    }

    #[test]
    fn legacy_modules_move_into_the_store() {
        let project = tempfile::tempdir().unwrap();
        let door = entry(project.path(), "Door", "abc");
        let legacy = door.dir.join("events/.build");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(MODULE_JSON_FILE), b"{}").unwrap();
        std::fs::write(legacy.join(LANGUAGE_FILE), b"typescript").unwrap();
        let registry = crate::ClassRegistry::from_entries(vec![door.clone()]);

        assert_eq!(migrate_legacy_modules(project.path(), &registry), 1);
        let store = ModuleStore::of_project(project.path());
        assert!(store.has_module(&door.id));
        assert_eq!(store.language(&door.id).as_deref(), Some("typescript"));
        assert!(!legacy.exists());
        let real = source_hash(&[door.dir.join("graph_save.json")]);
        assert!(
            !store.is_current(&door.id, &real),
            "a moved module's source is unknown: the next build compiles it"
        );
        assert_eq!(migrate_legacy_modules(project.path(), &registry), 0);
    }
}
