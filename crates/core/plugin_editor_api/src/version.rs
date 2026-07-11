use serde::{Deserialize, Serialize};

// ============================================================================
// Version Information
// ============================================================================

/// Version information for compatibility checking across the DLL boundary.
///
/// This struct ensures that plugins are loaded only if they were compiled with
/// compatible versions of the engine and Rust compiler.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Engine version (major, minor, patch)
    pub engine_version: (u32, u32, u32),
    /// Rustc version hash (hash of semver part only)
    pub rustc_version_hash: u64,
}

impl VersionInfo {
    /// Get the current version info for this build
    pub const fn current() -> Self {
        Self {
            engine_version: parse_engine_version(),
            rustc_version_hash: rustc_version_hash(),
        }
    }

    /// Check if two versions are compatible
    pub fn is_compatible(&self, other: &Self) -> bool {
        // Engine major version must match
        if self.engine_version.0 != other.engine_version.0 {
            return false;
        }

        // Rustc version must match exactly (ABI not stable across versions)
        if self.rustc_version_hash != other.rustc_version_hash {
            return false;
        }

        true
    }
}

/// Compile-time hash of the rustc version
/// This is set at compile time to ensure ABI compatibility
const fn rustc_version_hash() -> u64 {
    const RUSTC_VERSION: &str = env!("RUSTC_VERSION");
    hash_semver_only(RUSTC_VERSION)
}

/// Hash only the semver portion of rustc version string
/// e.g., "1.83.0" from "rustc 1.83.0 (90b35a623 2024-11-26)"
const fn hash_semver_only(version: &str) -> u64 {
    let bytes = version.as_bytes();
    let mut start = 0;
    let mut end = 0;
    let mut found_start = false;
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] >= b'0' && bytes[i] <= b'9' && !found_start {
            start = i;
            found_start = true;
        }
        if found_start && (bytes[i] == b' ' || bytes[i] == b'(') {
            end = i;
            break;
        }
        i += 1;
    }

    if end == 0 {
        end = bytes.len();
    }

    let mut hash: u64 = 0xcbf29ce484222325;
    let mut i = start;
    while i < end {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

/// Parse engine version from CARGO_PKG_VERSION at compile time
/// Expects format "major.minor.patch" e.g. "0.1.47"
const fn parse_engine_version() -> (u32, u32, u32) {
    const VERSION_STR: &str = env!("CARGO_PKG_VERSION");
    let bytes = VERSION_STR.as_bytes();

    let mut major: u32 = 0;
    let mut minor: u32 = 0;
    let mut patch: u32 = 0;
    let mut component = 0;
    let mut i = 0;

    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'.' {
            component += 1;
        } else if byte >= b'0' && byte <= b'9' {
            let digit = (byte - b'0') as u32;
            match component {
                0 => major = major * 10 + digit,
                1 => minor = minor * 10 + digit,
                2 => patch = patch * 10 + digit,
                _ => {}
            }
        }
        i += 1;
    }

    (major, minor, patch)
}
