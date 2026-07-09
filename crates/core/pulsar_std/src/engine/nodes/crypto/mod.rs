//! # Crypto and Hashing Module
//!
//! Cryptographic and hashing utility nodes for the Pulsar visual programming system.
//!
//! This module provides nodes for:
//! - Hash generation (simplified implementations)
//! - String encoding/decoding
//! - Checksum calculations
//! - UUID generation

use crate::blueprint;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};

// =============================================================================
// Hash Functions (Simplified Implementations)
// =============================================================================

/// Generate a simple hash of a string.
///
/// # Inputs
/// - `text`: The text to hash
///
/// # Returns
/// A hash value as a hexadecimal string
///
/// # Crypto Hash String
/// Generates a hash of the input string.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn hash_string(text: String) -> String {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    text.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Generate a simple hash of an integer.
///
/// # Inputs
/// - `value`: The integer to hash
///
/// # Returns
/// A hash value as a hexadecimal string
///
/// # Crypto Hash Int
/// Generates a hash of an integer value.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn hash_int(value: i64) -> String {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    value.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Generate a hash with a seed.
///
/// # Inputs
/// - `text`: The text to hash
/// - `seed`: Seed value for the hash
///
/// # Returns
/// A seeded hash value
///
/// # Crypto Hash Seeded
/// Generates a seeded hash of the input.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn hash_seeded(text: String, seed: i64) -> String {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    seed.hash(&mut hasher);
    text.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

// =============================================================================
// UUID Generation
// =============================================================================

/// Generate a simple UUID (v4-like, simplified).
///
/// # Returns
/// A UUID-like string
///
/// # Crypto UUID
/// Generates a UUID-like identifier.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn generate_uuid() -> String {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    let h1 = hasher.finish();

    std::time::SystemTime::now().hash(&mut hasher);
    let h2 = hasher.finish();

    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (h1 >> 32) as u32,
        (h1 >> 16) as u16,
        h1 as u16,
        (h2 >> 48) as u16,
        h2 & 0xFFFFFFFFFFFF
    )
}

/// Generate a short unique ID.
///
/// # Returns
/// A short unique identifier (8 characters)
///
/// # Crypto Short ID
/// Generates a short unique identifier.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn generate_short_id() -> String {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);
    format!("{:08x}", (hasher.finish() as u32))
}

/// Generate a numeric ID from timestamp.
///
/// # Returns
/// A numeric ID based on current timestamp
///
/// # Crypto Timestamp ID
/// Generates a numeric ID from the current timestamp.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn generate_timestamp_id() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// =============================================================================
// Encoding and Decoding
// =============================================================================

/// Encode a string to hexadecimal.
///
/// # Inputs
/// - `text`: The text to encode
///
/// # Returns
/// Hexadecimal encoded string
///
/// # Crypto Hex Encode
/// Encodes text to hexadecimal format.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn hex_encode(text: String) -> String {
    text.bytes()
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join("")
}

/// Decode a hexadecimal string.
///
/// # Inputs
/// - `hex`: The hexadecimal string to decode
///
/// # Returns
/// Decoded string (or error message if invalid)
///
/// # Crypto Hex Decode
/// Decodes a hexadecimal string.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn hex_decode(hex: String) -> String {
    let chars: Vec<char> = hex.chars().collect();
    let mut result = Vec::new();

    for i in (0..chars.len()).step_by(2) {
        if i + 1 < chars.len() {
            let byte_str = format!("{}{}", chars[i], chars[i + 1]);
            if let Ok(byte) = u8::from_str_radix(&byte_str, 16) {
                result.push(byte);
            }
        }
    }

    String::from_utf8(result).unwrap_or_else(|_| "[Invalid UTF-8]".to_string())
}

/// Encode a string to Base64 (simplified).
///
/// # Inputs
/// - `text`: The text to encode
///
/// # Returns
/// Base64-like encoded string (simplified)
///
/// # Crypto Base64 Encode
/// Encodes text to Base64 format (simplified).
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn base64_encode(text: String) -> String {
    // Simplified - just use hex for now
    hex_encode(text)
}

/// Encode text as URL-safe string.
///
/// # Inputs
/// - `text`: The text to encode
///
/// # Returns
/// URL-safe encoded string
///
/// # Crypto URL Encode Simple
/// Encodes text for safe use in URLs (simple implementation).
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn crypto_url_encode(text: String) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

// =============================================================================
// Checksum and Validation
// =============================================================================

/// Calculate a simple checksum of a string.
///
/// # Inputs
/// - `text`: The text to checksum
///
/// # Returns
/// A checksum value
///
/// # Crypto Checksum
/// Calculates a simple checksum of the input.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn checksum(text: String) -> i64 {
    text.bytes().map(|b| b as i64).sum()
}

/// Calculate CRC-like checksum.
///
/// # Inputs
/// - `text`: The text to checksum
///
/// # Returns
/// A CRC-like checksum value
///
/// # Crypto CRC
/// Calculates a CRC-like checksum.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn crc_checksum(text: String) -> i64 {
    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    text.hash(&mut hasher);
    (hasher.finish() & 0xFFFFFFFF) as i64
}

/// Verify a checksum.
///
/// # Inputs
/// - `text`: The text to verify
/// - `expected_checksum`: The expected checksum value
///
/// # Returns
/// True if checksums match
///
/// # Crypto Verify Checksum
/// Verifies if the checksum matches the expected value.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn verify_checksum(text: String, expected_checksum: i64) -> bool {
    checksum(text) == expected_checksum
}

// =============================================================================
// Obfuscation (Simple)
// =============================================================================

/// Simple XOR cipher encoding/decoding.
///
/// # Inputs
/// - `text`: The text to encode/decode
/// - `key`: The XOR key (single byte)
///
/// # Returns
/// XOR encoded/decoded text
///
/// # Crypto XOR Cipher
/// Applies simple XOR cipher to text.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn xor_cipher(text: String, key: i64) -> String {
    let key_byte = (key & 0xFF) as u8;
    text.bytes().map(|b| (b ^ key_byte) as char).collect()
}

/// Caesar cipher shift.
///
/// # Inputs
/// - `text`: The text to encode
/// - `shift`: The shift amount
///
/// # Returns
/// Caesar cipher encoded text
///
/// # Crypto Caesar Cipher
/// Applies Caesar cipher shift to text.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn caesar_cipher(text: String, shift: i64) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                let offset = ((c as i64 - 'a' as i64 + shift) % 26 + 26) % 26;
                ((offset + 'a' as i64) as u8) as char
            } else if c.is_ascii_uppercase() {
                let offset = ((c as i64 - 'A' as i64 + shift) % 26 + 26) % 26;
                ((offset + 'A' as i64) as u8) as char
            } else {
                c
            }
        })
        .collect()
}

/// Reverse a string (simple obfuscation).
///
/// # Inputs
/// - `text`: The text to reverse
///
/// # Returns
/// Reversed text
///
/// # Crypto Reverse String
/// Reverses a string for simple obfuscation.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn reverse_string(text: String) -> String {
    text.chars().rev().collect()
}

/// ROT13 cipher.
///
/// # Inputs
/// - `text`: The text to encode/decode
///
/// # Returns
/// ROT13 encoded/decoded text
///
/// # Crypto ROT13
/// Applies ROT13 cipher to text.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn rot13(text: String) -> String {
    caesar_cipher(text, 13)
}

// =============================================================================
// Hash Comparison
// =============================================================================

/// Compare two hashes for equality.
///
/// # Inputs
/// - `hash1`: First hash
/// - `hash2`: Second hash
///
/// # Returns
/// True if hashes match
///
/// # Crypto Compare Hashes
/// Compares two hash values for equality.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn compare_hashes(hash1: String, hash2: String) -> bool {
    hash1 == hash2
}

/// Validate hash format (hexadecimal check).
///
/// # Inputs
/// - `hash`: The hash to validate
///
/// # Returns
/// True if hash is valid hexadecimal
///
/// # Crypto Validate Hash
/// Validates if a string is valid hexadecimal hash.
#[blueprint(type: crate::NodeTypes::pure, category: "Crypto", color: "#8E44AD")]
pub fn validate_hash(hash: String) -> bool {
    hash.chars().all(|c| c.is_ascii_hexdigit())
}
