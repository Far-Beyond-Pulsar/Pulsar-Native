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
        names.push(String::from_utf8_lossy(&name[..end]).into_owned());
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
