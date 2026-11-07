//! Page upload scheduler: priority by request count and recency, LRU
//! eviction that never evicts root pages, a per-frame upload budget. Pure CPU
//! (no wgpu): the store applies the plan it returns.

use std::collections::{BTreeSet, HashMap};

/// Outcome of asking the CPU tier for a page's bytes.
pub enum Fetch<T> {
    Ready(T),
    /// A load was started (or is running); ask again later.
    NotYet,
    Failed,
}

#[derive(Clone, Copy, Debug)]
pub struct SchedulerConfig {
    /// A resident page is only evictable once unused for more than this many
    /// frames (readback feedback lags the GPU by a few frames).
    pub eviction_grace_frames: u64,
    /// A request not renewed by the GPU for this many frames is dropped.
    pub request_ttl_frames: u64,
    /// A failed page is not retried for this many frames.
    pub retry_after_frames: u64,
    /// How many candidates are inspected per plan (bounds fetch/prefetch work).
    pub lookahead: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            eviction_grace_frames: 4,
            request_ttl_frames: 240,
            retry_after_frames: 240,
            lookahead: 64,
        }
    }
}

#[derive(Debug, Default, Clone)]
struct PageState {
    slot: Option<u32>,
    root: bool,
    last_used: u64,
    req_count: u32,
    req_frame: Option<u64>,
    failed_until: u64,
    /// Pages that must be resident whenever this one is (parents of its clusters).
    deps: Vec<u32>,
    /// Resident pages that list this page as a dependency.
    dependents: u32,
}

/// One page to copy into a pool slot.
#[derive(Debug)]
pub struct Upload<T> {
    pub page: u32,
    pub slot: u32,
    /// Page that lost this slot; its residency-table entry must be cleared.
    pub evicted: Option<u32>,
    pub payload: T,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerStats {
    pub resident_pages: u32,
    pub root_pages_resident: u32,
    pub slot_capacity: u32,
    pub slot_limit: u32,
    pub uploads: u64,
    pub evictions: u64,
    pub failures: u64,
}

pub struct PageScheduler {
    cfg: SchedulerConfig,
    limit: u32,
    capacity: u32,
    grow_chunk: u32,
    free: Vec<u32>,
    pages: HashMap<u32, PageState>,
    /// Resident non-root pages ordered by last use.
    lru: BTreeSet<(u64, u32)>,
    roots_pending: BTreeSet<u32>,
    resident: u32,
    roots_resident: u32,
    uploads: u64,
    evictions: u64,
    failures: u64,
}

impl PageScheduler {
    pub fn new(cfg: SchedulerConfig, initial_slots: u32, grow_chunk: u32, slot_limit: u32) -> Self {
        let capacity = initial_slots.min(slot_limit);
        Self {
            cfg,
            limit: slot_limit,
            capacity,
            grow_chunk: grow_chunk.max(1),
            free: (0..capacity).rev().collect(),
            pages: HashMap::new(),
            lru: BTreeSet::new(),
            roots_pending: BTreeSet::new(),
            resident: 0,
            roots_resident: 0,
            uploads: 0,
            evictions: 0,
            failures: 0,
        }
    }

    pub fn slot_capacity(&self) -> u32 {
        self.capacity
    }
    pub fn slot_limit(&self) -> u32 {
        self.limit
    }
    pub fn slot_of(&self, page: u32) -> Option<u32> {
        self.pages.get(&page).and_then(|p| p.slot)
    }
    pub fn is_resident(&self, page: u32) -> bool {
        self.slot_of(page).is_some()
    }
    pub fn is_root(&self, page: u32) -> bool {
        self.pages.get(&page).is_some_and(|p| p.root)
    }
    pub fn stats(&self) -> SchedulerStats {
        SchedulerStats {
            resident_pages: self.resident,
            root_pages_resident: self.roots_resident,
            slot_capacity: self.capacity,
            slot_limit: self.limit,
            uploads: self.uploads,
            evictions: self.evictions,
            failures: self.failures,
        }
    }

    /// Number of root pages pinned (resident or not).
    pub fn pinned_roots(&self) -> u32 {
        self.roots_resident + self.roots_pending.len() as u32
    }

    /// Pin a page as a root: loaded with priority, never evicted.
    pub fn pin_root(&mut self, page: u32) {
        let st = self.pages.entry(page).or_default();
        if st.root {
            return;
        }
        st.root = true;
        if st.slot.is_some() {
            self.lru.remove(&(st.last_used, page));
            self.roots_resident += 1;
        } else {
            self.roots_pending.insert(page);
        }
    }

    /// The GPU asked for `page` (non-resident). Counts accumulate per readback.
    pub fn request(&mut self, page: u32, count: u32, frame: u64) {
        if self.is_resident(page) {
            self.touch(page, frame);
            return;
        }
        let st = self.pages.entry(page).or_default();
        if st.root {
            return;
        }
        st.req_count = st.req_count.saturating_add(count.max(1));
        st.req_frame = Some(frame);
    }

    /// A resident page was needed this frame (LRU recency).
    pub fn touch(&mut self, page: u32, frame: u64) {
        if let Some(st) = self.pages.get_mut(&page) {
            if st.slot.is_some() && frame > st.last_used {
                if !st.root {
                    self.lru.remove(&(st.last_used, page));
                    self.lru.insert((frame, page));
                }
                st.last_used = frame;
            }
        }
    }

    /// Forget a page (asset released). Returns its slot if it was resident.
    pub fn remove_page(&mut self, page: u32) -> Option<u32> {
        let st = self.pages.remove(&page)?;
        self.roots_pending.remove(&page);
        let slot = st.slot?;
        self.release_deps(&st.deps);
        if st.root {
            self.roots_resident -= 1;
        } else {
            self.lru.remove(&(st.last_used, page));
        }
        self.resident -= 1;
        self.free.push(slot);
        Some(slot)
    }

    fn acquire_slot(
        &mut self,
        frame: u64,
        root: bool,
        taken: &[u32],
    ) -> Option<(u32, Option<u32>)> {
        if let Some(s) = self.free.pop() {
            return Some((s, None));
        }
        if self.capacity < self.limit {
            let new_cap = (self.capacity + self.grow_chunk).min(self.limit);
            self.free.extend((self.capacity..new_cap).rev());
            self.capacity = new_cap;
            return self.free.pop().map(|s| (s, None));
        }
        let grace = self.cfg.eviction_grace_frames;
        let pages = &self.pages;
        let victim = self
            .lru
            .iter()
            .find(|&&(last, page)| {
                !taken.contains(&page)
                    && pages.get(&page).is_some_and(|s| s.dependents == 0)
                    && (root || frame.saturating_sub(last) > grace)
            })
            .copied();
        let (last, page) = victim?;
        self.lru.remove(&(last, page));
        let st = self.pages.get_mut(&page).expect("victim state");
        let slot = st.slot.take().expect("victim slot");
        let deps = st.deps.clone();
        self.release_deps(&deps);
        self.resident -= 1;
        self.evictions += 1;
        Some((slot, Some(page)))
    }

    fn release_deps(&mut self, deps: &[u32]) {
        for d in deps {
            if let Some(s) = self.pages.get_mut(d) {
                s.dependents = s.dependents.saturating_sub(1);
            }
        }
    }

    /// Declare the pages that must be resident whenever `page` is. A page is
    /// only ever uploaded after its dependencies, and a page with resident
    /// dependents is never evicted (the "upward closed" invariant the cull's
    /// cut rule relies on). Call before the page can become resident.
    pub fn set_deps(&mut self, page: u32, deps: Vec<u32>) {
        self.pages.entry(page).or_default().deps = deps;
    }

    /// Non-resident pages that must be uploaded, in order, to make `page`
    /// resident (dependencies first, `page` last). `None` if any is parked.
    fn chain(&self, page: u32, frame: u64) -> Option<Vec<u32>> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        fn visit(
            s: &PageScheduler,
            p: u32,
            frame: u64,
            seen: &mut std::collections::HashSet<u32>,
            out: &mut Vec<u32>,
        ) -> bool {
            if !seen.insert(p) || s.is_resident(p) {
                return true;
            }
            let Some(st) = s.pages.get(&p) else {
                out.push(p);
                return true;
            };
            if st.failed_until > frame {
                return false;
            }
            for &d in &st.deps {
                if !visit(s, d, frame, seen, out) {
                    return false;
                }
            }
            out.push(p);
            true
        }
        visit(self, page, frame, &mut seen, &mut out).then_some(out)
    }

    fn install(&mut self, page: u32, slot: u32, frame: u64) {
        let deps = self
            .pages
            .get(&page)
            .map(|s| s.deps.clone())
            .unwrap_or_default();
        for d in deps {
            self.pages.entry(d).or_default().dependents += 1;
        }
        let st = self.pages.entry(page).or_default();
        st.slot = Some(slot);
        st.last_used = frame;
        st.req_count = 0;
        st.req_frame = None;
        self.resident += 1;
        self.uploads += 1;
        if st.root {
            self.roots_pending.remove(&page);
            self.roots_resident += 1;
        } else {
            self.lru.insert((frame, page));
        }
    }

    /// Choose what to upload this frame. `fetch` supplies the CPU bytes (and
    /// may start background loads). Root pages use `max_root_uploads`, other
    /// pages `max_uploads`. Chosen pages are resident in the scheduler's view
    /// when this returns; the caller must perform the uploads.
    pub fn plan<T>(
        &mut self,
        frame: u64,
        max_uploads: u32,
        max_root_uploads: u32,
        mut fetch: impl FnMut(u32) -> Fetch<T>,
    ) -> Vec<Upload<T>> {
        let ttl = self.cfg.request_ttl_frames;
        for st in self.pages.values_mut() {
            if st.req_frame.is_some_and(|f| frame.saturating_sub(f) > ttl) {
                st.req_frame = None;
                st.req_count = 0;
            }
        }
        let mut out: Vec<Upload<T>> = Vec::new();
        let mut taken: Vec<u32> = Vec::new();

        // Roots first.
        let roots: Vec<u32> = self
            .roots_pending
            .iter()
            .copied()
            .take(self.cfg.lookahead)
            .collect();
        let mut root_n = 0;
        for page in roots {
            if root_n >= max_root_uploads {
                break;
            }
            if self
                .pages
                .get(&page)
                .is_some_and(|s| s.failed_until > frame)
            {
                continue;
            }
            match fetch(page) {
                Fetch::Ready(payload) => {
                    let Some((slot, evicted)) = self.acquire_slot(frame, true, &taken) else {
                        log::warn!(
                            "vg-stream: no slot for root page {page}; pool is full of root pages"
                        );
                        break;
                    };
                    self.install(page, slot, frame);
                    taken.push(page);
                    out.push(Upload {
                        page,
                        slot,
                        evicted,
                        payload,
                    });
                    root_n += 1;
                }
                Fetch::NotYet => {}
                Fetch::Failed => self.fail(page, frame),
            }
        }

        // Requested pages: most requested, then most recently requested.
        let mut cands: Vec<(u32, u64, u32)> = self
            .pages
            .iter()
            .filter(|(_, s)| {
                s.slot.is_none() && !s.root && s.req_frame.is_some() && s.failed_until <= frame
            })
            .map(|(&p, s)| (s.req_count, s.req_frame.unwrap_or(0), p))
            .collect();
        cands.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        let mut n = 0;
        'cands: for &(_, _, page) in cands.iter().take(self.cfg.lookahead) {
            if n >= max_uploads {
                break;
            }
            let Some(chain) = self.chain(page, frame) else {
                continue;
            };
            if n > 0 && n + chain.len() as u32 > max_uploads {
                continue;
            }
            // Dependencies first: every prefix of the chain is itself upward
            // closed, so stopping part way keeps the invariant.
            for p in chain {
                match fetch(p) {
                    Fetch::Ready(payload) => {
                        let mut prot = taken.clone();
                        if let Some(st) = self.pages.get(&p) {
                            prot.extend(st.deps.iter().copied());
                        }
                        let Some((slot, evicted)) = self.acquire_slot(frame, false, &prot) else {
                            break 'cands; // pool saturated with recently used pages
                        };
                        self.install(p, slot, frame);
                        taken.push(p);
                        out.push(Upload {
                            page: p,
                            slot,
                            evicted,
                            payload,
                        });
                        n += 1;
                    }
                    Fetch::NotYet => continue 'cands,
                    Fetch::Failed => {
                        self.fail(p, frame);
                        continue 'cands;
                    }
                }
            }
        }
        out
    }

    fn fail(&mut self, page: u32, frame: u64) {
        self.failures += 1;
        if let Some(st) = self.pages.get_mut(&page) {
            st.failed_until = frame + self.cfg.retry_after_frames;
            st.req_frame = None;
            st.req_count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(slots: u32) -> PageScheduler {
        PageScheduler::new(
            SchedulerConfig {
                eviction_grace_frames: 2,
                ..Default::default()
            },
            slots,
            2,
            slots,
        )
    }
    fn ready(_: u32) -> Fetch<()> {
        Fetch::Ready(())
    }

    #[test]
    fn roots_load_first_and_are_never_evicted() {
        let mut s = sched(3);
        s.pin_root(0);
        s.pin_root(1);
        let up = s.plan(1, 4, 4, ready);
        assert_eq!(up.len(), 2);
        assert!(s.is_resident(0) && s.is_resident(1));
        for f in 2..40 {
            for p in 10..20 {
                s.request(p, 1, f);
            }
            s.plan(f, 8, 8, ready);
            assert!(
                s.is_resident(0) && s.is_resident(1),
                "root evicted at frame {f}"
            );
        }
        assert_eq!(s.stats().resident_pages, 3);
    }

    #[test]
    fn priority_by_count_then_recency_and_budget() {
        let mut s = sched(8);
        s.request(5, 1, 10);
        s.request(6, 5, 9);
        s.request(7, 1, 11);
        let up = s.plan(12, 2, 2, ready);
        let pages: Vec<u32> = up.iter().map(|u| u.page).collect();
        assert_eq!(pages, vec![6, 7]); // count 5 first, then the more recent
        let up = s.plan(13, 2, 2, ready);
        assert_eq!(up.iter().map(|u| u.page).collect::<Vec<_>>(), vec![5]);
    }

    #[test]
    fn lru_eviction_respects_grace() {
        let mut s = sched(2);
        s.request(1, 1, 1);
        s.request(2, 1, 1);
        assert_eq!(s.plan(1, 8, 8, ready).len(), 2);
        s.touch(2, 5);
        s.request(3, 1, 5);
        let up = s.plan(5, 8, 8, ready);
        assert_eq!(up.len(), 1);
        assert_eq!(up[0].evicted, Some(1));
        assert!(!s.is_resident(1) && s.is_resident(2) && s.is_resident(3));
        // Everything fresh: a new request must wait (no victim inside grace).
        s.request(4, 1, 6);
        assert!(s.plan(6, 8, 8, ready).is_empty());
        assert!(!s.is_resident(4));
        let up = s.plan(20, 8, 8, ready);
        assert_eq!(up.len(), 1);
    }

    #[test]
    fn root_may_evict_inside_grace() {
        let mut s = sched(1);
        s.request(1, 1, 1);
        s.plan(1, 8, 8, ready);
        s.pin_root(9);
        let up = s.plan(1, 8, 8, ready);
        assert_eq!(up.len(), 1);
        assert_eq!(up[0].evicted, Some(1));
        assert!(s.is_root(9) && s.is_resident(9));
    }

    #[test]
    fn not_yet_and_failed_fetches() {
        let mut s = sched(4);
        s.request(1, 1, 1);
        s.request(2, 1, 1);
        let up = s.plan(1, 8, 8, |p| {
            if p == 1 {
                Fetch::<()>::NotYet
            } else {
                Fetch::Failed
            }
        });
        assert!(up.is_empty());
        assert_eq!(s.stats().failures, 1);
        // Page 1 stays pending, page 2 is parked until the retry window.
        let up = s.plan(2, 8, 8, ready);
        assert_eq!(up.iter().map(|u| u.page).collect::<Vec<_>>(), vec![1]);
        s.request(2, 1, 3);
        assert!(s.plan(3, 8, 8, ready).is_empty());
    }

    #[test]
    fn remove_page_frees_slot_and_grows_lazily() {
        let mut s = PageScheduler::new(Default::default(), 1, 2, 4);
        assert_eq!(s.slot_capacity(), 1);
        for p in 0..4 {
            s.request(p, 1, 1);
        }
        assert_eq!(s.plan(1, 8, 8, ready).len(), 4);
        assert_eq!(s.slot_capacity(), 4);
        let slot = s.remove_page(2).unwrap();
        s.request(9, 1, 2);
        assert_eq!(s.plan(2, 8, 8, ready)[0].slot, slot);
    }

    #[test]
    fn stale_requests_expire() {
        let mut s = PageScheduler::new(
            SchedulerConfig {
                request_ttl_frames: 5,
                ..Default::default()
            },
            4,
            4,
            4,
        );
        s.request(1, 1, 1);
        assert!(s.plan(100, 8, 8, ready).is_empty());
    }
}
