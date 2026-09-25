//! How engine things map onto Gamma [`Channel`]s.
//!
//! - An **entity** channel carries the entity handle's bits
//!   (`pulsar_scenedb::Entity::bits()`), the same value event payloads use
//!   for entity fields (`u64`). Events about one object (a hit, damage, a
//!   message sent to it) go to its channel and reach only its subscribers.
//! - A **class** channel carries [`class_channel_id`] of the class GUID
//!   (`class.json`'s `class_id`), so every instance of a class can listen on
//!   one channel whatever the class is renamed to.
//!
//! Gamma never fans out between channels: an event published on an entity
//! channel does not reach global or class subscribers.

pub use gamma::Channel;

/// The channel of the entity with handle bits `entity_bits`.
pub fn entity_channel(entity_bits: u64) -> Channel {
    Channel::Entity(entity_bits)
}

/// Stable 64-bit id of the class channel for class GUID `class_guid`
/// (FNV-1a over `"class:"` and the GUID).
pub fn class_channel_id(class_guid: &str) -> u64 {
    use gamma::stable_id::{FNV_OFFSET, fnv_bytes};
    fnv_bytes(fnv_bytes(FNV_OFFSET, b"class:"), class_guid.as_bytes())
}

/// The channel every instance of class `class_guid` can listen on.
pub fn class_channel(class_guid: &str) -> Channel {
    Channel::Class(class_channel_id(class_guid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_are_stable_and_distinct() {
        assert_eq!(class_channel_id("a"), class_channel_id("a"));
        assert_ne!(class_channel_id("a"), class_channel_id("b"));
        assert_eq!(class_channel("a"), Channel::Class(class_channel_id("a")));
    }
}
