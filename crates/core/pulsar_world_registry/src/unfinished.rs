//! Component classes this engine keeps but does not consume yet
//! (Pulsar-Native#1035, Phase 4).
//!
//! A class declares itself with [`declare_unfinished_component!`], naming
//! why and the issue that tracks the work. Its data is attached, edited,
//! saved and loaded like any other component; nothing renders or simulates
//! it yet. The properties card shows the reason and the issue, and
//! attaching an instance logs them once per class, so a component never
//! looks finished when it is not.

use std::collections::HashSet;
use std::sync::Mutex;

/// One unfinished class: why, and the issue tracking it.
pub struct UnfinishedComponentRegistration {
    pub class_name: &'static str,
    pub reason: &'static str,
    pub issue: &'static str,
}

inventory::collect!(UnfinishedComponentRegistration);

/// Declare that component class `class_name` is not consumed by this
/// engine yet: a short user-facing `reason` and the tracking `issue` URL.
#[macro_export]
macro_rules! declare_unfinished_component {
    ($class_name:expr, $reason:expr, $issue:expr $(,)?) => {
        $crate::inventory::submit! {
            $crate::unfinished::UnfinishedComponentRegistration {
                class_name: $class_name,
                reason: $reason,
                issue: $issue,
            }
        }
    };
}

/// `class_name`'s declaration, if it is unfinished.
pub fn unfinished_component(class_name: &str) -> Option<&'static UnfinishedComponentRegistration> {
    crate::runtime::unfinished().iter().copied()
        .into_iter()
        .find(|registration| registration.class_name == class_name)
}

/// Every declared unfinished class, for listings and audits.
pub fn unfinished_components() -> impl Iterator<Item = &'static UnfinishedComponentRegistration> {
    crate::runtime::unfinished().iter().copied().into_iter()
}

static REPORTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Log `class_name`'s reason and issue the first time an instance of it is
/// attached in this process. Returns whether this call logged.
pub(crate) fn report_attach(class_name: &str) -> bool {
    let Some(unfinished) = unfinished_component(class_name) else {
        return false;
    };
    let mut reported = REPORTED.lock().unwrap_or_else(|error| error.into_inner());
    if !reported
        .get_or_insert_with(HashSet::new)
        .insert(class_name.to_string())
    {
        return false;
    }
    tracing::warn!(
        class = class_name,
        issue = unfinished.issue,
        "{class_name} is unfinished in this engine: {} (tracked in {})",
        unfinished.reason,
        unfinished.issue
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::declare_unfinished_component!(
        "UnfinishedProbe",
        "nothing consumes it yet",
        "https://example.invalid/issues/1",
    );

    #[test]
    fn a_declared_class_reports_its_reason_once() {
        let probe = unfinished_component("UnfinishedProbe").expect("declared");
        assert_eq!(probe.reason, "nothing consumes it yet");
        assert_eq!(probe.issue, "https://example.invalid/issues/1");
        assert!(unfinished_component("NotDeclared").is_none());
        assert!(unfinished_components().any(|r| r.class_name == "UnfinishedProbe"));
        assert!(report_attach("UnfinishedProbe"));
        assert!(!report_attach("UnfinishedProbe"), "logged once per class");
        assert!(!report_attach("NotDeclared"));
    }
}
