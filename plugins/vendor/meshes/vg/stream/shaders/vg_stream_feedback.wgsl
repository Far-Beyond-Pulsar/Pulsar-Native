// Virtual geometry streaming: GPU->CPU page feedback (compute stages only; needs
// the read-write vg_feedback binding). Include after vg_stream.wgsl.

const VG_FB_COUNT: u32 = 0u;
const VG_FB_CAPACITY: u32 = 1u;
const VG_FB_BITS_WORDS: u32 = 2u;
const VG_FB_HEADER_WORDS: u32 = 4u;

/// Record that a non-resident page is wanted. Deduplicated by a bitset: the
/// first caller per page per readback appends it to the request list.
fn vg_request_page(global_page: u32) {
    let bits_base = VG_FB_HEADER_WORDS + atomicLoad(&vg_feedback[VG_FB_CAPACITY]);
    let bit = 1u << (global_page & 31u);
    let old = atomicOr(&vg_feedback[bits_base + (global_page >> 5u)], bit);
    if ((old & bit) == 0u) {
        let i = atomicAdd(&vg_feedback[VG_FB_COUNT], 1u);
        if (i < atomicLoad(&vg_feedback[VG_FB_CAPACITY])) {
            atomicStore(&vg_feedback[VG_FB_HEADER_WORDS + i], global_page);
        }
    }
}

/// Record that a resident page was needed (keeps it young in the LRU).
fn vg_touch_page(global_page: u32) {
    let cap = atomicLoad(&vg_feedback[VG_FB_CAPACITY]);
    let words = atomicLoad(&vg_feedback[VG_FB_BITS_WORDS]);
    let bit = 1u << (global_page & 31u);
    atomicOr(&vg_feedback[VG_FB_HEADER_WORDS + cap + words + (global_page >> 5u)], bit);
}

/// Resolve a page for drawing: returns its slot and touches it, or requests
/// it and returns VG_PAGE_NOT_RESIDENT (draw the resident parent instead).
fn vg_use_or_request_page(global_page: u32) -> u32 {
    let slot = vg_residency[global_page];
    if (slot == VG_PAGE_NOT_RESIDENT) {
        vg_request_page(global_page);
    } else {
        vg_touch_page(global_page);
    }
    return slot;
}
