//! End to end: the real tracking allocator, a real large allocation, and the
//! data the Memory Explorer shows for it.

use ui_log_viewer::{
    disable_tracking, enable_tracking, large_allocation_snapshot, TrackingAllocator,
};

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator::new();

#[inline(never)]
fn hold_a_big_buffer_for_the_test() -> Vec<u8> {
    // Non-zero fill so the pages are really committed and nothing is elided.
    vec![7u8; 32 * 1024 * 1024]
}

#[test]
fn a_large_allocation_is_listed_with_its_owner_until_it_is_freed() {
    enable_tracking();
    let buffer = hold_a_big_buffer_for_the_test();
    let during = large_allocation_snapshot();
    disable_tracking();

    let group = during
        .groups
        .iter()
        .find(|group| group.largest >= 32 * 1024 * 1024)
        .expect("the 32 MiB buffer is listed");
    assert!(group.live_bytes >= 32 * 1024 * 1024);
    assert!(
        group
            .stack
            .iter()
            .any(|frame| frame.contains("hold_a_big_buffer_for_the_test")),
        "the stack reaches the owner: {:#?}",
        group.stack
    );
    assert!(
        group.label.contains("hold_a_big_buffer_for_the_test"),
        "attributed to the owner, not RawVec plumbing: {}",
        group.label
    );

    enable_tracking();
    drop(buffer);
    let after = large_allocation_snapshot();
    disable_tracking();
    assert!(
        !after.groups.iter().any(|g| g.largest >= 32 * 1024 * 1024),
        "the freed buffer is gone"
    );
    assert!(after.total_frees >= 1);
}
