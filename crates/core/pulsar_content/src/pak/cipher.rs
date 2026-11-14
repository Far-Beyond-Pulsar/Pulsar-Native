//! The keystream: ChaCha20 (20 rounds) keyed by the [`ContentKey`], one
//! stream per encrypted unit, XORed over its bytes. A unit's stream number
//! is derived from where it lives, so every block decodes on its own.
//!
//! [`ContentKey`]: super::ContentKey

use chacha20::rand_core::{SeedableRng, TryRng};
use chacha20::ChaCha20Rng;

/// Stream of the table of contents' body.
pub(super) const TOC_STREAM: u64 = u64::MAX;
/// Stream whose first bytes are the key check in the table of contents'
/// header: a wrong key is told apart from a corrupt file.
pub(super) const KEY_CHECK_STREAM: u64 = u64::MAX - 1;
/// Blocks live at most this far into a container (1 TiB): the offset and
/// container index share one stream number.
pub(super) const MAX_CONTAINER_LEN: u64 = 1 << 40;
/// Containers below this index keep block streams clear of the two above.
pub(super) const MAX_CONTAINERS: u32 = 1 << 22;

/// The stream of the block at `offset` in container `container`.
pub(super) fn block_stream(container: u32, offset: u64) -> u64 {
    debug_assert!(container < MAX_CONTAINERS && offset < MAX_CONTAINER_LEN);
    (u64::from(container) << 40) | offset
}

/// XOR `data` with stream `stream` of `key`, from its start.
pub(super) fn apply(key: &[u8; 32], stream: u64, data: &mut [u8]) {
    let mut rng = ChaCha20Rng::from_seed(*key);
    rng.set_stream(stream);
    let mut pad = [0u8; 4096];
    for chunk in data.chunks_mut(pad.len()) {
        let pad = &mut pad[..chunk.len()];
        let Ok(()) = rng.try_fill_bytes(pad);
        for (byte, k) in chunk.iter_mut().zip(pad.iter()) {
            *byte ^= k;
        }
    }
}

/// The key check: the first eight bytes of [`KEY_CHECK_STREAM`].
pub(super) fn key_check(key: &[u8; 32]) -> [u8; 8] {
    let mut check = [0u8; 8];
    apply(key, KEY_CHECK_STREAM, &mut check);
    check
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_twice_restores_and_streams_differ() {
        let key = [7u8; 32];
        let plain: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let mut data = plain.clone();
        apply(&key, block_stream(0, 16), &mut data);
        assert_ne!(data, plain);
        let mut other = plain.clone();
        apply(&key, block_stream(0, 17), &mut other);
        assert_ne!(data, other, "each block has its own keystream");
        apply(&key, block_stream(0, 16), &mut data);
        assert_eq!(data, plain);
        assert_ne!(key_check(&key), key_check(&[8u8; 32]));
    }
}
