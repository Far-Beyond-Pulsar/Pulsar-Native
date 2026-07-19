//! Part VI — Test 2 (host half): stale-handle rejection.
//! Spec §21 Test 2 / CONTRACTS.md C1. The GPU-side validation is M3.

use pulsar_scenedb::{Aabb, SpatialCell};

#[test]
fn freed_handle_never_resolves() {
    let mut c = SpatialCell::new(64).unwrap();
    let h = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    assert!(c.row_of(h).is_some());
    c.free(h);
    assert_eq!(c.row_of(h), None, "freed handle must not resolve");
    c.compact();
    assert_eq!(c.row_of(h), None, "still dead after compaction");
}

#[test]
fn recycled_slot_rejects_old_generation() {
    let mut c = SpatialCell::new(64).unwrap();
    let h1 = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    c.free(h1);
    c.compact();
    let h2 = c.alloc(Aabb { min: [5.0; 3], max: [6.0; 3] }).unwrap();
    // Same physical slot index, bumped generation.
    assert_eq!(h2.index(), h1.index());
    assert!(h2.generation() > h1.generation());
    // The OLD handle must be rejected even though its slot is live again.
    assert_eq!(c.row_of(h1), None, "stale generation rejected");
    assert!(c.row_of(h2).is_some(), "fresh handle valid");
}

#[test]
fn stale_handle_absent_from_query_output() {
    // After free+compact, a query must never emit the freed element's row.
    let mut c = SpatialCell::new(64).unwrap();
    let ha = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    let hb = c.alloc(Aabb { min: [0.0; 3], max: [1.0; 3] }).unwrap();
    c.free(ha);
    c.compact();
    let mut out = vec![0u32; c.rows_in_use() as usize];
    let n = c.query_aabb(&Aabb { min: [-1.0; 3], max: [2.0; 3] }, &mut out);
    assert_eq!(n, 1, "only the surviving element is a hit");
    // The survivor (hb) resolves; ha does not.
    assert!(c.row_of(hb).is_some());
    assert_eq!(c.row_of(ha), None);
}
