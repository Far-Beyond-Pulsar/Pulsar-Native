//! Content hashes (SHA-256, lowercase hex) for the records.

use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// One hash over several named parts; each part is length-prefixed so no
/// two different sequences hash alike.
pub fn sha256_parts<'a>(parts: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut hasher = Sha256::new();
    for (name, bytes) in parts {
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}
