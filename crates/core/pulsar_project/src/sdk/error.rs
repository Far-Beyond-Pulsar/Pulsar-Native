//! Why reading, indexing or mirroring the engine SDK failed.

use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// A file or directory of the SDK or its mirror could not be read or
    /// written.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{}: no parent directory", path.display())]
    NoParent { path: PathBuf },
    /// Any other problem, described by its message (a malformed manifest or
    /// index, a failed `cargo` run, a path outside the SDK).
    #[error("{0}")]
    Message(String),
}

impl SdkError {
    pub(crate) fn io(path: &std::path::Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl From<String> for SdkError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}
