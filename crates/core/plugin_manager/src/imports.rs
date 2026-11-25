//! Why the operating system would not load a plugin library, in words.
//!
//! A plugin library is not rejected for its API version when `LoadLibraryExW` fails: the
//! operating system resolves the library's imports first, before any code of it (or of the
//! engine's version gate) runs. A plugin imports the shared libraries (`helio_dylib.dll`,
//! `scenedb_dylib-<hash>.dll`, ...) by file name and its Rust symbols by mangled name, and both
//! change whenever the shared libraries are rebuilt from another dependency graph: the file
//! name of a library of a package outside the workspace carries a hash of that graph, and the
//! symbols carry the hash of their crate. None of that is the plugin API version. So a plugin
//! built yesterday does not load beside a shared library rebuilt today, and the OS says only
//! "LoadLibraryExW failed". [`explain`] reads the plugin's import table and says which library
//! it asked for that is not there, and what to run.

use std::path::Path;

/// The names of the libraries `pe` imports (a Portable Executable image), or `None` when it is
/// not one this reads.
pub fn imported_libraries(pe: &[u8]) -> Option<Vec<String>> {
    Some(import_slots(pe)?.into_iter().map(|(name, _)| name).collect())
}

/// The imported libraries and where each name is in the file.
pub fn import_slots(pe: &[u8]) -> Option<Vec<(String, usize)>> {
    fn u16_at(b: &[u8], at: usize) -> Option<usize> {
        Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
    }
    fn u32_at(b: &[u8], at: usize) -> Option<usize> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
    }
    if pe.get(..2)? != b"MZ" {
        return None;
    }
    let header = u32_at(pe, 0x3c)?;
    if pe.get(header..header + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = u16_at(pe, header + 6)?;
    let optional_size = u16_at(pe, header + 20)?;
    let optional = header + 24;
    // PE32+ keeps the data directories at 112 bytes into the optional header, PE32 at 96.
    let directories = match u16_at(pe, optional)? {
        0x20b => optional + 112,
        0x10b => optional + 96,
        _ => return None,
    };
    let import_rva = u32_at(pe, directories + 8)?;
    if import_rva == 0 {
        return Some(Vec::new());
    }
    let table = optional + optional_size;
    let rva_to_offset = |rva: usize| -> Option<usize> {
        (0..sections).find_map(|index| {
            let section = table + index * 40;
            let virtual_size = u32_at(pe, section + 8)?;
            let virtual_address = u32_at(pe, section + 12)?;
            let raw_size = u32_at(pe, section + 16)?;
            let raw_pointer = u32_at(pe, section + 20)?;
            let extent = virtual_size.max(raw_size);
            (rva >= virtual_address && rva < virtual_address + extent)
                .then(|| raw_pointer + (rva - virtual_address))
        })
    };
    let mut names = Vec::new();
    let mut descriptor = rva_to_offset(import_rva)?;
    // One 20-byte descriptor per library; a zeroed one ends the table.
    while u32_at(pe, descriptor + 12)? != 0 || u32_at(pe, descriptor + 16)? != 0 {
        let name_at = rva_to_offset(u32_at(pe, descriptor + 12)?)?;
        let name = pe.get(name_at..)?;
        let end = name.iter().position(|&byte| byte == 0)?;
        names.push((String::from_utf8_lossy(&name[..end]).into_owned(), name_at));
        descriptor += 20;
        if names.len() > 4096 {
            return None;
        }
    }
    Some(names)
}

/// Whether `name` is a library of the operating system (found by the loader without help).
fn is_system_library(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("api-ms-win-")
        || lower.starts_with("ext-ms-")
        || std::env::var_os("SystemRoot")
            .map(|root| Path::new(&root).join("System32").join(&lower).is_file())
            .unwrap_or(false)
}

/// What to tell the person when the library at `path` would not load: the libraries it imports
/// that are in none of the places the loader looks, and what to run.
pub fn explain(path: &Path, os_message: &str) -> String {
    let bytes = std::fs::read(path).ok();
    let mut missing = Vec::new();
    if let Some(imports) = bytes.as_deref().and_then(imported_libraries) {
        let mut places = vec![path.parent().map(Path::to_path_buf)];
        places.push(
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
        );
        for name in imports {
            let found = places
                .iter()
                .flatten()
                .any(|dir| dir.join(&name).is_file())
                || is_system_library(&name);
            if !found {
                missing.push(name);
            }
        }
    }
    let advice = "The plugin was built against other shared libraries than the ones installed with \
        this engine. Those are rebuilt whenever the dependency graph changes, not only when the \
        plugin API does, so plugins and engine must be built together: run `just plugins` (or \
        `cargo run -p plugin_builder --release`), or reinstall the plugins from the Hub.";
    if missing.is_empty() {
        format!("{os_message}. {advice}")
    } else {
        format!(
            "{os_message}. The plugin imports {} which is not installed beside the engine. {advice}",
            missing.join(", ")
        )
    }
}

// ── The fuzzy renamer ──────────────────────────────────────────────────────────────────────────────────
//
// A shared library of a package outside the workspace is built under a file name with a hash of its
// dependency graph (`scenedb_dylib-<16 hex>.dll`), and the plugins built against it import that name.
// The name is the only thing that changes when the graph does: the library's own symbols are the same,
// so a plugin built against yesterday's build runs against today's, but the OS finds no library of the
// old name. Rebuilding every plugin for a file name is the cost this removes: the engine knows which
// file it linked itself, so it loads a copy of the plugin whose import of the old name names that file.
// (Until the library has a stable file name, which is the fix for future builds.)

/// The stem of a hash-named library (`scenedb_dylib` of `scenedb_dylib-0123456789abcdef.dll`).
pub fn hashed_stem(name: &str) -> Option<&str> {
    let base = name.strip_suffix(".dll").or_else(|| name.strip_suffix(".DLL"))?;
    let (stem, hash) = base.rsplit_once('-')?;
    (hash.len() == 16 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) && !stem.is_empty())
        .then_some(stem)
}

/// The stem of a library file name, hashed or not (`scenedb_dylib` of `scenedb_dylib.dll` and of
/// `scenedb_dylib-0123456789abcdef.dll`), so a plugin built against a hash-named library meets the
/// stable name the library has now.
fn library_stem(name: &str) -> Option<&str> {
    hashed_stem(name).or_else(|| name.strip_suffix(".dll").or_else(|| name.strip_suffix(".DLL")))
}

/// One rename the renamer made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

/// `bytes` of a plugin library with each import of a hash-named library the engine does not have
/// renamed to the same library's name in `available` (the libraries the engine itself imports), and
/// what was renamed. `None` when nothing needed renaming or a name cannot be written in place.
pub fn rename_hashed_imports(bytes: &[u8], available: &[String]) -> Option<(Vec<u8>, Vec<Rename>)> {
    let slots = import_slots(bytes)?;
    let mut patched = bytes.to_vec();
    let mut renames = Vec::new();
    for (name, at) in slots {
        let Some(stem) = hashed_stem(&name) else { continue };
        if available.iter().any(|have| have.eq_ignore_ascii_case(&name)) {
            continue;
        }
        let Some(to) = available
            .iter()
            .find(|have| library_stem(have) == Some(stem))
        else {
            continue;
        };
        // The name is a NUL-terminated string in place; a shorter one fits, a longer one does not.
        if to.len() > name.len() {
            continue;
        }
        patched[at..at + name.len()].fill(0);
        patched[at..at + to.len()].copy_from_slice(to.as_bytes());
        renames.push(Rename {
            from: name,
            to: to.clone(),
        });
    }
    (!renames.is_empty()).then_some((patched, renames))
}

/// The libraries the running engine imports (the ones it linked, by file name).
fn host_imports() -> Vec<String> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::read(exe).ok())
        .and_then(|bytes| imported_libraries(&bytes))
        .unwrap_or_default()
}

/// A copy of a plugin library that imports the libraries this engine has.
pub struct CompatCopy {
    /// The copy, to load in place of the original.
    pub path: std::path::PathBuf,
    /// SHA-256 of the original, which is what the integrity manifest lists.
    pub original_sha256: [u8; 32],
    pub renames: Vec<Rename>,
}

/// On Windows, a copy of the plugin at `path` with its imports of hash-named libraries the engine does
/// not have renamed to the ones it does (kept under the user's cache, one folder per original), or
/// `None` when the plugin needs none, or on another platform.
#[cfg(target_os = "windows")]
pub fn compat_copy(path: &Path, cache_dir: &Path) -> Option<CompatCopy> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).ok()?;
    let (patched, renames) = rename_hashed_imports(&bytes, &host_imports())?;
    let original_sha256: [u8; 32] = Sha256::digest(&bytes).into();
    // One folder per original and per set of names, so a changed plugin or engine never meets a stale copy.
    let mut key = Sha256::new();
    key.update(original_sha256);
    for rename in &renames {
        key.update(rename.to.as_bytes());
    }
    let folder: String = key.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    let dir = cache_dir.join(folder);
    let target = dir.join(path.file_name()?);
    let fresh = std::fs::read(&target).map(|have| have == patched).unwrap_or(false);
    if !fresh {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&target, &patched).ok()?;
    }
    Some(CompatCopy {
        path: target,
        original_sha256,
        renames,
    })
}

#[cfg(not(target_os = "windows"))]
pub fn compat_copy(_path: &Path, _cache_dir: &Path) -> Option<CompatCopy> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_binary_imports_kernel32() {
        let exe = std::env::current_exe().unwrap();
        let bytes = std::fs::read(exe).unwrap();
        if cfg!(windows) {
            let imports = imported_libraries(&bytes).expect("a PE image");
            assert!(imports.iter().any(|n| n.eq_ignore_ascii_case("kernel32.dll")));
        } else {
            assert!(imported_libraries(&bytes).is_none());
        }
    }

    #[test]
    fn something_that_is_not_an_image_has_no_imports() {
        assert!(imported_libraries(b"not a library").is_none());
    }
}

#[cfg(test)]
mod renamer_tests {
    use super::*;

    #[test]
    fn a_hashed_name_has_a_stem() {
        assert_eq!(hashed_stem("scenedb_dylib-286559dd0edfd722.dll"), Some("scenedb_dylib"));
        assert_eq!(hashed_stem("helio_dylib.dll"), None);
        assert_eq!(hashed_stem("a-b-0123456789abcdef.dll"), Some("a-b"));
        assert_eq!(hashed_stem("x-0123456789abcdeg.dll"), None);
    }

    #[test]
    fn an_old_hashed_import_meets_the_stable_name() {
        assert_eq!(library_stem("scenedb_dylib.dll"), Some("scenedb_dylib"));
        assert_eq!(library_stem("scenedb_dylib-286559dd0edfd722.dll"), Some("scenedb_dylib"));
        assert_ne!(library_stem("helio_dylib.dll"), Some("scenedb_dylib"));
    }
}
