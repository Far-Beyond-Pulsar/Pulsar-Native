//! The process-wide store: each source's latest complete set.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::Finding;

/// What every source published last, by source name.
pub type Published = Arc<BTreeMap<String, Arc<Vec<Finding>>>>;

static STORE: Mutex<Option<Published>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Replace everything `source` publishes (an empty set removes the source).
/// Returns whether anything changed; an unchanged republish does not move
/// [`generation`], so a source may publish on every poll.
pub fn publish(source: &str, findings: Vec<Finding>) -> bool {
    let mut store = STORE.lock();
    let current = store.get_or_insert_with(Default::default);
    let unchanged = match current.get(source) {
        Some(existing) => **existing == findings,
        None => findings.is_empty(),
    };
    if unchanged {
        return false;
    }
    let mut next: BTreeMap<String, Arc<Vec<Finding>>> = (**current).clone();
    if findings.is_empty() {
        next.remove(source);
    } else {
        next.insert(source.to_string(), Arc::new(findings));
    }
    *current = Arc::new(next);
    GENERATION.fetch_add(1, Ordering::AcqRel);
    true
}

/// Every source's findings, as last published. Cheap (shared).
pub fn snapshot() -> Published {
    STORE.lock().get_or_insert_with(Default::default).clone()
}

/// Moves whenever a source's set changes.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Category, Severity};

    fn finding(key: &str) -> Finding {
        Finding::new(key, Category::Performance, Severity::Warning, key)
    }

    #[test]
    fn a_source_replaces_its_set_and_an_unchanged_one_is_no_change() {
        let source = "test:replace";
        assert!(publish(source, vec![finding("a"), finding("b")]));
        let before = generation();
        assert!(
            !publish(source, vec![finding("a"), finding("b")]),
            "same set"
        );
        assert_eq!(generation(), before);
        assert!(publish(source, vec![finding("b")]));
        assert_eq!(snapshot()[source].len(), 1);
        assert!(publish(source, Vec::new()));
        assert!(!snapshot().contains_key(source), "empty removes the source");
        assert!(!publish(source, Vec::new()), "already gone");
    }
}
