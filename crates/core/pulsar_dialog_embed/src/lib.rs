//! The `pulsar-dialog` helper, carried inside the engine.
//!
//! The helper (Pulsar-Dialog) shows the engine's dialogs in their own process.
//! This crate's build script builds it for the target and embeds the
//! executable; at startup the engine calls [`ensure_helper`], which puts it
//! beside the engine's own executable when it is not already there, so
//! `pulsar_dialog_protocol::client` finds it. A user installs one file and
//! never hears about it.
//!
//! If the executable's directory cannot be written (a read-only install), the
//! helper goes to the per-user app-data directory instead and the client is
//! told where it is. If the helper could not be embedded at all (the build
//! script could not build it), nothing is placed and the client falls back to
//! the platform's message boxes.

// The engine's own executable directory and per-user app data, outside any project.
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::{Path, PathBuf};

use pulsar_dialog_protocol::client;

/// The helper's executable, as built by `build.rs`; empty when none was built.
static HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pulsar-dialog.bin"));

mod generated {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

/// Whether this build carries a helper.
pub fn is_embedded() -> bool {
    !HELPER.is_empty()
}

/// What [`ensure_helper`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Placement {
    /// This build carries no helper; the client uses the native fallback.
    NotEmbedded,
    /// Already beside the executable and intact.
    Present(PathBuf),
    /// Written beside the executable (first run, a new engine version, or a damaged file).
    Placed(PathBuf),
    /// The executable's directory is read-only; the helper is in the user's
    /// app-data directory and the client has been told.
    Cached(PathBuf),
}

/// Make sure the helper is where the engine can use it. Cheap when it is
/// already there (a length check and one hash of the file). Failure is not
/// fatal: the caller logs it and the dialogs fall back to the native ones.
pub fn ensure_helper() -> io::Result<Placement> {
    if !is_embedded() {
        return Ok(Placement::NotEmbedded);
    }
    let name = client::helper_file_name();
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("the executable has no directory"))?;
    match place(&exe_dir, name, HELPER, generated::SHA256) {
        Ok((path, true)) => Ok(Placement::Placed(path)),
        Ok((path, false)) => Ok(Placement::Present(path)),
        Err(beside_error) => {
            tracing::info!(
                "could not place the dialog helper beside the engine ({beside_error}); using the app-data directory"
            );
            let cache = pulsar_install_layout::appdata_dir().join("bin");
            let (path, _) = place(&cache, name, HELPER, generated::SHA256)?;
            client::set_helper_path(path.clone());
            Ok(Placement::Cached(path))
        }
    }
}

/// Put `bytes` at `dir/name` unless an intact copy (same length, same SHA-256)
/// is there. Returns the path and whether it was written. Written through a
/// temporary name and renamed, so a crash never leaves half an executable that
/// the client would launch.
fn place(dir: &Path, name: &str, bytes: &[u8], sha256: &str) -> io::Result<(PathBuf, bool)> {
    let path = dir.join(name);
    let intact = path
        .metadata()
        .is_ok_and(|meta| meta.is_file() && meta.len() == bytes.len() as u64)
        && pulsar_install_layout::hash_file(&path).is_ok_and(|hash| hash == sha256);
    if intact {
        return Ok((path, false));
    }
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o755))?;
    }
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok((path, true))
}

#[cfg(test)]
mod tests {
    use super::{generated, place, HELPER};
    use sha2::{Digest, Sha256};

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pulsar-dialog-embed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_helper_is_written_once_then_left_alone() {
        let dir = scratch("once");
        let bytes = b"pretend executable";
        let (path, written) = place(&dir, "pulsar-dialog", bytes, &sha(bytes)).unwrap();
        assert!(written);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let (_, written_again) = place(&dir, "pulsar-dialog", bytes, &sha(bytes)).unwrap();
        assert!(!written_again, "an intact copy is not rewritten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_or_older_copy_is_replaced() {
        let dir = scratch("damaged");
        let new = b"the helper this engine ships";
        std::fs::create_dir_all(&dir).unwrap();
        // Same length, different content: only the hash tells.
        std::fs::write(dir.join("pulsar-dialog"), vec![b'x'; new.len()]).unwrap();
        let (path, written) = place(&dir, "pulsar-dialog", new, &sha(new)).unwrap();
        assert!(written);
        assert_eq!(std::fs::read(path).unwrap(), new);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = scratch("tidy");
        place(&dir, "pulsar-dialog", b"abc", &sha(b"abc")).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_embedded_hash_is_the_hash_of_the_embedded_bytes() {
        if HELPER.is_empty() {
            assert_eq!(generated::SHA256, "");
        } else {
            assert_eq!(generated::SHA256, sha(HELPER));
        }
    }
}
