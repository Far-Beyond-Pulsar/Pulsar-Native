//! First-fit range allocator over a growable index space (nodes, group refs,
//! page ids). Pure CPU bookkeeping; the GPU arrays follow its capacity.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: u32,
    pub len: u32,
}

impl Range {
    pub fn end(&self) -> u32 {
        self.start + self.len
    }
}

#[derive(Debug, Default)]
pub struct RangeAlloc {
    capacity: u32,
    /// Sorted, non-adjacent free ranges.
    free: Vec<Range>,
}

impl RangeAlloc {
    pub fn new(capacity: u32) -> Self {
        let free = if capacity > 0 {
            vec![Range {
                start: 0,
                len: capacity,
            }]
        } else {
            vec![]
        };
        Self { capacity, free }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Allocate `len` contiguous entries, growing capacity (doubling, at least
    /// enough) when nothing fits. Returns the range; check [`Self::capacity`]
    /// afterwards to see whether the backing array must grow.
    pub fn alloc(&mut self, len: u32) -> Range {
        if len == 0 {
            return Range { start: 0, len: 0 };
        }
        loop {
            if let Some(i) = self.free.iter().position(|r| r.len >= len) {
                let r = self.free[i];
                if r.len == len {
                    self.free.remove(i);
                } else {
                    self.free[i] = Range {
                        start: r.start + len,
                        len: r.len - len,
                    };
                }
                return Range {
                    start: r.start,
                    len,
                };
            }
            let new_cap = (self.capacity.max(1) * 2).max(self.capacity + len);
            self.grow_to(new_cap);
        }
    }

    pub fn grow_to(&mut self, new_cap: u32) {
        if new_cap <= self.capacity {
            return;
        }
        let added = Range {
            start: self.capacity,
            len: new_cap - self.capacity,
        };
        self.capacity = new_cap;
        self.insert_free(added);
    }

    pub fn free(&mut self, r: Range) {
        if r.len > 0 {
            self.insert_free(r);
        }
    }

    fn insert_free(&mut self, r: Range) {
        let at = self.free.partition_point(|f| f.start < r.start);
        self.free.insert(at, r);
        // merge with next, then previous
        if at + 1 < self.free.len() && self.free[at].end() == self.free[at + 1].start {
            self.free[at].len += self.free[at + 1].len;
            self.free.remove(at + 1);
        }
        if at > 0 && self.free[at - 1].end() == self.free[at].start {
            self.free[at - 1].len += self.free[at].len;
            self.free.remove(at);
        }
    }

    pub fn free_total(&self) -> u32 {
        self.free.iter().map(|r| r.len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_free_merge_and_grow() {
        let mut a = RangeAlloc::new(8);
        let x = a.alloc(4);
        let y = a.alloc(4);
        assert_eq!((x.start, y.start), (0, 4));
        let z = a.alloc(3); // grows
        assert!(a.capacity() >= 11 && z.start == 8);
        a.free(x);
        a.free(y);
        // merged: a 8-long run is available again at 0
        assert_eq!(a.alloc(8).start, 0);
        a.free(z);
        assert_eq!(a.free_total(), a.capacity() - 8);
    }
}
