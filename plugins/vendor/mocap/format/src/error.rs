//! The format's one error type.

/// Everything reading or writing a `.mocap` can refuse.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// The file does not start with the `PMOC` magic.
    #[error("not a .mocap file (bad magic)")]
    BadMagic,
    /// The file is from a newer major version than this reader.
    #[error("unsupported .mocap version {found} (this reader supports up to {supported})")]
    UnsupportedVersion { found: u16, supported: u16 },
    /// The file ended inside the header or a mandatory chunk.
    #[error("the file is truncated: {0}")]
    Truncated(&'static str),
    /// A mandatory chunk (`META`, `RIGG`) is missing.
    #[error("missing mandatory chunk {0}")]
    MissingChunk(&'static str),
    /// A chunk's checksum did not match its payload.
    #[error("chunk {fourcc} at offset {offset} failed its checksum")]
    Checksum { fourcc: String, offset: u64 },
    /// A chunk's payload could not be decoded.
    #[error("malformed {chunk} chunk: {reason}")]
    Malformed { chunk: &'static str, reason: String },
    /// A frame pushed to a writer has the wrong number of samples.
    #[error("a frame has {got} samples, the rig has {expected}")]
    FrameWidth { got: usize, expected: usize },
    /// The rig schema is inconsistent.
    #[error("invalid rig: {0}")]
    InvalidRig(String),
    /// The destination refused a write.
    #[error("write failed: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON chunk would not serialise.
    #[error("metadata encoding failed: {0}")]
    Json(#[from] serde_json::Error),
}
