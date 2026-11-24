//! Findings a plugin contributes: a check the editor's collector runs
//! periodically while the plugin is enabled.
//!
//! ```ignore
//! p.add(pulsar_findings::FindingSource {
//!     origin: p.origin(),
//!     id: "budgets",
//!     label: "Virtual geometry",
//!     collect: my_budget_findings,
//! });
//! ```
//!
//! `collect` runs on a worker thread about once a second: read state the
//! plugin already keeps (atomics, the last frame's stats), never wait on the
//! GPU or a lock the frame holds, and return the complete set (empty when all
//! is well).

use std::collections::BTreeSet;

use crate::{Finding, publish};

/// A plugin's check.
pub struct FindingSource {
    /// The plugin's root crate (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// Stable within the plugin (`"budgets"`).
    pub id: &'static str,
    /// The source's name in the panel (where its findings without a
    /// subject are grouped).
    pub label: &'static str,
    pub collect: fn() -> Vec<Finding>,
}

impl pulsar_plugins::Contribution for FindingSource {
    const CATEGORY: &'static str = "findings.source";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_string()
    }

    fn label(&self) -> String {
        self.label.to_string()
    }
}

pulsar_plugins::registration!(FindingSource);

/// The store name of a registered source.
fn source_name(source: &FindingSource) -> String {
    format!("{}#{}", source.label, source.id)
}

/// Run every enabled plugin's check and publish each result; withdraw the
/// sources published by the previous call that are gone now (their plugin was
/// disabled). `previous` is what the last call returned.
pub fn collect_registered(previous: &BTreeSet<String>) -> BTreeSet<String> {
    let mut now = BTreeSet::new();
    for source in pulsar_plugins::registered::<FindingSource>() {
        let name = source_name(source);
        publish(&name, (source.collect)());
        now.insert(name);
    }
    for gone in previous.difference(&now) {
        publish(gone, Vec::new());
    }
    now
}
