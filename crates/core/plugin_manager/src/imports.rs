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

/// The parts of a Portable Executable image this reads: where its sections are and its data
/// directories.
struct Image<'a> {
    bytes: &'a [u8],
    sections: usize,
    table: usize,
    directories: usize,
    wide: bool,
}

impl<'a> Image<'a> {
    fn u16_at(&self, at: usize) -> Option<usize> {
        Some(u16::from_le_bytes(self.bytes.get(at..at + 2)?.try_into().ok()?) as usize)
    }

    fn u32_at(&self, at: usize) -> Option<usize> {
        Some(u32::from_le_bytes(self.bytes.get(at..at + 4)?.try_into().ok()?) as usize)
    }

    fn parse(bytes: &'a [u8]) -> Option<Self> {
        if bytes.get(..2)? != b"MZ" {
            return None;
        }
        let mut image = Image { bytes, sections: 0, table: 0, directories: 0, wide: true };
        let header = image.u32_at(0x3c)?;
        if bytes.get(header..header + 4)? != b"PE\0\0" {
            return None;
        }
        image.sections = image.u16_at(header + 6)?;
        let optional_size = image.u16_at(header + 20)?;
        let optional = header + 24;
        // PE32+ keeps the data directories at 112 bytes into the optional header, PE32 at 96.
        match image.u16_at(optional)? {
            0x20b => image.directories = optional + 112,
            0x10b => {
                image.directories = optional + 96;
                image.wide = false;
            }
            _ => return None,
        }
        image.table = optional + optional_size;
        Some(image)
    }

    /// The RVA and size of data directory `index` (0 exports, 1 imports).
    fn directory(&self, index: usize) -> Option<(usize, usize)> {
        Some((self.u32_at(self.directories + index * 8)?, self.u32_at(self.directories + index * 8 + 4)?))
    }

    fn offset(&self, rva: usize) -> Option<usize> {
        (0..self.sections).find_map(|index| {
            let section = self.table + index * 40;
            let virtual_size = self.u32_at(section + 8)?;
            let virtual_address = self.u32_at(section + 12)?;
            let raw_size = self.u32_at(section + 16)?;
            let raw_pointer = self.u32_at(section + 20)?;
            let extent = virtual_size.max(raw_size);
            (rva >= virtual_address && rva < virtual_address + extent)
                .then(|| raw_pointer + (rva - virtual_address))
        })
    }

    /// The NUL-terminated string at `offset` and where it is.
    fn string(&self, offset: usize) -> Option<String> {
        let name = self.bytes.get(offset..)?;
        let end = name.iter().position(|&byte| byte == 0)?;
        Some(String::from_utf8_lossy(&name[..end]).into_owned())
    }

    /// The offsets of the import descriptors (20 bytes each; a zeroed one ends the table).
    fn descriptors(&self) -> Option<Vec<usize>> {
        let (import_rva, _) = self.directory(1)?;
        if import_rva == 0 {
            return Some(Vec::new());
        }
        let mut at = self.offset(import_rva)?;
        let mut out = Vec::new();
        while self.u32_at(at + 12)? != 0 || self.u32_at(at + 16)? != 0 {
            out.push(at);
            at += 20;
            if out.len() > 4096 {
                return None;
            }
        }
        Some(out)
    }
}

/// The imported libraries and where each name is in the file.
pub fn import_slots(pe: &[u8]) -> Option<Vec<(String, usize)>> {
    let image = Image::parse(pe)?;
    image
        .descriptors()?
        .into_iter()
        .map(|descriptor| {
            let name_at = image.offset(image.u32_at(descriptor + 12)?)?;
            Some((image.string(name_at)?, name_at))
        })
        .collect()
}

/// A library `pe` imports and the symbols it imports from it by name (an import by ordinal has no
/// name to compare and is left out).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedLibrary {
    pub name: String,
    pub symbols: Vec<String>,
}

/// The libraries `pe` imports with the symbols it imports from each by name.
pub fn imported_symbols(pe: &[u8]) -> Option<Vec<ImportedLibrary>> {
    let image = Image::parse(pe)?;
    let mut libraries = Vec::new();
    for descriptor in image.descriptors()? {
        let name = image.string(image.offset(image.u32_at(descriptor + 12)?)?)?;
        // The lookup table (the import names table); an image bound without one keeps the names in
        // the address table.
        let lookup = match image.u32_at(descriptor)? {
            0 => image.u32_at(descriptor + 16)?,
            rva => rva,
        };
        let mut at = image.offset(lookup)?;
        let width = if image.wide { 8 } else { 4 };
        let mut symbols = Vec::new();
        loop {
            let entry = if image.wide {
                u64::from_le_bytes(pe.get(at..at + 8)?.try_into().ok()?)
            } else {
                u64::from(u32::from_le_bytes(pe.get(at..at + 4)?.try_into().ok()?))
            };
            if entry == 0 {
                break;
            }
            let by_ordinal = entry >> (width * 8 - 1) & 1 == 1;
            if !by_ordinal {
                // A hint (two bytes) and then the name.
                let hint_name = image.offset((entry & 0x7fff_ffff) as usize)?;
                symbols.push(image.string(hint_name + 2)?);
            }
            at += width;
            if symbols.len() > 1_000_000 {
                return None;
            }
        }
        libraries.push(ImportedLibrary { name, symbols });
    }
    Some(libraries)
}

/// The names `pe` exports (a library's export directory), or `None` when it is not an image this
/// reads. An image that exports nothing has an empty set.
pub fn exported_names(pe: &[u8]) -> Option<std::collections::HashSet<String>> {
    let image = Image::parse(pe)?;
    let (rva, _) = image.directory(0)?;
    if rva == 0 {
        return Some(Default::default());
    }
    let directory = image.offset(rva)?;
    let count = image.u32_at(directory + 24)?;
    let names = image.offset(image.u32_at(directory + 32)?)?;
    let mut out = std::collections::HashSet::with_capacity(count);
    for index in 0..count {
        let name_rva = image.u32_at(names + index * 4)?;
        out.insert(image.string(image.offset(name_rva)?)?);
    }
    Some(out)
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

/// A library a plugin imports symbols from that the library no longer exports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingSymbols {
    /// The library's file name as the plugin imports it.
    pub library: String,
    /// How many of the plugin's imports from it it does not export, and the first few.
    pub total: usize,
    pub first: Vec<String>,
}

/// What is wrong with the imports of the plugin image `bytes`, given the directories the loader
/// finds its libraries in: the libraries that are in none of them, and for those that are, the
/// symbols the plugin imports by name that the library does not export.
pub fn diagnose(bytes: &[u8], places: &[std::path::PathBuf]) -> (Vec<String>, Vec<MissingSymbols>) {
    let (mut absent, mut stale) = (Vec::new(), Vec::new());
    let Some(libraries) = imported_symbols(bytes) else {
        return (absent, stale);
    };
    for library in libraries {
        let found = places.iter().map(|dir| dir.join(&library.name)).find(|file| file.is_file());
        let Some(file) = found else {
            if !is_system_library(&library.name) {
                absent.push(library.name);
            }
            continue;
        };
        if is_system_library(&library.name) || library.symbols.is_empty() {
            continue;
        }
        let Some(exports) = std::fs::read(&file).ok().and_then(|b| exported_names(&b)) else {
            continue;
        };
        let missing: Vec<&String> = library.symbols.iter().filter(|s| !exports.contains(*s)).collect();
        if !missing.is_empty() {
            stale.push(MissingSymbols {
                library: library.name.clone(),
                total: missing.len(),
                first: missing.iter().take(3).map(|s| (*s).clone()).collect(),
            });
        }
    }
    (absent, stale)
}

/// What to tell the person when the library at `path` would not load: the libraries it imports
/// that are in none of the places the loader looks, the symbols it imports that a library there
/// does not export, and what to run.
pub fn explain(path: &Path, os_message: &str) -> String {
    explain_load(path, path, os_message)
}

/// [`explain`] for a plugin `original` that was loaded from `loaded` (a renamed copy, see
/// [`compat_copy`], or the plugin itself).
pub fn explain_load(original: &Path, loaded: &Path, os_message: &str) -> String {
    let mut places: Vec<std::path::PathBuf> = Vec::new();
    places.extend(original.parent().map(Path::to_path_buf));
    places.extend(std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)));
    let bytes = std::fs::read(loaded).ok();
    let (absent, stale) = bytes.as_deref().map(|b| diagnose(b, &places)).unwrap_or_default();
    explain_with(original, loaded, os_message, &absent, &stale)
}

fn explain_with(
    original: &Path,
    loaded: &Path,
    os_message: &str,
    absent: &[String],
    stale: &[MissingSymbols],
) -> String {
    let plugin = original.file_name().map_or_else(|| original.display().to_string(), |n| n.to_string_lossy().into_owned());
    let advice = "The plugin was built against other shared libraries than the ones installed with this engine. Those are rebuilt whenever their code or dependency graph changes, not only when the plugin API does, so plugins and engine must be built together: run `just plugins` (or `cargo run -p plugin_builder -- --release --out <repository root>`: a bare `cargo run -p plugin_builder --release` builds only the tool, which then builds the plugins in debug), then start the editor from the repository root as target/release/pulsar_engine.exe (a plain `cargo run` rebuilds it with another feature set that the plugin libraries no longer match), or reinstall the plugins from the Hub.";
    let mut out = format!("{os_message}.");
    for library in stale {
        out.push_str(&format!(
            " Plugin {plugin} imports symbol {} from {} which that library no longer exports{}.",
            library.first[0],
            library.library,
            match library.total {
                1 => String::new(),
                n => format!(" (and {} more: {})", n - 1, library.first[1..].join(", ")),
            }
        ));
    }
    if !absent.is_empty() {
        out.push_str(&format!(" The plugin imports {} which is not installed beside the engine.", absent.join(", ")));
    }
    if loaded != original {
        out.push_str(&format!(
            " It was loaded from a renamed copy ({}) made for this engine's libraries; a copy is remade when the plugin, the engine or any shared library it imports changes.",
            loaded.display()
        ));
    }
    out.push(' ');
    out.push_str(advice);
    out
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

/// A digest of the engine's own imported-libraries set and of each shared library it imports that
/// is installed beside it (file name, size, modification time): what a renamed copy of a plugin
/// depends on besides the plugin.
pub fn host_fingerprint() -> [u8; 32] {
    let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
    libraries_fingerprint(exe_dir.as_deref(), &host_imports())
}

/// [`host_fingerprint`] of the libraries `names` in `dir`.
pub fn libraries_fingerprint(dir: Option<&Path>, names: &[String]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut names = names.to_vec();
    names.sort();
    for name in names {
        digest.update(name.as_bytes());
        let stamp = dir.and_then(|dir| std::fs::metadata(dir.join(&name)).ok());
        if let Some(meta) = stamp {
            digest.update(meta.len().to_le_bytes());
            let mtime = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            digest.update(mtime.to_le_bytes());
        }
    }
    digest.finalize().into()
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
    // One folder per original, per set of names and per state of the engine's libraries, so a changed
    // plugin, a rebuilt engine or a rebuilt shared library never meets a stale copy.
    let mut key = Sha256::new();
    key.update(original_sha256);
    for rename in &renames {
        key.update(rename.to.as_bytes());
    }
    key.update(host_fingerprint());
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

#[cfg(test)]
mod symbol_tests {
    use super::*;

    const RVA: usize = 0x1000;
    const RAW: usize = 0x200;

    /// A minimal PE32+ image with one section holding an export directory of `exports` and an
    /// import table of `imports` (a library and the names imported from it).
    fn fake_pe(exports: &[&str], imports: &[(&str, &[&str])]) -> Vec<u8> {
        let mut d: Vec<u8> = Vec::new();
        let rva = |d: &Vec<u8>| (RVA + d.len()) as u32;
        let cstr = |d: &mut Vec<u8>, s: &str| -> u32 {
            let at = rva(d);
            d.extend_from_slice(s.as_bytes());
            d.push(0);
            at
        };
        let (mut export_rva, mut import_rva) = (0u32, 0u32);
        if !exports.is_empty() {
            let names: Vec<u32> = exports.iter().map(|n| cstr(&mut d, n)).collect();
            let array = rva(&d);
            for n in &names {
                d.extend_from_slice(&n.to_le_bytes());
            }
            export_rva = rva(&d);
            let mut dir = [0u8; 40];
            dir[24..28].copy_from_slice(&(names.len() as u32).to_le_bytes());
            dir[32..36].copy_from_slice(&array.to_le_bytes());
            d.extend_from_slice(&dir);
        }
        if !imports.is_empty() {
            let mut descriptors = Vec::new();
            for (library, symbols) in imports {
                let name = cstr(&mut d, library);
                let entries: Vec<u32> = symbols
                    .iter()
                    .map(|s| {
                        let at = rva(&d);
                        d.extend_from_slice(&[0, 0]);
                        cstr(&mut d, s);
                        at
                    })
                    .collect();
                let thunks = rva(&d);
                for e in &entries {
                    d.extend_from_slice(&u64::from(*e).to_le_bytes());
                }
                d.extend_from_slice(&0u64.to_le_bytes());
                descriptors.push((name, thunks));
            }
            import_rva = rva(&d);
            for (name, thunks) in &descriptors {
                let mut desc = [0u8; 20];
                desc[0..4].copy_from_slice(&thunks.to_le_bytes());
                desc[12..16].copy_from_slice(&name.to_le_bytes());
                desc[16..20].copy_from_slice(&thunks.to_le_bytes());
                d.extend_from_slice(&desc);
            }
            d.extend_from_slice(&[0u8; 20]);
        }
        let mut pe = vec![0u8; RAW];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        pe[0x40..0x44].copy_from_slice(b"PE\0\0");
        pe[0x46..0x48].copy_from_slice(&1u16.to_le_bytes()); // sections
        pe[0x54..0x56].copy_from_slice(&240u16.to_le_bytes()); // optional header size
        pe[0x58..0x5a].copy_from_slice(&0x20bu16.to_le_bytes());
        let dirs = 0x58 + 112;
        pe[dirs..dirs + 4].copy_from_slice(&export_rva.to_le_bytes());
        pe[dirs + 8..dirs + 12].copy_from_slice(&import_rva.to_le_bytes());
        let section = 0x58 + 240;
        pe[section + 8..section + 12].copy_from_slice(&(d.len() as u32).to_le_bytes());
        pe[section + 12..section + 16].copy_from_slice(&(RVA as u32).to_le_bytes());
        pe[section + 16..section + 20].copy_from_slice(&(d.len() as u32).to_le_bytes());
        pe[section + 20..section + 24].copy_from_slice(&(RAW as u32).to_le_bytes());
        pe.extend_from_slice(&d);
        pe
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pulsar_imports_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn imports_are_read_by_symbol_and_exports_by_name() {
        let plugin = fake_pe(&[], &[("helio_dylib.dll", &["alpha", "beta"]), ("kernel32.dll", &["Sleep"])]);
        let libs = imported_symbols(&plugin).unwrap();
        assert_eq!(libs.len(), 2);
        assert_eq!(libs[0].name, "helio_dylib.dll");
        assert_eq!(libs[0].symbols, ["alpha", "beta"]);
        let library = fake_pe(&["alpha", "gamma"], &[]);
        let exports = exported_names(&library).unwrap();
        assert!(exports.contains("alpha") && exports.contains("gamma") && !exports.contains("beta"));
        assert!(exported_names(&fake_pe(&[], &[])).unwrap().is_empty());
        assert!(exported_names(b"not an image").is_none());
    }

    #[test]
    fn a_symbol_the_installed_library_no_longer_exports_is_named() {
        let dir = scratch("missing");
        std::fs::write(dir.join("helio_dylib.dll"), fake_pe(&["alpha"], &[])).unwrap();
        let plugin = fake_pe(&[], &[("helio_dylib.dll", &["alpha", "beta", "gamma"]), ("gone_dylib.dll", &["x"])]);
        let (absent, stale) = diagnose(&plugin, &[dir.clone()]);
        assert_eq!(absent, ["gone_dylib.dll"]);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].total, 2);
        assert_eq!(stale[0].first, ["beta", "gamma"]);
        let text = explain_with(
            Path::new("plugins/editor/sky_editor.dll"),
            Path::new("plugins/editor/sky_editor.dll"),
            "LoadLibraryExW failed",
            &absent,
            &stale,
        );
        assert!(
            text.contains("sky_editor.dll imports symbol beta from helio_dylib.dll which that library no longer exports"),
            "{text}"
        );
        assert!(text.contains("gone_dylib.dll"), "{text}");
        assert!(text.contains("plugin_builder"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_renamed_copy_is_named_in_the_message() {
        let text = explain_with(Path::new("a/p.dll"), Path::new("cache/ab/p.dll"), "failed", &[], &[]);
        assert!(text.contains("renamed copy"), "{text}");
    }

    #[test]
    fn the_cache_key_follows_the_shared_libraries() {
        let dir = scratch("fingerprint");
        let names = vec!["helio_dylib.dll".to_string()];
        std::fs::write(dir.join("helio_dylib.dll"), b"one").unwrap();
        let first = libraries_fingerprint(Some(&dir), &names);
        assert_eq!(first, libraries_fingerprint(Some(&dir), &names));
        std::fs::write(dir.join("helio_dylib.dll"), b"longer than one").unwrap();
        assert_ne!(first, libraries_fingerprint(Some(&dir), &names), "size changed");
        let other = vec!["helio_dylib.dll".to_string(), "ui_dylib.dll".to_string()];
        assert_ne!(
            libraries_fingerprint(Some(&dir), &names),
            libraries_fingerprint(Some(&dir), &other),
            "the set changed"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
