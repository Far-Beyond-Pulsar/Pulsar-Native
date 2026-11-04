//! GPU -> CPU page feedback: one storage buffer holding a deduplicated
//! request list, a request bitset (dedup) and a "used" bitset (LRU recency).
//!
//! Layout in u32 words (mirrored by `shaders/vg_stream.wgsl`):
//!
//! ```text
//! [0] count          atomic append counter (may exceed capacity)
//! [1] list_capacity  written once by the CPU
//! [2] bits_words     words per bitset, written once by the CPU
//! [3] reserved
//! [4 .. 4+cap]                 request list (global page ids)
//! [4+cap .. 4+cap+bits]        request bitset (dedup)
//! [4+cap+bits .. 4+cap+2*bits] used bitset (resident pages needed)
//! ```

pub const HEADER_WORDS: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedbackLayout {
    pub list_capacity: u32,
    pub bits_words: u32,
}

impl FeedbackLayout {
    pub fn new(list_capacity: u32, page_capacity: u32) -> Self {
        Self {
            list_capacity,
            bits_words: page_capacity.div_ceil(32).max(1),
        }
    }
    pub fn total_words(&self) -> u32 {
        HEADER_WORDS + self.list_capacity + 2 * self.bits_words
    }
    pub fn total_bytes(&self) -> u64 {
        u64::from(self.total_words()) * 4
    }
    pub fn request_bits_offset_words(&self) -> u32 {
        HEADER_WORDS + self.list_capacity
    }
    pub fn used_bits_offset_words(&self) -> u32 {
        self.request_bits_offset_words() + self.bits_words
    }
    /// Initial contents: counter 0, constants filled in, everything else 0.
    pub fn initial_words(&self) -> Vec<u32> {
        let mut w = vec![0u32; self.total_words() as usize];
        w[1] = self.list_capacity;
        w[2] = self.bits_words;
        w
    }
}

/// Decoded feedback of one readback.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Feedback {
    /// Distinct non-resident pages the GPU wanted (ascending page id).
    pub requests: Vec<u32>,
    /// Distinct resident pages the GPU used (ascending page id).
    pub used: Vec<u32>,
    /// The request list filled up; some requests were lost this round.
    pub overflowed: bool,
}

/// Decode a readback of the whole feedback buffer. Defensive: ids at or past
/// `page_count`, duplicates, and a counter past the capacity are all handled.
pub fn decode(words: &[u32], layout: FeedbackLayout, page_count: u32) -> Feedback {
    let mut fb = Feedback::default();
    if words.len() < layout.total_words() as usize {
        return fb;
    }
    let count = words[0];
    let cap = layout.list_capacity;
    fb.overflowed = count > cap;
    let n = count.min(cap) as usize;
    let list = &words[HEADER_WORDS as usize..HEADER_WORDS as usize + n];
    fb.requests = list.iter().copied().filter(|&p| p < page_count).collect();
    fb.requests.sort_unstable();
    fb.requests.dedup();

    let ub = layout.used_bits_offset_words() as usize;
    for (wi, &w) in words[ub..ub + layout.bits_words as usize]
        .iter()
        .enumerate()
    {
        let mut bits = w;
        while bits != 0 {
            let b = bits.trailing_zeros();
            bits &= bits - 1;
            let page = wi as u32 * 32 + b;
            if page < page_count {
                fb.used.push(page);
            }
        }
    }
    fb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_dedups_clamps_and_bounds() {
        let layout = FeedbackLayout::new(4, 100);
        let mut w = layout.initial_words();
        w[0] = 6; // exceeds capacity 4
        w[4..8].copy_from_slice(&[9, 3, 9, 500]);
        let ub = layout.used_bits_offset_words() as usize;
        w[ub] = 0b1010_0001; // pages 0, 5, 7
        w[ub + 3] = 1 << 5; // page 101: beyond page_count 100
        let fb = decode(&w, layout, 100);
        assert_eq!(fb.requests, vec![3, 9]);
        assert_eq!(fb.used, vec![0, 5, 7]);
        assert!(fb.overflowed);
        assert_eq!(layout.bits_words, 4);
        assert_eq!(decode(&w[..3], layout, 100), Feedback::default());
    }
}
