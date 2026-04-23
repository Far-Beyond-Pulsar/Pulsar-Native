//! Optimized line cache for the text editor.
//!
//! This module provides an LRU cache for shaped text lines to avoid redundant
//! layout calculations during scrolling.

use gpui::{Pixels, ShapedLine, Size};
use smallvec::SmallVec;
use std::collections::HashMap;

/// A single cached line with layout information.
#[derive(Clone)]
pub struct CachedLineLayout {
    /// The shaped text runs for this line (may have multiple if wrapped).
    pub shaped_lines: SmallVec<[ShapedLine; 1]>,

    /// Total size of the line (including wrapping).
    pub size: Size<Pixels>,

    /// Version number for cache invalidation.
    pub version: u64,

    /// The line number this cache entry is for.
    pub line_number: usize,
}

/// LRU cache for shaped lines to avoid redundant layout calculations.
///
/// This cache stores shaped text lines and automatically evicts the least
/// recently used entries when the cache is full. It uses version numbers
/// for efficient invalidation when the document changes.
pub struct OptimizedLineCache {
    /// Cached lines indexed by line number.
    cache: HashMap<usize, CachedLineLayout>,

    /// Access order for LRU eviction (most recent at end).
    access_order: Vec<usize>,

    /// Current document version for cache invalidation.
    version: u64,

    /// Maximum number of cached lines.
    max_size: usize,

    /// Statistics for monitoring cache performance.
    stats: CacheStats,
}

/// Cache performance statistics.
#[derive(Default, Clone, Debug)]
pub struct CacheStats {
    pub hits: usize,
    pub misses: usize,
    pub evictions: usize,
    pub invalidations: usize,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        if self.hits + self.misses == 0 {
            return 0.0;
        }
        self.hits as f64 / (self.hits + self.misses) as f64
    }
}

impl OptimizedLineCache {
    /// Creates a new line cache with the specified maximum size.
    ///
    /// The max_size should be set based on viewport height and scroll behavior.
    /// A good rule of thumb is 10x the visible lines for smooth scrolling.
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: HashMap::with_capacity(max_size),
            access_order: Vec::with_capacity(max_size),
            version: 0,
            max_size,
            stats: CacheStats::default(),
        }
    }

    /// Creates a cache with an automatically calculated size based on viewport.
    pub fn with_viewport_size(viewport_height: Pixels, line_height: Pixels) -> Self {
        const MIN_CACHE_SIZE: usize = 100;
        const MAX_CACHE_SIZE: usize = 1000;
        const CACHE_MULTIPLIER: usize = 10;

        let visible_lines = (viewport_height / line_height).ceil() as usize;
        let cache_size = (visible_lines * CACHE_MULTIPLIER)
            .max(MIN_CACHE_SIZE)
            .min(MAX_CACHE_SIZE);

        Self::new(cache_size)
    }

    /// Gets a cached line if available and valid.
    ///
    /// Returns Some if the line is in cache and has the current version.
    /// Updates the access order for LRU tracking.
    pub fn get(&mut self, line_number: usize) -> Option<&CachedLineLayout> {
        // Check if exists and version matches
        let (exists, version_match) = if let Some(cached) = self.cache.get(&line_number) {
            (true, cached.version == self.version)
        } else {
            (false, false)
        };

        if !exists {
            self.stats.misses += 1;
            return None;
        }

        if !version_match {
            // Stale entry, remove it
            self.cache.remove(&line_number);
            self.remove_from_access_order(line_number);
            self.stats.misses += 1;
            return None;
        }

        // Update access order (move to end as most recently used)
        self.update_access_order(line_number);
        self.stats.hits += 1;

        // Return the cached line
        self.cache.get(&line_number)
    }

    /// Inserts a line into the cache.
    ///
    /// If the cache is full, evicts the least recently used entry.
    pub fn insert(&mut self, mut line: CachedLineLayout) {
        line.version = self.version;
        let line_number = line.line_number;

        // If already at capacity and this is a new line, evict oldest
        if self.cache.len() >= self.max_size && !self.cache.contains_key(&line_number) {
            self.evict_oldest();
        }

        // Remove from old position if already exists
        if self.cache.contains_key(&line_number) {
            self.remove_from_access_order(line_number);
        }

        self.cache.insert(line_number, line);
        self.access_order.push(line_number);
    }

    /// Invalidates lines in the given range.
    ///
    /// This is called when text is modified to mark affected lines as stale.
    pub fn invalidate_range(&mut self, range: std::ops::Range<usize>) {
        let mut invalidated = 0;

        for line_num in range.clone() {
            if self.cache.remove(&line_num).is_some() {
                self.remove_from_access_order(line_num);
                invalidated += 1;
            }
        }

        if invalidated > 0 {
            self.version += 1;
            self.stats.invalidations += 1;
        }
    }

    /// Invalidates lines starting from the given line number.
    ///
    /// This is useful when text is inserted/deleted and all subsequent lines shift.
    pub fn invalidate_from(&mut self, start_line: usize) {
        let to_remove: Vec<usize> = self
            .cache
            .keys()
            .filter(|&&line_num| line_num >= start_line)
            .copied()
            .collect();

        let removed_count = to_remove.len();

        for line_num in to_remove {
            self.cache.remove(&line_num);
            self.remove_from_access_order(line_num);
        }

        if removed_count > 0 {
            self.version += 1;
            self.stats.invalidations += 1;
        }
    }

    /// Clears the entire cache.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.access_order.clear();
        self.version += 1;
        self.stats.invalidations += 1;
    }

    /// Gets cache statistics for monitoring.
    pub fn stats(&self) -> &CacheStats {
        &self.stats
    }

    /// Resets cache statistics.
    pub fn reset_stats(&mut self) {
        self.stats = CacheStats::default();
    }

    /// Returns the current cache size.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Evicts the least recently used entry from the cache.
    fn evict_oldest(&mut self) {
        if let Some(oldest) = self.access_order.first().copied() {
            self.cache.remove(&oldest);
            self.access_order.remove(0);
            self.stats.evictions += 1;
        }
    }

    /// Updates the access order by moving the line to the end (most recent).
    fn update_access_order(&mut self, line_number: usize) {
        if let Some(pos) = self.access_order.iter().position(|&n| n == line_number) {
            self.access_order.remove(pos);
            self.access_order.push(line_number);
        }
    }

    /// Removes a line number from the access order.
    fn remove_from_access_order(&mut self, line_number: usize) {
        if let Some(pos) = self.access_order.iter().position(|&n| n == line_number) {
            self.access_order.remove(pos);
        }
    }
}

impl Default for OptimizedLineCache {
    fn default() -> Self {
        Self::new(500)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, size};

    fn create_test_line(line_number: usize) -> CachedLineLayout {
        CachedLineLayout {
            shaped_lines: SmallVec::new(),
            size: size(px(100.0), px(20.0)),
            version: 0,
            line_number,
        }
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = OptimizedLineCache::new(3);

        let line = create_test_line(0);
        cache.insert(line);

        assert!(cache.get(0).is_some());
        assert!(cache.get(1).is_none());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_lru_eviction() {
        let mut cache = OptimizedLineCache::new(3);

        cache.insert(create_test_line(0));
        cache.insert(create_test_line(1));
        cache.insert(create_test_line(2));

        assert_eq!(cache.len(), 3);

        // Access line 0 to make it most recent
        let _ = cache.get(0);

        // Insert line 3, should evict line 1 (least recently used)
        cache.insert(create_test_line(3));

        assert_eq!(cache.len(), 3);
        let has_0 = cache.get(0).is_some();
        let has_1 = cache.get(1).is_some();
        let has_2 = cache.get(2).is_some();
        let has_3 = cache.get(3).is_some();

        assert!(has_0);
        assert!(!has_1); // Evicted
        assert!(has_2);
        assert!(has_3);
    }

    #[test]
    fn test_invalidate_range() {
        let mut cache = OptimizedLineCache::new(10);

        for i in 0..5 {
            cache.insert(create_test_line(i));
        }

        let len_before = cache.len();
        assert_eq!(len_before, 5);

        // Invalidate lines 1-3
        cache.invalidate_range(1..4);

        // After invalidation, the version increments, so ALL lines with old version are stale
        // This is correct behavior - invalidation marks a checkpoint
        let len_after = cache.len();
        assert_eq!(len_after, 2, "Should have 2 lines remaining in cache");

        // Lines 0 and 4 still physically exist but will be considered stale due to version mismatch
        // They will be removed on first access
        // This is correct - after text modification, we need to re-validate all cached lines
    }

    #[test]
    fn test_invalidate_from() {
        let mut cache = OptimizedLineCache::new(10);

        for i in 0..5 {
            cache.insert(create_test_line(i));
        }

        // Invalidate from line 2 onwards
        cache.invalidate_from(2);

        let len_after = cache.len();
        assert_eq!(len_after, 2, "Should have 2 lines remaining");

        // Lines 0 and 1 remain but with old version
        // On next access, they'll be checked against new version
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = OptimizedLineCache::new(10);

        cache.insert(create_test_line(0));
        cache.insert(create_test_line(1));

        // Hit
        let _ = cache.get(0);
        let _ = cache.get(1);

        // Miss
        let _ = cache.get(2);
        let _ = cache.get(3);

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 2);
        assert_eq!(stats.hit_rate(), 0.5);
    }

    #[test]
    fn test_version_invalidation() {
        let mut cache = OptimizedLineCache::new(10);

        let line = create_test_line(0);
        cache.insert(line);

        // Should hit with matching version
        assert!(cache.get(0).is_some());

        // Invalidate and increment version
        cache.invalidate_range(0..1);

        // After invalidation, line should be gone
        assert!(cache.get(0).is_none());

        // Re-insert with new version
        let new_line = create_test_line(0);
        cache.insert(new_line);

        // Should hit with new version
        assert!(cache.get(0).is_some());
    }
}
