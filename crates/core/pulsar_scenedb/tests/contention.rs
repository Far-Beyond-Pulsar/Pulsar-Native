//! Part VI — Test 1: multi-threaded contention.
//! Spec §21 Test 1. Models the phase contract: a write window (exclusive)
//! followed by a read window where N reader threads query a shared immutable
//! view concurrently. Verifies no data races (run with
//! `RUSTFLAGS="-Zsanitizer=thread"` on nightly where available) and no
//! deadlock over many rounds.

use pulsar_scenedb::{Aabb, SpatialCell};
use std::sync::Arc;
use std::thread;

#[test]
fn concurrent_readers_no_races_no_deadlock() {
    // Build a populated cell (write window, exclusive).
    let mut cell = SpatialCell::new(1024).unwrap();
    for i in 0..1024u32 {
        let f = i as f32;
        cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
    }
    // Freeze the cell as a shared immutable view for the read window.
    let cell = Arc::new(cell);

    // 8 reader threads, each running many AABB queries into its OWN scratch
    // buffer (spec §8.4: one scratch per view, concurrent reads are safe with
    // no in-progress writes).
    let mut handles = Vec::new();
    for t in 0..8u32 {
        let cell = Arc::clone(&cell);
        handles.push(thread::spawn(move || {
            let mut out = vec![0u32; cell.rows_in_use() as usize];
            let mut total = 0u64;
            for round in 0..2000u32 {
                let lo = ((t * 53 + round) % 1024) as f32;
                let q = Aabb { min: [lo, 0.0, 0.0], max: [lo + 64.0, 1.0, 1.0] };
                total += cell.query_aabb(&q, &mut out) as u64;
            }
            total
        }));
    }
    // All readers complete (no deadlock) and return plausible counts.
    let mut grand = 0u64;
    for h in handles {
        grand += h.join().expect("reader thread panicked");
    }
    assert!(grand > 0, "readers found hits");
}

#[test]
fn concurrent_queries_match_single_threaded() {
    // Determinism under concurrency: each thread's result for a fixed query
    // equals the single-threaded result.
    let mut cell = SpatialCell::new(512).unwrap();
    for i in 0..512u32 {
        let f = i as f32;
        cell.alloc(Aabb { min: [f, 0.0, 0.0], max: [f + 1.0, 1.0, 1.0] }).unwrap();
    }
    let q = Aabb { min: [0.0, 0.0, 0.0], max: [255.0, 1.0, 1.0] };
    let mut ref_out = vec![0u32; cell.rows_in_use() as usize];
    let expected = cell.query_aabb(&q, &mut ref_out);

    let cell = Arc::new(cell);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let cell = Arc::clone(&cell);
        let q = q;
        handles.push(thread::spawn(move || {
            let mut out = vec![0u32; cell.rows_in_use() as usize];
            cell.query_aabb(&q, &mut out)
        }));
    }
    for h in handles {
        assert_eq!(h.join().unwrap(), expected, "concurrent query result is deterministic");
    }
}
