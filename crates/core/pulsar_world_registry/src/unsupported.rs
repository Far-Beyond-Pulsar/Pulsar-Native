//! Component classes this engine keeps but does not consume
//! (Pulsar-Native#1035, Phase 4).
//!
//! A class declares itself with [`declare_unsupported_component!`]. Its
//! data is attached, edited, saved and loaded like any other component;
//! nothing renders or simulates it. The properties card shows the reason,
//! and attaching an instance logs it once per class, so a component never
//! looks supported when it is not.

use std::collections::HashSet;
use std::sync::Mutex;

/// One unsupported class and why.
pub struct UnsupportedComponentRegistration {
    pub class_name: &'static str,
    pub reason: &'static str,
}

inventory::collect!(UnsupportedComponentRegistration);

/// Declare that component class `class_name` has no consumer in this
/// engine, with a short user-facing `reason`.
#[macro_export]
macro_rules! declare_unsupported_component {
    ($class_name:expr, $reason:expr $(,)?) => {
        $crate::inventory::submit! {
            $crate::unsupported::UnsupportedComponentRegistration {
                class_name: $class_name,
                reason: $reason,
            }
        }
    };
}

/// Why `class_name` is not supported, if it declared so.
pub fn unsupported_reason(class_name: &str) -> Option<&'static str> {
    inventory::iter::<UnsupportedComponentRegistration>
        .into_iter()
        .find(|registration| registration.class_name == class_name)
        .map(|registration| registration.reason)
}

/// Every declared unsupported class, for listings and audits.
pub fn unsupported_classes() -> impl Iterator<Item = &'static UnsupportedComponentRegistration> {
    inventory::iter::<UnsupportedComponentRegistration>.into_iter()
}

static REPORTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Log `class_name`'s reason the first time an instance of it is attached
/// in this process. Returns whether this call logged.
pub(crate) fn report_attach(class_name: &str) -> bool {
    let Some(reason) = unsupported_reason(class_name) else {
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
        "{class_name} is not supported by this engine: {reason}"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::declare_unsupported_component!("UnsupportedProbe", "nothing consumes it");

    #[test]
    fn a_declared_class_reports_its_reason_once() {
        assert_eq!(
            unsupported_reason("UnsupportedProbe"),
            Some("nothing consumes it")
        );
        assert_eq!(unsupported_reason("NotDeclared"), None);
        assert!(unsupported_classes().any(|r| r.class_name == "UnsupportedProbe"));
        assert!(report_attach("UnsupportedProbe"));
        assert!(!report_attach("UnsupportedProbe"), "logged once per class");
        assert!(!report_attach("NotDeclared"));
    }
}
