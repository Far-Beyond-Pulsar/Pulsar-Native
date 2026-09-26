//! Native capabilities (#869).
//!
//! A native can be gated behind a *capability* (its
//! [`CAPABILITY_ATTR`] attribute, set with
//! [`NativeBuilder::capability`](crate::NativeBuilder::capability)): `fs`,
//! `net`, `process`, `env`, `debug`, ... A [`CapabilityPolicy`] says which
//! capabilities a module may use; linking a module that imports a native
//! outside its policy fails with [`LinkError::CapabilityDenied`](crate::LinkError::CapabilityDenied).
//! Natives without a capability are always allowed, and the default
//! policy allows everything.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Attribute key naming a native's capability.
pub const CAPABILITY_ATTR: &str = "capability";

/// Which native capabilities modules may import.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityPolicy {
    /// `None`: every capability. `Some(set)`: only those.
    allowed: Option<BTreeSet<String>>,
}

impl CapabilityPolicy {
    /// Allow every capability (the default).
    pub fn allow_all() -> Self {
        Self { allowed: None }
    }

    /// Allow only `capabilities`.
    pub fn only<S: Into<String>>(capabilities: impl IntoIterator<Item = S>) -> Self {
        Self { allowed: Some(capabilities.into_iter().map(Into::into).collect()) }
    }

    /// Whether a native needing `capability` may be imported.
    pub fn allows(&self, capability: Option<&str>) -> bool {
        match (capability, &self.allowed) {
            (None, _) | (_, None) => true,
            (Some(capability), Some(allowed)) => allowed.contains(capability),
        }
    }

    /// Whether this policy allows everything.
    pub fn is_unrestricted(&self) -> bool {
        self.allowed.is_none()
    }

    /// The allowed capabilities, `None` for all.
    pub fn allowed(&self) -> Option<&BTreeSet<String>> {
        self.allowed.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_allows_everything_and_ungated_natives_always_link() {
        assert!(CapabilityPolicy::default().allows(Some("fs")));
        let policy = CapabilityPolicy::only(["net"]);
        assert!(policy.allows(None));
        assert!(policy.allows(Some("net")));
        assert!(!policy.allows(Some("fs")));
        let json: CapabilityPolicy = serde_json::from_str(r#"["fs"]"#).unwrap();
        assert_eq!(json, CapabilityPolicy::only(["fs"]));
        let all: CapabilityPolicy = serde_json::from_str("null").unwrap();
        assert!(all.is_unrestricted());
    }
}
