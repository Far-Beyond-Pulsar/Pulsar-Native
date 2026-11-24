//! The content key: the 256-bit key a project's pak sets are encrypted
//! with.
//!
//! One key per project, made once and kept in the project at
//! [`CONTENT_KEY_FILE`] (commit it, so every machine builds content the same
//! game can read). The packer reads it there; the game is given it at build
//! time (the generated game code holds it and installs it with
//! [`install_content_key`] before content is found), so content and the
//! binary that reads it always agree. A pak set packed with another key is
//! refused ([`super::PakError::WrongKey`]) rather than read as garbage.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use engine_fs::virtual_fs;
use parking_lot::RwLock;

/// Where a project keeps its content key, as 64 hex digits.
pub const CONTENT_KEY_FILE: &str = ".pulsar/content.key";

/// Why a content key could not be made, parsed or loaded.
#[derive(Debug, thiserror::Error)]
pub enum ContentKeyError {
    #[error("no random source: {0}")]
    NoRandomSource(getrandom::Error),
    #[error("a content key is 64 hex digits")]
    BadHex,
    #[error("{0}")]
    BadHexDigit(#[from] std::num::ParseIntError),
    /// The key file at `path` could not be read, created or parsed.
    #[error("{}: {source}", path.display())]
    File {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ContentKey(pub [u8; 32]);

impl std::fmt::Debug for ContentKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContentKey(..)")
    }
}

impl ContentKey {
    /// A new random key from the operating system's generator.
    pub fn generate() -> Result<Self, ContentKeyError> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(ContentKeyError::NoRandomSource)?;
        Ok(Self(key))
    }

    pub fn to_hex(&self) -> String {
        use std::fmt::Write;
        self.0.iter().fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    }

    pub fn from_hex(text: &str) -> Result<Self, ContentKeyError> {
        let text = text.trim();
        if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ContentKeyError::BadHex);
        }
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)?;
        }
        Ok(Self(key))
    }

    /// The project's key, made and saved the first time it is asked for.
    pub fn load_or_create(project_root: &Path) -> Result<Self, ContentKeyError> {
        let path = project_root.join(CONTENT_KEY_FILE);
        let file_error =
            |path: &Path, source: Box<dyn std::error::Error + Send + Sync>| ContentKeyError::File {
                path: path.to_path_buf(),
                source,
            };
        if virtual_fs::is_file(&path) {
            let text =
                virtual_fs::read_to_string(&path).map_err(|e| file_error(&path, e.into()))?;
            return Self::from_hex(&text).map_err(|e| file_error(&path, e.into()));
        }
        let key = Self::generate()?;
        if let Some(parent) = path.parent() {
            virtual_fs::create_dir_all(parent).map_err(|e| file_error(parent, e.into()))?;
        }
        // `create_file` fails if another process made the key first; then
        // that key is the project's.
        match virtual_fs::create_file(&path, format!("{}\n", key.to_hex()).as_bytes()) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "Created the project's content key");
                Ok(key)
            }
            Err(_) if virtual_fs::is_file(&path) => Self::load_or_create(project_root),
            Err(e) => Err(file_error(&path, e.into())),
        }
    }
}

/// For callers that still report errors as text; the message is unchanged.
impl From<ContentKeyError> for String {
    fn from(error: ContentKeyError) -> Self {
        error.to_string()
    }
}

fn slot() -> &'static RwLock<Option<ContentKey>> {
    // Static: the game installs it from `main`, before anything finds content.
    static KEY: OnceLock<RwLock<Option<ContentKey>>> = OnceLock::new();
    KEY.get_or_init(Default::default)
}

/// Give this process the key its packaged content opens with. The generated
/// game code calls it first thing; nothing else does.
pub fn install_content_key(key: ContentKey) {
    *slot().write() = Some(key);
}

/// The key installed with [`install_content_key`].
pub fn installed_content_key() -> Option<ContentKey> {
    *slot().read()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_through_hex_and_are_made_once() {
        let key = ContentKey::generate().unwrap();
        assert_eq!(ContentKey::from_hex(&key.to_hex()).unwrap(), key);
        assert!(ContentKey::from_hex("abc").is_err());
        assert!(
            !format!("{key:?}").contains(&key.to_hex()),
            "Debug hides it"
        );

        let project = tempfile::tempdir().unwrap();
        let made = ContentKey::load_or_create(project.path()).unwrap();
        assert_eq!(ContentKey::load_or_create(project.path()).unwrap(), made);
        assert!(project.path().join(CONTENT_KEY_FILE).is_file());
    }
}
