//! Metadata-only catalog for externally stored, immutable payloads.
//!
//! The catalog stores references, not payload bytes. It provides validated,
//! revision-checked metadata batch mutation semantics only. It does not make
//! payloads durable, transact against `pulsar_scenedb::World`, or persist the
//! catalog to a file; an owning scene-storage layer must provide those duties.
//! Callers must hold the scene's exclusive write boundary while applying a
//! batch so readers cannot observe intermediate per-key updates.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Opaque stable content identifier, conventionally a cryptographic digest.
///
/// The catalog cannot verify this identifier because payload bytes are stored
/// elsewhere. Producers are responsible for deriving it consistently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentId(pub [u8; 32]);

/// A reference to immutable content stored outside the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadRef {
    pub content_id: ContentId,
    /// Stable codec/format name, e.g. `"application/octet-stream"` or an
    /// application-defined encoding identifier.
    pub encoding: String,
    /// Version of the payload schema for this encoding.
    pub schema_version: u32,
    /// Exact payload size in bytes; bytes themselves are not kept here.
    pub byte_length: u64,
}

/// Generic key-to-content metadata. This type intentionally has no domain-
/// specific knowledge and owns no payload bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadCatalog {
    revision: u64,
    entries: BTreeMap<String, PayloadRef>,
}

impl PayloadCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn get(&self, key: &str) -> Option<&PayloadRef> {
        self.entries.get(key)
    }

    pub fn entries(&self) -> &BTreeMap<String, PayloadRef> {
        &self.entries
    }

    /// Validate the complete batch before changing catalog state. Duplicate
    /// keys are rejected even when one mutation deletes and another re-adds a
    /// key. Validation errors leave this catalog unchanged. After validation,
    /// updates are applied in place in O(batch_size · log(entry_count)) rather
    /// than cloning the whole catalog. Callers must serialize readers and
    /// writers at the owning SceneDB/scene boundary; this is not a rollback
    /// transaction for arbitrary SceneDB mutations. A non-empty successful
    /// batch increments the catalog-local revision once.
    pub fn apply_batch(
        &mut self,
        expected_revision: u64,
        mutations: &[PayloadMutation],
    ) -> Result<u64, PayloadCatalogError> {
        if expected_revision != self.revision {
            return Err(PayloadCatalogError::StaleRevision {
                expected: expected_revision,
                actual: self.revision,
            });
        }

        let mut seen = BTreeSet::new();
        for mutation in mutations {
            let (key, payload) = match mutation {
                PayloadMutation::Upsert { key, payload } => (key, Some(payload)),
                PayloadMutation::Delete { key } => (key, None),
            };
            validate_key(key)?;
            if !seen.insert(key.as_str()) {
                return Err(PayloadCatalogError::DuplicateKey(key.clone()));
            }
            if let Some(payload) = payload {
                validate_payload(payload)?;
            }
        }

        if mutations.is_empty() {
            return Ok(self.revision);
        }
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or(PayloadCatalogError::RevisionOverflow)?;

        // All recoverable errors have been ruled out. Mutate in place so a
        // small update does not clone a catalog that may contain millions of
        // chunk references. The owning scene write lock is the visibility
        // boundary for the whole batch.
        for mutation in mutations {
            match mutation {
                PayloadMutation::Upsert { key, payload } => {
                    self.entries.insert(key.clone(), payload.clone());
                }
                PayloadMutation::Delete { key } => {
                    self.entries.remove(key);
                }
            }
        }
        self.revision = next_revision;
        Ok(next_revision)
    }
}

/// One metadata update in a catalog batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadMutation {
    Upsert { key: String, payload: PayloadRef },
    Delete { key: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PayloadCatalogError {
    #[error("catalog revision is stale: expected {expected}, actual {actual}")]
    StaleRevision { expected: u64, actual: u64 },
    #[error("catalog key must not be empty or contain surrounding whitespace")]
    InvalidKey,
    #[error("catalog batch contains duplicate key {0:?}")]
    DuplicateKey(String),
    #[error("payload encoding must not be empty or contain surrounding whitespace")]
    InvalidEncoding,
    #[error("payload schema version must be nonzero")]
    InvalidSchemaVersion,
    #[error("catalog revision cannot be incremented further")]
    RevisionOverflow,
}

fn validate_key(key: &str) -> Result<(), PayloadCatalogError> {
    if key.is_empty() || key.trim() != key {
        return Err(PayloadCatalogError::InvalidKey);
    }
    Ok(())
}

fn validate_payload(payload: &PayloadRef) -> Result<(), PayloadCatalogError> {
    if payload.encoding.is_empty() || payload.encoding.trim() != payload.encoding {
        return Err(PayloadCatalogError::InvalidEncoding);
    }
    if payload.schema_version == 0 {
        return Err(PayloadCatalogError::InvalidSchemaVersion);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(seed: u8) -> PayloadRef {
        PayloadRef {
            content_id: ContentId([seed; 32]),
            encoding: "test-bytes".to_owned(),
            schema_version: 1,
            byte_length: 42,
        }
    }

    #[test]
    fn batch_upserts_and_deletes() {
        let mut catalog = PayloadCatalog::new();
        assert_eq!(
            catalog.apply_batch(
                0,
                &[
                    PayloadMutation::Upsert {
                        key: "alpha".into(),
                        payload: payload(1),
                    },
                    PayloadMutation::Upsert {
                        key: "beta".into(),
                        payload: payload(2),
                    },
                ],
            ),
            Ok(1)
        );
        assert_eq!(catalog.get("alpha"), Some(&payload(1)));
        assert_eq!(catalog.apply_batch(1, &[PayloadMutation::Delete { key: "alpha".into() }]), Ok(2));
        assert_eq!(catalog.get("alpha"), None);
        assert_eq!(catalog.get("beta"), Some(&payload(2)));
    }

    #[test]
    fn stale_revision_does_not_change_catalog() {
        let mut catalog = PayloadCatalog::new();
        catalog
            .apply_batch(
                0,
                &[PayloadMutation::Upsert {
                    key: "kept".into(),
                    payload: payload(1),
                }],
            )
            .unwrap();
        let before = catalog.clone();
        assert_eq!(
            catalog.apply_batch(0, &[PayloadMutation::Delete { key: "kept".into() }]),
            Err(PayloadCatalogError::StaleRevision {
                expected: 0,
                actual: 1,
            })
        );
        assert_eq!(catalog, before);
    }

    #[test]
    fn duplicate_or_invalid_key_rejects_entire_batch() {
        for batch in [
            vec![
                PayloadMutation::Upsert {
                    key: "new".into(),
                    payload: payload(3),
                },
                PayloadMutation::Delete { key: "new".into() },
            ],
            vec![
                PayloadMutation::Upsert {
                    key: "new".into(),
                    payload: payload(3),
                },
                PayloadMutation::Delete { key: "  ".into() },
            ],
        ] {
            let mut catalog = PayloadCatalog::new();
            catalog
                .apply_batch(
                    0,
                    &[PayloadMutation::Upsert {
                        key: "existing".into(),
                        payload: payload(1),
                    }],
                )
                .unwrap();
            let before = catalog.clone();
            assert!(catalog.apply_batch(1, &batch).is_err());
            assert_eq!(catalog, before);
        }
    }

    #[test]
    fn serde_round_trip_preserves_catalog_metadata() {
        let mut catalog = PayloadCatalog::new();
        catalog
            .apply_batch(
                0,
                &[PayloadMutation::Upsert {
                    key: "record/one".into(),
                    payload: payload(7),
                }],
            )
            .unwrap();
        let encoded = serde_json::to_vec(&catalog).unwrap();
        let decoded: PayloadCatalog = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, catalog);
    }
}
